//! Tauri-команды. Доступны **только** chrome-окну и его всплывающему окну
//! (см. capabilities/default.json).
//!
//! Вкладки в этот список не входят: у них нет ни `window.__TAURI__`, ни
//! init-скрипта Tauri. Страница может достучаться до приложения единственным
//! путём — `window.chrome.webview.postMessage`, и всё оттуда считается
//! недоверенным вводом.

use std::path::PathBuf;
use std::time::Duration;

use browser190x4_store::{
    bookmarks_html, passwords_csv, BookmarkNode, Download, DownloadKind, DownloadState,
    HistoryEntry, HistoryHit, ImportReport, NeverSite, PasswordEntry, SessionTab, Store,
    BAR_FOLDER,
};
use browser190x4_webview::{DialogAnswer, DownloadPolicy, Layout, PermissionSetting, TabId};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::browser_windows::{self as windows, WindowKind};
use crate::state::{with_any_host, with_host, with_tab, App};
use crate::{external, passwords, popup, transfers, vault};

fn text(err: impl std::fmt::Display) -> String {
    format!("{err:#}")
}

/// Окно браузера, от имени которого пришла команда. Всплывающее окно работает
/// от имени своего окна: меню второго окна не должно трогать первое.
fn owner(window: &tauri::Window) -> String {
    let label = window.label();
    popup::owner_of(label).unwrap_or(label).to_string()
}

/// Приватное окно ничего не пишет на диск: ни истории, ни сессии, ни паролей.
fn is_private(app: &AppHandle, window: &tauri::Window) -> bool {
    app.state::<App>().windows.is_private(&owner(window))
}

/* ── Вкладки ────────────────────────────────────────────────────────────── */

#[tauri::command]
pub fn tab_open(
    app: AppHandle,
    window: tauri::Window,
    state: State<'_, App>,
    url: String,
) -> Result<u32, String> {
    let url = normalize_url(&url, &search_engine(&state.store));
    tracing::info!(%url, "открываем вкладку");
    let result = with_host(&app, &owner(&window), move |host| {
        host.open(&url).map(|id| id.0).map_err(text)
    })?;
    if let Err(err) = &result {
        tracing::error!(%err, "вкладка не открылась");
    }
    result
}

#[tauri::command]
pub fn tab_close(app: AppHandle, state: State<'_, App>, id: u32) -> Result<Option<u32>, String> {
    passwords::forget_tab(&app, id);
    external::forget_tab(&state, id);
    with_tab(&app, id, move |host| {
        host.close(TabId(id))
            .map(|next| next.map(|t| t.0))
            .map_err(text)
    })?
}

#[tauri::command]
pub fn tab_activate(app: AppHandle, id: u32) -> Result<(), String> {
    with_tab(&app, id, move |host| host.activate(TabId(id)).map_err(text))?
}

/// Разделённый экран: вторая вкладка встаёт справа от активной. `id = None`
/// возвращает окно к одной странице.
#[tauri::command]
pub fn tab_split(app: AppHandle, window: tauri::Window, id: Option<u32>) -> Result<(), String> {
    match id {
        Some(id) => with_tab(&app, id, move |host| {
            host.set_split(Some(TabId(id))).map_err(text)
        })?,
        None => with_host(&app, &owner(&window), |host| {
            host.set_split(None).map_err(text)
        })?,
    }
}

#[tauri::command]
pub fn tab_navigate(
    app: AppHandle,
    state: State<'_, App>,
    id: u32,
    url: String,
) -> Result<(), String> {
    let url = normalize_url(&url, &search_engine(&state.store));
    with_tab(&app, id, move |host| {
        host.with_tab(TabId(id), |tab| tab.navigate(&url))
            .ok_or_else(|| "вкладка ещё не готова".to_string())?
            .map_err(text)
    })?
}

#[tauri::command]
pub fn tab_action(app: AppHandle, id: u32, action: String) -> Result<(), String> {
    with_tab(&app, id, move |host| {
        host.with_tab(TabId(id), |tab| match action.as_str() {
            "back" => tab.go_back().map_err(text),
            "forward" => tab.go_forward().map_err(text),
            "reload" => tab.reload().map_err(text),
            "reload_hard" => tab.reload_ignoring_cache().map_err(text),
            "print" => tab.print().map_err(text),
            "devtools" => tab.open_devtools().map_err(text),
            "zoom_in" => tab.zoom(1).map(|_| ()).map_err(text),
            "zoom_out" => tab.zoom(-1).map(|_| ()).map_err(text),
            "zoom_reset" => tab.zoom(0).map(|_| ()).map_err(text),
            other => Err(format!("неизвестное действие {other}")),
        })
        .ok_or_else(|| "вкладка ещё не готова".to_string())?
    })?
}

/// Выбор в меню страницы: команда движка или `None` — меню закрыто.
#[tauri::command]
pub fn tab_context_menu(
    app: AppHandle,
    id: u32,
    menu: u64,
    command: Option<i32>,
) -> Result<(), String> {
    with_tab(&app, id, move |host| {
        host.with_tab(TabId(id), |tab| tab.context_menu_done(menu, command))
            .unwrap_or(Ok(()))
            .map_err(text)
    })?
}

/// Поставить вкладке масштаб сайта: его помнит не вкладка, а сайт.
#[tauri::command]
pub fn tab_zoom_set(app: AppHandle, id: u32, factor: f64) -> Result<(), String> {
    with_tab(&app, id, move |host| {
        host.with_tab(TabId(id), |tab| tab.set_zoom(factor))
            .unwrap_or(Ok(()))
            .map_err(text)
    })?
}

/// Ответ на окно страницы. Несколько номеров — одно окно на несколько
/// запросов (камера и микрофон сразу).
#[tauri::command]
pub fn tab_dialog(
    app: AppHandle,
    window: tauri::Window,
    state: State<'_, App>,
    id: u32,
    tokens: Vec<u64>,
    answer: DialogAnswer,
) -> Result<(), String> {
    let engine: Vec<u64> = tokens
        .iter()
        .copied()
        .filter(|token| !external::answer(&app, &state, id, *token, &answer))
        .collect();
    if !engine.is_empty() {
        let reply = answer.clone();
        with_tab(&app, id, move |host| {
            host.with_tab(TabId(id), |tab| {
                for token in engine {
                    if let Err(err) = tab.dialog_done(token, &reply) {
                        tracing::warn!(%err, "ответ на окно страницы не записан");
                    }
                }
            });
        })?;
    }
    let _ = app.emit_to(
        owner(&window).as_str(),
        "dialog-done",
        serde_json::json!({ "id": id, "tokens": tokens }),
    );
    Ok(())
}

