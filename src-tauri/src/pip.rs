//! Расширение «Мини-плеер»: видео страницы в маленьком окне поверх всех, с
//! паузой, перемоткой и громкостью.
//!
//! Вкладка переезжает в окно мини-плеера живой (`TabHost::pip_open`), а видео
//! на всё окно и кнопки делает скрипт страницы (`inject/pip.js`). Мини-плеер
//! один на весь браузер: открытый для другой вкладки возвращает прежнюю.
//! Страница просит перенести окно (`pip_drag`), вернуть вкладку (`pip_back`) или
//! закрыть мини-плеер (`pip_close`); переход на другую страницу возвращает
//! вкладку сам.

use browser190x4_webview::TabId;
use parking_lot::Mutex;
use serde::Deserialize;
use tauri::{AppHandle, Manager};

use crate::state::with_tab;

/// Вкладка в мини-плеере и окно браузера, которому она принадлежит.
static CURRENT: Mutex<Option<(String, u32)>> = Mutex::new(None);

#[derive(Deserialize)]
#[serde(tag = "evt")]
enum PageEvent {
    #[serde(rename = "pip_drag")]
    Drag,
    #[serde(rename = "pip_back")]
    Back,
    #[serde(rename = "pip_close")]
    Close,
}

/// Открыть вкладку в мини-плеере или вернуть, если она уже там.
#[tauri::command]
pub fn pip_toggle(app: AppHandle, id: u32) -> Result<(), String> {
    let current = CURRENT.lock().clone();
    if let Some((_, tab)) = current {
        with_tab(&app, tab, |host| host.pip_close(false, "back"))?;
        if tab == id {
            return Ok(());
        }
    }
    with_tab(&app, id, move |host| {
        host.pip_open(TabId(id)).map_err(|err| err.to_string())
    })?
}

/// Вкладка переехала в мини-плеер или вернулась: запомнить, а вернувшуюся
/// «во вкладку» — показать в её окне.
pub fn on_event(app: &AppHandle, label: &str, id: u32, on: bool, reason: &str) {
    let mut current = CURRENT.lock();
    if on {
        *current = Some((label.to_string(), id));
    } else if current.as_ref().is_some_and(|(_, tab)| *tab == id) {
        *current = None;
    }
    drop(current);
    if !on && reason == "back" {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }
}

/// Страница ушла на другой адрес — мини-плеер своё видео потерял.
pub fn on_navigation(app: &AppHandle, id: u32) {
    if !CURRENT.lock().as_ref().is_some_and(|(_, tab)| *tab == id) {
        return;
    }
    crate::state::later(app, id, |host| host.pip_close(false, "navigated"));
}

/// Сообщение скрипта мини-плеера. `true` — оно наше и дальше не идёт.
pub fn handle_message(app: &AppHandle, tab: u32, frame: Option<u32>, payload: &str) -> bool {
    let Ok(event) = serde_json::from_str::<PageEvent>(payload) else {
        return false;
    };
    // Только документ вкладки, которая сейчас в мини-плеере.
    if frame.is_some() || !CURRENT.lock().as_ref().is_some_and(|(_, id)| *id == tab) {
        return true;
    }
    crate::state::later(app, tab, move |host| match event {
        PageEvent::Drag => host.pip_drag(),
        PageEvent::Back => host.pip_close(false, "back"),
        PageEvent::Close => host.pip_close(true, "closed"),
    });
    true
}
