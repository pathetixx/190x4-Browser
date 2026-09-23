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

use browser190x4_store::SessionTab;
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
    /// Окно прошлого сеанса: его вкладки восстанавливаются. Новое окно
    /// (Ctrl+N, вкладка в новое окно) начинается с чистого листа, даже если
    /// под его номером в базе что-то осталось.
    pub restore: bool,
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

    /// Номер сессии, которую окно восстанавливает при открытии.
    pub fn restored_session(&self, label: &str) -> Option<i64> {
        self.get(label)
            .filter(|meta| meta.restore)
            .and_then(|meta| meta.session)
    }

    /// Открыты ли обычные окна, кроме этого.
    fn others_normal(&self, label: &str) -> bool {
        self.0
            .lock()
            .iter()
            .any(|(other, meta)| other != label && !meta.kind.is_private())
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

/// Последняя геометрия окна. Движение и ресайз приходят десятками в секунду на
/// главный поток — писать каждое в базу значит подвешивать перетаскивание окна.
static LAST_GEOMETRY: Mutex<Option<Geometry>> = Mutex::new(None);

/// Записать запомненную геометрию в базу: при закрытии окна и выходе.
pub fn save_geometry(app: &AppHandle) {
    if let Some(geometry) = *LAST_GEOMETRY.lock() {
        let _ = app.state::<App>().store.set_setting(
            GEOMETRY_KEY,
            &serde_json::to_value(geometry).unwrap_or_default(),
        );
    }
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
        let last = *LAST_GEOMETRY.lock();
        let mut geometry = last.or_else(|| saved_geometry(app)).unwrap_or_default();
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
    *LAST_GEOMETRY.lock() = Some(geometry);
}

/// Видно ли сохранённое место окна хотя бы на одном мониторе. Монитор, на
/// котором окно было в прошлый раз, могли отключить — и окно открылось бы за
/// краем экрана, где его не достать.
fn on_screen(app: &AppHandle, geometry: &Geometry) -> bool {
    let monitors = app.available_monitors().unwrap_or_default();
    if monitors.is_empty() {
        return true;
    }
    monitors.iter().any(|monitor| {
        let scale = monitor.scale_factor();
        let position = monitor.position().to_logical::<f64>(scale);
        let size = monitor.size().to_logical::<f64>(scale);
        // Заголовок окна должен быть на экране: за него окно перетаскивают.
        let left = geometry.x.max(position.x);
        let right = (geometry.x + geometry.width).min(position.x + size.width);
        right - left >= 120.0
            && geometry.y >= position.y - 16.0
            && geometry.y <= position.y + size.height - 48.0
    })
}

/// Создать окно браузера. Возвращает его ярлык. `restore` — окно прошлого
/// сеанса: оно восстановит вкладки, сохранённые под его номером.
pub fn create(
    app: &AppHandle,
    kind: WindowKind,
    first: bool,
    restore: bool,
) -> tauri::Result<String> {
    let opening = Opening {
        first,
        restore,
        ..Opening::default()
    };
    create_window(app, kind, opening)
}

/// С чем открывается окно.
#[derive(Default)]
struct Opening<'a> {
    /// Первое окно запуска — ярлык `chrome`.
    first: bool,
    /// Окно прошлого сеанса: восстановит вкладки своей сессии.
    restore: bool,
    /// Вкладки закрытого окна: пишутся в сессию нового окна до того, как его
    /// интерфейс её прочтёт, — так возвращается окно, закрытое крестиком.
    tabs: Option<&'a [SessionTab]>,
    /// Где встать (логические пиксели экрана): вкладку вытащили из строки и
    /// отпустили здесь.
    at: Option<(f64, f64)>,
    /// Вкладка, которая переедет в окно живой, когда интерфейс её попросит.
    adopt: Option<u32>,
}