/// Отправить сообщение на страницу вкладки.
#[tauri::command]
pub fn tab_post(app: AppHandle, id: u32, payload: Value) -> Result<(), String> {
    let json = payload.to_string();
    with_tab(&app, id, move |host| {
        host.with_tab(TabId(id), |tab| tab.post(&json))
            .ok_or_else(|| "вкладка ещё не готова".to_string())?
            .map_err(text)
    })?
}

/// Заглушить вкладку или вернуть ей звук.
#[tauri::command]
pub fn tab_mute(app: AppHandle, id: u32, muted: bool) -> Result<(), String> {
    with_tab(&app, id, move |host| {
        host.with_tab(TabId(id), |tab| tab.set_muted(muted))
            .ok_or_else(|| "вкладка ещё не готова".to_string())?
            .map_err(text)
    })?
}

/// Начать поиск по странице. Результаты приходят событиями `find`.
#[tauri::command]
pub fn tab_find(app: AppHandle, id: u32, query: String) -> Result<(), String> {
    with_tab(&app, id, move |host| {
        host.find(TabId(id), &query).map_err(text)
    })?
}

/// Следующее/предыдущее совпадение или конец поиска.
#[tauri::command]
pub fn tab_find_step(app: AppHandle, id: u32, action: String) -> Result<(), String> {
    with_tab(&app, id, move |host| {
        host.with_tab(TabId(id), |tab| match action.as_str() {
            "next" => tab.find_step(true),
            "prev" => tab.find_step(false),
            "stop" => tab.find_stop(),
            other => Err(anyhow::anyhow!("неизвестное действие поиска {other}")),
        })
        .ok_or_else(|| "вкладка ещё не готова".to_string())?
        .map_err(text)
    })?
}

/// Chrome сообщает, где теперь «дырка» под страницу.
///
/// `scale` — devicePixelRatio окна: chrome считает в CSS-пикселях, WebView2
/// живёт в физических.
#[tauri::command]
pub fn layout_set(
    app: AppHandle,
    window: tauri::Window,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    scale: f64,
) -> Result<(), String> {
    let layout = Layout {
        x: (x * scale).round() as i32,
        y: (y * scale).round() as i32,
        width: (width * scale).round() as i32,
        height: (height * scale).round() as i32,
    };
    with_host(&app, &owner(&window), move |host| {
        host.set_layout(layout).map_err(text)
    })?
}

/// Показать/убрать нативную поверхность под оверлеем chrome-а.
#[tauri::command]
pub fn overlay_set(app: AppHandle, window: tauri::Window, on: bool) -> Result<(), String> {
    with_host(&app, &owner(&window), move |host| {
        host.set_overlay(on).map_err(text)
    })?
}

#[tauri::command]
pub fn window_command(app: AppHandle, window: tauri::Window, action: String) -> Result<(), String> {
    let window = app
        .get_webview_window(&owner(&window))
        .ok_or("нет окна браузера")?;
    match action.as_str() {
        "minimize" => window.minimize().map_err(text),
        "maximize" => window.maximize().map_err(text),
        "unmaximize" => window.unmaximize().map_err(text),
        "toggle_maximize" => {
            if window.is_maximized().unwrap_or(false) {
                window.unmaximize().map_err(text)
            } else {
                window.maximize().map_err(text)
            }
        }
        "close" => window.close().map_err(text),
        other => Err(format!("неизвестное действие окна {other}")),
    }
}

/// Вернуть клавиатуру интерфейсу (см. `TabHost::focus_chrome`).
#[tauri::command]
pub fn chrome_focus(app: AppHandle, window: tauri::Window) -> Result<(), String> {
    let result = with_host(&app, &owner(&window), |host| {
        host.focus_chrome().map_err(text)
    })?;
    tracing::debug!(ok = result.is_ok(), "chrome focus requested");
    result
}

#[tauri::command]
pub fn window_state(app: AppHandle, window: tauri::Window) -> Value {
    let label = owner(&window);
    let maximized = app
        .get_webview_window(&label)
        .and_then(|window| window.is_maximized().ok())
        .unwrap_or(false);
    serde_json::json!({ "maximized": maximized })
}

/// Что это за окно: приватное или обычное и под каким номером его сессия.
#[tauri::command]
pub fn window_info(app: AppHandle, window: tauri::Window) -> Value {
    let label = owner(&window);
    let registry = &app.state::<App>().windows;
    serde_json::json!({
        "label": label,
        "private": registry.is_private(&label),
        "session": registry.session(&label),
        "windows": registry.len(),
    })
}

/// Новое окно браузера: обычное или приватное, с адресом или пустое.
#[tauri::command]
pub fn window_open(
    app: AppHandle,
    private: Option<bool>,
    url: Option<String>,
) -> Result<(), String> {
    let kind = if private.unwrap_or(false) {
        WindowKind::Private
    } else {
        WindowKind::Normal
    };
    windows::open(&app, kind, url).map(|_| ()).map_err(text)
}

/* ── Адресная строка ────────────────────────────────────────────────────── */

fn search_engine(store: &Store) -> String {
    store
        .setting_str("search_engine")
        .unwrap_or_else(|| "duckduckgo".into())
}

fn search_prefix(engine: &str) -> &'static str {
    match engine {
        "yandex" => "https://yandex.ru/search/?text=",
        "google" => "https://www.google.com/search?q=",
        "bing" => "https://www.bing.com/search?q=",
        _ => "https://duckduckgo.com/?q=",
    }
}

/// Ввод из адресной строки: либо URL, либо поисковый запрос.
///
/// Одна строка «что это было» на весь браузер — держим её здесь, а не в JS,
/// чтобы правило совпадало с тем, что реально уходит в `Navigate`.
fn normalize_url(input: &str, engine: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return "about:blank".into();
    }
    // Собственные страницы отдаются через virtual host: у них нормальный
    // http-origin, а не ограниченный file://.
    if trimmed == "about:newtab" {
        return format!("http://{}/newtab.html", browser190x4_webview::PAGES_HOST);
    }
    // Схемы, которые исполняют код в той странице, где их открыли: вставленные
    // в адресную строку, они бывают только просьбой мошенника «вставьте это
    // сюда». Ищем их как текст, как это делает Chrome.
    let lower = trimmed.to_ascii_lowercase();
    let dangerous = ["javascript:", "data:", "vbscript:", "view-source:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme));
    if dangerous {
        return format!("{}{}", search_prefix(engine), urlencode(trimmed));
    }
    if trimmed.contains("://") || trimmed.starts_with("about:") {
        return trimmed.to_string();
    }
    let looks_like_host = !trimmed.contains(' ')
        && trimmed.contains('.')
        && !trimmed.starts_with('.')
        && !trimmed.ends_with('.');
    if looks_like_host || trimmed == "localhost" || trimmed.starts_with("localhost:") {
        // Домашний роутер и сосед по локальной сети по https не отвечают:
        // туда идём по http, во внешний интернет — по https.
        let scheme = if is_local_address(trimmed) {
            "http"
        } else {
            "https"
        };
        format!("{scheme}://{trimmed}")
    } else {
        format!("{}{}", search_prefix(engine), urlencode(trimmed))
    }
}

