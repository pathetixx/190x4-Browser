//! Новая вкладка: плитки сайтов, погода и монитор ресурсов.
//!
//! Страница — обычная вкладка на `http://190x4-pages.invalid/newtab.html`, IPC
//! Tauri у неё нет. Запросы приходят через `postMessage`, ответы уходят через
//! `PostWebMessageAsJson`. Запросы принимаются только от страниц браузера
//! (адрес документа сообщает движок), а ответ отправляется, только если вкладка
//! всё ещё на странице браузера: в отчёте о ресурсах есть заголовки других вкладок.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use browser190x4_store::Store;
use browser190x4_webview::{TabId, PAGES_HOST};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::resources::Monitor;
use crate::state::{with_host, with_host_later, App};
use crate::weather::{self, Place};

/// Прогноз для того же места берётся из памяти.
const WEATHER_TTL: Duration = Duration::from_secs(20 * 60);
/// Автоматически найденное место переспрашивается не чаще.
const LOCATE_TTL: Duration = Duration::from_secs(60 * 60);
const MAX_TILES: usize = 24;

/// Строка браузера: иначе часть сайтов отдаёт вместо главной страницу-заглушку
/// без значков.
const BROWSER_AGENT: &str = concat!(
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) ",
    "Chrome/140.0.0.0 Safari/537.36 190x4Browser/",
    env!("CARGO_PKG_VERSION")
);

const DEFAULT_TILES: [(&str, &str); 6] = [
    ("Кинопоиск", "https://www.kinopoisk.ru/"),
    ("YouTube", "https://www.youtube.com/"),
    ("GitHub", "https://github.com/"),
    ("Хабр", "https://habr.com/ru/"),
    ("Telegram", "https://web.telegram.org/"),
    ("Дзен", "https://dzen.ru/"),
];

