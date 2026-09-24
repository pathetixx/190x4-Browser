//! Расширение SponsorBlock: сегменты видео YouTube (спонсорские вставки,
//! просьбы подписаться, заставки), которые встроенный скрипт плеера
//! пропускает, предлагает пропустить или отмечает на полосе прокрутки.
//!
//! Сегменты размечает сообщество SponsorBlock (sponsor.ajay.app, данные — CC
//! BY-NC-SA 4.0). Запрос анонимный, как у официального расширения: на сервер
//! уходит не номер видео, а первые знаки его SHA-256, и сервер отдаёт сегменты
//! всех видео с таким началом — нужное выбирается здесь.
//!
//! Скрипт (`crates/webview/src/inject/sponsorblock.js`) живёт в каждом
//! документе и фрейме, но работает только на YouTube: сообщает номер видео и
//! ждёт сегменты. Сеть и настройки — здесь; сообщению страницы верят только в
//! номере видео, адрес документа берётся у движка.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use browser190x4_store::Store;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};

use crate::state::App;

const API: &str = "https://sponsor.ajay.app/api/skipSegments/";
/// Сколько знаков хеша уходит на сервер: четыре — как у официального
/// расширения, под каждым началом сотни видео.
const PREFIX: usize = 4;
/// Сегменты видео берутся из памяти: разметку правят редко, а одно видео
/// открывают снова и снова (перемотка, соседняя вкладка, возврат назад).
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const CACHE_LIMIT: usize = 64;
const TIMEOUT: Duration = Duration::from_secs(8);

/// Категории SponsorBlock, которые знает браузер, и режим каждой по умолчанию.
/// Те же умолчания — в `ui/js/prefs.js` (`sponsorblock_<категория>`).
pub const CATEGORIES: [(&str, Mode); 8] = [
    ("sponsor", Mode::Skip),
    ("selfpromo", Mode::Show),
    ("interaction", Mode::Show),
    ("intro", Mode::Show),
    ("outro", Mode::Show),
    ("preview", Mode::Show),
    ("filler", Mode::Off),
    ("music_offtopic", Mode::Off),
];

/// Что делать с сегментом категории.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Пропускать самому и предлагать вернуться.
    Skip,
    /// Показывать кнопку «Пропустить», пока идёт сегмент.
    Ask,
    /// Только отметка на полосе прокрутки.
    Show,
    Off,
}

impl Mode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "skip" => Some(Self::Skip),
            "ask" => Some(Self::Ask),
            "show" => Some(Self::Show),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

/// Сегмент для скрипта плеера.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub category: String,
    pub mode: Mode,
    pub uuid: String,
    /// Длина видео по разметке — для отметок, пока плеер свою не знает.
    pub duration: f64,
}

/// Сегмент в ответе сервера — до выбора по настройкам.
#[derive(Debug, Clone, Deserialize)]
struct RawSegment {
    segment: [f64; 2],
    #[serde(rename = "UUID", default)]
    uuid: String,
    #[serde(default)]
    category: String,
    #[serde(rename = "actionType", default)]
    action: String,
    #[serde(rename = "videoDuration", default)]
    duration: f64,
}

#[derive(Debug, Deserialize)]
struct RawVideo {
    #[serde(rename = "videoID")]
    video: String,
    #[serde(default)]
    segments: Vec<RawSegment>,
}

#[derive(Default)]
pub struct SponsorBlock {
    cache: Mutex<HashMap<String, (Instant, Vec<RawSegment>)>>,
}

#[derive(Deserialize)]
#[serde(tag = "evt")]
enum PageEvent {
    /// Плеер показывает это видео — пришлите его сегменты.
    #[serde(rename = "sponsorblock_video")]
    Video { video: String },
}