/// Адрес внутри локальной сети: его открываем по http.
fn is_local_address(input: &str) -> bool {
    let host = input
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(input)
        .rsplit_once(':')
        .map(|(host, port)| {
            if port.chars().all(|c| c.is_ascii_digit()) {
                host
            } else {
                input
            }
        })
        .unwrap_or(input)
        .trim_end_matches('.')
        .to_ascii_lowercase();

    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    for suffix in [".local", ".lan", ".home", ".internal", ".intranet"] {
        if host.ends_with(suffix) {
            return true;
        }
    }
    let Ok(ip) = host.parse::<std::net::Ipv4Addr>() else {
        return false;
    };
    ip.is_private() || ip.is_loopback() || ip.is_link_local()
}

fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::normalize_url;

    #[test]
    fn bare_host_becomes_https() {
        assert_eq!(
            normalize_url("kinopoisk.ru", "duckduckgo"),
            "https://kinopoisk.ru"
        );
    }

    #[test]
    fn words_go_to_chosen_search() {
        assert!(normalize_url("как дела", "duckduckgo").starts_with("https://duckduckgo.com/?q="));
        assert!(normalize_url("как дела", "yandex").starts_with("https://yandex.ru/search/?text="));
    }

    #[test]
    fn newtab_goes_to_virtual_host() {
        assert!(normalize_url("about:newtab", "google").starts_with("http://190x4-pages.invalid/"));
    }

    #[test]
    fn scheme_is_kept() {
        assert_eq!(
            normalize_url("http://192.168.3.2:8080/x", "google"),
            "http://192.168.3.2:8080/x"
        );
    }

    #[test]
    fn local_addresses_go_over_http() {
        // Домашний роутер и сосед по сети по https не отвечают.
        assert_eq!(
            normalize_url("localhost:5173", "google"),
            "http://localhost:5173"
        );
        assert_eq!(normalize_url("192.168.3.2", "google"), "http://192.168.3.2");
        assert_eq!(
            normalize_url("10.0.0.1:8080", "google"),
            "http://10.0.0.1:8080"
        );
        assert_eq!(normalize_url("nas.local", "google"), "http://nas.local");
        assert_eq!(normalize_url("habr.com", "google"), "https://habr.com");
        assert_eq!(normalize_url("8.8.8.8", "google"), "https://8.8.8.8");
    }

    #[test]
    fn code_schemes_are_searched_not_opened() {
        // «Вставьте это в адресную строку» — всегда мошенничество.
        for input in [
            "javascript:alert(1)",
            "JavaScript:void(0)",
            "data:text/html,<script>x</script>",
            "view-source:https://habr.com",
        ] {
            assert!(
                normalize_url(input, "duckduckgo").starts_with("https://duckduckgo.com/?q="),
                "{input}"
            );
        }
    }
}

/* ── Фильтр ─────────────────────────────────────────────────────────────── */

#[tauri::command]
pub fn adblock_stats(state: State<'_, App>) -> browser190x4_adblock::Snapshot {
    state.guard.stats().snapshot()
}

#[tauri::command]
pub fn adblock_set_enabled(app: AppHandle, state: State<'_, App>, on: bool) -> Result<(), String> {
    state.guard.set_enabled(on);
    state
        .store
        .set_setting("adblock_enabled", &Value::Bool(on))
        .map_err(text)?;
    let _ = app.emit(
        "settings",
        serde_json::json!({ "key": "adblock_enabled", "value": on }),
    );
    Ok(())
}

/// Сайты, где пользователь выключил блокировку, — ключи `site_key`.
pub fn exempt_sites(store: &Store) -> Vec<String> {
    match store.setting("adblock_exempt_sites").ok().flatten() {
        Some(Value::Array(items)) => items
            .into_iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

#[derive(Serialize)]
pub struct SiteBlocking {
    /// `None` — страница не сайт (встроенная, `about:`, файл): переключать нечего.
    site: Option<String>,
    blocking: bool,
}

/// Работает ли блокировка на сайте этой страницы.
#[tauri::command]
pub fn adblock_site(state: State<'_, App>, url: String) -> SiteBlocking {
    // Встроенные страницы браузера сайтом не считаются.
    let site = browser190x4_adblock::site_key(&url)
        .filter(|site| site != browser190x4_webview::PAGES_HOST);
    let blocking = site.is_some() && !state.guard.is_exempt(&url);
    SiteBlocking { site, blocking }
}

/// Включить или выключить блокировку на сайте страницы. Действует со следующей
/// загрузки: уже встроенную в страницу рекламу снимает только перезагрузка.
///
/// Включение снимает и исключение родительского домена: иначе на `m.youtube.com`
/// переключатель не работал бы, пока выключен `youtube.com`.
#[tauri::command]
pub fn adblock_site_set(
    app: AppHandle,
    state: State<'_, App>,
    url: String,
    blocking: bool,
) -> Result<SiteBlocking, String> {
    let site = browser190x4_adblock::site_key(&url)
        .ok_or_else(|| "на этой странице блокировка не переключается".to_string())?;
    let mut sites = exempt_sites(&state.store);
    if blocking {
        sites.retain(|entry| site != *entry && !site.ends_with(&format!(".{entry}")));
    } else if !state.guard.is_exempt(&url) {
        sites.push(site.clone());
        sites.sort();
    }
    let value = Value::from(sites.clone());
    state
        .store
        .set_setting("adblock_exempt_sites", &value)
        .map_err(text)?;
    state.guard.set_exempt_sites(sites);
    let _ = app.emit(
        "settings",
        serde_json::json!({ "key": "adblock_exempt_sites", "value": value }),
    );
    Ok(SiteBlocking {
        blocking: !state.guard.is_exempt(&url),
        site: Some(site),
    })
}

#[derive(Serialize)]
pub struct FilterList {
    id: String,
    title: String,
    enabled: bool,
}

#[tauri::command]
pub fn adblock_lists(state: State<'_, App>) -> Vec<FilterList> {
    let enabled = crate::enabled_lists(&state.store);
    browser190x4_adblock::Subscriptions::default()
        .lists
        .into_iter()
        .map(|spec| FilterList {
            enabled: enabled.contains(&spec.id),
            id: spec.id,
            title: spec.title,
        })
        .collect()
}

/* ── Настройки ──────────────────────────────────────────────────────────── */

#[tauri::command]
pub fn settings_get(state: State<'_, App>) -> Result<serde_json::Map<String, Value>, String> {
    state.store.settings().map_err(text)
}

/// Сохранить настройку и применить то, что касается движка.
#[tauri::command]
pub fn settings_set(
    app: AppHandle,
    state: State<'_, App>,
    key: String,
    value: Value,
) -> Result<(), String> {
    if key.is_empty()
        || key.len() > 64
        || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err("недопустимое имя настройки".into());
    }
    state.store.set_setting(&key, &value).map_err(text)?;
    apply_setting(&app, &state, &key);
    // Попап и chrome держат свою копию настроек — пусть обновятся оба.
    let _ = app.emit(
        "settings",
        serde_json::json!({ "key": key, "value": value }),
    );
    Ok(())
}

fn apply_setting(app: &AppHandle, state: &App, key: &str) {
    match key {
        "download_dir" | "download_ask" => {
            let policy = download_policy(&state.store);
            // Папка загрузок общая на все окна, а политика живёт в каждом хосте.
            for label in state.windows.labels() {
                let policy = policy.clone();
                let _ = with_host(app, &label, move |host| host.set_download_policy(policy));
            }
        }
        "adblock_enabled" => state
            .guard
            .set_enabled(state.store.setting_bool("adblock_enabled", true)),
        "adblock_exempt_sites" => state.guard.set_exempt_sites(exempt_sites(&state.store)),
        "adblock_lists" => {
            crate::rebuild_filter(state.guard.clone(), state.store.clone(), app.clone())
        }
        _ => {}
    }
}

pub fn download_policy(store: &Store) -> DownloadPolicy {
    DownloadPolicy {
        dir: store
            .setting_str("download_dir")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from),
        ask: store.setting_bool("download_ask", false),
    }
}

