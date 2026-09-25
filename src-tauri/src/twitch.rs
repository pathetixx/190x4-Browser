//! Расширение Twitch: лучшее качество трансляций, бонусы баллов канала сами и
//! смайлы 7TV, BetterTTV и FrankerFaceZ в чате.
//!
//! Качество. Twitch решает, какие качества дать, по стране зрителя — и в токене
//! просмотра, и в главном плейлисте трансляции. Из России выше 720p он не даёт.
//! Поэтому главный плейлист (`usher.ttvnw.net/api/…/channel/hls/…`) браузер
//! берёт сам через сервер за пределами России: тот получает токен и плейлист у
//! Twitch от себя и отдаёт плейлист со всеми качествами. Движок держит запрос
//! плеера под отсрочкой (`crates/webview/src/filter.rs`), пока не придёт ответ.
//! Само видео идёт с серверов Twitch напрямую, через сервер — только плейлист,
//! несколько килобайт. Сервер не ответил — запрос уходит к Twitch как был, и
//! трансляция играет в том качестве, что дают.
//!
//! По умолчанию сервер — прокси ReYohoho (github.com/reyohoho/twitch_quality_proxy);
//! подойдёт любой с тем же устройством адреса: `<сервер><адрес плейлиста>`.
//! Токен входа Twitch ему уходит, только если человек сам это включил
//! (`twitch_proxy_token`): с ним сервер берёт плейлист от имени зрителя —
//! без рекламной заставки и с 1440p.
//!
//! Смайлы и бонусы — скрипт страницы (`crates/webview/src/inject/twitch.js`).
//! Списки смайлов браузер берёт у API BetterTTV (там же и смайлы FFZ) и 7TV
//! и отдаёт странице готовыми.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use browser190x4_store::Store;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::json;
use tauri::{AppHandle, Manager};

use crate::state::App;

/// Серверы по умолчанию — по очереди, если первый не ответил.
const PROXIES: [&str; 4] = [
    "https://proxy4.rte.net.ru/",
    "https://proxy7.rte.net.ru/",
    "https://proxy5.rte.net.ru/",
    "https://proxy6.rte.net.ru/",
];
/// Сколько серверов пробовать на один плейлист: плеер ждёт.
const TRIES: usize = 2;
const PLAYLIST_TIMEOUT: Duration = Duration::from_secs(5);
const PLAYLIST_LIMIT: usize = 512 * 1024;
/// Каналы только для России: их плейлист через заграничный сервер не отдаётся.
/// Список ведёт сервер ReYohoho.
const RUSSIA_ONLY_TTL: Duration = Duration::from_secs(10 * 60);

const API_TIMEOUT: Duration = Duration::from_secs(8);
const GLOBAL_TTL: Duration = Duration::from_secs(60 * 60);
const CHANNEL_TTL: Duration = Duration::from_secs(10 * 60);
const CACHE_LIMIT: usize = 64;
/// Смайлов на канал — не больше: столько чат не покажет, а сообщение странице
/// не должно расти без края.
const EMOTES_LIMIT: usize = 4000;
/// Публичный Client-ID веб-клиента Twitch: без него GQL не отвечает даже на
/// «какой номер у канала».
const TWITCH_CLIENT_ID: &str = "kimne78kx3ncx6brgo4mv6wki5h1ko";

