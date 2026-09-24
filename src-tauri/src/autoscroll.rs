//! Расширение «Автопролистывание»: ролик в YouTube Shorts, Reels в Instagram
//! или в TikTok доиграл — лента сама переходит к следующему.
//!
//! Скрипт страницы (`crates/webview/src/inject/autoscroll.js`) узнаёт, что
//! ролик кончается, и спрашивает браузер; решает Rust — по настройкам: общей
//! (`ext_autoscroll_enabled`, её же переключает кнопка на панели) и сайта
//! (`autoscroll_<сайт>`). Состояние в страницу не передаётся, и выключенное
//! пролистывание не оставляет ей ничего, кроме вопроса раз на ролик.

use serde::Deserialize;
use serde_json::json;
use tauri::{AppHandle, Manager};

use crate::state::App;

/// Сайты, которые листает расширение: ключ настройки и домен.
pub const SITES: [(&str, &str); 3] = [
    ("youtube", "youtube.com"),
    ("instagram", "instagram.com"),
    ("tiktok", "tiktok.com"),
];

#[derive(Deserialize)]
#[serde(tag = "evt")]
enum PageEvent {
    /// Ролик доигрывает — листать ли дальше.
    #[serde(rename = "autoscroll_end")]
    End,
}

/// Сообщение скрипта ленты. `true` — оно наше и дальше не идёт.
pub fn handle_message(
    app: &AppHandle,
    tab: u32,
    frame: Option<u32>,
    source: &str,
    payload: &str,
) -> bool {
    let Ok(PageEvent::End) = serde_json::from_str::<PageEvent>(payload) else {
        return false;
    };
    // Листает только документ вкладки, и сайт — по адресу от движка.
    let Some((site, origin)) = site_of(source).filter(|_| frame.is_none()) else {
        return true;
    };
    let app = app.clone();
    // Сообщение пришло на главный поток, настройки — в базе.
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<App>().store.clone();
        if !store.setting_bool("ext_autoscroll_enabled", true)
            || !store.setting_bool(&format!("autoscroll_{site}"), true)
        {
            return;
        }
        let message = json!({ "cmd": "autoscroll_next", "origin": origin }).to_string();
        crate::state::later(&app, tab, move |host| {
            host.with_tab(browser190x4_webview::TabId(tab), |view| {
                if let Err(err) = view.post(&message) {
                    tracing::debug!(%err, "ленте не сказали листать");
                }
            });
        });
    });
    true
}

/// Сайт ленты и origin документа: только https и только эти домены с
/// поддоменами.
fn site_of(source: &str) -> Option<(&'static str, String)> {
    let url = reqwest::Url::parse(source).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let host = url.host_str()?;
    let (site, _) = SITES
        .iter()
        .find(|(_, domain)| host == *domain || host.ends_with(&format!(".{domain}")))?;
    Some((site, url.origin().ascii_serialization()))
}

#[cfg(test)]
mod tests {
    use super::site_of;

    #[test]
    fn only_feed_sites_are_answered() {
        assert_eq!(
            site_of("https://www.youtube.com/shorts/dQw4w9WgXcQ"),
            Some(("youtube", "https://www.youtube.com".to_string()))
        );
        assert_eq!(
            site_of("https://www.instagram.com/reels/C1/").map(|(site, _)| site),
            Some("instagram")
        );
        assert_eq!(
            site_of("https://www.tiktok.com/foryou").map(|(site, _)| site),
            Some("tiktok")
        );
        assert!(site_of("https://tiktok.com.evil.example/").is_none());
        assert!(site_of("https://nottiktok.com/").is_none());
        assert!(site_of("http://www.tiktok.com/").is_none());
    }
}
