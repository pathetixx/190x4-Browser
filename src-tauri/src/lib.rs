//! 190x4 Browser — точка сборки.
//!
//! Разделение ролей:
//! * `ui/` — chrome браузера, обычная веб-страница в Tauri-вебвью. Только она
//!   (и её всплывающее окно) имеет доступ к командам из [`ipc`];
//! * [`browser190x4_webview`] — вкладки поверх того же HWND, из того же Environment;
//! * [`browser190x4_adblock`] — сетевой фильтр на горячем пути WebResourceRequested.
//!
//! Окон браузера может быть несколько (обычные и приватные), у каждого свой
//! хост вкладок — см. [`browser_windows`].

mod autoscroll;
mod browser_windows;
mod default_browser;
mod external;
mod filters;
mod hello;
mod import;
pub mod ipc;
mod launch;
mod newtab;
mod passwords;
mod popup;
mod resources;
mod site_icons;
mod sponsorblock;
mod state;
mod suggest;
mod transfers;
mod updates;
mod vault;
mod watchdog;
mod weather;

use std::sync::Arc;

use browser190x4_adblock::Guard;
use browser190x4_services::{Services, ServicesConfig};
use browser190x4_store::Store;
use tauri::{Emitter, Manager};

use browser_windows::WindowKind;
use state::App;

/// Перезапуск после падения движка: новый процесс получает номер старого и
/// ждёт, пока тот выйдет, — иначе застал бы его живым и, как повторный запуск,
/// отдал бы ему свои адреса.
const WAIT_PID: &str = "BROWSER190X4_WAIT_PID";

/// Когда запущен процесс: падение движка в первые секунды перезапуском не
/// лечится, а только пошло бы по кругу.
static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

