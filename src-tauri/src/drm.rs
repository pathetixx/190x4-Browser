//! Защищённое видео (DRM): Widevine для сайтов и что делать, когда видео не
//! пошло.
//!
//! В движке два модуля защиты: Widevine от Google и PlayReady от Microsoft.
//! Движок для сайтов — Edge, и сайт сам выбирает, каким модулем брать лицензию.
//! Бывает, что Widevine лицензию не получает (сервер лицензий не верит
//! встроенному движку) — тогда видео грузится, но стоит на нуле. Тот же сайт с
//! PlayReady играет.
//!
//! Скрипт страницы (`crates/webview/src/inject/drm.js`) замечает, на каком шаге
//! видео застряло, и сообщает сюда. Если застрял Widevine, браузер выключает его
//! для этого сайта (`drm_widevine_off_sites`) и загружает страницу заново —
//! сайт берёт PlayReady. Иначе — говорит человеку, что случилось. Widevine можно
//! выключить и совсем (`drm_widevine`).

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use browser190x4_store::Store;

use crate::state::App;

#[derive(Deserialize)]
#[serde(tag = "evt")]
enum PageEvent {
    /// Защищённое видео не пошло: `system` — модуль защиты, `stage` — шаг.
    #[serde(rename = "drm_problem")]
    Problem {
        system: String,
        stage: String,
        #[serde(default)]
        detail: String,
        /// Widevine на этой странице уже был спрятан.
        #[serde(default)]
        hidden: bool,
    },
}

/// Сообщение скрипта защищённого видео — от документа или фрейма (плеер часто
/// во фрейме). `true` — оно наше и дальше не идёт.
pub fn handle_message(app: &AppHandle, label: &str, tab: u32, source: &str, payload: &str) -> bool {
    let Ok(PageEvent::Problem {
        system,
        stage,
        detail,
        hidden,
    }) = serde_json::from_str::<PageEvent>(payload)
    else {
        return false;
    };
    if !source.starts_with("https://") {
        return true;
    }
    let (app, label) = (app.clone(), label.to_string());
    // Адрес вкладки и настройки — не на главном потоке, куда пришло сообщение.
    tauri::async_runtime::spawn_blocking(move || {
        let url = crate::state::with_tab(&app, tab, move |host| {
            host.with_tab(browser190x4_webview::TabId(tab), |view| view.source_url())
        });
        let Some(site) = url
            .ok()
            .flatten()
            .and_then(|url| browser190x4_adblock::site_key(&url))
        else {
            return;
        };
        tracing::warn!(%site, %system, %stage, %detail, hidden, "защищённое видео не пошло");
        on_problem(&app, &label, tab, &site, &system, &stage, hidden);
    });
    true
}

fn on_problem(
    app: &AppHandle,
    label: &str,
    tab: u32,
    site: &str,
    system: &str,
    stage: &str,
    hidden: bool,
) {
    let store = app.state::<App>().store.clone();
    let reason = reason(stage);
    let widevine = store.setting_bool("drm_widevine", true);
    let fallback = system == "widevine"
        && !hidden
        && widevine
        && store.setting_bool("drm_fallback", true)
        && !off_sites(&store).iter().any(|other| other == site);
    if !fallback {
        let text = match system {
            "playready" if hidden => format!(
                "Защищённое видео не запустилось и через PlayReady: {reason}. Вернуть сайту Widevine — в настройках, «Конфиденциальность»"
            ),
            _ if !widevine => {
                format!("Защищённое видео не запустилось: {reason}. Widevine выключен в настройках")
            }
            _ => format!("Защищённое видео не запустилось: {reason}"),
        };
        notice(app, label, tab, &text);
        return;
    }

    let mut sites = off_sites(&store);
    sites.push(site.to_string());
    if let Err(err) = store.set_setting("drm_widevine_off_sites", &json!(sites)) {
        tracing::warn!(%err, "сайт без Widevine не записан");
        return;
    }
    let _ = app.emit(
        "settings",
        json!({ "key": "drm_widevine_off_sites", "value": sites }),
    );
    apply(app);
    notice(
        app,
        label,
        tab,
        &format!(
            "Защищённое видео не пошло через Widevine ({reason}) — открываю его через PlayReady"
        ),
    );
    // Страница перечитает скрипт с новыми настройками только при загрузке.
    let _ = crate::state::with_tab(app, tab, move |host| {
        host.with_tab(browser190x4_webview::TabId(tab), |view| {
            if let Err(err) = view.reload() {
                tracing::debug!(%err, "страница с видео не перезагружена");
            }
        })
    });
}

/// Что сказать человеку о шаге, на котором видео застряло.
fn reason(stage: &str) -> &'static str {
    match stage {
        "cdm" => "модуль защиты не запустился",
        "license" => "сервер лицензий отказал",
        "no-license" => "сайт так и не выдал лицензию",
        "no-keys" => "ключи к видео не подошли",
        "keys" => "модуль защиты не принял ключи",
        "no-request" | "no-keys-system" => "плеер не запросил ключ",
        "media" => "видео не расшифровалось",
        _ => "видео не расшифровывается",
    }
}

/// Сайты, где Widevine выключен.
pub fn off_sites(store: &Store) -> Vec<String> {
    match store.setting("drm_widevine_off_sites").ok().flatten() {
        Some(Value::Array(items)) => items
            .into_iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// Настройки Widevine — движку для новых вкладок и открытым вкладкам всех окон.
pub fn apply(app: &AppHandle) {
    let state = app.state::<App>();
    init(&state.store);
    for label in state.windows.labels() {
        let _ = crate::state::with_host(app, &label, |host| host.apply_drm());
    }
}

/// Настройки Widevine для вкладок, которые ещё откроются.
pub fn init(store: &Store) {
    browser190x4_webview::tab::set_drm_config(
        store.setting_bool("drm_widevine", true),
        &off_sites(store),
    );
}

/// Сообщение над страницей вкладки, если она на экране.
fn notice(app: &AppHandle, label: &str, tab: u32, text: &str) {
    let _ = app.emit_to(label, "notice", json!({ "id": tab, "text": text }));
}
