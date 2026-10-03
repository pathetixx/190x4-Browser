//! Расширение YouTube NonStop: окно «Видео приостановлено. Продолжить
//! просмотр?» на YouTube и «Вы ещё здесь?» на YouTube Music закрывается само, а
//! видео играет дальше.
//!
//! Скрипт страницы (`crates/webview/src/inject/nonstop.js`) при загрузке
//! спрашивает, включено ли расширение: да — он держит свежим время последнего
//! действия, по которому YouTube решает спросить, и вопрос не появляется даже
//! в фоновой вкладке. Окно, которое YouTube всё же показал, пока человек
//! ничего не нажимал, скрипт тоже отдаёт браузеру. Решает Rust — по настройке
//! `ext_nonstop_enabled`.

use serde::Deserialize;
use serde_json::json;
use tauri::{AppHandle, Manager};

use crate::state::App;

#[derive(Deserialize)]
#[serde(tag = "evt")]
enum PageEvent {
    /// Страница YouTube загрузилась: включено ли расширение?
    #[serde(rename = "nonstop_hello")]
    Hello,
    /// YouTube спросил «Продолжить просмотр?».
    #[serde(rename = "nonstop_ask")]
    Ask,
}

/// Сообщение скрипта NonStop. `true` — оно наше и дальше не идёт.
pub fn handle_message(
    app: &AppHandle,
    tab: u32,
    frame: Option<u32>,
    source: &str,
    payload: &str,
) -> bool {
    let Ok(event) = serde_json::from_str::<PageEvent>(payload) else {
        return false;
    };
    // Только документ вкладки на YouTube — адрес от движка.
    let Some(origin) = youtube_origin(source).filter(|_| frame.is_none()) else {
        return true;
    };
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if !app
            .state::<App>()
            .store
            .setting_bool("ext_nonstop_enabled", true)
        {
            return;
        }
        let cmd = match event {
            PageEvent::Hello => "nonstop_on",
            PageEvent::Ask => "nonstop_continue",
        };
        let message = json!({ "cmd": cmd, "origin": origin }).to_string();
        crate::state::later(&app, tab, move |host| {
            host.with_tab(browser190x4_webview::TabId(tab), |view| {
                if let Err(err) = view.post(&message) {
                    tracing::debug!(%err, "YouTube не сказали продолжить");
                }
            });
        });
    });
    true
}

/// Origin документа YouTube или YouTube Music: только https.
fn youtube_origin(source: &str) -> Option<String> {
    let url = reqwest::Url::parse(source).ok()?;
    let host = url.host_str()?;
    let youtube = host == "youtube.com" || host.ends_with(".youtube.com");
    (url.scheme() == "https" && youtube).then(|| url.origin().ascii_serialization())
}

#[cfg(test)]
mod tests {
    use super::youtube_origin;

    #[test]
    fn only_youtube_is_answered() {
        assert_eq!(
            youtube_origin("https://www.youtube.com/watch?v=dQw4w9WgXcQ").as_deref(),
            Some("https://www.youtube.com")
        );
        assert_eq!(
            youtube_origin("https://music.youtube.com/watch?v=1").as_deref(),
            Some("https://music.youtube.com")
        );
        assert_eq!(youtube_origin("https://notyoutube.com/"), None);
        assert_eq!(youtube_origin("http://www.youtube.com/"), None);
    }
}