/* ── История и сессия ───────────────────────────────────────────────────── */

/// Записать посещение.
///
/// Зовёт chrome, а не движковая часть: там в одном месте известны и адрес, и
/// заголовок, а в событиях вкладки они приходят порознь.
#[tauri::command]
pub fn history_record(
    app: AppHandle,
    window: tauri::Window,
    state: State<'_, App>,
    url: String,
    title: String,
) -> Result<(), String> {
    // Приватное окно истории не оставляет.
    if is_private(&app, &window) {
        return Ok(());
    }
    state.store.record_visit(&url, &title).map_err(text)
}

/// Страница истории: посещения по времени с поиском и подгрузкой по мере
/// прокрутки.
#[tauri::command]
pub fn history_page(
    state: State<'_, App>,
    query: Option<String>,
    before: Option<i64>,
    limit: Option<u32>,
) -> Result<Vec<browser190x4_store::Visit>, String> {
    state
        .store
        .history_visits(
            query.as_deref().unwrap_or(""),
            before,
            limit.unwrap_or(120).min(500),
        )
        .map_err(text)
}

#[tauri::command]
pub fn history_forget_visit(state: State<'_, App>, id: i64) -> Result<(), String> {
    state.store.forget_visit(id).map_err(text)
}

/// Очистить историю за период: `hour`, `day`, `week` или всю.
#[tauri::command]
pub fn history_clear_period(state: State<'_, App>, period: String) -> Result<(), String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    match period.as_str() {
        "hour" => state.store.clear_history_since(now - 3600).map_err(text),
        "day" => state.store.clear_history_since(now - 86_400).map_err(text),
        "week" => state.store.clear_history_since(now - 604_800).map_err(text),
        "all" => state.store.clear_history().map_err(text),
        other => Err(format!("неизвестный период {other}")),
    }
}

/// Значок сайта из кэша профиля — для списков паролей и истории.
///
/// В сеть не ходит: иначе открытие списка паролей означало бы запрос на
/// каждый сайт, где у пользователя есть пароль.
#[tauri::command]
pub fn site_icon(url: String) -> Option<String> {
    let dir = crate::profile_dir().join("site-icons");
    crate::site_icons::cached(&dir, &url).map(|icon| icon.data)
}

/// Подсказки поисковика для адресной строки.
///
/// Запрос уходит на сторону, поэтому в приватном окне подсказок нет, а
/// настройка `search_suggest` выключает их совсем.
#[tauri::command]
pub async fn search_suggest(
    app: AppHandle,
    window: tauri::Window,
    query: String,
) -> Result<Vec<String>, String> {
    let (engine, client) = {
        let state = app.state::<App>();
        if is_private(&app, &window) || !state.store.setting_bool("search_suggest", true) {
            return Ok(Vec::new());
        }
        (
            search_engine(&state.store),
            app.state::<crate::newtab::NewTab>().client().clone(),
        )
    };
    Ok(crate::suggest::fetch(&client, &engine, &query).await)
}

#[tauri::command]
pub fn history_recent(
    state: State<'_, App>,
    limit: Option<u32>,
) -> Result<Vec<HistoryEntry>, String> {
    state
        .store
        .recent_history(limit.unwrap_or(100))
        .map_err(text)
}

#[tauri::command]
pub fn history_search(
    state: State<'_, App>,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<HistoryHit>, String> {
    state
        .store
        .search_history(&query, limit.unwrap_or(6))
        .map_err(text)
}

#[tauri::command]
pub fn history_forget(state: State<'_, App>, url: String) -> Result<(), String> {
    state.store.forget_history(&url).map_err(text)
}

#[tauri::command]
pub fn history_clear(state: State<'_, App>) -> Result<(), String> {
    state.store.clear_history().map_err(text)
}

/// Сохранить раскладку вкладок. Chrome зовёт это с задержкой после изменений,
/// чтобы серия открытий не превратилась в серию записей на диск.
#[tauri::command]
pub fn session_save(
    window: tauri::Window,
    state: State<'_, App>,
    tabs: Vec<SessionTab>,
) -> Result<(), String> {
    let label = owner(&window);
    // Копию держим в памяти: окно закрывается раньше, чем интерфейс успевает
    // записать сессию сам, и тогда её сохраняет обработчик закрытия окна.
    state.sessions.lock().insert(label.clone(), tabs.clone());
    let Some(session) = state.windows.session(&label) else {
        return Ok(()); // приватное окно
    };
    state.store.save_session(session, &tabs).map_err(text)
}