pub fn run() {
    let _ = STARTED.get_or_init(std::time::Instant::now);
    // Установщик запускает exe только ради регистрации в Windows.
    if let Some(code) = default_browser::installer_flag() {
        std::process::exit(code);
    }
    #[cfg(windows)]
    wait_for_previous();

    // Повторный запуск передаёт адреса первому процессу и выходит (плагин
    // single-instance). Профиль ему не трогать: лог открывается с обрезкой, а
    // незавершённые загрузки при старте помечаются прерванными.
    let separate = separate_profile();
    if let Some(dir) = &separate {
        // Данные движка — тоже при отдельном профиле, а не в общей папке.
        if std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_none() {
            std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", dir.join("EBWebView"));
        }
    }
    let secondary = separate.is_none() && launch::already_running();
    if secondary {
        launch::allow_foreground();
    } else {
        init_logging();
        if separate.is_none() {
            migrate_profile();
        }
    }

    let guard = Arc::new(Guard::empty());
    let store = Arc::new(open_store());
    let services = Arc::new(open_services());
    let launched = launch::Launch::default();
    launched.push(launch::from_command_line());

    // Цвет, которым движок заливает новый вебвью до первой отрисовки: без него
    // первый кадр каждой новой вкладки был бы белым или серым.
    ipc::apply_engine_background(&store);
    init_browser_args(&store);
    #[cfg(windows)]
    browser190x4_webview::tab::set_reputation_checking(store.setting_bool("smartscreen", true));
    guard.set_enabled(store.setting_bool("adblock_enabled", true));
    guard.set_exempt_sites(ipc::exempt_sites(&store));
    if !secondary {
        if let Err(err) = store.fail_interrupted_downloads() {
            tracing::warn!(%err, "незавершённые загрузки не отмечены");
        }
    }

    let mut builder = tauri::Builder::default();
    if separate.is_none() {
        // Первым: второй процесс должен выйти раньше, чем проснутся другие плагины.
        builder = builder.plugin(tauri_plugin_single_instance::init(
            launch::on_second_instance,
        ));
    }
    builder
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(App {
            guard: guard.clone(),
            store: store.clone(),
            services: services.clone(),
            passwords: Default::default(),
            transfers: Default::default(),
            popup: Default::default(),
            external: Default::default(),
            windows: Default::default(),
            engine: parking_lot::RwLock::new(
                store
                    .setting_str("search_engine")
                    .unwrap_or_else(|| "duckduckgo".into()),
            ),
            sessions: Default::default(),
            closed_windows: Default::default(),
        })
        .manage(updates::Updates::default())
        .manage(newtab::NewTab::default())
        .manage(sponsorblock::SponsorBlock::default())
        .manage(launched)
        .invoke_handler(tauri::generate_handler![
            ipc::tab_open,
            ipc::tab_popup_deny,
            ipc::tab_prewarm,
            ipc::tab_close_request,
            ipc::tab_close,
            ipc::tab_activate,
            ipc::tab_split,
            ipc::tab_navigate,
            ipc::tab_action,
            ipc::tab_post,
            ipc::tab_context_menu,
            ipc::tab_dialog,
            ipc::tab_mute,
            ipc::tab_zoom_set,
            ipc::tab_find,
            ipc::tab_find_step,
            ipc::layout_set,
            ipc::overlay_set,
            ipc::window_command,
            ipc::window_state,
            ipc::window_info,
            ipc::window_open,
            ipc::window_reopen_closed,
            ipc::window_move,
            ipc::tab_to_window,
            ipc::tab_adopt,
            ipc::tab_suspend,
            ipc::tab_history,
            ipc::tab_history_go,
            ipc::tab_selection,
            ipc::app_quit,
            ipc::chrome_focus,
            ipc::adblock_stats,
            ipc::adblock_set_enabled,
            ipc::adblock_lists,
            ipc::adblock_site,
            ipc::adblock_site_set,
            ipc::settings_get,
            ipc::settings_set,
            ipc::history_record,
            ipc::history_title,
            ipc::history_recent,
            ipc::history_search,
            ipc::history_page,
            ipc::history_forget,
            ipc::history_forget_visit,
            ipc::history_clear,
            ipc::history_clear_period,
            ipc::search_suggest,
            ipc::site_icon,
            ipc::bookmarks_tree,
            ipc::bookmark_find,
            ipc::bookmark_add,
            ipc::bookmark_folder_add,
            ipc::bookmark_update,
            ipc::bookmark_move,
            ipc::bookmark_remove,
            ipc::bookmark_remove_url,
            ipc::bookmarks_import,
            ipc::bookmarks_export,
            ipc::passwords_list,
            ipc::password_reveal,
            ipc::password_add,
            ipc::password_update,
            ipc::password_delete,
            ipc::passwords_import,
            ipc::passwords_export,
            ipc::password_never_list,
            ipc::password_never_forget,
            ipc::password_offer_answer,
            ipc::password_fill,
            ipc::passwords_for_site,
            ipc::session_save,
            ipc::session_restore,
            ipc::downloads_list,
            ipc::download_control,
            ipc::downloads_clear,
            ipc::downloads_folder_open,
            ipc::download_folder_pick,
            ipc::browsing_data_clear,
            ipc::site_permissions,
            ipc::site_permission_reset,
            ipc::zoom_sites,
            ipc::zoom_site_set,
            ipc::about_info,
            ipc::profile_open,
            ipc::popup_open,
            ipc::popup_pending,
            ipc::popup_show,
            ipc::popup_resize,
            ipc::popup_hide,
            ipc::translate_text,
            ipc::media_probe,
            ipc::media_download,
            ipc::media_cancel,
            ipc::services_state,
            updates::update_check,
            updates::update_state,
            updates::update_install,
            launch::launch_take,
            launch::launch_adopt_take,
            default_browser::default_browser_state,
            default_browser::default_browser_set,
            import::browsers_found,
            import::browser_import,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            // Сторож снимает хэндл главного потока — поэтому первым и отсюда.
            watchdog::spawn(handle.clone());
            updates::spawn_checker(handle.clone());
            filters::spawn(handle.clone());
            rebuild_filter(guard.clone(), store.clone(), handle.clone());

            // Первое окно — всегда; остальные поднимаются, если в прошлый раз
            // их было больше и пользователь просил восстанавливать сессию.
            // Окна получают номера сессий подряд, а в базе после закрытых окон
            // бывают дыры — номера выравниваются до того, как окна спросят.
            if restores_session(&store) {
                if let Err(err) = store.compact_sessions() {
                    tracing::warn!(%err, "сессии окон не пронумерованы");
                }
            }
            browser_windows::create(&handle, WindowKind::Normal, true, true)?;
            restore_windows(&handle, &store);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("не удалось запустить 190x4 Browser")
        .run(|handle, event| {
            // Выход: в базе остаются сессии только тех окон, что были открыты.
            if let tauri::RunEvent::ExitRequested { .. } = event {
                browser_windows::save_geometry(handle);
                let state = handle.state::<App>();
                let alive: Vec<i64> = state
                    .windows
                    .labels()
                    .iter()
                    .filter_map(|label| state.windows.session(label))
                    .collect();
                if !alive.is_empty() {
                    let _ = state.store.keep_sessions(&alive);
                }
            }
        });
}

/// Восстанавливать ли вкладки прошлого сеанса (настройка «При запуске»).
fn restores_session(store: &Store) -> bool {
    store.setting_str("startup").as_deref().unwrap_or("restore") == "restore"
}

/// Второе и следующие окна прошлого сеанса. Первое уже создано, его вкладки
/// восстановит интерфейс сам.
fn restore_windows(app: &tauri::AppHandle, store: &Store) {
    if !restores_session(store) {
        return;
    }
    let saved = store.session_windows().unwrap_or_default();
    for _ in saved.iter().skip(1) {
        if let Err(err) = browser_windows::create(app, WindowKind::Normal, false, true) {
            tracing::warn!(%err, "окно прошлого сеанса не открылось");
            break;
        }
    }
}

/// Разводка событий вкладок.
///
/// Часть событий chrome-у не нужна вовсе или нужна уже обработанной:
/// загрузки сначала попадают в базу, пароли не должны покидать Rust. Ярлык
/// окна нужен, чтобы событие ушло именно в то окно, где живёт вкладка.
#[cfg(windows)]
pub(crate) fn route_event(
    app: &tauri::AppHandle,
    label: &str,
    event: browser190x4_webview::TabEvent,
) {
    use browser190x4_webview::{DialogRequest, TabEvent};

    match &event {
        TabEvent::Download {
            key,
            phase,
            url,
            path,
            bytes,
            total,
            error,
            ..
        } => {
            transfers::on_engine_event(app, label, *key, phase, url, path, *bytes, *total, error);
            return;
        }
        TabEvent::DownloadAsk { key, path, .. } => {
            transfers::ask_target(app, label, *key, path.clone());
            return;
        }
        TabEvent::Message {
            id,
            frame,
            source,
            payload,
        } => {
            // `postMessage` доступен любой странице, и разбирается он здесь, на
            // главном потоке — том же, что рисует окна. Свои сообщения короткие
            // (самое длинное — плитки новой вкладки), большие не разбираем вовсе.
            if payload.len() > MAX_PAGE_MESSAGE {
                return;
            }
            // Фреймам доступны только менеджер паролей и SponsorBlock (плеер
            // YouTube, встроенный в чужую страницу): новая вкладка и chrome
            // принимают сообщения лишь от документа вкладки.
            if passwords::handle_message(app, label, *id, *frame, source, payload)
                || sponsorblock::handle_message(app, *id, *frame, source, payload)
                || autoscroll::handle_message(app, *id, *frame, source, payload)
                || frame.is_some()
                || newtab::handle_message(app, *id, source, payload)
                || !for_interface(payload)
            {
                return;
            }
        }
        // Ссылку на приложение открывает браузер: спросить или открыть сразу.
        TabEvent::Dialog {
            id,
            token,
            request:
                DialogRequest::External {
                    uri,
                    origin,
                    user_initiated,
                },
        } => {
            external::on_request(app, label, *id, *token, uri, origin, *user_initiated);
            return;
        }
        // Упал весь движок: вместе с вкладками умер и интерфейс окон.
        TabEvent::Crashed { what, .. } if *what == "browser" => {
            relaunch_after_engine_crash(app);
            return;
        }
        // Сайт открыли с неверным сертификатом: движок помнит это решение до
        // выхода для всех окон профиля, значит, и помечать его надо во всех.
        TabEvent::Insecure { host, .. } => {
            let _ = app.emit("insecure-host", host);
        }
        TabEvent::Started { id, .. } => {
            passwords::on_navigation(app, *id);
            // Ссылки на приложения прежней страницы больше никто не откроет.
            external::forget_tab(&app.state::<App>(), *id);
        }
        // Приватное окно закладкам значков не пишет, а отсутствие значка не
        // стирает тот, что у закладки уже есть.
        TabEvent::Favicon { page, url, .. }
            if !url.is_empty() && !app.state::<App>().windows.is_private(label) =>
        {
            // Запись в базу — не на главном потоке: событие приходит с него.
            let (app, page, url) = (app.clone(), page.clone(), url.clone());
            tauri::async_runtime::spawn_blocking(move || {
                let state = app.state::<App>();
                if state.store.set_bookmark_icon(&page, &url).unwrap_or(false) {
                    let _ = app.emit("bookmarks", ());
                }
            });
        }
        _ => {}
    }
    let _ = app.emit_to(label, "tab", &event);
}

/// Самое длинное сообщение страницы, которое браузер разбирает.
#[cfg(windows)]
const MAX_PAGE_MESSAGE: usize = 64 * 1024;

/// Сообщение страницы, которое ждёт интерфейс окна (`handlePageMessage` в
/// `ui/js/main.js`). Остальные туда не идут: страница, шлющая `postMessage` в
/// цикле, иначе загружала бы интерфейс браузера событиями.
#[cfg(windows)]
fn for_interface(payload: &str) -> bool {
    #[derive(serde::Deserialize)]
    struct Message<'a> {
        #[serde(borrow)]
        evt: std::borrow::Cow<'a, str>,
    }
    serde_json::from_str::<Message>(payload).is_ok_and(|message| {
        matches!(
            message.evt.as_ref(),
            "navigate" | "middle_click" | "media_found"
        )
    })
}

/// Движок WebView2 упал целиком: окна пусты и не отвечают, вернуть их можно
/// только новым процессом — вкладки придут из сессии в базе. Один раз за
/// процесс и не в первые полминуты работы.
#[cfg(windows)]
fn relaunch_after_engine_crash(app: &tauri::AppHandle) {
    use std::sync::atomic::{AtomicBool, Ordering};

    static ONCE: AtomicBool = AtomicBool::new(false);
    if ONCE.swap(true, Ordering::SeqCst) {
        return;
    }
    let uptime = STARTED.get().map(|at| at.elapsed()).unwrap_or_default();
    if uptime < std::time::Duration::from_secs(30) {
        tracing::error!(?uptime, "движок WebView2 упал сразу после запуска");
        return;
    }
    tracing::error!("движок WebView2 упал — браузер перезапускается");
    let spawned = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .env(WAIT_PID, std::process::id().to_string())
            .spawn()
    });
    match spawned {
        Ok(_) => app.exit(0),
        Err(err) => tracing::error!(%err, "браузер не перезапущен"),
    }
}