/// Смайл из набора: код, картинка 1x и 2x, откуда он.
type Emote = (String, String, String, &'static str);
/// Смайл для страницы: то же и чей он — `channel` или `global`.
type PageEmote = (String, String, String, &'static str, &'static str);
/// Наборы смайлов по ключу `поставщик:global` или `поставщик:<номер канала>`.
type EmoteCache = HashMap<String, (Instant, Arc<Vec<Emote>>)>;
/// Номера каналов по логину; `None` — такого канала нет.
type ChannelIds = HashMap<String, (Instant, Option<String>)>;
/// Каналы только для России и когда список спрашивали.
type RussiaOnly = (Option<Instant>, HashSet<String>);

#[derive(Default)]
pub struct Twitch {
    emotes: Mutex<EmoteCache>,
    ids: Mutex<ChannelIds>,
    /// Токен входа зрителя — только пока включена его передача серверу.
    token: Mutex<Option<String>>,
    russia_only: Mutex<RussiaOnly>,
}

#[derive(Deserialize)]
#[serde(tag = "evt")]
enum PageEvent {
    /// Страница открыла канал (или ушла с него — тогда `channel` пустой).
    #[serde(rename = "twitch_channel")]
    Channel {
        #[serde(default)]
        channel: String,
    },
    /// Токен входа — в ответ на `token: true` в настройках.
    #[serde(rename = "twitch_token")]
    Token { token: String },
}

/// Включён ли перехват плейлистов — движку для всех вкладок.
pub fn init(store: &Store) {
    browser190x4_webview::filter::set_twitch_playlists(
        store.setting_bool("ext_twitch_enabled", true)
            && store.setting_bool("twitch_quality", true),
    );
}

/// Настройки расширения сменились. Открытые страницы Twitch берут их сразу:
/// скрипт заново сообщает канал и получает в ответ настройки и смайлы.
pub fn apply(app: &AppHandle) {
    let state = app.state::<App>();
    init(&state.store);
    if !state.store.setting_bool("twitch_proxy_token", false) {
        *app.state::<Twitch>().token.lock() = None;
    }
    for label in state.windows.labels() {
        let _ = crate::state::with_host(app, &label, |host| {
            for brief in host.tabs_brief() {
                let Some(origin) = twitch_origin(&brief.url) else {
                    continue;
                };
                let message = json!({ "cmd": "twitch_refresh", "origin": origin }).to_string();
                host.with_tab(browser190x4_webview::TabId(brief.id), |view| {
                    if let Err(err) = view.post(&message) {
                        tracing::debug!(%err, "странице Twitch не сказали о новых настройках");
                    }
                });
            }
        });
    }
}

/// Сообщение скрипта Twitch. `true` — оно наше и дальше не идёт.
pub fn handle_message(app: &AppHandle, tab: u32, source: &str, payload: &str) -> bool {
    let Ok(event) = serde_json::from_str::<PageEvent>(payload) else {
        return false;
    };
    // Адрес — от движка: чужая страница выдать себя за Twitch не может.
    let Some(origin) = twitch_origin(source) else {
        return true;
    };
    match event {
        PageEvent::Token { token } => {
            let store = &app.state::<App>().store;
            if store.setting_bool("twitch_proxy_token", false) && valid_token(&token) {
                *app.state::<Twitch>().token.lock() = Some(token);
            }
        }
        PageEvent::Channel { channel } => {
            let channel = channel.to_ascii_lowercase();
            let channel = valid_login(&channel).then_some(channel);
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                send_channel(&app, tab, &origin, channel).await;
            });
        }
    }
    true
}

/// Настройки для скрипта и, если смайлы включены, смайлы канала.
async fn send_channel(app: &AppHandle, tab: u32, origin: &str, channel: Option<String>) {
    let store = app.state::<App>().store.clone();
    let on = store.setting_bool("ext_twitch_enabled", true);
    let bttv = on && store.setting_bool("twitch_bttv", true);
    let ffz = on && store.setting_bool("twitch_ffz", true);
    let seventv = on && store.setting_bool("twitch_7tv", true);
    let config = json!({
        "cmd": "twitch_config",
        "origin": origin,
        "enabled": on,
        "points": on && store.setting_bool("twitch_points", true),
        "emotes": bttv || ffz || seventv,
        "token": on && store.setting_bool("twitch_quality", true)
            && store.setting_bool("twitch_proxy_token", false),
    })
    .to_string();
    post(app, tab, config);
    let providers = Providers { bttv, ffz, seventv };
    if !(bttv || ffz || seventv) {
        return;
    }

    let emotes = emotes_for(app, channel.as_deref(), providers).await;
    tracing::debug!(tab, ?channel, count = emotes.len(), "смайлы Twitch");
    let message = json!({
        "cmd": "twitch_emotes",
        "origin": origin,
        "channel": channel.unwrap_or_default(),
        "emotes": emotes,
    })
    .to_string();
    post(app, tab, message);
}

