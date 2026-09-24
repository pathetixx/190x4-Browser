//! Загрузки на стороне приложения: запись в базу, события для интерфейса,
//! действия над файлом.
//!
//! Движок сообщает о загрузке своим номером операции (`key`), интерфейс знает
//! загрузку по номеру записи в базе. Связка между ними живёт здесь — так
//! список загрузок переживает и закрытие вкладки, и перезапуск браузера.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use browser190x4_store::{DownloadKind, DownloadState};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;

use crate::state::{with_download, App};

/// В базу — не чаще раза в секунду: интерфейс получает прогресс событиями, а
/// диску хватает редких записей.
const WRITE_EVERY: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct Transfers {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    by_key: HashMap<u64, Entry>,
    key_by_id: HashMap<i64, u64>,
    /// Загрузки через загрузчик видео: номер записи → задание на сервере.
    media_jobs: HashMap<i64, String>,
    /// Файлы, скачанные в приватном окне: записи в базе у них нет, а открыть
    /// файл и показать его в папке из пузыря загрузок всё равно нужно. Живут до
    /// выхода из браузера.
    private_files: HashMap<i64, PathBuf>,
}

struct Entry {
    id: i64,
    written: Instant,
}

/// Загрузка из приватного окна: её нет ни в базе, ни в списке — как в Chrome.
/// Номера у таких загрузок свои, чтобы не путаться с записями базы.
fn private_id(key: u64) -> i64 {
    -(key as i64) - 1
}

/// Событие `download` для интерфейса. Одинаковое для обычных загрузок и
/// загрузчика видео — список загрузок у браузера один.
#[derive(Debug, Clone, Serialize)]
pub struct DownloadEvent {
    pub id: i64,
    pub kind: DownloadKind,
    pub phase: String,
    pub url: String,
    pub path: String,
    pub bytes: i64,
    pub total: Option<i64>,
    pub error: String,
}

impl Transfers {
    pub fn register_media(&self, id: i64, job: String) {
        self.inner.lock().media_jobs.insert(id, job);
    }

    pub fn finish_media(&self, id: i64) {
        self.inner.lock().media_jobs.remove(&id);
    }

    fn media_job(&self, id: i64) -> Option<String> {
        self.inner.lock().media_jobs.get(&id).cloned()
    }
}

type Job = Box<dyn FnOnce() + Send>;

/// Очередь записей о загрузках. Событие движка приходит на главный поток, а
/// запись в базу там — это подвисание интерфейса на каждом куске прогресса.
/// Поток один: события одной загрузки (начало, прогресс, конец) должны лечь в
/// базу в том же порядке, в каком пришли.
fn writer() -> &'static std::sync::mpsc::Sender<Job> {
    static WRITER: std::sync::OnceLock<std::sync::mpsc::Sender<Job>> = std::sync::OnceLock::new();
    WRITER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("downloads-writer".into())
            .spawn(move || {
                for job in rx {
                    job();
                }
            })
            .expect("поток записи загрузок создаётся всегда");
        tx
    })
}

/// Событие движка о загрузке. Зовётся на главном потоке — работа уходит в
/// очередь записи.
#[allow(clippy::too_many_arguments)]
pub fn on_engine_event(
    app: &AppHandle,
    window: &str,
    key: u64,
    phase: &str,
    url: &str,
    path: &str,
    bytes: i64,
    total: Option<i64>,
    error: &str,
) {
    let (app, window, phase, url, path, error) = (
        app.clone(),
        window.to_string(),
        phase.to_string(),
        url.to_string(),
        path.to_string(),
        error.to_string(),
    );
    let _ = writer().send(Box::new(move || {
        apply_engine_event(
            &app, &window, key, &phase, &url, &path, bytes, total, &error,
        )
    }));
}