/// Процесс, перезапущенный после падения движка, ждёт выхода прежнего.
#[cfg(windows)]
fn wait_for_previous() {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };

    let pid = std::env::var(WAIT_PID).ok();
    std::env::remove_var(WAIT_PID);
    let Some(pid) = pid.and_then(|pid| pid.parse::<u32>().ok()) else {
        return;
    };
    unsafe {
        if let Ok(process) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(process, 10_000);
            let _ = CloseHandle(process);
        }
    }
}

/// Аргументы движка — одни на все вебвью процесса: окна и их попапы делят
/// окружение WebView2, а вебвью с другими аргументами движок не создаст.
/// Читаются из настроек один раз при запуске.
static BROWSER_ARGS: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Без своих аргументов wry выключает SmartScreen и разрешает сайтам включать
/// звук без щелчка. SmartScreen в движке включён, а вкладкам его включает и
/// выключает настройка `smartscreen`. Звук — как в Chrome: только после
/// действия человека на странице, если он не разрешил автозапуск сам
/// (`media_autoplay`, действует после перезапуска).
fn init_browser_args(store: &Store) {
    let autoplay = if store.setting_bool("media_autoplay", false) {
        "no-user-gesture-required"
    } else {
        "document-user-activation-required"
    };
    let debug = debug_port()
        .map(|port| format!(" --remote-debugging-port={port}"))
        .unwrap_or_default();
    let features = "--disable-features=msWebOOUI,msPdfOOUI";
    let _ = BROWSER_ARGS.set(format!("{features} --autoplay-policy={autoplay}{debug}"));
}

