//! Загрузки файлов: перехват, папка сохранения, «спросить, куда сохранить»,
//! пауза, продолжение и отмена.
//!
//! Операции загрузки живут в реестре хоста, а не во вкладке: вкладку можно
//! закрыть, а файл при этом должен докачаться и остаться управляемым из
//! списка загрузок.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Deferral, ICoreWebView2DownloadOperation,
    ICoreWebView2DownloadStartingEventArgs, ICoreWebView2_4,
    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON,
    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_ACCESS_DENIED,
    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_BLOCKED_BY_POLICY,
    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_MALICIOUS,
    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_NO_SPACE,
    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_SECURITY_CHECK_FAILED,
    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_DISCONNECTED,
    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_FAILED,
    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_SERVER_DOWN,
    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_TIMEOUT,
    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE,
    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_FAILED,
    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_FORBIDDEN,
    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_UNAUTHORIZED,
    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED,
    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED,
    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_SHUTDOWN, COREWEBVIEW2_DOWNLOAD_STATE,
    COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED, COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS,
};
use webview2_com::{
    take_pwstr, BytesReceivedChangedEventHandler, DownloadStartingEventHandler,
    StateChangedEventHandler,
};
use windows_core::{Interface, HSTRING, PWSTR};

use crate::host::TabId;
use crate::tab::{EventSink, TabEvent};

/// Прогресс в интерфейс — не чаще этого. Движок шлёт его на каждый
/// принятый кусок, а цифре в списке загрузок хватает пяти кадров в секунду.
const PROGRESS_EVERY: Duration = Duration::from_millis(200);

/// Куда сохранять. Меняется из настроек без перезапуска.
#[derive(Debug, Clone, Default)]
pub struct DownloadPolicy {
    /// `None` — папка по умолчанию, которую предлагает движок (Загрузки).
    pub dir: Option<PathBuf>,
    /// Всегда спрашивать, куда сохранить файл.
    pub ask: bool,
}

/// Загрузка, ждущая ответа пользователя на «куда сохранить».
struct Pending {
    args: ICoreWebView2DownloadStartingEventArgs,
    deferral: ICoreWebView2Deferral,
    operation: ICoreWebView2DownloadOperation,
    tab: u32,
    url: String,
    total: Option<i64>,
    sink: EventSink,
}

#[derive(Default)]
pub struct Downloads {
    next: Cell<u64>,
    ops: RefCell<HashMap<u64, ICoreWebView2DownloadOperation>>,
    pending: RefCell<HashMap<u64, Pending>>,
    policy: RefCell<DownloadPolicy>,
}

pub type SharedDownloads = Rc<Downloads>;

impl Downloads {
    pub fn set_policy(&self, policy: DownloadPolicy) {
        *self.policy.borrow_mut() = policy;
    }

    fn next_key(&self) -> u64 {
        let key = self.next.get() + 1;
        self.next.set(key);
        key
    }

    /// Пауза, продолжение, отмена.
    pub fn control(&self, key: u64, action: &str) -> anyhow::Result<()> {
        let operation = self
            .ops
            .borrow()
            .get(&key)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("загрузка уже завершена"))?;
        unsafe {
            match action {
                "cancel" => operation.Cancel()?,
                "pause" => operation.Pause()?,
                "resume" => {
                    let mut can = windows_core::BOOL::default();
                    operation.CanResume(&mut can)?;
                    anyhow::ensure!(can.as_bool(), "эту загрузку не продолжить — начните заново");
                    operation.Resume()?;
                }
                other => anyhow::bail!("неизвестное действие загрузки {other}"),
            }
        }
        Ok(())
    }
}

