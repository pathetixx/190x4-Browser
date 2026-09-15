//! Состояние приложения и правило «всё, что COM — на главном потоке».
//!
//! `TabHost` держит COM-объекты STA и принципиально не `Send`. Хранить его в
//! `tauri::State` нельзя. Поэтому он живёт в thread-local главного потока, а
//! команды попадают к нему через `run_on_main_thread` + канал для ответа.

use std::cell::RefCell;
use std::sync::mpsc;
use std::sync::Arc;

use browser190x4_adblock::Guard;
use browser190x4_services::Services;
use browser190x4_store::Store;
use browser190x4_webview::TabHost;
use tauri::AppHandle;

use crate::passwords::Passwords;
use crate::popup::Popup;
use crate::transfers::Transfers;

thread_local! {
    static HOST: RefCell<Option<TabHost>> = const { RefCell::new(None) };
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
    pub popup: Popup,
    /// Ссылки на приложения, о которых спросили пользователя.
    pub external: crate::external::Offers,
}

pub fn install_host(host: TabHost) {
    HOST.with(|cell| *cell.borrow_mut() = Some(host));
}

/// Выполнить действие над хостом вкладок на главном потоке и дождаться ответа.
///
/// Блокирующий вызов: Tauri-команды исполняются на своём пуле, а не на UI, и
/// без ожидания chrome получал бы `ok` раньше, чем вкладка вообще создана.
///
/// Из обработчиков событий вкладки (они сами на главном потоке) звать нельзя:
/// хост в этот момент может быть занят, и повторный заём уронит процесс.
/// Оттуда — через [`with_host_later`].
pub fn with_host<R>(
    app: &AppHandle,
    f: impl FnOnce(&mut TabHost) -> R + Send + 'static,
) -> Result<R, String>
where
    R: Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    app.run_on_main_thread(move || {
        let result = HOST.with(|cell| match cell.try_borrow_mut() {
            Ok(mut borrow) => match borrow.as_mut() {
                Some(host) => Ok(f(host)),
                None => Err("хост вкладок ещё не поднят".to_string()),
            },
            Err(_) => Err("хост вкладок занят".to_string()),
        });
        let _ = tx.send(result);
    })
    .map_err(|e| e.to_string())?;

    rx.recv()
        .map_err(|_| "главный поток не ответил".to_string())?
}

/// То же, но без ожидания и с гарантией, что задача встанет в очередь, а не
/// выполнится прямо внутри текущего обработчика.
pub fn with_host_later(app: &AppHandle, f: impl FnOnce(&mut TabHost) + Send + 'static) {
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(err) = with_host(&app, f) {
            tracing::warn!(%err, "отложенная задача хоста не выполнена");
        }
    });
}