/// Аргументы движка для нового окна или попапа (см. [`init_browser_args`]).
pub(crate) fn browser_args() -> &'static str {
    BROWSER_ARGS
        .get()
        .map_or("--disable-features=msWebOOUI,msPdfOOUI", String::as_str)
}

/// Порт отладки движка (CDP) — только при отдельном профиле и заданном
/// `BROWSER190X4_DEBUG_PORT`.
fn debug_port() -> Option<u16> {
    separate_profile()?;
    let port = std::env::var("BROWSER190X4_DEBUG_PORT").ok()?;
    port.parse().ok()
}

/// Значок окна для панели задач и Alt+Tab — из ресурсов exe, нужного размера.
///
/// Tauri ставит окну одну картинку, и Windows растягивала её под панель задач.
/// В ресурсах exe лежат все размеры `icon.ico` (tauri-build кладёт иконку под
/// номером 32512), и `LoadImageW` берёт нарисованный под текущий DPI.
#[cfg(windows)]
pub(crate) fn apply_window_icon(window: &tauri::WebviewWindow) {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HINSTANCE, LPARAM, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows::Win32::UI::WindowsAndMessaging::{
        LoadImageW, SendMessageW, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, SM_CXICON,
        SM_CXSMICON, WM_SETICON,
    };

    const APP_ICON: u16 = 32512;
    let Ok(hwnd) = window.hwnd() else { return };
    unsafe {
        let Ok(module) = GetModuleHandleW(None) else {
            return;
        };
        let dpi = GetDpiForWindow(hwnd);
        for (kind, metric) in [(ICON_BIG, SM_CXICON), (ICON_SMALL, SM_CXSMICON)] {
            let size = GetSystemMetricsForDpi(metric, dpi);
            match LoadImageW(
                Some(HINSTANCE(module.0)),
                PCWSTR(APP_ICON as usize as *const u16),
                IMAGE_ICON,
                size,
                size,
                LR_DEFAULTCOLOR,
            ) {
                Ok(icon) => {
                    SendMessageW(
                        hwnd,
                        WM_SETICON,
                        Some(WPARAM(kind as usize)),
                        Some(LPARAM(icon.0 as isize)),
                    );
                }
                Err(err) => tracing::warn!(%err, size, "значок окна не загружен"),
            }
        }
    }
}

