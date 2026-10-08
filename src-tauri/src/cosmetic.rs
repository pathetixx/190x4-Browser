//! Общие правила скрытия по классам и id.
//!
//! Таких правил в списках десятки тысяч (`##.ad-banner`, `###sidebar-ads`), и
//! встраивать их все в каждую страницу — мегабайты стиля. Поэтому страница
//! (исполнитель косметики, `crates/adblock/src/cosmetic.js`) сообщает, какие
//! классы и id у неё есть (`cosmetic_ids`), а браузер отвечает селекторами тех
//! правил, что к ним относятся (`cosmetic_css`). Так же это делает Brave.

use serde::Deserialize;
use serde_json::json;
use tauri::{AppHandle, Manager};

use crate::state::App;

/// Больше имён за раз страница не присылает: сообщение не длиннее 64 КБ.
const NAMES_LIMIT: usize = 5_000;
/// Имя класса или id длиннее этого — не то, на что пишут правила.
const NAME_LIMIT: usize = 256;

#[derive(Deserialize)]
#[serde(tag = "evt")]
enum PageEvent {
    /// Новые классы и id документа.
    #[serde(rename = "cosmetic_ids")]
    Ids {
        #[serde(default)]
        classes: Vec<String>,
        #[serde(default)]
        ids: Vec<String>,
    },
}

/// Сообщение исполнителя косметики. `true` — оно наше и дальше не идёт.
pub fn handle_message(
    app: &AppHandle,
    tab: u32,
    frame: Option<u32>,
    source: &str,
    payload: &str,
) -> bool {
    let Ok(PageEvent::Ids {
        mut classes,
        mut ids,
    }) = serde_json::from_str(payload)
    else {
        return false;
    };
    // Адрес документа — от движка: по нему считаются исключения сайта.
    let Some(origin) = origin_of(source) else {
        return true;
    };
    for names in [&mut classes, &mut ids] {
        names.retain(|name| !name.is_empty() && name.len() <= NAME_LIMIT);
        names.truncate(NAMES_LIMIT);
    }
    if classes.is_empty() && ids.is_empty() {
        return true;
    }
    let (app, source) = (app.clone(), source.to_string());
    // Сообщение пришло на главный поток, а подбор правил ему не нужен.
    tauri::async_runtime::spawn_blocking(move || {
        let found = app
            .state::<App>()
            .guard
            .generic_hide(&source, &classes, &ids);
        if found.is_empty() {
            return;
        }
        let message = json!({
            "cmd": "cosmetic_css",
            "origin": origin,
            "selectors": found.hide,
            "force": found.force,
        })
        .to_string();
        crate::state::later(&app, tab, move |host| {
            host.with_tab(browser190x4_webview::TabId(tab), |view| {
                if let Err(err) = view.post_to(frame, &message) {
                    tracing::debug!(%err, "общие правила косметики не отправлены");
                }
            });
        });
    });
    true
}

/// Origin документа сайта: только http и https.
fn origin_of(source: &str) -> Option<String> {
    let url = reqwest::Url::parse(source).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.origin().ascii_serialization())
}

#[cfg(test)]
mod tests {
    use super::origin_of;

    #[test]
    fn only_sites_get_generic_rules() {
        assert_eq!(
            origin_of("https://vk.com/feed").as_deref(),
            Some("https://vk.com")
        );
        assert_eq!(origin_of("file:///C:/page.html"), None);
        assert_eq!(origin_of("about:blank"), None);
    }
}