/// Сообщение скрипта плеера. `true` — оно наше и дальше не идёт.
pub fn handle_message(
    app: &AppHandle,
    tab: u32,
    frame: Option<u32>,
    source: &str,
    payload: &str,
) -> bool {
    let Ok(PageEvent::Video { video }) = serde_json::from_str::<PageEvent>(payload) else {
        return false;
    };
    // Адрес документа — от движка: чужой сайт выдать себя за YouTube не может.
    let Some(origin) = youtube_origin(source) else {
        return true;
    };
    if !valid_video(&video) {
        return true;
    }
    let app = app.clone();
    // Сообщение пришло на главный поток: сеть и база — в фоне.
    tauri::async_runtime::spawn(async move {
        let store = app.state::<App>().store.clone();
        if !store.setting_bool("ext_sponsorblock_enabled", true) {
            return;
        }
        let modes = modes(&store);
        if modes.values().all(|mode| *mode == Mode::Off) {
            return;
        }
        let raw = match segments_of(&app, &video).await {
            Ok(raw) => raw,
            Err(err) => {
                tracing::debug!(%err, "сегменты SponsorBlock не получены");
                return;
            }
        };
        let segments = choose(&raw, &modes);
        let message = json!({
            "cmd": "sponsorblock_segments",
            "origin": origin,
            "video": video,
            "segments": segments,
        })
        .to_string();
        crate::state::later(&app, tab, move |host| {
            host.with_tab(browser190x4_webview::TabId(tab), |view| {
                if let Err(err) = view.post_to(frame, &message) {
                    tracing::debug!(%err, "сегменты не отправлены плееру");
                }
            });
        });
    });
    true
}

/// Режим каждой категории из настроек, без настройки — умолчание.
fn modes(store: &Store) -> HashMap<&'static str, Mode> {
    CATEGORIES
        .iter()
        .map(|(category, default)| {
            let mode = store
                .setting_str(&format!("sponsorblock_{category}"))
                .and_then(|value| Mode::parse(&value))
                .unwrap_or(*default);
            (*category, mode)
        })
        .collect()
}

/// Сегменты видео: из памяти или с сервера — все категории сразу, чтобы смена
/// настроек не требовала нового запроса.
async fn segments_of(app: &AppHandle, video: &str) -> anyhow::Result<Vec<RawSegment>> {
    let state = app.state::<SponsorBlock>();
    if let Some((at, segments)) = state.cache.lock().get(video) {
        if at.elapsed() < CACHE_TTL {
            return Ok(segments.clone());
        }
    }

    let categories = serde_json::to_string(
        &CATEGORIES
            .iter()
            .map(|(category, _)| *category)
            .collect::<Vec<_>>(),
    )?;
    let client = app.state::<crate::newtab::NewTab>().client().clone();
    let response = client
        .get(format!("{API}{}", hash_prefix(video)))
        .query(&[("categories", categories.as_str())])
        .timeout(TIMEOUT)
        .send()
        .await?;
    // 404 — ни у одного видео с таким началом хеша разметки нет.
    let videos: Vec<RawVideo> = match response.status() {
        reqwest::StatusCode::NOT_FOUND => Vec::new(),
        status if status.is_success() => serde_json::from_str(&response.text().await?)?,
        status => anyhow::bail!("SponsorBlock ответил {status}"),
    };
    let segments: Vec<RawSegment> = videos
        .into_iter()
        .filter(|entry| entry.video == video)
        .flat_map(|entry| entry.segments)
        .collect();

    let mut cache = state.cache.lock();
    if cache.len() >= CACHE_LIMIT {
        cache.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
        if cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
    }
    cache.insert(video.to_string(), (Instant::now(), segments.clone()));
    Ok(segments)
}