#[tauri::command]
pub fn session_restore(
    window: tauri::Window,
    state: State<'_, App>,
) -> Result<Vec<SessionTab>, String> {
    let label = owner(&window);
    let Some(session) = state.windows.session(&label) else {
        return Ok(Vec::new());
    };
    state.store.restore_session(session).map_err(text)
}

/* ── Закладки ───────────────────────────────────────────────────────────── */

fn bookmarks_changed(app: &AppHandle) {
    let _ = app.emit("bookmarks", ());
}

#[tauri::command]
pub fn bookmarks_tree(state: State<'_, App>) -> Result<Vec<BookmarkNode>, String> {
    state.store.bookmark_nodes().map_err(text)
}

#[tauri::command]
pub fn bookmark_find(state: State<'_, App>, url: String) -> Result<Option<BookmarkNode>, String> {
    state.store.bookmark_by_url(&url).map_err(text)
}

/// Добавить закладку. Без папки — туда, куда пользователь сохранял в прошлый
/// раз, как это делает Chrome.
#[tauri::command]
pub fn bookmark_add(
    app: AppHandle,
    state: State<'_, App>,
    parent: Option<i64>,
    title: String,
    url: String,
    icon: Option<String>,
) -> Result<BookmarkNode, String> {
    let store = &state.store;
    let parent = parent
        .or_else(|| {
            store
                .setting("bookmark_folder")
                .ok()
                .flatten()
                .and_then(|v| v.as_i64())
        })
        .filter(|id| {
            store
                .bookmark(*id)
                .ok()
                .flatten()
                .is_some_and(|node| node.url.is_none())
        })
        .unwrap_or(BAR_FOLDER);
    let icon = icon
        .filter(|icon| icon.starts_with("https://") || icon.starts_with("data:image/"))
        .unwrap_or_default();

    let id = store
        .add_bookmark(parent, &title, &url, &icon)
        .map_err(text)?;
    bookmarks_changed(&app);
    store
        .bookmark(id)
        .map_err(text)?
        .ok_or_else(|| "закладка не сохранилась".to_string())
}

#[tauri::command]
pub fn bookmark_folder_add(
    app: AppHandle,
    state: State<'_, App>,
    parent: i64,
    title: String,
) -> Result<i64, String> {
    let id = state
        .store
        .add_bookmark_folder(parent, &title)
        .map_err(text)?;
    bookmarks_changed(&app);
    Ok(id)
}

