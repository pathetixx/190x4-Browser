//! Окна браузера: обычные и приватные.
//!
//! Одно окно — один `TabHost` со своим контейнером вкладок и своим
//! всплывающим окном, но `ICoreWebView2Environment` у всех общий: иначе
//! каждое окно подняло бы свой процесс движка и вся экономия памяти
//! закончилась бы на втором окне.
//!
//! Приватное окно отличается профилем движка (InPrivate) и тем, что ничего не
//! пишет на диск: ни истории, ни паролей, ни сессии, ни загрузок в список.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

use browser190x4_webview::{TabEvent, TabHost};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WindowEvent};

use crate::state::{self, App};

/// Ярлык первого окна. Он же в `tauri.conf.json` и в capabilities.
pub const FIRST: &str = "chrome";

static NEXT_WINDOW: AtomicU32 = AtomicU32::new(2);

/// Что за окно: по нему интерфейс решает, показывать ли значок приватности, а
/// Rust — писать ли историю и сессию.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    Normal,
    Private,
}

impl WindowKind {
    pub fn is_private(self) -> bool {
        self == WindowKind::Private
    }
}

#[derive(Debug, Clone)]
pub struct WindowMeta {
    pub kind: WindowKind,
    /// Номер окна в сохранённой сессии; у приватных окон сессии нет.
    pub session: Option<i64>,
}

#[derive(Default)]
pub struct WindowRegistry(Mutex<HashMap<String, WindowMeta>>);

impl WindowRegistry {
    pub fn add(&self, label: &str, meta: WindowMeta) {
        self.0.lock().insert(label.to_string(), meta);
    }

    pub fn remove(&self, label: &str) {
        self.0.lock().remove(label);
    }

    pub fn get(&self, label: &str) -> Option<WindowMeta> {
        self.0.lock().get(label).cloned()
    }

    pub fn is_private(&self, label: &str) -> bool {
        self.get(label).is_some_and(|meta| meta.kind.is_private())
    }

    /// Номер окна в сессии — под ним его вкладки лежат в базе.
    pub fn session(&self, label: &str) -> Option<i64> {
        self.get(label).and_then(|meta| meta.session)
    }

    /// Сколько окон браузера открыто (без всплывающих).
    pub fn len(&self) -> usize {
        self.0.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.lock().is_empty()
    }

    pub fn labels(&self) -> Vec<String> {
        self.0.lock().keys().cloned().collect()
    }

    /// Свободный номер сессии: окна нумеруются подряд, дыры занимают новые окна.
    fn free_session(&self) -> i64 {
        let taken: Vec<i64> = self
            .0
            .lock()
            .values()
            .filter_map(|meta| meta.session)
            .collect();
        (0..).find(|index| !taken.contains(index)).unwrap_or(0)
    }
}

/// Сохранённая геометрия окна.
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct Geometry {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub maximized: bool,
}

const GEOMETRY_KEY: &str = "window_geometry";

fn saved_geometry(app: &AppHandle) -> Option<Geometry> {
    let value = app.state::<App>().store.setting(GEOMETRY_KEY).ok()??;
    let geometry: Geometry = serde_json::from_value(value).ok()?;
    (geometry.width >= 400.0 && geometry.height >= 300.0).then_some(geometry)
}

/// Запомнить, каким окно осталось: размер, место и «развёрнуто».
pub fn remember_geometry(app: &AppHandle, label: &str) {
    if app.state::<App>().windows.is_private(label) {
        return;
    }
    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    let Ok(scale) = window.scale_factor() else {
        return;
    };
    let maximized = window.is_maximized().unwrap_or(false);
    if window.is_minimized().unwrap_or(false) {
        return;
    }
    // У развёрнутого окна запоминаем прежний размер: иначе «свернуть в окно»
    // после перезапуска давало бы окно во весь экран.
    let geometry = if maximized {
        let mut geometry = saved_geometry(app).unwrap_or_default();
        geometry.maximized = true;
        if geometry.width < 400.0 || geometry.height < 300.0 {
            geometry.width = 1440.0;
            geometry.height = 900.0;
        }
        geometry
    } else {
        let (Ok(position), Ok(size)) = (window.outer_position(), window.inner_size()) else {
            return;
        };
        let position = position.to_logical::<f64>(scale);
        let size = size.to_logical::<f64>(scale);
        Geometry {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
            maximized: false,
        }
    };
    let _ = app.state::<App>().store.set_setting(
        GEOMETRY_KEY,
        &serde_json::to_value(geometry).unwrap_or_default(),
    );
}