#[allow(clippy::too_many_arguments)]
fn apply_engine_event(
    app: &AppHandle,
    window: &str,
    key: u64,
    phase: &str,
    url: &str,
    path: &str,
    bytes: i64,
    total: Option<i64>,
    error: &str,
) {
    let state = app.state::<App>();
    let store = &state.store;

    // Приватное окно не оставляет следов: прогресс виден в пузыре загрузок,
    // но в списке и в базе такой загрузки нет.
    if state.windows.is_private(window) {
        let id = private_id(key);
        if phase == "done" && !path.is_empty() {
            state
                .transfers
                .inner
                .lock()
                .private_files
                .insert(id, PathBuf::from(path));
        }
        let event = DownloadEvent {
            id,
            kind: DownloadKind::Web,
            phase: phase.to_string(),
            url: url.to_string(),
            path: path.to_string(),
            bytes,
            total,
            error: error.to_string(),
        };
        // Пузырь загрузок — во всплывающем окне, и оно слушает только свои
        // события: без него прогресс приватной загрузки видела лишь кнопка.
        let _ = app.emit_to(window, "download", event.clone());
        let _ = app.emit_to(crate::popup::label_for(window).as_str(), "download", event);
        return;
    }

    let id = if phase == "started" {
        let id = match store.start_download(DownloadKind::Web, url, path, total) {
            Ok(id) => id,
            Err(err) => {
                tracing::warn!(%err, "загрузка не записана в базу");
                return;
            }
        };
        let mut inner = state.transfers.inner.lock();
        inner.by_key.insert(
            key,
            Entry {
                id,
                written: Instant::now(),
            },
        );
        inner.key_by_id.insert(id, key);
        id
    } else {
        let mut inner = state.transfers.inner.lock();
        let Some(entry) = inner.by_key.get_mut(&key) else {
            tracing::debug!(key, phase, "событие загрузки без записи");
            return;
        };
        let id = entry.id;
        match phase {
            "progress" => {
                if entry.written.elapsed() >= WRITE_EVERY {
                    entry.written = Instant::now();
                    let _ = store.update_download(id, bytes, DownloadState::Running);
                }
            }
            "paused" => {
                let _ = store.update_download(id, bytes, DownloadState::Paused);
            }
            "done" | "cancelled" => {
                let final_state = if phase == "done" {
                    DownloadState::Done
                } else {
                    DownloadState::Cancelled
                };
                let _ = store.finish_download(id, bytes, final_state, "");
                inner.by_key.remove(&key);
                inner.key_by_id.remove(&id);
            }
            _ => {
                // Оборванную загрузку оставляем в связке: её можно продолжить.
                let _ = store.finish_download(id, bytes, DownloadState::Failed, error);
            }
        }
        id
    };

    if phase != "progress" {
        tracing::debug!(key, id, phase, "загрузка");
    }
    let _ = app.emit(
        "download",
        DownloadEvent {
            id,
            kind: DownloadKind::Web,
            phase: phase.to_string(),
            url: url.to_string(),
            path: path.to_string(),
            bytes,
            total,
            error: error.to_string(),
        },
    );
}

/// Событие загрузчика видео — в тот же поток, что и обычные загрузки.
#[allow(clippy::too_many_arguments)]
pub fn emit_media(
    app: &AppHandle,
    id: i64,
    phase: &str,
    url: &str,
    path: &str,
    bytes: i64,
    total: Option<i64>,
    error: &str,
) {
    let _ = app.emit(
        "download",
        DownloadEvent {
            id,
            kind: DownloadKind::Media,
            phase: phase.to_string(),
            url: url.to_string(),
            path: path.to_string(),
            bytes,
            total,
            error: error.to_string(),
        },
    );
}

/// «Всегда спрашивать, куда сохранить»: системный диалог над окном, где
/// началась загрузка, ответ — в движок.
pub fn ask_target(app: &AppHandle, window: &str, key: u64, suggested: String) {
    let app = app.clone();
    let window = window.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let suggested = PathBuf::from(suggested);
        let mut dialog = app.dialog().file().set_title("Сохранить как");
        if let Some(name) = suggested.file_name() {
            dialog = dialog.set_file_name(name.to_string_lossy());
        }
        if let Some(dir) = suggested.parent() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(main) = app.get_webview_window(&window) {
            dialog = dialog.set_parent(&main);
        }
        let target = dialog
            .blocking_save_file()
            .and_then(|path| path.into_path().ok());
        if let Err(err) = with_download(&app, key, move |host| host.download_answer(key, target)) {
            tracing::warn!(%err, "ответ на загрузку не принят");
        }
    });
}

