//! 190x4 Browser — точка сборки.
//!
//! Разделение ролей:
//! * `ui/` — chrome браузера, обычная веб-страница в Tauri-вебвью. Только она
//!   (и её всплывающее окно) имеет доступ к командам из [`ipc`];
//! * [`browser190x4_webview`] — вкладки поверх того же HWND, из того же Environment;
//! * [`browser190x4_adblock`] — сетевой фильтр на горячем пути WebResourceRequested.

mod default_browser;
mod filters;
pub mod ipc;
mod launch;
mod newtab;
mod passwords;
mod popup;
mod resources;
mod site_icons;
mod state;
mod transfers;
mod updates;
mod vault;
mod weather;

use std::sync::Arc;
use std::time::Duration;

use browser190x4_adblock::Guard;
use browser190x4_services::{Services, ServicesConfig};
use browser190x4_store::Store;
use tauri::{Emitter, Manager, WindowEvent};

use state::App;

pub fn run() {
    // Установщик запускает exe только ради регистрации в Windows.
    if let Some(code) = default_browser::installer_flag() {
        std::process::exit(code);
    }

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
        })
        .manage(updates::Updates::default())
        .manage(newtab::NewTab::default())
        .manage(launched)
        .invoke_handler(tauri::generate_handler![
            ipc::tab_open,
            ipc::tab_close,
            ipc::tab_activate,
            ipc::tab_navigate,
            ipc::tab_action,
            ipc::tab_post,
            ipc::tab_context_menu,
            ipc::tab_mute,
            ipc::tab_find,
            ipc::tab_find_step,
            ipc::layout_set,
            ipc::overlay_set,
            ipc::window_command,
            ipc::window_state,
            ipc::chrome_focus,
            ipc::adblock_stats,
            ipc::adblock_set_enabled,
            ipc::adblock_lists,
            ipc::adblock_site,
            ipc::adblock_site_set,
            ipc::settings_get,
            ipc::settings_set,
            ipc::history_record,
            ipc::history_recent,
            ipc::history_search,
            ipc::history_forget,
            ipc::history_clear,
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
            default_browser::default_browser_state,
            default_browser::default_browser_set,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            create_chrome_window(app)?;
            updates::spawn_checker(handle.clone());
            filters::spawn(handle.clone());
            rebuild_filter(guard.clone(), store.clone(), handle.clone());

            #[cfg(windows)]
            {
                let window = app
                    .get_webview_window("chrome")
                    .expect("окно chrome описано в tauri.conf.json");
                // `with_webview` требует Send-замыкание, а HWND — сырой
                // указатель и не Send. Переносим его числом и собираем
                // обратно уже внутри, на UI-потоке, где он и живёт.
                let hwnd_bits = window.hwnd()?.0 as isize;
                apply_window_icon(&window);
                let guard = guard.clone();
                let policy = ipc::download_policy(&store);
                // Папка со встроенными страницами: в dev — из репозитория,
                // в бандле — из ресурсов приложения.
                let pages_dir = handle.path().resource_dir().map(|dir| dir.join("pages"));
                let handle_for_sink = handle.clone();

                // Выполняется на главном потоке — там же, где живёт COM.
                window.with_webview(move |platform| {
                    let hwnd = windows::Win32::Foundation::HWND(hwnd_bits as *mut std::ffi::c_void);
                    let controller = platform.controller();
                    let env = browser190x4_webview::interop::environment_of(&controller)
                        .expect("Environment chrome-вебвью");

                    let sink: browser190x4_webview::tab::EventSink =
                        std::rc::Rc::new(move |event: browser190x4_webview::TabEvent| {
                            route_event(&handle_for_sink, event);
                        });

                    match browser190x4_webview::TabHost::new(hwnd, env, guard, sink) {
                        Ok(host) => {
                            if let Ok(dir) = pages_dir {
                                host.set_pages_dir(dir);
                            }
                            host.set_download_policy(policy);
                            host.set_chrome_controller(controller.clone());
                            state::install_host(host);
                        }
                        Err(err) => tracing::error!(%err, "контейнер вкладок не создан"),
                    }
                })?;
            }

            let window = app.get_webview_window("chrome").unwrap();
            wire_main_window(&handle, &window);
            window.show()?;

            // Всплывающее окно поднимаем заранее, когда браузер уже на экране:
            // первое меню не должно ждать запуска ещё одного вебвью.
            let popup_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_millis(1500)).await;
                if let Err(err) = popup::ensure(&popup_handle) {
                    tracing::warn!(%err, "всплывающее окно не создано");
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("не удалось запустить 190x4 Browser");
}

/// Разводка событий вкладок.
///
/// Часть событий chrome-у не нужна вовсе или нужна уже обработанной:
/// загрузки сначала попадают в базу, пароли не должны покидать Rust.
#[cfg(windows)]
fn route_event(app: &tauri::AppHandle, event: browser190x4_webview::TabEvent) {
    use browser190x4_webview::TabEvent;

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
            transfers::on_engine_event(app, *key, phase, url, path, *bytes, *total, error);
            return;
        }
        TabEvent::DownloadAsk { key, path, .. } => {
            transfers::ask_target(app, *key, path.clone());
            return;
        }
        TabEvent::Message {
            id,
            source,
            payload,
        } => {
            if passwords::handle_message(app, *id, source, payload)
                || newtab::handle_message(app, *id, source, payload)
            {
                return;
            }
        }
        TabEvent::Started { id, .. } => passwords::on_navigation(app, *id),
        TabEvent::Favicon { page, url, .. } => {
            let state = app.state::<App>();
            if state.store.set_bookmark_icon(page, url).unwrap_or(false) {
                let _ = app.emit("bookmarks", ());
            }
        }
        _ => {}
    }
    let _ = app.emit_to("chrome", "tab", &event);
}