#[derive(Default)]
pub struct NewTab {
    client: OnceLock<reqwest::Client>,
    weather: Mutex<Option<(Instant, String, Value)>>,
    located: Mutex<Option<(Instant, Place, &'static str)>>,
    pub monitor: Monitor,
}

impl NewTab {
    fn client(&self) -> &reqwest::Client {
        self.client.get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(12))
                .connect_timeout(Duration::from_secs(6))
                .user_agent(BROWSER_AGENT)
                .build()
                .unwrap_or_default()
        })
    }

    async fn located(&self) -> anyhow::Result<(Place, &'static str)> {
        let cached = self.located.lock().clone();
        if let Some((at, place, source)) = cached {
            if at.elapsed() < LOCATE_TTL {
                return Ok((place, source));
            }
        }
        let (place, source) = weather::locate(self.client()).await?;
        *self.located.lock() = Some((Instant::now(), place.clone(), source));
        Ok((place, source))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tile {
    pub title: String,
    pub url: String,
}

#[derive(Deserialize)]
#[serde(tag = "evt")]
enum PageEvent {
    #[serde(rename = "newtab_init")]
    Init,
    #[serde(rename = "newtab_tiles")]
    Tiles { tiles: Vec<Tile> },
    #[serde(rename = "newtab_prefs")]
    Prefs {
        weather: Option<bool>,
        monitor: Option<bool>,
        seconds: Option<bool>,
    },
    #[serde(rename = "newtab_icon")]
    Icon { url: String },
    #[serde(rename = "newtab_weather")]
    Weather {
        #[serde(default)]
        force: bool,
    },
    #[serde(rename = "newtab_city_search")]
    CitySearch { query: String },
    #[serde(rename = "newtab_city_set")]
    CitySet { city: Option<Place> },
    #[serde(rename = "newtab_resources")]
    Resources,
}

fn from_pages(url: &str) -> bool {
    url.strip_prefix("http://")
        .and_then(|rest| rest.strip_prefix(PAGES_HOST))
        .is_some_and(|rest| rest.starts_with('/'))
}

/// Сообщение со страницы. `true` — оно наше и дальше не идёт.
pub fn handle_message(app: &AppHandle, tab: u32, source: &str, payload: &str) -> bool {
    let Ok(event) = serde_json::from_str::<PageEvent>(payload) else {
        return false;
    };
    if !from_pages(source) {
        return true;
    }
    let app = app.clone();

    match event {
        PageEvent::Init => post_later(&app, tab, page_state(&app)),
        PageEvent::Tiles { tiles } => {
            let tiles: Vec<Tile> = tiles
                .into_iter()
                .filter_map(clean_tile)
                .take(MAX_TILES)
                .collect();
            if let Err(err) = store(&app).set_setting("newtab_tiles", &json!(tiles)) {
                tracing::warn!(%err, "плитки новой вкладки не сохранены");
            }
            post_later(&app, tab, page_state(&app));
        }
        PageEvent::Prefs {
            weather,
            monitor,
            seconds,
        } => {
            for (key, value) in [
                ("newtab_weather", weather),
                ("newtab_monitor", monitor),
                ("newtab_seconds", seconds),
            ] {
                if let Some(value) = value {
                    let _ = store(&app).set_setting(key, &Value::Bool(value));
                }
            }
            post_later(&app, tab, page_state(&app));
        }
        PageEvent::Icon { url } => {
            tauri::async_runtime::spawn(async move {
                let newtab = app.state::<NewTab>();
                let dir = crate::profile_dir().join("site-icons");
                let icon = crate::site_icons::icon(newtab.client(), &dir, &url).await;
                let message = json!({ "type": "icon", "url": url, "icon": icon });
                post_soon(app.clone(), tab, message);
            });
        }
        PageEvent::Weather { force } => {
            tauri::async_runtime::spawn(async move {
                let message = weather_message(&app, force).await;
                post_soon(app.clone(), tab, message);
            });
        }
        PageEvent::CitySearch { query } => {
            let query = query.trim().chars().take(80).collect::<String>();
            if query.chars().count() < 2 {
                return true;
            }
            tauri::async_runtime::spawn(async move {
                let newtab = app.state::<NewTab>();
                let message = match weather::search(newtab.client(), &query).await {
                    Ok(places) => json!({ "type": "cities", "query": query, "places": places }),
                    Err(err) => {
                        tracing::debug!(%err, "города не найдены");
                        json!({ "type": "cities", "query": query, "error": "Поиск городов сейчас недоступен" })
                    }
                };
                post_soon(app.clone(), tab, message);
            });
        }
        PageEvent::CitySet { city } => {
            let value = match city.filter(Place::is_valid) {
                Some(place) => json!(place),
                None => Value::Null,
            };
            let _ = store(&app).set_setting("newtab_city", &value);
            *app.state::<NewTab>().weather.lock() = None;
            post_later(&app, tab, page_state(&app));
            tauri::async_runtime::spawn(async move {
                let message = weather_message(&app, false).await;
                post_soon(app.clone(), tab, message);
            });
        }
        PageEvent::Resources => {
            tauri::async_runtime::spawn_blocking(move || {
                let report = app.state::<NewTab>().monitor.report(&app);
                post(&app, tab, json!({ "type": "resources", "report": report }));
            });
        }
    }
    true
}

fn store(app: &AppHandle) -> std::sync::Arc<Store> {
    app.state::<App>().store.clone()
}

/// Всё, что странице нужно для первой отрисовки.
fn page_state(app: &AppHandle) -> Value {
    let store = store(app);
    json!({
        "type": "state",
        "tiles": tiles(&store),
        "city": store.setting("newtab_city").ok().flatten().filter(Value::is_object),
        "weather": store.setting_bool("newtab_weather", true),
        "monitor": store.setting_bool("newtab_monitor", true),
        "seconds": store.setting_bool("newtab_seconds", true),
        "theme": store.setting_str("theme").unwrap_or_else(|| "kurogane".to_string()),
    })
}

fn tiles(store: &Store) -> Vec<Tile> {
    match store.setting("newtab_tiles").ok().flatten() {
        Some(value) => serde_json::from_value::<Vec<Tile>>(value)
            .unwrap_or_default()
            .into_iter()
            .filter_map(clean_tile)
            .collect(),
        None => DEFAULT_TILES
            .iter()
            .map(|(title, url)| Tile {
                title: title.to_string(),
                url: url.to_string(),
            })
            .collect(),
    }
}

/// Плитка со страницы: адрес только http(s), без схемы — https; пустое название
/// заменяется адресом сайта.
fn clean_tile(tile: Tile) -> Option<Tile> {
    let raw = tile.url.trim();
    if raw.is_empty() || raw.len() > 2048 {
        return None;
    }
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let url = reqwest::Url::parse(&with_scheme).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.to_string();
    let title: String = tile.title.trim().chars().take(40).collect();
    let title = if title.is_empty() {
        host.strip_prefix("www.").unwrap_or(&host).to_string()
    } else {
        title
    };
    Some(Tile {
        title,
        url: url.to_string(),
    })
}

async fn weather_message(app: &AppHandle, force: bool) -> Value {
    let newtab = app.state::<NewTab>();
    let city = store(app)
        .setting("newtab_city")
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value::<Place>(value).ok())
        .filter(Place::is_valid);

    let (place, source) = match city {
        Some(place) => (place, "city"),
        None => match newtab.located().await {
            Ok(found) => found,
            Err(err) => {
                tracing::debug!(%err, "место для погоды не найдено");
                return json!({ "type": "weather", "error": "Не удалось определить, где вы. Выберите город вручную." });
            }
        },
    };

    let key = format!("{:.2},{:.2}", place.lat, place.lon);
    if !force {
        let cached = newtab.weather.lock().clone();
        if let Some((at, cached_key, data)) = cached {
            if cached_key == key && at.elapsed() < WEATHER_TTL {
                return json!({ "type": "weather", "place": place, "source": source, "data": data });
            }
        }
    }

    match weather::forecast(newtab.client(), &place).await {
        Ok(data) => {
            *newtab.weather.lock() = Some((Instant::now(), key, data.clone()));
            json!({ "type": "weather", "place": place, "source": source, "data": data })
        }
        Err(err) => {
            tracing::debug!(%err, "прогноз не получен");
            json!({ "type": "weather", "place": place, "source": source, "error": "Прогноз сейчас недоступен" })
        }
    }
}