/// Правка закладки из пузыря или диспетчера: название, адрес, папка.
#[tauri::command]
pub fn bookmark_update(
    app: AppHandle,
    state: State<'_, App>,
    id: i64,
    title: String,
    url: Option<String>,
    parent: Option<i64>,
) -> Result<(), String> {
    let store = &state.store;
    store
        .update_bookmark(id, &title, url.as_deref())
        .map_err(text)?;
    if let Some(parent) = parent {
        let current = store.bookmark(id).map_err(text)?.and_then(|n| n.parent_id);
        if current != Some(parent) {
            store.move_bookmark(id, parent, usize::MAX).map_err(text)?;
        }
        let _ = store.set_setting("bookmark_folder", &Value::from(parent));
    }
    bookmarks_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn bookmark_move(
    app: AppHandle,
    state: State<'_, App>,
    id: i64,
    parent: i64,
    index: usize,
) -> Result<(), String> {
    state.store.move_bookmark(id, parent, index).map_err(text)?;
    bookmarks_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn bookmark_remove(app: AppHandle, state: State<'_, App>, id: i64) -> Result<(), String> {
    state.store.remove_bookmark(id).map_err(text)?;
    bookmarks_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn bookmark_remove_url(
    app: AppHandle,
    state: State<'_, App>,
    url: String,
) -> Result<(), String> {
    state.store.remove_bookmarks_by_url(&url).map_err(text)?;
    bookmarks_changed(&app);
    Ok(())
}

/// Импорт закладок из HTML-файла любого браузера.
#[tauri::command]
pub async fn bookmarks_import(
    app: AppHandle,
    state: State<'_, App>,
) -> Result<Option<ImportReport>, String> {
    let Some(path) = pick_file(&app, "Импорт закладок", "Файл закладок", &["html", "htm"]).await
    else {
        return Ok(None);
    };
    let bytes = std::fs::read(&path).map_err(text)?;
    let tree = bookmarks_html::parse(&String::from_utf8_lossy(&bytes));
    if tree.is_empty() {
        return Err("в файле не нашлось закладок".into());
    }
    let report = state.store.import_bookmarks(&tree).map_err(text)?;
    bookmarks_changed(&app);
    Ok(Some(report))
}

#[tauri::command]
pub async fn bookmarks_export(
    app: AppHandle,
    state: State<'_, App>,
) -> Result<Option<String>, String> {
    let name = format!("bookmarks_190x4_{}.html", today());
    let Some(path) = save_file(&app, "Экспорт закладок", "Файл закладок", &["html"], name).await
    else {
        return Ok(None);
    };
    let (bar, other) = state.store.bookmarks_for_export().map_err(text)?;
    let html = bookmarks_html::render("Панель закладок", &bar, &other);
    std::fs::write(&path, html).map_err(text)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/* ── Пароли ─────────────────────────────────────────────────────────────── */

fn passwords_changed(app: &AppHandle) {
    let _ = app.emit("passwords", ());
}

#[tauri::command]
pub fn passwords_list(state: State<'_, App>) -> Result<Vec<PasswordEntry>, String> {
    state.store.password_entries().map_err(text)
}

/// Показать пароль. Единственное место, где он уходит в интерфейс открытым
/// текстом — по явному нажатию «показать» или «скопировать».
#[tauri::command]
pub async fn password_reveal(
    app: AppHandle,
    window: tauri::Window,
    id: i64,
) -> Result<String, String> {
    // Показать чужой пароль на незалоченном компьютере — ровно то, ради чего к
    // нему и садятся. Спрашиваем Windows Hello, как Chrome и Edge.
    let label = owner(&window);
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let main = handle.get_webview_window(&label);
        let confirmed = crate::hello::confirm_recent(main.as_ref(), "Показать сохранённый пароль")
            .map_err(text)?;
        if !confirmed {
            return Err("вход не подтверждён".to_string());
        }
        let state = handle.state::<App>();
        let (_, secret) = state
            .store
            .password_secret(id)
            .map_err(text)?
            .ok_or("пароль удалён")?;
        vault::reveal(&secret).map_err(text)
    })
    .await
    .map_err(text)?
}

#[tauri::command]
pub fn password_add(
    app: AppHandle,
    state: State<'_, App>,
    url: String,
    username: String,
    password: String,
) -> Result<i64, String> {
    let origin = passwords_csv::origin_of(&url)
        .or_else(|| passwords_csv::origin_of(&format!("https://{}", url.trim())))
        .ok_or("адрес сайта не распознан")?;
    if password.is_empty() {
        return Err("пароль пустой".into());
    }
    let secret = vault::protect(&password).map_err(text)?;
    let id = state
        .store
        .save_password(&origin, username.trim(), &secret)
        .map_err(text)?;
    passwords_changed(&app);
    Ok(id)
}

#[tauri::command]
pub fn password_update(
    app: AppHandle,
    state: State<'_, App>,
    id: i64,
    username: String,
    password: Option<String>,
) -> Result<(), String> {
    let secret = match password.filter(|p| !p.is_empty()) {
        Some(password) => Some(vault::protect(&password).map_err(text)?),
        None => None,
    };
    state
        .store
        .update_password(id, username.trim(), secret.as_deref())
        .map_err(text)?;
    passwords_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn password_delete(app: AppHandle, state: State<'_, App>, id: i64) -> Result<(), String> {
    state.store.delete_password(id).map_err(text)?;
    passwords_changed(&app);
    Ok(())
}

#[derive(Serialize)]
pub struct PasswordImport {
    imported: usize,
    skipped: usize,
}

/// Импорт паролей из CSV (Chrome, Edge, Firefox, Яндекс, Bitwarden…).
#[tauri::command]
pub async fn passwords_import(
    app: AppHandle,
    state: State<'_, App>,
) -> Result<Option<PasswordImport>, String> {
    let Some(path) = pick_file(&app, "Импорт паролей", "CSV", &["csv"]).await else {
        return Ok(None);
    };
    let bytes = std::fs::read(&path).map_err(text)?;
    let (logins, skipped) =
        passwords_csv::parse_logins(&String::from_utf8_lossy(&bytes)).map_err(text)?;

    let mut imported = 0;
    for login in &logins {
        let secret = vault::protect(&login.password).map_err(text)?;
        state
            .store
            .save_password(&login.origin, &login.username, &secret)
            .map_err(text)?;
        imported += 1;
    }
    passwords_changed(&app);
    Ok(Some(PasswordImport { imported, skipped }))
}

/// Экспорт в CSV. Файл открытым текстом — интерфейс предупреждает об этом
/// до вызова.
#[tauri::command]
pub async fn passwords_export(
    app: AppHandle,
    window: tauri::Window,
    state: State<'_, App>,
) -> Result<Option<String>, String> {
    // Файл с паролями открытым текстом — тоже повод спросить Windows Hello.
    let label = owner(&window);
    let handle = app.clone();
    let confirmed = tauri::async_runtime::spawn_blocking(move || {
        let main = handle.get_webview_window(&label);
        crate::hello::confirm_recent(main.as_ref(), "Экспортировать сохранённые пароли")
    })
    .await
    .map_err(text)?
    .map_err(text)?;
    if !confirmed {
        return Err("вход не подтверждён".into());
    }

    let name = format!("passwords_190x4_{}.csv", today());
    let Some(path) = save_file(&app, "Экспорт паролей", "CSV", &["csv"], name).await
    else {
        return Ok(None);
    };

    let mut logins = Vec::new();
    for entry in state.store.password_entries().map_err(text)? {
        let Some((_, secret)) = state.store.password_secret(entry.id).map_err(text)? else {
            continue;
        };
        logins.push(passwords_csv::CsvLogin {
            origin: entry.origin,
            username: entry.username,
            password: vault::reveal(&secret).map_err(text)?,
        });
    }
    let csv = passwords_csv::render_logins(&logins).map_err(text)?;
    std::fs::write(&path, csv).map_err(text)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command]
pub fn password_never_list(state: State<'_, App>) -> Result<Vec<NeverSite>, String> {
    state.store.password_never_sites().map_err(text)
}

#[tauri::command]
pub fn password_never_forget(
    app: AppHandle,
    state: State<'_, App>,
    origin: String,
) -> Result<(), String> {
    state.store.forget_password_never(&origin).map_err(text)?;
    passwords_changed(&app);
    Ok(())
}

/// Ответ на «Сохранить пароль?»: `save`, `never`, `dismiss`.
#[tauri::command]
pub fn password_offer_answer(app: AppHandle, tab: u32, action: String) -> Result<(), String> {
    passwords::answer(&app, tab, &action).map_err(text)
}

/// Заполнить форму на вкладке выбранной учёткой.
#[tauri::command]
pub fn password_fill(app: AppHandle, tab: u32, id: i64) -> Result<(), String> {
    passwords::fill(&app, tab, id).map_err(text)
}

#[derive(Serialize)]
pub struct SiteAccount {
    id: i64,
    username: String,
}

#[tauri::command]
pub fn passwords_for_site(state: State<'_, App>, url: String) -> Result<Vec<SiteAccount>, String> {
    let Some(origin) = passwords_csv::origin_of(&url) else {
        return Ok(Vec::new());
    };
    Ok(state
        .store
        .password_secrets_for(&origin)
        .map_err(text)?
        .into_iter()
        .map(|account| SiteAccount {
            id: account.id,
            username: account.username,
        })
        .collect())
}

/* ── Загрузки ───────────────────────────────────────────────────────────── */

#[tauri::command]
pub fn downloads_list(state: State<'_, App>, limit: Option<u32>) -> Result<Vec<Download>, String> {
    state.store.downloads(limit.unwrap_or(200)).map_err(text)
}

/// Действие над загрузкой: open, show, cancel, pause, resume, retry, remove, delete.
#[tauri::command]
pub async fn download_control(app: AppHandle, id: i64, action: String) -> Result<(), String> {
    transfers::control(&app, id, &action).await.map_err(text)
}

/// Масштаб, который сайт запомнил. Chrome помнит его на сайт, а не на вкладку:
/// открыл тот же сайт в новой вкладке — масштаб тот же.
#[tauri::command]
pub fn zoom_sites(state: State<'_, App>) -> Value {
    state
        .store
        .setting("zoom_sites")
        .ok()
        .flatten()
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Default::default()))
}

/// Запомнить масштаб сайта (или забыть, если он вернулся к 100%).
#[tauri::command]
pub fn zoom_site_set(
    app: AppHandle,
    state: State<'_, App>,
    host: String,
    factor: f64,
) -> Result<(), String> {
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() || host.len() > 255 {
        return Ok(());
    }
    let mut sites = match state.store.setting("zoom_sites").ok().flatten() {
        Some(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    if (factor - 1.0).abs() < 0.01 {
        sites.remove(&host);
    } else {
        sites.insert(host, Value::from(factor));
    }
    let value = Value::Object(sites);
    state
        .store
        .set_setting("zoom_sites", &value)
        .map_err(text)?;
    let _ = app.emit(
        "settings",
        serde_json::json!({ "key": "zoom_sites", "value": value }),
    );
    Ok(())
}

#[tauri::command]
pub fn downloads_clear(app: AppHandle, state: State<'_, App>) -> Result<(), String> {
    state.store.clear_downloads().map_err(text)?;
    let _ = app.emit("downloads", ());
    Ok(())
}

fn download_dir(app: &AppHandle, store: &Store) -> PathBuf {
    store
        .setting_str("download_dir")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| app.path().download_dir().ok())
        .unwrap_or_else(std::env::temp_dir)
}

#[tauri::command]
pub fn downloads_folder_open(app: AppHandle, state: State<'_, App>) -> Result<(), String> {
    let dir = download_dir(&app, &state.store);
    tauri_plugin_opener::open_path(&dir, None::<&str>).map_err(text)
}

/// Выбрать папку загрузок. Возвращает выбранный путь.
#[tauri::command]
pub async fn download_folder_pick(
    app: AppHandle,
    state: State<'_, App>,
) -> Result<Option<String>, String> {
    let current = download_dir(&app, &state.store);
    let handle = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = handle
            .dialog()
            .file()
            .set_title("Папка для загрузок")
            .set_directory(current);
        if let Some(main) = handle.get_webview_window("chrome") {
            dialog = dialog.set_parent(&main);
        }
        dialog
            .blocking_pick_folder()
            .and_then(|p| p.into_path().ok())
    })
    .await
    .map_err(text)?;

    let Some(dir) = picked else {
        return Ok(None);
    };
    let value = dir.to_string_lossy().into_owned();
    state
        .store
        .set_setting("download_dir", &Value::String(value.clone()))
        .map_err(text)?;
    apply_setting(&app, &state, "download_dir");
    let _ = app.emit(
        "settings",
        serde_json::json!({ "key": "download_dir", "value": value }),
    );
    Ok(Some(value))
}