/// База профиля рядом с логом. Если её не открыть (диск только на чтение,
/// снесённый профиль), браузер должен работать дальше — просто без истории.
fn open_store() -> Store {
    let path = profile_dir().join("browser.db");
    match Store::open(&path) {
        Ok(store) => store,
        Err(err) => {
            tracing::error!(%err, path = %path.display(), "профиль не открыт, история отключена");
            Store::memory().expect("база в памяти создаётся всегда")
        }
    }
}

/// Идентификатор приложения из `tauri.conf.json`: под ним WebView2 держит свои
/// данные (`EBWebView`), и там же живёт профиль браузера.
const IDENTIFIER: &str = "pw.x190x4.browser";

fn local_app_data() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// Профиль — в папке данных приложения, а не в `%LOCALAPPDATA%\190x4 Browser`:
/// туда установщик ставит саму программу. Флажок «удалить данные» при удалении
/// программы чистит именно папку данных приложения.
pub(crate) fn profile_dir() -> std::path::PathBuf {
    separate_profile().unwrap_or_else(|| local_app_data().join(IDENTIFIER))
}

/// Отдельный профиль для проверок рядом с основным браузером: своя база и свои
/// данные движка, и с уже запущенным браузером этот не объединяется.
fn separate_profile() -> Option<std::path::PathBuf> {
    std::env::var_os("BROWSER190X4_PROFILE").map(std::path::PathBuf::from)
}

/// Профиль до установщика лежал в `%LOCALAPPDATA%\190x4 Browser`. Переносим его
/// один раз, пока база не открыта. База и её журналы переезжают только вместе:
/// если хоть один файл не перенёсся, уже перенесённые возвращаются назад.
fn migrate_profile() {
    const FILES: [&str; 4] = [
        "browser.db",
        "browser.db-wal",
        "browser.db-shm",
        "services.json",
    ];

    let old = local_app_data().join("190x4 Browser");
    let new = profile_dir();
    if new.join("browser.db").exists() || !old.join("browser.db").exists() {
        return;
    }
    let _ = std::fs::create_dir_all(&new);

    let mut moved = Vec::new();
    for name in FILES {
        let from = old.join(name);
        if !from.exists() {
            continue;
        }
        if let Err(err) = std::fs::rename(&from, new.join(name)) {
            tracing::error!(%err, file = name, "профиль не перенесён, остаётся на месте");
            for done in moved.iter().rev() {
                let _ = std::fs::rename(new.join(done), old.join(done));
            }
            return;
        }
        moved.push(name);
    }
    tracing::info!(from = %old.display(), to = %new.display(), "профиль перенесён");
}

