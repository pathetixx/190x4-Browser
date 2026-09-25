//! Кем браузер представляется сайтам: Edge, как движок есть, или Chrome.
//!
//! Настройки — `identity` (для всех сайтов) и `identity_sites` (исключения:
//! сайт → `edge` или `chrome`, сайт — ключ `site_key`, домен с поддоменами).
//! Подмену ставит вкладка сама (`crates/webview/src/identity.rs`); здесь —
//! настройки для движка и настоящие Client Hints, которые присылает интерфейс.

use browser190x4_store::Store;
use browser190x4_webview::identity::{self, Identity};
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::state::App;

/// Настройки — движку для вкладок, которые ещё откроются.
pub fn init(store: &Store) {
    let default = store
        .setting_str("identity")
        .as_deref()
        .and_then(Identity::parse)
        .unwrap_or(Identity::Edge);
    identity::set_identity(default, sites(store));
}

/// Настройки сменились или пришли Client Hints: открытым вкладкам всех окон.
pub fn apply(app: &AppHandle) {
    let state = app.state::<App>();
    init(&state.store);
    for label in state.windows.labels() {
        let _ = crate::state::with_host(app, &label, |host| host.apply_identity());
    }
}

/// Настоящие Client Hints движка (`getHighEntropyValues` в окне интерфейса):
/// из них собирается вид Chrome.
pub fn set_engine_hints(app: &AppHandle, hints: Value) {
    if !hints.is_object() {
        return;
    }
    identity::set_engine_hints(hints);
    apply(app);
}

fn sites(store: &Store) -> Vec<(String, Identity)> {
    match store.setting("identity_sites").ok().flatten() {
        Some(Value::Object(sites)) => sites
            .iter()
            .filter_map(|(site, value)| {
                Some((site.to_ascii_lowercase(), Identity::parse(value.as_str()?)?))
            })
            .collect(),
        _ => Vec::new(),
    }
}