fn post(app: &AppHandle, tab: u32, message: String) {
    crate::state::later(app, tab, move |host| {
        host.with_tab(browser190x4_webview::TabId(tab), |view| {
            if let Err(err) = view.post(&message) {
                tracing::debug!(%err, "сообщение странице Twitch не отправлено");
            }
        });
    });
}

/// Какие сервисы смайлов включены.
#[derive(Clone, Copy)]
struct Providers {
    bttv: bool,
    ffz: bool,
    seventv: bool,
}

/// Смайлы чата: общие и канала, FFZ, BTTV и 7TV. Смайл канала с тем же кодом
/// перекрывает общий, 7TV — BTTV, BTTV — FFZ: страница берёт последний.
async fn emotes_for(app: &AppHandle, channel: Option<&str>, on: Providers) -> Vec<PageEmote> {
    let id = match channel {
        Some(login) => channel_id(app, login).await,
        None => None,
    };
    let scopes = std::iter::once("global".to_string()).chain(id);
    let mut keys = Vec::new();
    for scope in scopes {
        for (provider, enabled) in [("ffz", on.ffz), ("bttv", on.bttv), ("7tv", on.seventv)] {
            if enabled {
                keys.push(format!("{provider}:{scope}"));
            }
        }
    }
    let mut out = Vec::new();
    for key in keys {
        let scope = if key.ends_with(":global") {
            "global"
        } else {
            "channel"
        };
        match emote_set(app, &key).await {
            Ok(set) => out.extend(set.iter().map(|(code, one, two, provider)| {
                (code.clone(), one.clone(), two.clone(), *provider, scope)
            })),
            Err(err) => tracing::debug!(%err, key, "смайлы не получены"),
        }
    }
    let overflow = out.len().saturating_sub(EMOTES_LIMIT);
    out.drain(..overflow);
    out
}

/// Набор смайлов по ключу `поставщик:global` или `поставщик:<номер канала>` —
/// из памяти или с API.
async fn emote_set(app: &AppHandle, key: &str) -> anyhow::Result<Arc<Vec<Emote>>> {
    let state = app.state::<Twitch>();
    let ttl = if key.ends_with(":global") {
        GLOBAL_TTL
    } else {
        CHANNEL_TTL
    };
    if let Some((at, set)) = state.emotes.lock().get(key) {
        if at.elapsed() < ttl {
            return Ok(set.clone());
        }
    }
    let (provider, scope) = key.split_once(':').unwrap_or((key, "global"));
    let url = match (provider, scope) {
        ("bttv", "global") => "https://api.betterttv.net/3/cached/emotes/global".to_string(),
        ("bttv", id) => format!("https://api.betterttv.net/3/cached/users/twitch/{id}"),
        ("7tv", "global") => "https://7tv.io/v3/emote-sets/global".to_string(),
        ("7tv", id) => format!("https://7tv.io/v3/users/twitch/{id}"),
        ("ffz", "global") => {
            "https://api.betterttv.net/3/cached/frankerfacez/emotes/global".to_string()
        }
        (_, id) => format!("https://api.betterttv.net/3/cached/frankerfacez/users/twitch/{id}"),
    };
    let client = app.state::<crate::newtab::NewTab>().client().clone();
    let response = client.get(&url).timeout(API_TIMEOUT).send().await?;
    // 404 — у канала нет своих смайлов в этом сервисе.
    let body = match response.status() {
        reqwest::StatusCode::NOT_FOUND => String::new(),
        status if status.is_success() => response.text().await?,
        status => anyhow::bail!("API смайлов ответил {status}"),
    };
    let set = Arc::new(if body.is_empty() {
        Vec::new()
    } else if provider == "bttv" {
        parse_bttv(&body, scope == "global")?
    } else if provider == "7tv" {
        parse_7tv(&body, scope == "global")?
    } else {
        parse_ffz(&body)?
    });

    let mut cache = state.emotes.lock();
    if cache.len() >= CACHE_LIMIT {
        cache.retain(|key, (at, _)| key.ends_with(":global") || at.elapsed() < CHANNEL_TTL);
    }
    cache.insert(key.to_string(), (Instant::now(), set.clone()));
    Ok(set)
}