/// Окно браузера — из `tauri.conf.json`, но создаётся здесь: пробе с отдельным
/// профилем движку нужен порт отладки.
fn create_chrome_window(app: &tauri::App) -> tauri::Result<()> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == "chrome")
        .expect("окно chrome описано в tauri.conf.json")
        .clone();
    let mut builder = tauri::WebviewWindowBuilder::from_config(app.handle(), &config)?;
    if let Some(args) = debug_browser_args() {
        builder = builder.additional_browser_args(&args);
    }
    builder.build()?;
    Ok(())
}

/// Аргументы движка с портом отладки (CDP) — только при отдельном профиле и
/// заданном `BROWSER190X4_DEBUG_PORT`. Окна браузера делят одно окружение
/// WebView2, поэтому попап получает те же аргументы. Первые два — те, что wry
/// передаёт по умолчанию.
pub(crate) fn debug_browser_args() -> Option<String> {
    separate_profile()?;
    let port: u16 = std::env::var("BROWSER190X4_DEBUG_PORT")
        .ok()?
        .parse()
        .ok()?;
    Some(format!(
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
         --autoplay-policy=no-user-gesture-required --remote-debugging-port={port}"
    ))
}

/// Главное окно: состояние «развёрнуто» для кнопки окна и закрытие попапа,
/// когда окно уехало из-под него.
fn wire_main_window(app: &tauri::AppHandle, window: &tauri::WebviewWindow) {
    let handle = app.clone();
    let main = window.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Moved(_) => popup::hide(&handle),
        #[cfg(windows)]
        WindowEvent::ScaleFactorChanged { .. } => apply_window_icon(&main),
        WindowEvent::Resized(_) => {
            popup::hide(&handle);
            let maximized = main.is_maximized().unwrap_or(false);
            let _ = handle.emit_to(
                "chrome",
                "window-state",
                serde_json::json!({ "maximized": maximized }),
            );
        }
        _ => {}
    });
}

/// Значок окна для панели задач и Alt+Tab — из ресурсов exe, нужного размера.
///
/// Tauri ставит окну одну картинку, и Windows растягивала её под панель задач.
/// В ресурсах exe лежат все размеры `icon.ico` (tauri-build кладёт иконку под
/// номером 32512), и `LoadImageW` берёт нарисованный под текущий DPI.
#[cfg(windows)]
fn apply_window_icon(window: &tauri::WebviewWindow) {
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

    match std::fs::File::create(dir.join("browser.log")) {
        Ok(file) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .init(),
        Err(_) => tracing_subscriber::fmt().with_env_filter(filter).init(),
    }
}

/// Какие списки фильтров включены: из настроек, иначе — стартовый набор.
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

/// Списки фильтров собираются в фоне и въезжают одним `swap`.
///
/// До этого момента браузер уже работает — просто без блокировок (или со
/// старым набором правил). Первый запуск не должен ждать разбор сотен тысяч
/// правил.
/// Скачанные фильтры: расширенные списки и ресурсы скриптлетов.
pub(crate) fn filters_dir() -> std::path::PathBuf {
    profile_dir().join("filters")
}

pub(crate) fn rebuild_filter(guard: Arc<Guard>, store: Arc<Store>, app: tauri::AppHandle) {
    use browser190x4_adblock::{FilterList, ListSource, Subscriptions};

    std::thread::spawn(move || {
        let bundled = match app.path().resource_dir() {
            Ok(dir) => dir.join("lists"),
            Err(err) => {
                tracing::warn!(%err, "нет каталога ресурсов — фильтр остаётся пустым");
                return;
            }
        };
        let downloaded = filters_dir();

        let enabled = enabled_lists(&store);
        let mut lists = Vec::new();
        for spec in Subscriptions::default()
            .lists
            .iter()
            .filter(|spec| enabled.contains(&spec.id))
        {
            let path = match &spec.source {
                ListSource::Bundled(name) => bundled.join(name),
                ListSource::Downloaded(name) => downloaded.join(name),
            };
            match std::fs::read_to_string(&path) {
                Ok(text) => lists.push(FilterList {
                    text,
                    trusted: spec.trusted,
                }),
                // Скачанного списка нет до первого обновления фильтров.
                Err(err)
                    if err.kind() == std::io::ErrorKind::NotFound
                        && matches!(spec.source, ListSource::Downloaded(_)) => {}
                Err(err) => tracing::warn!(list = %spec.id, %err, "список не прочитан"),
            }
        }

        let resources = match std::fs::read_to_string(downloaded.join("resources.json")) {
            Ok(json) => Guard::parse_resources(&json).unwrap_or_else(|err| {
                tracing::warn!(%err, "ресурсы скриптлетов не разобраны");
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };

        let started = std::time::Instant::now();
        let count = lists.len();
        let scriptlets = resources.len();
        let engine = Guard::build(lists, resources);
        guard.swap(engine);
        tracing::info!(
            lists = count,
            scriptlets,
            ms = started.elapsed().as_millis(),
            "фильтр собран"
        );
        let _ = app.emit("adblock-ready", count);
    });
}