fn create_window(app: &AppHandle, kind: WindowKind, opening: Opening) -> tauri::Result<String> {
    let label = if opening.first {
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
    // Место, оставшееся за отключённым монитором, не годится: окно встаёт по
    // центру экрана с прежним размером.
    let geometry = saved_geometry(app);
    if let Some(geometry) = geometry {
        config.width = geometry.width;
        config.height = geometry.height;
        if on_screen(app, &geometry) {
            config.center = false;
            let shift = f64::from(app.state::<App>().windows.len() as u32) * 28.0;
            config.x = Some(geometry.x + shift);
            config.y = Some(geometry.y + shift);
        }
    }
    // Вкладку вытащили из строки: окно встаёт там, где её отпустили.
    if let Some((x, y)) = opening.at {
        config.center = false;
        config.x = Some(x);
        config.y = Some(y);
    }
    if let Some(tab) = opening.adopt {
        app.state::<crate::launch::Launch>().push_adopt(&label, tab);
    }

    let window = tauri::WebviewWindowBuilder::from_config(app, &config)?
        .additional_browser_args(crate::browser_args())
        .build()?;

    let session = if kind.is_private() {
        None
    } else {
        Some(app.state::<App>().windows.free_session())
    };
    if let (Some(session), Some(tabs)) = (session, opening.tabs) {
        if let Err(err) = app.state::<App>().store.save_session(session, tabs) {
            tracing::warn!(%err, "вкладки закрытого окна не записаны");
        }
    }
    app.state::<App>().windows.add(
        &label,
        WindowMeta {
            kind,
            session,
            restore: opening.restore || opening.tabs.is_some(),
        },
    );

    #[cfg(windows)]
    {
        crate::apply_window_icon(&window);
        accept_files(app, &window);
        let handle = app.clone();
        let sink_label = label.clone();
        let hwnd_bits = window.hwnd()?.0 as isize;
        let guard = app.state::<App>().guard.clone();
        let policy = crate::ipc::download_policy(&app.state::<App>().store);
        let page_color = crate::ipc::page_color(&app.state::<App>().store);
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
                    host.set_page_color(page_color);
                    host.set_chrome_controller(controller.clone());
                    state::install_host(&install_label, host);
                }
                Err(err) => tracing::error!(%err, "контейнер вкладок не создан"),
            }
        })?;
    }

    wire_window(app, &window);
    // Окно под вытащенную вкладку не разворачивается, как в Chrome.
    if opening.at.is_none() && geometry.is_some_and(|geometry| geometry.maximized) {
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
    let hwnd_bits = hwnd.0 as isize;
    let target = Target {
        app: app.clone(),
        label: window.label().to_string(),
    };
    // Подкласс окна ставится только с того потока, которому окно принадлежит, —
    // а новое окно создаёт команда из пула.
    let _ = app.run_on_main_thread(move || {
        let hwnd = windows::Win32::Foundation::HWND(hwnd_bits as *mut std::ffi::c_void);
        let target = Box::into_raw(Box::new(target));
        unsafe {
            DragAcceptFiles(hwnd, true);
            if !SetWindowSubclass(hwnd, Some(proc), 190, target as usize).as_bool() {
                drop(Box::from_raw(target));
                tracing::warn!("перетаскивание файлов в окно не подключено");
            }
        }
    });
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
        WindowEvent::CloseRequested { api, .. } => {
            let force = take_mark(&CLOSING_ANYWAY, &label);
            // Сначала страницы: каждая может попросить «Покинуть сайт?», как при
            // закрытии вкладки. Спрашивает интерфейс, ответ — `close_asked`.
            if !force && !pages_asked(&label) {
                api.prevent_close();
                let _ = handle.emit_to(label.as_str(), "close-asked", ());
                return;
            }
            // Закрытие окна обрывает его загрузки — как Chrome, сначала спросить.
            let downloads = state::active_downloads(&label);
            if !force && downloads > 0 {
                api.prevent_close();
                let _ = handle.emit_to(
                    label.as_str(),
                    "close-blocked",
                    serde_json::json!({ "downloads": downloads }),
                );
                return;
            }
            closing(&handle, &label);
        }
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

/// Окно закрывается: запомнить геометрию и решить, что будет с его вкладками.
///
/// Как в Chrome: окно, закрытое крестиком, пока открыты другие, при следующем
/// запуске не возвращается — его сессия забывается. Последнее обычное окно
/// записывает свою сессию, пока вкладки ещё живы: отложенная запись из
/// интерфейса сюда уже не успеет. «Закрыть браузер» из меню закрывает все окна
/// разом, мимо этого обработчика, и они все вернутся.
fn closing(app: &AppHandle, label: &str) {
    remember_geometry(app, label);
    save_geometry(app);
    let state = app.state::<App>();
    let tabs = state.sessions.lock().remove(label).unwrap_or_default();
    let Some(session) = state.windows.session(label) else {
        return;
    };
    let result = if state.windows.others_normal(label) {
        // При запуске такое окно не вернётся, но до выхода его возвращает
        // Ctrl+Shift+T — как в Chrome.
        remember_closed(app, tabs);
        state.store.forget_session(session)
    } else {
        state.store.save_session(session, &tabs)
    };
    if let Err(err) = result {
        tracing::warn!(%err, "сессия окна не записана");
    }
}

/// Окна, которые закрывают, несмотря на идущие загрузки: человек подтвердил.
static CLOSING_ANYWAY: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Окна, чьи страницы уже согласились закрыться («Покинуть сайт?» пройден).
static PAGES_ASKED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Когда страницы окна спросили в последний раз. Интерфейс, который не
/// ответил (завис, упал), не должен держать окно: повторное закрытие позже
/// этого срока закрывает без вопроса страницам.
static ASKED_AT: Mutex<Vec<(String, std::time::Instant)>> = Mutex::new(Vec::new());
const ASK_PATIENCE: std::time::Duration = std::time::Duration::from_secs(8);

/// Снять отметку окна; `true` — она была.
fn take_mark(marks: &Mutex<Vec<String>>, label: &str) -> bool {
    let mut marks = marks.lock();
    let before = marks.len();
    marks.retain(|other| other != label);
    marks.len() != before
}

/// Спрошены ли уже страницы окна. Нет — запомнить, что спрашиваем сейчас.
fn pages_asked(label: &str) -> bool {
    if take_mark(&PAGES_ASKED, label) {
        ASKED_AT.lock().retain(|(other, _)| other != label);
        return true;
    }
    let mut asked = ASKED_AT.lock();
    let now = std::time::Instant::now();
    match asked.iter().position(|(other, _)| other == label) {
        Some(index) if now.duration_since(asked[index].1) >= ASK_PATIENCE => {
            asked.remove(index);
            true
        }
        Some(_) => false,
        None => {
            asked.push((label.to_string(), now));
            false
        }
    }
}

/// Закрыть окно без вопроса о загрузках — на него уже ответили «Закрыть».
pub fn close_anyway(app: &AppHandle, label: &str) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window(label) else {
        return Ok(());
    };
    CLOSING_ANYWAY.lock().push(label.to_string());
    window.close()
}