/// Клиент сервисов 190x4. Ключи — из профиля, не из кода.
fn open_services() -> Services {
    let config = ServicesConfig::load(&profile_dir().join("services.json"));
    tracing::info!(
        translate = config.translate_enabled(),
        media = config.media_enabled(),
        "сервисы"
    );
    Services::new(config).expect("HTTP-клиент создаётся всегда")
}

/// Логи — в файл, а не в stdout.
///
/// Приложение собрано с `windows_subsystem = "windows"`: консоли нет, и всё,
/// что пишется в stdout, пропадает. Первый же живой запуск уткнулся именно в
/// это — вкладка не открывалась, а причины не видно.
fn init_logging() {
    let dir = profile_dir();
    let _ = std::fs::create_dir_all(&dir);

    let filter =
        tracing_subscriber::EnvFilter::try_from_env("BROWSER190X4_LOG").unwrap_or_else(|_| {
            tracing_subscriber::EnvFilter::new("info,browser190x4=debug,browser190x4_webview=debug")
        });

    // Лог прошлого запуска остаётся рядом: браузер после падения открывают
    // заново, и без этого первый же запуск стирал бы причину.
    let _ = std::fs::rename(dir.join("browser.log"), dir.join("browser.old.log"));
    match std::fs::File::create(dir.join("browser.log")) {
        Ok(file) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .init(),
        Err(_) => tracing_subscriber::fmt().with_env_filter(filter).init(),
    }
}

/// Какие списки фильтров включены. `adblock_lists` хранит включённые списки;
/// в сохранённом до версии 2 наборе нет списков, появившихся позже, и они не
/// должны оказаться выключенными молча.
pub(crate) fn enabled_lists(store: &Store) -> Vec<String> {
    let defaults = browser190x4_adblock::Subscriptions::default().lists;
    match store.setting("adblock_lists").ok().flatten() {
        Some(serde_json::Value::Array(ids)) => {
            let mut enabled: Vec<String> = ids
                .into_iter()
                .filter_map(|id| id.as_str().map(str::to_string))
                .collect();
            let version = store
                .setting("adblock_lists_version")
                .ok()
                .flatten()
                .and_then(|value| value.as_u64())
                .unwrap_or(1);
            if version < 2 {
                const FIRST: [&str; 3] = ["easylist", "easyprivacy", "ruadlist"];
                for spec in defaults
                    .iter()
                    .filter(|spec| spec.enabled && !FIRST.contains(&spec.id.as_str()))
                {
                    if !enabled.contains(&spec.id) {
                        enabled.push(spec.id.clone());
                    }
                }
            }
            enabled
        }
        _ => defaults
            .into_iter()
            .filter(|spec| spec.enabled)
            .map(|spec| spec.id)
            .collect(),
    }
}

/// Скачанные фильтры: расширенные списки и ресурсы скриптлетов.
pub(crate) fn filters_dir() -> std::path::PathBuf {
    profile_dir().join("filters")
}