#[derive(Deserialize)]
struct BttvEmote {
    id: String,
    code: String,
    /// Модификаторы (`w!`, `h!`) меняют соседний смайл — их не рисуем.
    #[serde(default)]
    modifier: bool,
}

#[derive(Deserialize)]
struct BttvChannel {
    #[serde(rename = "channelEmotes", default)]
    channel: Vec<BttvEmote>,
    #[serde(rename = "sharedEmotes", default)]
    shared: Vec<BttvEmote>,
}

fn parse_bttv(body: &str, global: bool) -> anyhow::Result<Vec<Emote>> {
    let emotes = if global {
        serde_json::from_str::<Vec<BttvEmote>>(body)?
    } else {
        let channel: BttvChannel = serde_json::from_str(body)?;
        channel.shared.into_iter().chain(channel.channel).collect()
    };
    Ok(emotes
        .into_iter()
        .filter(|emote| !emote.modifier && valid_code(&emote.code))
        .filter(|emote| emote.id.len() == 24 && emote.id.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(|emote| {
            let base = format!("https://cdn.betterttv.net/emote/{}", emote.id);
            (
                emote.code,
                format!("{base}/1x.webp"),
                format!("{base}/2x.webp"),
                "BTTV",
            )
        })
        .collect())
}

#[derive(Deserialize)]
struct FfzEmote {
    code: String,
    images: HashMap<String, Option<String>>,
    #[serde(default)]
    modifier: bool,
}

fn parse_ffz(body: &str) -> anyhow::Result<Vec<Emote>> {
    let emotes: Vec<FfzEmote> = serde_json::from_str(body)?;
    Ok(emotes
        .into_iter()
        .filter(|emote| !emote.modifier && valid_code(&emote.code))
        .filter_map(|mut emote| {
            let one = emote
                .images
                .remove("1x")
                .flatten()
                .filter(|url| valid_image(url))?;
            let two = emote
                .images
                .remove("2x")
                .flatten()
                .filter(|url| valid_image(url))
                .unwrap_or_else(|| one.clone());
            Some((emote.code, one, two, "FFZ"))
        })
        .collect())
}

#[derive(Deserialize)]
struct SevenTvEmote {
    /// Код в чате: у канала он может отличаться от исходного названия смайла.
    name: String,
    /// 1 — смайл-наложение (zero-width): рисуется поверх соседнего, не рисуем.
    #[serde(default)]
    flags: u32,
    data: SevenTvEmoteData,
}

#[derive(Deserialize)]
struct SevenTvEmoteData {
    id: String,
}

#[derive(Deserialize)]
struct SevenTvSet {
    #[serde(default)]
    emotes: Option<Vec<SevenTvEmote>>,
}

#[derive(Deserialize)]
struct SevenTvUser {
    emote_set: Option<SevenTvSet>,
}

fn parse_7tv(body: &str, global: bool) -> anyhow::Result<Vec<Emote>> {
    let set = if global {
        Some(serde_json::from_str::<SevenTvSet>(body)?)
    } else {
        serde_json::from_str::<SevenTvUser>(body)?.emote_set
    };
    let emotes = set.and_then(|set| set.emotes).unwrap_or_default();
    Ok(emotes
        .into_iter()
        .filter(|emote| emote.flags & 1 == 0 && valid_code(&emote.name))
        .filter(|emote| {
            (20..=32).contains(&emote.data.id.len())
                && emote.data.id.bytes().all(|b| b.is_ascii_alphanumeric())
        })
        .map(|emote| {
            let base = format!("https://cdn.7tv.app/emote/{}", emote.data.id);
            (
                emote.name,
                format!("{base}/1x.webp"),
                format!("{base}/2x.webp"),
                "7TV",
            )
        })
        .collect())
}

/// Картинки смайлов — только с CDN самих сервисов.
fn valid_image(url: &str) -> bool {
    url.starts_with("https://cdn.betterttv.net/")
        || url.starts_with("https://cdn.frankerfacez.com/")
}

fn valid_code(code: &str) -> bool {
    !code.is_empty() && code.len() <= 64 && !code.chars().any(char::is_whitespace)
}

/// Номер канала по логину — через GQL Twitch, из памяти на время смайлов.
async fn channel_id(app: &AppHandle, login: &str) -> Option<String> {
    let state = app.state::<Twitch>();
    if let Some((at, id)) = state.ids.lock().get(login) {
        if at.elapsed() < CHANNEL_TTL {
            return id.clone();
        }
    }
    let client = app.state::<crate::newtab::NewTab>().client().clone();
    let body = json!({
        "query": "query($login: String!) { user(login: $login) { id } }",
        "variables": { "login": login },
    });
    let request = client
        .post("https://gql.twitch.tv/gql")
        .header("Client-ID", TWITCH_CLIENT_ID)
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .timeout(API_TIMEOUT);
    let id = match request.send().await {
        Ok(response) if response.status().is_success() => response
            .text()
            .await
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|value| value.pointer("/data/user/id")?.as_str().map(str::to_string))
            .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())),
        Ok(response) => {
            tracing::debug!(status = %response.status(), "номер канала Twitch не получен");
            return None;
        }
        Err(err) => {
            tracing::debug!(%err, "номер канала Twitch не получен");
            return None;
        }
    };
    let mut ids = state.ids.lock();
    if ids.len() >= CACHE_LIMIT {
        ids.clear();
    }
    ids.insert(login.to_string(), (Instant::now(), id.clone()));
    id
}