/// Страницы окна согласились закрыться — дальше вопрос только о загрузках.
pub fn close_asked(app: &AppHandle, label: &str) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window(label) else {
        return Ok(());
    };
    PAGES_ASKED.lock().push(label.to_string());
    window.close()
}

/// Сколько окон, закрытых крестиком, браузер помнит до выхода.
const CLOSED_WINDOWS: usize = 10;

/// Запомнить вкладки закрытого окна: Ctrl+Shift+T вернёт его целиком.
fn remember_closed(app: &AppHandle, tabs: Vec<SessionTab>) {
    if tabs.is_empty() {
        return;
    }
    let state = app.state::<App>();
    let count = {
        let mut closed = state.closed_windows.lock();
        closed.push(tabs);
        if closed.len() > CLOSED_WINDOWS {
            closed.remove(0);
        }
        closed.len()
    };
    let _ = app.emit("closed-windows", count);
}

/// Сколько закрытых окон можно вернуть.
pub fn closed_count(app: &AppHandle) -> usize {
    app.state::<App>().closed_windows.lock().len()
}

/// Забрать последнее закрытое окно — его вкладки откроет [`reopen`].
pub fn take_closed(app: &AppHandle) -> Option<Vec<SessionTab>> {
    let state = app.state::<App>();
    let (tabs, count) = {
        let mut closed = state.closed_windows.lock();
        (closed.pop()?, closed.len())
    };
    let _ = app.emit("closed-windows", count);
    Some(tabs)
}

/// Вернуть закрытое окно: вкладки встают спящими, как после перезапуска.
pub fn reopen(app: &AppHandle, tabs: &[SessionTab]) -> tauri::Result<String> {
    let opening = Opening {
        restore: true,
        tabs: Some(tabs),
        ..Opening::default()
    };
    create_window(app, WindowKind::Normal, opening)
}

/// Окно под вытащенную из строки вкладку: встаёт у точки, где её отпустили, и
/// забирает вкладку живой, когда его интерфейс будет готов.
pub fn open_for_tab(
    app: &AppHandle,
    kind: WindowKind,
    tab: u32,
    at: Option<(f64, f64)>,
) -> tauri::Result<String> {
    let opening = Opening {
        at,
        adopt: Some(tab),
        ..Opening::default()
    };
    create_window(app, kind, opening)
}

/// Обычное окно, если оно открыто: то, что впереди, иначе первое попавшееся.
pub fn normal_label(app: &AppHandle) -> Option<String> {
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
    focused.cloned().or_else(|| normal.first().cloned())
}

/// Окно, которому отдать ссылку из другой программы: то, что сейчас впереди,
/// иначе первое обычное. В приватное окно чужие ссылки не уходят — его
/// содержимое не должно смешиваться с обычной работой.
pub fn foreground_label(app: &AppHandle) -> String {
    normal_label(app).unwrap_or_else(|| FIRST.to_string())
}

/// Открыть новое окно браузера по требованию интерфейса.
pub fn open(app: &AppHandle, kind: WindowKind, url: Option<String>) -> tauri::Result<String> {
    let label = create(app, kind, false, false)?;
    if let Some(url) = url {
        // Интерфейс нового окна ещё грузится: адрес ждёт в очереди запуска,
        // её окно заберёт после восстановления сессии.
        app.state::<crate::launch::Launch>()
            .push_for(&label, vec![url]);
    }
    Ok(label)
}
