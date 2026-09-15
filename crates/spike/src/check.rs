//! Какие запросы страницы блокирует фильтр. Запросы снимаются в любом Chromium
//! (CDP `Network.requestWillBeSent`), проверка — теми же списками, что у
//! браузера: так сломанный сайт разбирается без запуска браузера.

use std::path::Path;

use browser190x4_adblock::{Decision, ResourceKind};
use serde::Deserialize;

#[derive(Deserialize)]
struct Captured {
    url: String,
    #[serde(rename = "type", default)]
    kind: String,
}

pub fn run(page: &str, requests: &Path, bundled: &Path, downloaded: &Path) -> anyhow::Result<()> {
    let guard = crate::cosmetics::load_guard(bundled, downloaded)?;
    let captured: Vec<Captured> = serde_json::from_str(&std::fs::read_to_string(requests)?)?;
    let mut blocked = 0;
    for request in &captured {
        // Источник — документ вкладки, как в браузере: запросы фреймов тоже от него.
        match guard.check(&request.url, page, kind_of(&request.kind), "GET") {
            Decision::Allow => {}
            Decision::Block => {
                blocked += 1;
                println!("BLOCK {:<10} {}", request.kind, request.url);
            }
            Decision::Rewrite(to) => println!("REWRITE {} -> {to}", request.url),
        }
    }
    println!("checked {}, blocked {blocked}", captured.len());
    Ok(())
}

fn kind_of(cdp: &str) -> ResourceKind {
    match cdp {
        "Document" => ResourceKind::Document,
        "Stylesheet" => ResourceKind::Stylesheet,
        "Image" => ResourceKind::Image,
        "Media" => ResourceKind::Media,
        "Font" => ResourceKind::Font,
        "Script" => ResourceKind::Script,
        "XHR" => ResourceKind::Xhr,
        "Fetch" => ResourceKind::Fetch,
        "WebSocket" => ResourceKind::Websocket,
        "Ping" => ResourceKind::Ping,
        _ => ResourceKind::Other,
    }
}
