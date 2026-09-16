//! Состояние приложения и правило «всё, что COM — на главном потоке».
//!
//! `TabHost` держит COM-объекты STA и принципиально не `Send`. Хранить его в
//! `tauri::State` нельзя. Поэтому хосты живут в thread-local главного потока, а
//! команды попадают к ним через `run_on_main_thread` + канал для ответа.
//!
//! Окон браузера может быть несколько, у каждого свой `TabHost` под своим
//! ярлыком окна. Номера вкладок и загрузок общие, поэтому команда, знающая
//! только номер вкладки, находит нужное окно сама ([`with_tab`]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use browser190x4_adblock::Guard;
use browser190x4_services::Services;
use browser190x4_store::Store;
use browser190x4_webview::{TabHost, TabId};
use parking_lot::Mutex;
use tauri::AppHandle;

use crate::browser_windows::WindowRegistry;
use crate::passwords::Passwords;
use crate::popup::Popup;
use crate::transfers::Transfers;

/// Сколько ждать главный поток. Он занят кадром, а не вечностью: если ответа
/// нет и через это время, значит что-то встало намертво, и держать команду
/// (а с ней и поток из пула Tauri) дальше незачем.
const MAIN_THREAD_TIMEOUT: Duration = Duration::from_secs(10);

thread_local! {
    static HOSTS: RefCell<HashMap<String, TabHost>> = RefCell::new(HashMap::new());
}

/// Глобальное состояние, которое *можно* шарить между потоками.
///
/// `Store` сюда попадает целиком: Tauri-команды исполняются в своём пуле, а
/// не на UI-потоке, поэтому короткая блокировка на SQLite кадрам не мешает.
pub struct App {
    pub guard: Arc<Guard>,
    pub store: Arc<Store>,
    pub services: Arc<Services>,
    pub passwords: Passwords,
    pub transfers: Transfers,
    /// Всплывающие окна: по одному на окно браузера.
    pub popup: Popup,
    /// Ссылки на приложения, о которых спросили пользователя.
    pub external: crate::external::Offers,
    /// Окна браузера: ярлык → что это за окно (обычное или приватное).
    pub windows: WindowRegistry,
    /// Поисковая система из настроек: её читает открытие вкладки на главном
    /// потоке, где ждать замок базы нельзя.
    pub engine: parking_lot::RwLock<String>,
    /// Последняя сохранённая сессия каждого окна: окно закрывается раньше,
    /// чем интерфейс успевает записать её сам.
    pub sessions: Mutex<HashMap<String, Vec<browser190x4_store::SessionTab>>>,
}

pub fn install_host(label: &str, host: TabHost) {
    HOSTS.with(|cell| cell.borrow_mut().insert(label.to_string(), host));
}

/// Окно закрылось: его вкладки закрыл движок вместе с окном.
pub fn remove_host(label: &str) {
    HOSTS.with(|cell| {
        cell.borrow_mut().remove(label);
    });
}

/// Выполнить действие над хостом окна на главном потоке и дождаться ответа.
///
/// Блокирующий вызов: Tauri-команды исполняются на своём пуле, а не на UI, и
/// без ожидания chrome получал бы `ok` раньше, чем вкладка вообще создана.
///
/// Из обработчиков событий вкладки (они сами на главном потоке) звать нельзя:
/// хост в этот момент может быть занят, и повторный заём уронит процесс.
/// Оттуда — через [`later`].
pub fn with_host<R>(
    app: &AppHandle,
    label: &str,
    f: impl FnOnce(&mut TabHost) -> R + Send + 'static,
) -> Result<R, String>
where
    R: Send + 'static,
{
    let label = label.to_string();
    on_main(app, move || {
        HOSTS.with(|cell| match cell.try_borrow_mut() {
            Ok(mut hosts) => match hosts.get_mut(&label) {
                Some(host) => Ok(f(host)),
                None => Err("окно браузера уже закрыто".to_string()),
            },
            Err(_) => Err("хост вкладок занят".to_string()),
        })
    })
}

/// То же, но окно ищется по номеру вкладки: номера общие на все окна.
pub fn with_tab<R>(
    app: &AppHandle,
    tab: u32,
    f: impl FnOnce(&mut TabHost) -> R + Send + 'static,
) -> Result<R, String>
where
    R: Send + 'static,
{
    on_main(app, move || {
        HOSTS.with(|cell| match cell.try_borrow_mut() {
            Ok(mut hosts) => match hosts.values_mut().find(|host| host.has_tab(TabId(tab))) {
                Some(host) => Ok(f(host)),
                None => Err("вкладка уже закрыта".to_string()),
            },
            Err(_) => Err("хост вкладок занят".to_string()),
        })
    })
}

/// Окно, которое ведёт эту загрузку: список загрузок знает номер записи, но
/// не окно, из которого её начали.
pub fn with_download<R>(
    app: &AppHandle,
    key: u64,
    f: impl FnOnce(&mut TabHost) -> R + Send + 'static,
) -> Result<R, String>
where
    R: Send + 'static,
{
    on_main(app, move || {
        HOSTS.with(|cell| match cell.try_borrow_mut() {
            Ok(mut hosts) => match hosts.values_mut().find(|host| host.has_download(key)) {
                Some(host) => Ok(f(host)),
                None => Err("эту загрузку уже не продолжить — начните заново".to_string()),
            },
            Err(_) => Err("хост вкладок занят".to_string()),
        })
    })
}

/// Любое живое окно — для команд, которым нужен движок, а не конкретная
/// вкладка (разрешения сайтов, очистка данных, версия рантайма).
pub fn with_any_host<R>(
    app: &AppHandle,
    f: impl FnOnce(&mut TabHost) -> R + Send + 'static,
) -> Result<R, String>
where
    R: Send + 'static,
{
    on_main(app, move || {
        HOSTS.with(|cell| match cell.try_borrow_mut() {
            Ok(mut hosts) => {
                // Приватное окно держит свой профиль движка: для общих данных
                // берём обычное, если оно есть.
                let label = hosts
                    .iter()
                    .find(|(_, host)| !host.is_private())
                    .or_else(|| hosts.iter().next())
                    .map(|(label, _)| label.clone());
                match label.and_then(|label| hosts.get_mut(&label)) {
                    Some(host) => Ok(f(host)),
                    None => Err("окно браузера ещё не поднято".to_string()),
                }
            }
            Err(_) => Err("хост вкладок занят".to_string()),
        })
    })
}

fn on_main<R>(
    app: &AppHandle,
    f: impl FnOnce() -> Result<R, String> + Send + 'static,
) -> Result<R, String>
where
    R: Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(f());
    })
    .map_err(|e| e.to_string())?;

    rx.recv_timeout(MAIN_THREAD_TIMEOUT)
        .map_err(|_| "главный поток не ответил".to_string())?
}

/// Отложенное действие над вкладкой: не ждёт и не выполняется прямо внутри
/// текущего обработчика события. Зовётся с главного потока, поэтому ожидание
/// уезжает в пул блокирующих задач, а не в новый поток на каждое сообщение
/// страницы — иначе сайт, шлющий сообщения в цикле, поднимал бы тысячи потоков.
pub fn later(app: &AppHandle, tab: u32, f: impl FnOnce(&mut TabHost) + Send + 'static) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(err) = with_tab(&app, tab, f) {
            tracing::debug!(%err, "отложенная задача вкладки не выполнена");
        }
    });
}