/// Создать окно браузера. Возвращает его ярлык.
pub fn create(app: &AppHandle, kind: WindowKind, first: bool) -> tauri::Result<String> {
    let label = if first {
        FIRST.to_string()
    } else {
        format!("chrome-{}", NEXT_WINDOW.fetch_add(1, Ordering::Relaxed))
    };

    let mut config = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == FIRST)
        .expect("окно chrome описано в tauri.conf.json")
        .clone();
    config.label = label.clone();
    config.create = false;
    if kind.is_private() {
        config.title = "190x4 · приватное окно".into();
    }

    // Размер и место — те, что остались от прошлого раза; каждое следующее
    // окно сеанса встаёт со сдвигом, как в Chrome.
    let geometry = saved_geometry(app);
    if let Some(geometry) = geometry {
        config.width = geometry.width;
        config.height = geometry.height;
        config.center = false;
        let shift = f64::from(app.state::<App>().windows.len() as u32) * 28.0;
        config.x = Some(geometry.x + shift);
        config.y = Some(geometry.y + shift);
    }

    let mut builder = tauri::WebviewWindowBuilder::from_config(app, &config)?;
    if let Some(args) = crate::debug_browser_args() {
        builder = builder.additional_browser_args(&args);
    }
    let window = builder.build()?;

    let session = if kind.is_private() {
        None
    } else {
        Some(app.state::<App>().windows.free_session())
    };
    app.state::<App>()
        .windows
        .add(&label, WindowMeta { kind, session });

    #[cfg(windows)]
    {
        crate::apply_window_icon(&window);
        accept_files(app, &window);
        let handle = app.clone();
        let sink_label = label.clone();
        let hwnd_bits = window.hwnd()?.0 as isize;
        let guard = app.state::<App>().guard.clone();
        let policy = crate::ipc::download_policy(&app.state::<App>().store);
        let pages_dir = app.path().resource_dir().map(|dir| dir.join("pages"));
        let private = kind.is_private();

        // Выполняется на главном потоке — там же, где живёт COM.
        let install_label = label.clone();
        window.with_webview(move |platform| {
            let hwnd = windows::Win32::Foundation::HWND(hwnd_bits as *mut std::ffi::c_void);
            let controller = platform.controller();
            let env = match browser190x4_webview::interop::environment_of(&controller) {
                Ok(env) => env,
                Err(err) => {
                    tracing::error!(%err, "Environment окна не получен");
                    return;
                }
            };

            let sink: browser190x4_webview::tab::EventSink = std::rc::Rc::new({
                let handle = handle.clone();
                move |event: TabEvent| {
                    crate::route_event(&handle, &sink_label, event);
                }
            });

            match TabHost::new(hwnd, env, guard, sink, private) {
                Ok(host) => {
                    if let Ok(dir) = pages_dir {
                        host.set_pages_dir(dir);
                    }
                    host.set_download_policy(policy);
                    host.set_chrome_controller(controller.clone());
                    state::install_host(&install_label, host);
                }
                Err(err) => tracing::error!(%err, "контейнер вкладок не создан"),
            }
        })?;
    }

    wire_window(app, &window);
    if geometry.is_some_and(|geometry| geometry.maximized) {
        let _ = window.maximize();
    }
    window.show()?;

    // Всплывающее окно поднимаем заранее: первое меню не должно ждать запуска
    // ещё одного вебвью.
    let popup_handle = app.clone();
    let popup_label = label.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        if let Err(err) = crate::popup::ensure(&popup_handle, &popup_label) {
            tracing::warn!(%err, "всплывающее окно не создано");
        }
    });

    tracing::info!(%label, private = kind.is_private(), "окно браузера открыто");
    Ok(label)
}