/* ── Плейлист трансляции ─────────────────────────────────────── */

/// Движок держит запрос плейлиста и ждёт ответа: ответ приходит всегда — либо
/// плейлист от сервера, либо «пусть идёт к Twitch как был».
pub fn intercept(app: &AppHandle, tab: u32, token: u64, url: String) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let reply = playlist(&app, &url).await;
        if reply.is_none() {
            tracing::debug!("плейлист Twitch идёт напрямую");
        }
        answer(&app, tab, token, reply).await;
    });
}

/// Ответ движку. Хост окна бывает занят как раз в этот миг — тогда ещё раз:
/// без ответа запрос плеера висел бы, пока вкладку не закроют.
async fn answer(
    app: &AppHandle,
    tab: u32,
    token: u64,
    reply: Option<browser190x4_webview::InterceptReply>,
) {
    for _ in 0..20 {
        let (target, reply) = (app.clone(), reply.clone());
        let done = tauri::async_runtime::spawn_blocking(move || {
            crate::state::with_tab(&target, tab, move |host| {
                host.with_tab(browser190x4_webview::TabId(tab), |view| {
                    view.answer_intercept(token, reply)
                })
            })
        })
        .await;
        if matches!(done, Ok(Ok(_))) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tracing::warn!("ответ на запрос плейлиста Twitch не доставлен");
}

async fn playlist(app: &AppHandle, url: &str) -> Option<browser190x4_webview::InterceptReply> {
    let store = app.state::<App>().store.clone();
    if !(store.setting_bool("ext_twitch_enabled", true)
        && store.setting_bool("twitch_quality", true))
    {
        return None;
    }
    let channel = playlist_channel(url)?;
    let servers = servers(&store);
    if russia_only(app, &servers[0]).await.contains(&channel) {
        return None;
    }
    let token = app.state::<Twitch>().token.lock().clone();
    let client = app.state::<crate::newtab::NewTab>().client().clone();
    for server in servers.iter().take(TRIES) {
        let mut target = format!("{server}{url}");
        if let Some(token) = &token {
            target.push_str(if url.contains('?') {
                "&auth="
            } else {
                "?auth="
            });
            target.push_str(token);
        }
        match fetch_playlist(&client, &target).await {
            Ok(body) => {
                return Some(browser190x4_webview::InterceptReply {
                    status: 200,
                    reason: "OK".into(),
                    headers: "Content-Type: application/vnd.apple.mpegurl\r\n\
                              Access-Control-Allow-Origin: *\r\n\
                              Cache-Control: no-cache, no-store"
                        .into(),
                    body: clean_playlist(&body).into_bytes(),
                });
            }
            Err(err) => tracing::debug!(%err, server, "сервер плейлистов Twitch не ответил"),
        }
    }
    None
}

async fn fetch_playlist(client: &reqwest::Client, url: &str) -> anyhow::Result<String> {
    let response = client.get(url).timeout(PLAYLIST_TIMEOUT).send().await?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("ответ {status}");
    }
    if response
        .content_length()
        .is_some_and(|length| length as usize > PLAYLIST_LIMIT)
    {
        anyhow::bail!("плейлист слишком большой");
    }
    let body = response.text().await?;
    if !body.starts_with("#EXTM3U") || body.len() > PLAYLIST_LIMIT {
        anyhow::bail!("в ответе не плейлист");
    }
    Ok(body)
}