/// Ответ странице. Блокирует до главного потока — звать не с него.
fn post(app: &AppHandle, tab: u32, message: Value) {
    let json = message.to_string();
    let _ = with_host(app, move |host| {
        host.with_tab(TabId(tab), |view| {
            if from_pages(&view.source_url()) {
                if let Err(err) = view.post(&json) {
                    tracing::debug!(%err, "ответ новой вкладке не отправлен");
                }
            }
        });
    });
}

/// Ответ из асинхронной задачи: ожидание главного потока — в пуле блокирующих.
fn post_soon(app: AppHandle, tab: u32, message: Value) {
    tauri::async_runtime::spawn_blocking(move || post(&app, tab, message));
}

/// Ответ из обработчика события вкладки — он сам на главном потоке.
fn post_later(app: &AppHandle, tab: u32, message: Value) {
    let json = message.to_string();
    with_host_later(app, move |host| {
        host.with_tab(TabId(tab), |view| {
            if from_pages(&view.source_url()) {
                let _ = view.post(&json);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(title: &str, url: &str) -> Tile {
        Tile {
            title: title.to_string(),
            url: url.to_string(),
        }
    }

    #[test]
    fn tiles_from_page_are_cleaned() {
        assert_eq!(
            clean_tile(tile("  ", "www.example.com/path")),
            Some(tile("example.com", "https://www.example.com/path"))
        );
        assert_eq!(
            clean_tile(tile("Почта", "http://mail.example")),
            Some(tile("Почта", "http://mail.example/"))
        );
        assert_eq!(clean_tile(tile("x", "javascript:alert(1)")), None);
        assert_eq!(clean_tile(tile("x", "file:///C:/Windows")), None);
        assert_eq!(clean_tile(tile("x", "")), None);
    }

    #[test]
    fn only_browser_pages_are_trusted() {
        assert!(from_pages("http://190x4-pages.invalid/newtab.html"));
        assert!(!from_pages(
            "http://190x4-pages.invalid.evil.com/newtab.html"
        ));
        assert!(!from_pages(
            "https://example.com/?http://190x4-pages.invalid/"
        ));
    }
}