/* ── Данные браузера и сведения ─────────────────────────────────────────── */

/// «Удалить данные о работе в браузере».
#[tauri::command]
pub fn browsing_data_clear(
    app: AppHandle,
    state: State<'_, App>,
    history: bool,
    downloads: bool,
    site_data: bool,
    cache: bool,
) -> Result<(), String> {
    if history {
        state.store.clear_history().map_err(text)?;
    }
    if downloads {
        state.store.clear_downloads().map_err(text)?;
        let _ = app.emit("downloads", ());
    }
    if site_data || cache {
        with_any_host(&app, move |host| host.clear_browsing_data(site_data, cache))?
            .map_err(text)?;
    }
    Ok(())
}

/// Сколько ждать ответа движка на запрос о разрешениях сайтов.
const ENGINE_TIMEOUT: Duration = Duration::from_secs(5);

/// Разрешения, которые пользователь дал или запретил сайтам (хранит движок).
#[tauri::command]
pub async fn site_permissions(app: AppHandle) -> Result<Vec<PermissionSetting>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        with_any_host(&app, move |host| {
            host.permission_settings(move |list| {
                let _ = tx.send(list);
            })
        })?
        .map_err(text)?;
        rx.recv_timeout(ENGINE_TIMEOUT)
            .map_err(|_| "движок не ответил".to_string())
    })
    .await
    .map_err(text)?
}

/// Забыть решение о разрешении: сайт спросит снова.
#[tauri::command]
pub async fn site_permission_reset(
    app: AppHandle,
    permission: String,
    origin: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        with_any_host(&app, move |host| {
            host.permission_reset(&permission, &origin, move |ok| {
                let _ = tx.send(ok);
            })
        })?
        .map_err(text)?;
        match rx.recv_timeout(ENGINE_TIMEOUT) {
            Ok(true) => Ok(()),
            Ok(false) => Err("движок не сбросил разрешение".to_string()),
            Err(_) => Err("движок не ответил".to_string()),
        }
    })
    .await
    .map_err(text)?
}

#[tauri::command]
pub fn about_info(app: AppHandle) -> Value {
    let webview = with_any_host(&app, |host| host.browser_version()).unwrap_or_default();
    serde_json::json!({
        "version": app.package_info().version.to_string(),
        "webview": webview,
        "profile": crate::profile_dir().to_string_lossy(),
    })
}

#[tauri::command]
pub fn profile_open() -> Result<(), String> {
    tauri_plugin_opener::open_path(crate::profile_dir(), None::<&str>).map_err(text)
}

/* ── Всплывающее окно ───────────────────────────────────────────────────── */

#[tauri::command]
pub async fn popup_open(
    app: AppHandle,
    window: tauri::Window,
    kind: String,
    anchor: popup::Anchor,
    width: f64,
    align: Option<String>,
    payload: Option<Value>,
) -> Result<(), String> {
    popup::open(
        &app,
        &owner(&window),
        &kind,
        anchor,
        width,
        align.as_deref().unwrap_or("start"),
        payload.unwrap_or(Value::Null),
    )
    .map_err(text)
}

#[tauri::command]
pub fn popup_pending(app: AppHandle, window: tauri::Window) -> Option<Value> {
    popup::take_pending(&app, &owner(&window))
}

#[tauri::command]
pub async fn popup_show(
    app: AppHandle,
    window: tauri::Window,
    height: f64,
    focus: Option<bool>,
) -> Result<f64, String> {
    popup::show(&app, &owner(&window), height, focus.unwrap_or(true)).map_err(text)
}

#[tauri::command]
pub async fn popup_resize(
    app: AppHandle,
    window: tauri::Window,
    height: f64,
) -> Result<f64, String> {
    popup::resize(&app, &owner(&window), height).map_err(text)
}

#[tauri::command]
pub async fn popup_hide(app: AppHandle, window: tauri::Window) -> Result<(), String> {
    popup::hide(&app, &owner(&window));
    Ok(())
}

/* ── Диалоги файлов ─────────────────────────────────────────────────────── */

async fn pick_file(
    app: &AppHandle,
    title: &'static str,
    filter: &'static str,
    extensions: &'static [&'static str],
) -> Option<PathBuf> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = app
            .dialog()
            .file()
            .set_title(title)
            .add_filter(filter, extensions);
        if let Some(main) = app.get_webview_window("chrome") {
            dialog = dialog.set_parent(&main);
        }
        dialog.blocking_pick_file().and_then(|p| p.into_path().ok())
    })
    .await
    .ok()
    .flatten()
}