/// Плейлист как от Twitch: без подписи сервера в названии качества.
fn clean_playlist(body: &str) -> String {
    body.replace(" (1080/1440 by ReYohoho)", "")
}

/// Серверы плейлистов: свой из настроек или по умолчанию.
fn servers(store: &Store) -> Vec<String> {
    let custom = store.setting_str("twitch_proxy").unwrap_or_default();
    let custom = custom.trim();
    if custom.starts_with("https://") && reqwest::Url::parse(custom).is_ok() {
        let mut base = custom.to_string();
        if !base.ends_with('/') {
            base.push('/');
        }
        return vec![base];
    }
    PROXIES.iter().map(|server| server.to_string()).collect()
}

/// Каналы, которые смотрят только из России, — из памяти или с сервера.
async fn russia_only(app: &AppHandle, server: &str) -> HashSet<String> {
    let state = app.state::<Twitch>();
    {
        let cached = state.russia_only.lock();
        if cached.0.is_some_and(|at| at.elapsed() < RUSSIA_ONLY_TTL) {
            return cached.1.clone();
        }
    }
    let client = app.state::<crate::newtab::NewTab>().client().clone();
    let fetched = async {
        let response = client
            .get(format!("{server}russia-only-channels"))
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let list: Vec<String> = serde_json::from_str(&response.text().await.ok()?).ok()?;
        Some(
            list.into_iter()
                .map(|channel| channel.to_ascii_lowercase())
                .collect::<HashSet<_>>(),
        )
    }
    .await;
    let mut cached = state.russia_only.lock();
    // Не ответил — прежний список, а спросить снова через тот же срок.
    cached.0 = Some(Instant::now());
    if let Some(list) = fetched {
        cached.1 = list;
    }
    cached.1.clone()
}

/// Логин канала из адреса плейлиста: `…/channel/hls/<логин>.m3u8?…`.
fn playlist_channel(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    let name = path
        .rsplit('/')
        .next()?
        .strip_suffix(".m3u8")?
        .to_ascii_lowercase();
    valid_login(&name).then_some(name)
}

