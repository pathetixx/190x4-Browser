//! 190x4 Browser — точка сборки.
//!
//! Разделение ролей:
//! * `ui/` — chrome браузера, обычная веб-страница в Tauri-вебвью. Только она
//!   (и её всплывающее окно) имеет доступ к командам из [`ipc`];
//! * [`browser190x4_webview`] — вкладки поверх того же HWND, из того же Environment;
//! * [`browser190x4_adblock`] — сетевой фильтр на горячем пути WebResourceRequested.

pub mod ipc;
mod passwords;
mod popup;
mod state;
mod transfers;
mod vault;

use std::sync::Arc;
use std::time::Duration;

use browser190x4_adblock::Guard;
use browser190x4_services::{Services, ServicesConfig};
use browser190x4_store::Store;
use tauri::{Emitter, Manager, WindowEvent};

use state::App;

pub fn run() {
    init_logging();

    let guard = Arc::new(Guard::empty());
    let store = Arc::new(open_store());
    let services = Arc::new(open_services());

    guard.set_enabled(store.setting_bool("adblock_enabled", true));
    if let Err(err) = store.fail_interrupted_downloads() {
        tracing::warn!(%err, "незавершённые загрузки не отмечены");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(App {
            guard: guard.clone(),
            store: store.clone(),
            services: services.clone(),
            passwords: Default::default(),
            transfers: Default::default(),
            popup: Default::default(),
        })
        .invoke_handler(tauri::generate_handler![
            ipc::tab_open,
            ipc::tab_close,
            ipc::tab_activate,
            ipc::tab_navigate,
            ipc::tab_action,
            ipc::tab_post,
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
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
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
            if passwords::handle_message(app, *id, source, payload) {
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

/// Главное окно: состояние «развёрнуто» для кнопки окна и закрытие попапа,
/// когда окно уехало из-под него.
fn wire_main_window(app: &tauri::AppHandle, window: &tauri::WebviewWindow) {
    let handle = app.clone();
    let main = window.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Moved(_) => popup::hide(&handle),
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

pub(crate) fn profile_dir() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("190x4 Browser")
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
pub(crate) fn enabled_lists(store: &Store) -> Vec<String> {
    match store.setting("adblock_lists").ok().flatten() {
        Some(serde_json::Value::Array(ids)) => ids
            .into_iter()
            .filter_map(|id| id.as_str().map(str::to_string))
            .collect(),
        _ => browser190x4_adblock::Subscriptions::default()
            .lists
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
pub(crate) fn rebuild_filter(guard: Arc<Guard>, store: Arc<Store>, app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let dir = match app.path().resource_dir() {
            Ok(dir) => dir.join("lists"),
            Err(err) => {
                tracing::warn!(%err, "нет каталога ресурсов — фильтр остаётся пустым");
                return;
            }
        };

        let enabled = enabled_lists(&store);
        let mut raw = Vec::new();
        for spec in browser190x4_adblock::Subscriptions::default()
            .lists
            .iter()
            .filter(|spec| enabled.contains(&spec.id))
        {
            if let browser190x4_adblock::ListSource::Bundled(name) = &spec.source {
                match std::fs::read_to_string(dir.join(name)) {
                    Ok(text) => raw.push(text),
                    Err(err) => tracing::warn!(list = %spec.id, %err, "список не прочитан"),
                }
            }
        }

        let started = std::time::Instant::now();
        let count = raw.len();
        let engine = Guard::build(raw);
        guard.swap(engine);
        tracing::info!(
            lists = count,
            ms = started.elapsed().as_millis(),
            "фильтр собран"
        );
        let _ = app.emit("adblock-ready", count);
    });
}