/// Номер последней пересборки фильтра. Списки переключают быстрее, чем они
/// собираются: набор, начатый раньше, но собранный позже, не должен перебить
/// свежий.
static FILTER_BUILD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Списки фильтров собираются в фоне и въезжают одним `swap`.
///
/// До этого момента браузер уже работает — просто без блокировок (или со
/// старым набором правил). Первый запуск не должен ждать разбор сотен тысяч
/// правил, а следующие не разбирают их вовсе: собранный движок лежит снимком
/// (`engine.bin`) и поднимается заново, пока набор списков тот же.
pub(crate) fn rebuild_filter(guard: Arc<Guard>, store: Arc<Store>, app: tauri::AppHandle) {
    use browser190x4_adblock::{FilterList, ListSource, Subscriptions};
    use std::sync::atomic::Ordering;

    let build = FILTER_BUILD.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        let bundled = match app.path().resource_dir() {
            Ok(dir) => dir.join("lists"),
            Err(err) => {
                tracing::warn!(%err, "нет каталога ресурсов — фильтр остаётся пустым");
                return;
            }
        };
        let downloaded = filters_dir();

        // Вшитые в установщик списки свежее скачанных не бывают: канал фильтров
        // обновляет их каждый день, установщик — только с новой версией.
        let enabled = enabled_lists(&store);
        let lists: Vec<ListFile> = Subscriptions::default()
            .lists
            .into_iter()
            .filter(|spec| enabled.contains(&spec.id))
            .map(|spec| {
                let (path, optional) = match &spec.source {
                    ListSource::Bundled(name) => {
                        let fresh = downloaded.join(name);
                        let path = if fresh.exists() {
                            fresh
                        } else {
                            bundled.join(name)
                        };
                        (path, false)
                    }
                    ListSource::Downloaded(name) => (downloaded.join(name), true),
                };
                ListFile {
                    id: spec.id,
                    trusted: spec.trusted,
                    optional,
                    path,
                }
            })
            .collect();

        let resources = || match std::fs::read_to_string(downloaded.join("resources.json")) {
            Ok(json) => Guard::parse_resources(&json).unwrap_or_else(|err| {
                tracing::warn!(%err, "ресурсы скриптлетов не разобраны");
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };

        let started = std::time::Instant::now();
        let count = lists.iter().filter(|list| list.path.exists()).count();
        let key = snapshot_key(&lists);
        let snapshot = downloaded.join(SNAPSHOT);
        let restored = std::fs::read_to_string(downloaded.join(SNAPSHOT_KEY))
            .ok()
            .filter(|saved| *saved == key)
            .and_then(|_| std::fs::read(&snapshot).ok())
            .and_then(|bytes| Guard::restore(&bytes, resources()));
        let fresh = restored.is_none();
        let engine = match restored {
            Some(engine) => engine,
            None => {
                let mut texts = Vec::new();
                for list in &lists {
                    match std::fs::read_to_string(&list.path) {
                        Ok(text) => texts.push(FilterList {
                            text,
                            trusted: list.trusted,
                        }),
                        Err(err) => {
                            // Скачанного списка нет до первого обновления фильтров.
                            if !(list.optional && err.kind() == std::io::ErrorKind::NotFound) {
                                tracing::warn!(list = %list.id, %err, "список не прочитан");
                            }
                        }
                    }
                }
                Guard::build(texts, resources())
            }
        };
        if FILTER_BUILD.load(Ordering::SeqCst) != build {
            tracing::debug!("списки сменились, пока фильтр собирался — этот набор устарел");
            return;
        }
        let bytes = fresh.then(|| Guard::snapshot(&engine));
        guard.swap(engine);
        tracing::info!(
            lists = count,
            snapshot = !fresh,
            ms = started.elapsed().as_millis(),
            "фильтр собран"
        );
        let _ = app.emit("adblock-ready", count);

        // Снимок пишется после того, как фильтр уже работает: ключ — последним,
        // иначе оборванная запись выглядела бы готовым снимком.
        if let Some(bytes) = bytes {
            let saved = std::fs::create_dir_all(&downloaded)
                .and_then(|()| write_replacing(&snapshot, &bytes))
                .and_then(|()| write_replacing(&downloaded.join(SNAPSHOT_KEY), key.as_bytes()));
            if let Err(err) = saved {
                tracing::debug!(%err, "снимок фильтра не записан");
            }
        }
    });
}

/// Собранный движок фильтра и отпечаток набора списков, из которого он собран.
const SNAPSHOT: &str = "engine.bin";
const SNAPSHOT_KEY: &str = "engine.key";

/// Включённый список фильтров и файл, из которого он читается.
struct ListFile {
    id: String,
    trusted: bool,
    /// Скачиваемый список: до первого обновления фильтров его может не быть.
    optional: bool,
    path: std::path::PathBuf,
}

/// Отпечаток набора списков: версия браузера (с ней меняется движок), какие
/// списки включены и какие файлы лежат на диске — размер и время записи.
/// Сменилось что угодно — снимок устарел, списки собираются заново.
fn snapshot_key(lists: &[ListFile]) -> String {
    let mut lines = vec![env!("CARGO_PKG_VERSION").to_string()];
    for list in lists {
        let stamp = match std::fs::metadata(&list.path) {
            Ok(meta) => {
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |since| since.as_nanos());
                format!("{} {modified}", meta.len())
            }
            Err(_) => "-".to_string(),
        };
        lines.push(format!(
            "{} {} {} {stamp}",
            list.id,
            list.trusted,
            list.path.display()
        ));
    }
    lines.join("\n")
}

/// Записать файл целиком: через временный рядом и переименование.
fn write_replacing(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, path)
}