fn valid_login(login: &str) -> bool {
    (1..=25).contains(&login.len())
        && login
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn valid_token(token: &str) -> bool {
    (20..=64).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Origin документа Twitch: только https, только twitch.tv и поддомены.
fn twitch_origin(source: &str) -> Option<String> {
    let url = reqwest::Url::parse(source).ok()?;
    let host = url.host_str()?;
    (url.scheme() == "https" && (host == "twitch.tv" || host.ends_with(".twitch.tv")))
        .then(|| url.origin().ascii_serialization())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlist_channel_comes_from_the_path() {
        assert_eq!(
            playlist_channel(
                "https://usher.ttvnw.net/api/v2/channel/hls/OhnePixel.m3u8?token=%7B%7D&sig=1"
            ),
            Some("ohnepixel".to_string())
        );
        assert_eq!(
            playlist_channel("https://usher.ttvnw.net/api/channel/hls/../x.json"),
            None
        );
    }

    #[test]
    fn only_twitch_documents_are_answered() {
        assert_eq!(
            twitch_origin("https://www.twitch.tv/ohnepixel"),
            Some("https://www.twitch.tv".to_string())
        );
        assert!(twitch_origin("https://twitch.tv.evil.example/").is_none());
        assert!(twitch_origin("http://www.twitch.tv/").is_none());
    }

    #[test]
    fn seventv_sets_are_parsed() {
        let global = r#"{"id":"g","emotes":[
            {"id":"01FCY771D800007PQ2DF3GDTN6","name":"RainTime","flags":1,"data":{"id":"01FCY771D800007PQ2DF3GDTN6","flags":256}},
            {"id":"60ae958e229664e8667aea38","name":"EZ","flags":0,"data":{"id":"60ae958e229664e8667aea38","flags":0}},
            {"id":"x","name":"Bad","flags":0,"data":{"id":"../../evil"}}]}"#;
        let parsed = parse_7tv(global, true).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "EZ");
        assert_eq!(
            parsed[0].1,
            "https://cdn.7tv.app/emote/60ae958e229664e8667aea38/1x.webp"
        );

        let channel = r#"{"id":"u","emote_set":{"emotes":[{"id":"01GK4EW2AG0004SH49XX2J74KJ","name":"peepoShy","flags":0,"data":{"id":"01GK4EW2AG0004SH49XX2J74KJ"}}]}}"#;
        assert_eq!(parse_7tv(channel, false).unwrap()[0].0, "peepoShy");
        assert!(parse_7tv(r#"{"id":"u","emote_set":null}"#, false)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn emote_lists_are_parsed_and_checked() {
        let bttv = r#"[{"id":"54fa8f1401e468494b85b537","code":":tf:","imageType":"png","animated":false},
            {"id":"5e76d338d6581c3724c0f0b2","code":"w!","modifier":true},
            {"id":"not-hex","code":"Bad"}]"#;
        let parsed = parse_bttv(bttv, true).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            parsed[0].1,
            "https://cdn.betterttv.net/emote/54fa8f1401e468494b85b537/1x.webp"
        );

        let channel = r#"{"id":"x","channelEmotes":[{"id":"54fa8f1401e468494b85b537","code":"Mine"}],"sharedEmotes":[]}"#;
        assert_eq!(parse_bttv(channel, false).unwrap()[0].0, "Mine");

        let ffz = r#"[{"id":9,"code":"ZrehplaR","images":{"1x":"https://cdn.betterttv.net/frankerfacez_emote/9/1","2x":null,"4x":null}},
            {"id":10,"code":"Evil","images":{"1x":"https://evil.example/x.png"}}]"#;
        let parsed = parse_ffz(ffz).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].2, parsed[0].1);
    }

    #[test]
    fn proxy_branding_is_removed() {
        let body = "#EXT-X-MEDIA:TYPE=VIDEO,GROUP-ID=\"audio_only\",NAME=\"Audio Only (1080/1440 by ReYohoho)\"";
        assert_eq!(
            clean_playlist(body),
            "#EXT-X-MEDIA:TYPE=VIDEO,GROUP-ID=\"audio_only\",NAME=\"Audio Only\""
        );
    }
}