/// Что отдать плееру: только пропуски (не метки глав и не «всё видео —
/// реклама»), только включённые категории, по порядку во времени.
fn choose(raw: &[RawSegment], modes: &HashMap<&'static str, Mode>) -> Vec<Segment> {
    let mut out: Vec<Segment> = raw
        .iter()
        .filter(|segment| segment.action.is_empty() || segment.action == "skip")
        .filter_map(|segment| {
            let (&category, &mode) = modes.get_key_value(segment.category.as_str())?;
            let [start, end] = segment.segment;
            let valid = start.is_finite() && end.is_finite() && start >= 0.0 && end - start >= 0.5;
            (mode != Mode::Off && valid).then(|| Segment {
                start,
                end,
                category: category.to_string(),
                mode,
                uuid: segment.uuid.chars().take(80).collect(),
                duration: if segment.duration.is_finite() {
                    segment.duration.max(0.0)
                } else {
                    0.0
                },
            })
        })
        .collect();
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// Первые знаки SHA-256 номера видео, шестнадцатеричные.
fn hash_prefix(video: &str) -> String {
    Sha256::digest(video.as_bytes())
        .iter()
        .take(PREFIX / 2)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Номер видео YouTube: 11 знаков из латиницы, цифр, `-` и `_`.
fn valid_video(video: &str) -> bool {
    video.len() == 11
        && video
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Origin документа, если это YouTube (с поддоменами) или его плеер без кук.
fn youtube_origin(source: &str) -> Option<String> {
    let url = reqwest::Url::parse(source).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let host = url.host_str()?;
    let youtube = ["youtube.com", "youtube-nocookie.com"]
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")));
    youtube.then(|| url.origin().ascii_serialization())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(start: f64, end: f64, category: &str, action: &str) -> RawSegment {
        RawSegment {
            segment: [start, end],
            uuid: format!("{category}-{start}"),
            category: category.to_string(),
            action: action.to_string(),
            duration: 600.0,
        }
    }

    #[test]
    fn hash_prefix_is_the_start_of_sha256() {
        // sha256("dQw4w9WgXcQ") = 5f6b…
        assert_eq!(hash_prefix("dQw4w9WgXcQ"), "5f6b");
    }

    #[test]
    fn video_ids_are_checked() {
        assert!(valid_video("dQw4w9WgXcQ"));
        assert!(valid_video("a-b_c1234XY"));
        assert!(!valid_video("dQw4w9WgXc"));
        assert!(!valid_video("dQw4w9WgXc/"));
        assert!(!valid_video("../../etc/p"));
    }

    #[test]
    fn only_youtube_documents_are_answered() {
        assert_eq!(
            youtube_origin("https://www.youtube.com/watch?v=dQw4w9WgXcQ").as_deref(),
            Some("https://www.youtube.com")
        );
        assert_eq!(
            youtube_origin("https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ").as_deref(),
            Some("https://www.youtube-nocookie.com")
        );
        assert!(youtube_origin("https://m.youtube.com/watch?v=x").is_some());
        assert!(youtube_origin("https://youtube.com.evil.example/watch").is_none());
        assert!(youtube_origin("https://notyoutube.com/watch").is_none());
        assert!(youtube_origin("http://www.youtube.com/watch").is_none());
    }

    #[test]
    fn segments_follow_the_settings() {
        let modes: HashMap<&'static str, Mode> = CATEGORIES.iter().copied().collect();
        let chosen = choose(
            &[
                raw(120.0, 150.0, "sponsor", "skip"),
                raw(0.0, 12.0, "intro", "skip"),
                raw(300.0, 320.0, "filler", "skip"),
                raw(400.0, 400.2, "sponsor", "skip"),
                raw(0.0, 600.0, "sponsor", "full"),
                raw(200.0, 210.0, "sponsor", "mute"),
                raw(500.0, 510.0, "unknown", "skip"),
            ],
            &modes,
        );
        let got: Vec<(&str, f64, Mode)> = chosen
            .iter()
            .map(|segment| (segment.category.as_str(), segment.start, segment.mode))
            .collect();
        assert_eq!(
            got,
            vec![("intro", 0.0, Mode::Show), ("sponsor", 120.0, Mode::Skip)]
        );
    }
}