async fn save_file(
    app: &AppHandle,
    title: &'static str,
    filter: &'static str,
    extensions: &'static [&'static str],
    name: String,
) -> Option<PathBuf> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = app
            .dialog()
            .file()
            .set_title(title)
            .add_filter(filter, extensions)
            .set_file_name(name);
        if let Ok(dir) = app.path().document_dir() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(main) = app.get_webview_window("chrome") {
            dialog = dialog.set_parent(&main);
        }
        dialog.blocking_save_file().and_then(|p| p.into_path().ok())
    })
    .await
    .ok()
    .flatten()
}

/// Дата для имени файла экспорта: `2026-09-13`.
fn today() -> String {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0) as i64;
    // Гражданская дата из дней с эпохи (алгоритм Хиннанта), без зависимостей.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/* ── Переводчик и загрузчик ─────────────────────────────────────────────── */

use browser190x4_services::{MediaInfo, Translation};

/// Перевести текст через релей 190x4.
///
/// Ключ модели живёт на сервере и в браузер не попадает — клиент знает только
/// свой ключ доступа к релею, да и тот лежит в профиле, а не в коде.
#[tauri::command]
pub async fn translate_text(
    state: State<'_, App>,
    text: String,
    target_lang: String,
) -> Result<Translation, String> {
    state
        .services
        .translate(&text, &target_lang)
        .await
        .map_err(self::text)
}

/// Что за ссылка и что с неё можно скачать.
#[tauri::command]
pub async fn media_probe(state: State<'_, App>, url: String) -> Result<MediaInfo, String> {
    tracing::info!(%url, "разбираем ссылку");
    state.services.media_info(&url).await.map_err(text)
}

/// Скачать медиа: ставим задание на сервере, следим за прогрессом, забираем файл.
///
/// Всё это — фоновая задача: интерфейс получает события `media` (для
/// расширения) и `download` (для общего списка загрузок), а не ждёт ответа
/// команды. Ролик может качаться минутами.
#[tauri::command]
pub async fn media_download(
    app: AppHandle,
    state: State<'_, App>,
    url: String,
    format: String,
) -> Result<String, String> {
    let services = state.services.clone();
    let store = state.store.clone();

    tracing::info!(%url, %format, "загрузка медиа");
    let job_id = services.media_start(&url, &format).await.map_err(|err| {
        tracing::error!(%err, "загрузка не началась");
        text(err)
    })?;

    let target_dir = download_dir(&app, &store);
    let id = store
        .start_download(DownloadKind::Media, &url, "", None)
        .map_err(text)?;
    state.transfers.register_media(id, job_id.clone());
    transfers::emit_media(&app, id, "started", &url, "", 0, None, "");

    let job = job_id.clone();
    tauri::async_runtime::spawn(async move {
        let mut name = String::new();

        let fail = |app: &AppHandle, phase: &str, bytes: i64, name: &str, error: &str| {
            let final_state = if phase == "cancelled" {
                DownloadState::Cancelled
            } else {
                DownloadState::Failed
            };
            let _ = store.finish_download(id, bytes, final_state, error);
            transfers::emit_media(app, id, phase, &url, name, bytes, None, error);
            let _ = app.emit(
                "media",
                serde_json::json!({ "job": job, "phase": phase, "error": error }),
            );
            app.state::<App>().transfers.finish_media(id);
        };

        loop {
            tokio::time::sleep(std::time::Duration::from_millis(700)).await;

            let progress = match services.media_progress(&job).await {
                Ok(progress) => progress,
                Err(err) => {
                    fail(&app, "failed", 0, &name, &text(err));
                    return;
                }
            };

            let total = (progress.total > 0).then_some(progress.total);
            if !progress.file_name.is_empty() && progress.file_name != name {
                name = progress.file_name.clone();
                let _ = store.set_download_target(id, &name, total);
            }
            let _ = store.update_download(id, progress.downloaded, DownloadState::Running);
            transfers::emit_media(
                &app,
                id,
                "progress",
                &url,
                &name,
                progress.downloaded,
                total,
                "",
            );
            let _ = app.emit(
                "media",
                serde_json::json!({
                    "job": job,
                    "phase": "progress",
                    "percent": progress.percent,
                    "downloaded": progress.downloaded,
                    "total": progress.total,
                    "file_name": progress.file_name,
                }),
            );

            if progress.failed() {
                let cancelled = progress.status == "cancelled"
                    || progress.error.as_deref() == Some("cancelled");
                tracing::error!(job = %job, status = %progress.status, error = ?progress.error, "загрузка не удалась");
                if cancelled {
                    fail(&app, "cancelled", progress.downloaded, &name, "");
                } else {
                    let error = progress
                        .error
                        .clone()
                        .unwrap_or_else(|| "загрузка прервалась".into());
                    fail(&app, "failed", progress.downloaded, &name, &error);
                }
                return;
            }

            if progress.finished() {
                match services
                    .media_fetch(&job, &target_dir, &progress.file_name)
                    .await
                {
                    Ok(path) => {
                        let path_text = path.to_string_lossy().to_string();
                        let bytes = tokio::fs::metadata(&path)
                            .await
                            .map(|meta| meta.len() as i64)
                            .unwrap_or(progress.downloaded);
                        let _ = store.set_download_target(id, &path_text, Some(bytes));
                        let _ = store.finish_download(id, bytes, DownloadState::Done, "");
                        transfers::emit_media(
                            &app,
                            id,
                            "done",
                            &url,
                            &path_text,
                            bytes,
                            Some(bytes),
                            "",
                        );
                        let _ = app.emit(
                            "media",
                            serde_json::json!({ "job": job, "phase": "done", "path": path_text }),
                        );
                        app.state::<App>().transfers.finish_media(id);
                    }
                    Err(err) => {
                        tracing::error!(job = %job, %err, "файл не забран с сервера");
                        fail(&app, "failed", progress.downloaded, &name, &text(err));
                    }
                }
                return;
            }
        }
    });

    Ok(job_id)
}

/// Отменить загрузку видео из окна расширения.
#[tauri::command]
pub async fn media_cancel(state: State<'_, App>, job: String) -> Result<(), String> {
    state.services.media_cancel(&job).await.map_err(text)
}

/// Настроены ли сервисы — интерфейс не должен предлагать то, чего нет.
#[tauri::command]
pub fn services_state(state: State<'_, App>) -> Value {
    let config = state.services.config();
    serde_json::json!({
        "translate": config.translate_enabled(),
        "media": config.media_enabled(),
    })
}
