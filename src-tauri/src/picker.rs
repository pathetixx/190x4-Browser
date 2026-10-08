//! «Скрыть элемент» и «Мои правила».
//!
//! Пункт меню страницы открывает на ней выбор блока (`inject/picker.js`), а
//! выбранный селектор становится правилом `сайт##селектор` в настройке
//! `adblock_user_rules` — отдельном списке, который фильтр собирает вместе с
//! подписками (`rebuild_filter`).
//!
//! Канал `chrome.webview` слышит любой скрипт страницы, поэтому правило
//! принимается, только пока идёт выбор, открытый браузером на этой вкладке, и
//! только как косметика для сайта самой страницы: подсунуть правило для
//! чужого сайта, сетевое правило или скриптлет страница не может.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use browser190x4_webview::TabId;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::state::App;

/// Настройка со своими правилами: массив строк в синтаксисе списков.
pub const SETTING: &str = "adblock_user_rules";
/// Больше правил не храним: список собирается в фильтр при каждом изменении.
const RULES_LIMIT: usize = 2_000;
const SELECTOR_LIMIT: usize = 500;

/// Идущий выбор на вкладке: его номер.
static SESSIONS: Mutex<Option<HashMap<u32, u64>>> = Mutex::new(None);
static NEXT: AtomicU64 = AtomicU64::new(1);

#[derive(Deserialize)]
#[serde(tag = "evt")]
enum PageEvent {
    #[serde(rename = "picker_rule")]
    Rule { token: u64, selector: String },
    #[serde(rename = "picker_cancel")]
    Cancel { token: u64 },
}

/// Открыть на вкладке выбор элемента. `x`, `y` — точка щелчка в CSS-пикселях
/// страницы: с неё выбор начинается.
#[tauri::command]
pub fn adblock_pick(app: AppHandle, id: u32, x: f64, y: f64) -> Result<(), String> {
    let token = NEXT.fetch_add(1, Ordering::Relaxed);
    SESSIONS
        .lock()
        .get_or_insert_with(HashMap::new)
        .insert(id, token);
    let config = json!({ "token": token, "x": x, "y": y }).to_string();
    crate::state::with_tab(&app, id, move |host| {
        host.with_tab(TabId(id), |tab| tab.start_picker(&config));
    })
}

/// Сообщение выбора. `true` — оно наше и дальше не идёт.
pub fn handle_message(
    app: &AppHandle,
    label: &str,
    tab: u32,
    frame: Option<u32>,
    source: &str,
    payload: &str,
) -> bool {
    let Ok(event) = serde_json::from_str::<PageEvent>(payload) else {
        return false;
    };
    // Выбор идёт в главном документе: фрейм его закончить не может.
    if frame.is_some() {
        return true;
    }
    let token = match &event {
        PageEvent::Rule { token, .. } | PageEvent::Cancel { token } => *token,
    };
    let current = SESSIONS
        .lock()
        .as_mut()
        .and_then(|sessions| match sessions.get(&tab) {
            Some(open) if *open == token => sessions.remove(&tab),
            _ => None,
        });
    if current.is_none() {
        return true;
    }
    let PageEvent::Rule { selector, .. } = event else {
        return true;
    };
    let Some(rule) = rule_for(source, &selector) else {
        tracing::debug!("выбранный элемент не превратился в правило");
        return true;
    };
    let (app, label, site) = (
        app.clone(),
        label.to_string(),
        browser190x4_adblock::site_key(source).unwrap_or_default(),
    );
    // Главный поток не ждёт базу и пересборку фильтра.
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<App>();
        let mut rules = user_rules(&state.store);
        if !rules.contains(&rule) {
            rules.push(rule);
            if rules.len() > RULES_LIMIT {
                rules.drain(..rules.len() - RULES_LIMIT);
            }
        }
        let value = Value::from(rules);
        if let Err(err) = state.store.set_setting(SETTING, &value) {
            tracing::warn!(%err, "своё правило не сохранено");
            return;
        }
        crate::rebuild_filter(state.guard.clone(), state.store.clone(), app.clone());
        let _ = app.emit("settings", json!({ "key": SETTING, "value": value }));
        let _ = app.emit_to(
            &label,
            "notice",
            json!({ "id": tab, "text": format!("Элемент скрыт на {site}. Правило — в настройках, «Мои правила»") }),
        );
    });
    true
}

/// Свои правила из настроек.
pub fn user_rules(store: &browser190x4_store::Store) -> Vec<String> {
    match store.setting(SETTING).ok().flatten() {
        Some(Value::Array(items)) => items
            .into_iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// Правило скрытия для сайта страницы или `None`, если селектор не годится:
/// только обычный CSS-селектор — ни блока стилей, ни второго правила, ни
/// процедурных действий.
fn rule_for(page_url: &str, selector: &str) -> Option<String> {
    let site = browser190x4_adblock::site_key(page_url)?;
    let selector = selector.trim();
    let allowed = !selector.is_empty()
        && selector.len() <= SELECTOR_LIMIT
        && !selector.contains(['{', '}', '\n', '\r', '\0'])
        && !selector.contains("##")
        && !selector.contains("#@#")
        && !selector.contains("#?#")
        && !selector.starts_with('+')
        && !selector.contains(":style(")
        && !selector.contains(":remove");
    allowed.then(|| format!("{site}##{selector}"))
}

#[cfg(test)]
mod tests {
    use super::rule_for;

    #[test]
    fn rules_are_cosmetic_and_for_the_page_site() {
        assert_eq!(
            rule_for("https://www.dzen.ru/a?b", "div.ad > span").as_deref(),
            Some("dzen.ru##div.ad > span")
        );
        assert_eq!(rule_for("about:blank", "div"), None);
        assert_eq!(rule_for("https://x.example/", "div{color:red}"), None);
        assert_eq!(rule_for("https://x.example/", "div##+js(foo)"), None);
        assert_eq!(rule_for("https://x.example/", "+js(foo)"), None);
        assert_eq!(rule_for("https://x.example/", "div:remove()"), None);
        assert_eq!(rule_for("https://x.example/", "a\n||evil.example^"), None);
        assert_eq!(rule_for("https://x.example/", "  "), None);
    }
}