/// Открыть скачанный файл или показать его в папке.
fn reveal_or_open(path: &std::path::Path, action: &str) -> anyhow::Result<()> {
    if action == "open" {
        anyhow::ensure!(path.is_file(), "файл удалён или перемещён");
        tauri_plugin_opener::open_path(path, None::<&str>)?;
    } else if path.exists() {
        tauri_plugin_opener::reveal_item_in_dir(path)?;
    } else if let Some(dir) = path.parent().filter(|dir| dir.is_dir()) {
        tauri_plugin_opener::open_path(dir, None::<&str>)?;
    } else {
        anyhow::bail!("файл удалён или перемещён");
    }
    Ok(())
}

/// Действие из списка загрузок.
pub async fn control(app: &AppHandle, id: i64, action: &str) -> anyhow::Result<()> {
    let state = app.state::<App>();
    // Загрузка приватного окна в базу не попадает, но управлять ею всё равно
    // нужно: её номер — это номер операции движка со знаком минус.
    if id < 0 {
        let key = (-id - 1) as u64;
        if matches!(action, "cancel" | "pause" | "resume") {
            let action = action.to_string();
            return with_download(app, key, move |host| host.download_control(key, &action))
                .map_err(anyhow::Error::msg)?;
        }
        let mut inner = state.transfers.inner.lock();
        let path = match action {
            "remove" => {
                inner.private_files.remove(&id);
                return Ok(());
            }
            "open" | "show" => inner
                .private_files
                .get(&id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("этой загрузки нет в списке"))?,
            other => anyhow::bail!("с загрузкой приватного окна так нельзя: {other}"),
        };
        drop(inner);
        return reveal_or_open(&path, action);
    }
    let download = state
        .store
        .download(id)?
        .ok_or_else(|| anyhow::anyhow!("загрузки нет в списке"))?;
    let path = PathBuf::from(&download.path);

    match action {
        "open" | "show" => reveal_or_open(&path, action)?,
        "cancel" if download.kind == DownloadKind::Media => {
            let job = state
                .transfers
                .media_job(id)
                .ok_or_else(|| anyhow::anyhow!("загрузка уже завершена"))?;
            state.services.media_cancel(&job).await?;
        }
        "cancel" | "pause" | "resume" => {
            anyhow::ensure!(
                download.kind == DownloadKind::Web,
                "загрузку видео можно только отменить"
            );
            let key = state
                .transfers
                .inner
                .lock()
                .key_by_id
                .get(&id)
                .copied()
                .ok_or_else(|| {
                    anyhow::anyhow!("эту загрузку уже не продолжить — начните заново")
                })?;
            let action = action.to_string();
            with_download(app, key, move |host| host.download_control(key, &action))
                .map_err(anyhow::Error::msg)??;
        }
        "retry" => {
            anyhow::ensure!(
                download.kind == DownloadKind::Web,
                "видео скачайте заново через загрузчик"
            );
            // Повтор открывает ссылку фоновой вкладкой в окне, которое сейчас
            // впереди. Ссылка на файл уходит в загрузку, и пустая вкладка
            // закрывается сама (`TabEvent::CloseRequested`); если файла по
            // ссылке больше нет, вкладка останется со страницей сайта — а не
            // уведёт со своего места страницу, открытую у пользователя.
            let url = download.url.clone();
            state.store.remove_download(id)?;
            let label = crate::browser_windows::foreground_label(app);
            crate::state::with_host(app, &label, move |host| host.open(&url).map(|_| ()))
                .map_err(anyhow::Error::msg)??;
        }
        "remove" => {
            anyhow::ensure!(!download.state.is_active(), "сначала отмените загрузку");
            state.store.remove_download(id)?;
        }
        "delete" => {
            anyhow::ensure!(!download.state.is_active(), "сначала отмените загрузку");
            if path.is_file() {
                std::fs::remove_file(&path)?;
            }
            state.store.remove_download(id)?;
        }
        other => anyhow::bail!("неизвестное действие {other}"),
    }

    let _ = app.emit("downloads", ());
    Ok(())
}