/// Ответ на «куда сохранить»: путь или отказ.
pub fn answer(registry: &SharedDownloads, key: u64, path: Option<PathBuf>) -> anyhow::Result<()> {
    let pending = registry
        .pending
        .borrow_mut()
        .remove(&key)
        .ok_or_else(|| anyhow::anyhow!("запрос на загрузку уже закрыт"))?;

    let result = match path {
        Some(path) => unsafe {
            pending
                .args
                .SetResultFilePath(&HSTRING::from(path.as_os_str()))
                .map_err(anyhow::Error::from)
                .and_then(|()| {
                    track(
                        registry,
                        key,
                        pending.tab,
                        pending.operation.clone(),
                        pending.url.clone(),
                        path.to_string_lossy().into_owned(),
                        pending.total,
                        pending.sink.clone(),
                    )
                    .map_err(anyhow::Error::from)
                })
        },
        None => unsafe { pending.args.SetCancel(true).map_err(anyhow::Error::from) },
    };

    // Отложенное решение обязано завершиться при любом исходе: иначе движок
    // держит загрузку в подвешенном состоянии до закрытия вкладки.
    unsafe { pending.deferral.Complete()? };
    result
}

/// Перехват загрузок вкладки.
pub fn wire(
    id: TabId,
    core: &ICoreWebView2,
    registry: SharedDownloads,
    sink: EventSink,
) -> windows_core::Result<()> {
    let Ok(core4) = core.cast::<ICoreWebView2_4>() else {
        tracing::warn!("движок старее 1.0.902: загрузки не перехватываются");
        return Ok(());
    };

    let tab = id.0;
    let mut token = 0i64;

    unsafe {
        core4.add_DownloadStarting(
            &DownloadStartingEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let operation = args.DownloadOperation()?;
                // Полку загрузок движка гасим: у браузера она своя.
                args.SetHandled(true)?;

                let url = {
                    let mut raw = PWSTR::null();
                    operation.Uri(&mut raw)?;
                    take_pwstr(raw)
                };
                let suggested = {
                    let mut raw = PWSTR::null();
                    args.ResultFilePath(&mut raw)?;
                    take_pwstr(raw)
                };
                let mut total = 0i64;
                operation.TotalBytesToReceive(&mut total)?;
                let total = (total > 0).then_some(total);

                let policy = registry.policy.borrow().clone();
                let target = match &policy.dir {
                    Some(dir) => unique_path(dir, &file_name_of(&suggested)),
                    None => PathBuf::from(&suggested),
                };
                let key = registry.next_key();
                tracing::debug!(tab, key, ask = policy.ask, "загрузка началась");

                if policy.ask {
                    let deferral = args.GetDeferral()?;
                    registry.pending.borrow_mut().insert(
                        key,
                        Pending {
                            args: args.clone(),
                            deferral,
                            operation,
                            tab,
                            url,
                            total,
                            sink: sink.clone(),
                        },
                    );
                    sink(TabEvent::DownloadAsk {
                        id: tab,
                        key,
                        path: target.to_string_lossy().into_owned(),
                    });
                    return Ok(());
                }

                if policy.dir.is_some() {
                    args.SetResultFilePath(&HSTRING::from(target.as_os_str()))?;
                }
                track(
                    &registry,
                    key,
                    tab,
                    operation,
                    url,
                    target.to_string_lossy().into_owned(),
                    total,
                    sink.clone(),
                )
            })),
            &mut token,
        )?;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn track(
    registry: &SharedDownloads,
    key: u64,
    tab: u32,
    operation: ICoreWebView2DownloadOperation,
    url: String,
    path: String,
    total: Option<i64>,
    sink: EventSink,
) -> windows_core::Result<()> {
    registry.ops.borrow_mut().insert(key, operation.clone());
    sink(TabEvent::Download {
        id: tab,
        key,
        phase: "started",
        url: url.clone(),
        path: path.clone(),
        bytes: 0,
        total,
        error: "",
    });

    let mut token = 0i64;
    unsafe {
        let progress_sink = sink.clone();
        let progress_url = url.clone();
        let progress_path = path.clone();
        let last: Cell<Option<Instant>> = Cell::new(None);
        operation.add_BytesReceivedChanged(
            &BytesReceivedChangedEventHandler::create(Box::new(move |sender, _| {
                let Some(operation) = sender else {
                    return Ok(());
                };
                let now = Instant::now();
                if last
                    .get()
                    .is_some_and(|at| now.duration_since(at) < PROGRESS_EVERY)
                {
                    return Ok(());
                }
                last.set(Some(now));

                let mut bytes = 0i64;
                operation.BytesReceived(&mut bytes)?;
                let mut total = 0i64;
                operation.TotalBytesToReceive(&mut total)?;
                progress_sink(TabEvent::Download {
                    id: tab,
                    key,
                    phase: "progress",
                    url: progress_url.clone(),
                    path: progress_path.clone(),
                    bytes,
                    total: (total > 0).then_some(total),
                    error: "",
                });
                Ok(())
            })),
            &mut token,
        )?;

        // Слабая ссылка: операция держит обработчик, обработчик — реестр,
        // реестр — операцию. Сильная ссылка здесь замкнула бы круг.
        let registry: Weak<Downloads> = Rc::downgrade(registry);
        operation.add_StateChanged(
            &StateChangedEventHandler::create(Box::new(move |sender, _| {
                let Some(operation) = sender else {
                    return Ok(());
                };
                let mut state = COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS;
                operation.State(&mut state)?;
                let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE;
                operation.InterruptReason(&mut reason)?;
                let mut bytes = 0i64;
                operation.BytesReceived(&mut bytes)?;
                let mut total = 0i64;
                operation.TotalBytesToReceive(&mut total)?;

                let (phase, error) = classify(state, reason);
                let finished = match phase {
                    "done" | "cancelled" => true,
                    // Оборванную загрузку оставляем в реестре, только если
                    // движок умеет её продолжить.
                    "failed" => {
                        let mut can = windows_core::BOOL::default();
                        operation.CanResume(&mut can)?;
                        !can.as_bool()
                    }
                    _ => false,
                };
                if finished {
                    if let Some(registry) = registry.upgrade() {
                        registry.ops.borrow_mut().remove(&key);
                    }
                }

                sink(TabEvent::Download {
                    id: tab,
                    key,
                    phase,
                    url: url.clone(),
                    path: path.clone(),
                    bytes,
                    total: (total > 0).then_some(total),
                    error,
                });
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}

/// Состояние движка → фаза для интерфейса и короткий код причины.
fn classify(
    state: COREWEBVIEW2_DOWNLOAD_STATE,
    reason: COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON,
) -> (&'static str, &'static str) {
    if state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
        return ("done", "");
    }
    if state == COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS {
        return ("progress", "");
    }

    let any = |list: &[COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON]| list.contains(&reason);
    if reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED {
        ("paused", "")
    } else if reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED {
        ("cancelled", "")
    } else if reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_SHUTDOWN {
        ("failed", "shutdown")
    } else if reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_NO_SPACE {
        ("failed", "no_space")
    } else if reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_ACCESS_DENIED {
        ("failed", "access_denied")
    } else if any(&[
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_MALICIOUS,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_BLOCKED_BY_POLICY,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_SECURITY_CHECK_FAILED,
    ]) {
        ("failed", "blocked")
    } else if any(&[
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_FAILED,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_TIMEOUT,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_DISCONNECTED,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_SERVER_DOWN,
    ]) {
        ("failed", "network")
    } else if any(&[
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_FAILED,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_FORBIDDEN,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_UNAUTHORIZED,
    ]) {
        ("failed", "server")
    } else {
        ("failed", "failed")
    }
}

fn file_name_of(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "download".into())
}

/// `отчёт.pdf` → `отчёт (1).pdf`, если такой файл уже есть.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string());
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (1..10_000)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|candidate| !candidate.exists())
        .unwrap_or(candidate)
}