/// Перетаскивание файлов в окно браузера.
///
/// Область страницы занята нативной поверхностью вкладки — туда файл уходит
/// самой странице, как в любом браузере. А вот полоса вкладок, тулбар и панель
/// закладок — наши: брошенный на них файл открывается вкладкой. Tauri для
/// этого не годится: его перехват перетаскивания ломает обычный HTML-drag,
/// которым двигаются вкладки и закладки.
#[cfg(windows)]
fn accept_files(app: &AppHandle, window: &tauri::WebviewWindow) {
    use windows::Win32::UI::Shell::{DragAcceptFiles, SetWindowSubclass};

    struct Target {
        app: AppHandle,
        label: String,
    }

    unsafe extern "system" fn proc(
        hwnd: windows::Win32::Foundation::HWND,
        message: u32,
        wparam: windows::Win32::Foundation::WPARAM,
        lparam: windows::Win32::Foundation::LPARAM,
        _id: usize,
        data: usize,
    ) -> windows::Win32::Foundation::LRESULT {
        // Импорты здесь свои: вложенная функция не видит те, что стоят во
        // внешней.
        use windows::Win32::UI::Shell::{DefSubclassProc, DragFinish, DragQueryFileW, HDROP};
        use windows::Win32::UI::WindowsAndMessaging::{WM_DROPFILES, WM_NCDESTROY};

        if message == WM_DROPFILES && data != 0 {
            let target = unsafe { &*(data as *const Target) };
            let dropped = HDROP(wparam.0 as *mut std::ffi::c_void);
            let count = unsafe { DragQueryFileW(dropped, u32::MAX, None) };
            let mut urls = Vec::new();
            for index in 0..count {
                let mut buffer = [0u16; 1024];
                let length = unsafe { DragQueryFileW(dropped, index, Some(&mut buffer)) } as usize;
                if length == 0 {
                    continue;
                }
                let path = std::path::PathBuf::from(String::from_utf16_lossy(&buffer[..length]));
                if let Ok(url) = tauri::Url::from_file_path(&path) {
                    urls.push(url.to_string());
                }
            }
            unsafe { DragFinish(dropped) };
            if !urls.is_empty() {
                target
                    .app
                    .state::<crate::launch::Launch>()
                    .push_for(&target.label, urls);
                let _ = target.app.emit_to(target.label.as_str(), "launch", ());
            }
            return windows::Win32::Foundation::LRESULT(0);
        }
        if message == WM_NCDESTROY && data != 0 {
            drop(unsafe { Box::from_raw(data as *mut Target) });
        }
        unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
    }

    let Ok(hwnd) = window.hwnd() else { return };
    let target = Box::into_raw(Box::new(Target {
        app: app.clone(),
        label: window.label().to_string(),
    }));
    unsafe {
        DragAcceptFiles(hwnd, true);
        let _ = SetWindowSubclass(hwnd, Some(proc), 190, target as usize);
    }
}

/// Подписки окна: попап ездит за окном, состояние кнопок и закрытие.
fn wire_window(app: &AppHandle, window: &tauri::WebviewWindow) {
    let handle = app.clone();
    let main = window.clone();
    let label = window.label().to_string();
    window.on_window_event(move |event| match event {
        WindowEvent::Moved(_) => {
            crate::popup::main_moved(&handle, &label);
            remember_geometry(&handle, &label);
        }
        #[cfg(windows)]
        WindowEvent::ScaleFactorChanged { .. } => crate::apply_window_icon(&main),
        WindowEvent::Resized(_) => {
            crate::popup::hide(&handle, &label);
            let maximized = main.is_maximized().unwrap_or(false);
            let minimized = main.is_minimized().unwrap_or(false);
            let _ = handle.emit_to(
                label.as_str(),
                "window-state",
                serde_json::json!({ "maximized": maximized, "minimized": minimized }),
            );
            remember_geometry(&handle, &label);
        }
        WindowEvent::CloseRequested { .. } => closing(&handle, &label),
        WindowEvent::Destroyed => {
            state::remove_host(&label);
            handle.state::<App>().windows.remove(&label);
            crate::popup::destroy(&handle, &label);
            // Последнее окно браузера закрыли: всплывающие окна сами приложение
            // на плаву не держат.
            if handle.state::<App>().windows.is_empty() {
                handle.exit(0);
            }
        }
        _ => {}
    });
}

/// Окно закрывается: запомнить геометрию и записать его сессию, пока вкладки
/// ещё живы. Отложенная запись из интерфейса сюда уже не успеет.
fn closing(app: &AppHandle, label: &str) {
    remember_geometry(app, label);
    let state = app.state::<App>();
    let Some(session) = state.windows.session(label) else {
        return;
    };
    let tabs = state
        .sessions
        .lock()
        .get(label)
        .cloned()
        .unwrap_or_default();
    if let Err(err) = state.store.save_session(session, &tabs) {
        tracing::warn!(%err, "сессия окна не сохранена");
    }
}

/// Окно, которому отдать ссылку из другой программы: то, что сейчас впереди,
/// иначе первое обычное. В приватное окно чужие ссылки не уходят — его
/// содержимое не должно смешиваться с обычной работой.
pub fn foreground_label(app: &AppHandle) -> String {
    let registry = &app.state::<App>().windows;
    let normal: Vec<String> = registry
        .labels()
        .into_iter()
        .filter(|label| !registry.is_private(label))
        .collect();
    let focused = normal.iter().find(|label| {
        app.get_webview_window(label)
            .and_then(|window| window.is_focused().ok())
            .unwrap_or(false)
    });
    focused
        .cloned()
        .or_else(|| normal.first().cloned())
        .unwrap_or_else(|| FIRST.to_string())
}

/// Открыть новое окно браузера по требованию интерфейса.
pub fn open(app: &AppHandle, kind: WindowKind, url: Option<String>) -> tauri::Result<String> {
    let label = create(app, kind, false)?;
    if let Some(url) = url {
        // Интерфейс нового окна ещё грузится: адрес ждёт в очереди запуска,
        // её окно заберёт после восстановления сессии.
        app.state::<crate::launch::Launch>()
            .push_for(&label, vec![url]);
    }
    Ok(label)
}
