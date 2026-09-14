//! Значки сайтов для плиток новой вкладки.
//!
//! Плитка крупная, и значка 16 px, который браузер показывает на вкладке, для
//! неё мало. Поэтому смотрим, что отдаёт сам сайт: `apple-touch-icon`, значки из
//! `<link rel="icon" sizes="…">`, SVG и иконки манифеста — и берём ближайший к
//! 180 px. Последний вариант — `/favicon.ico`. Готовый значок хранится в профиле
//! неделю, неудача — сутки: сайт без значка не опрашивается на каждом открытии.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const PAGE_LIMIT: usize = 1024 * 1024;
const MANIFEST_LIMIT: usize = 256 * 1024;
const IMAGE_LIMIT: usize = 512 * 1024;
const FOUND_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const MISSING_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// Сколько кандидатов пробовать скачать, пока один не окажется картинкой.
const ATTEMPTS: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Icon {
    /// `data:`-адрес картинки.
    pub data: String,
    /// Сторона в пикселях, если её удалось узнать; у SVG — 512.
    pub size: u32,
}

#[derive(Serialize, Deserialize)]
struct Cached {
    at: u64,
    icon: Option<Icon>,
}

#[derive(Debug, Clone)]
struct Candidate {
    url: Url,
    size: u32,
    svg: bool,
    /// Маскируемые иконки манифеста залиты фоном до краёв — хуже обычных.
    maskable: bool,
}

/// Значок сайта страницы `page` или `None`, если у сайта его не нашлось.
pub async fn icon(client: &reqwest::Client, cache_dir: &Path, page: &str) -> Option<Icon> {
    let page = Url::parse(page).ok()?;
    if !matches!(page.scheme(), "http" | "https") || page.host_str().is_none() {
        return None;
    }
    let key = cache_key(&page);
    let path = cache_dir.join(format!("{key}.json"));

    if let Some(cached) = read_cache(&path) {
        return cached;
    }
    let icon = find(client, &page).await;
    write_cache(cache_dir, &path, icon.as_ref());
    icon
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Один значок на сайт: ключ — схема, хост и порт.
fn cache_key(page: &Url) -> String {
    let origin = page.origin().ascii_serialization();
    let digest = Sha256::digest(origin.as_bytes());
    digest[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `Some(значок или его отсутствие)` — запись свежая, сеть не нужна.
fn read_cache(path: &Path) -> Option<Option<Icon>> {
    let cached: Cached = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let ttl = if cached.icon.is_some() {
        FOUND_TTL
    } else {
        MISSING_TTL
    };
    (now().saturating_sub(cached.at) < ttl.as_secs()).then_some(cached.icon)
}

fn write_cache(dir: &Path, path: &Path, icon: Option<&Icon>) {
    let record = Cached {
        at: now(),
        icon: icon.cloned(),
    };
    let result = std::fs::create_dir_all(dir).and_then(|()| {
        let temp = path.with_extension("tmp");
        std::fs::write(&temp, serde_json::to_vec(&record).unwrap_or_default())?;
        std::fs::rename(&temp, path)
    });
    if let Err(err) = result {
        tracing::debug!(%err, "значок сайта не сохранён");
    }
}

async fn find(client: &reqwest::Client, page: &Url) -> Option<Icon> {
    let mut candidates = Vec::new();

    match fetch(client, page.clone(), PAGE_LIMIT, "text/html,*/*;q=0.5").await {
        Ok((base, body)) => {
            let html = String::from_utf8_lossy(&body);
            let (found, manifest) = candidates_from_html(&html, &base);
            candidates.extend(found);
            let best = candidates.iter().map(|c| c.size).max().unwrap_or(0);
            if best < 96 {
                if let Some(manifest) = manifest {
                    candidates.extend(manifest_candidates(client, manifest).await);
                }
            }
        }
        Err(err) => tracing::debug!(%err, page = %page, "страница сайта не загрузилась"),
    }
    if let Ok(ico) = page.join("/favicon.ico") {
        candidates.push(Candidate {
            url: ico,
            size: 32,
            svg: false,
            maskable: false,
        });
    }

    candidates.sort_by_key(|candidate| std::cmp::Reverse(score(candidate)));
    let mut seen = HashSet::new();
    for candidate in candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.url.clone()))
        .take(ATTEMPTS)
    {
        if let Some(icon) = download(client, &candidate).await {
            return Some(icon);
        }
    }
    None
}

/// Чем ближе к 180 px, тем лучше; SVG хорош при любом размере плитки.
fn score(candidate: &Candidate) -> i64 {
    let base = if candidate.svg {
        900
    } else if candidate.size >= 64 {
        1000 - (i64::from(candidate.size) - 180).abs().min(900)
    } else {
        i64::from(candidate.size)
    };
    if candidate.maskable {
        base - 120
    } else {
        base
    }
}

/// Кандидаты из `<link>` и адрес манифеста.
fn candidates_from_html(html: &str, base: &Url) -> (Vec<Candidate>, Option<Url>) {
    // Значки объявляют в <head>; дальше читать незачем.
    let head = match html.to_ascii_lowercase().find("</head>") {
        Some(end) => &html[..end],
        None => html,
    };
    let mut candidates = Vec::new();
    let mut manifest = None;

    for attrs in link_tags(head) {
        let rel = attrs
            .get("rel")
            .map(|rel| rel.to_ascii_lowercase())
            .unwrap_or_default();
        let Some(href) = attrs
            .get("href")
            .map(|href| href.trim().replace("&amp;", "&"))
        else {
            continue;
        };
        let Ok(url) = base.join(&href) else {
            continue;
        };
        if !matches!(url.scheme(), "http" | "https") {
            continue;
        }
        let tokens: Vec<&str> = rel.split_ascii_whitespace().collect();
        if tokens.contains(&"manifest") {
            manifest.get_or_insert(url);
            continue;
        }
        let apple = tokens
            .iter()
            .any(|token| token.starts_with("apple-touch-icon"));
        if !apple && !tokens.contains(&"icon") {
            continue;
        }
        let svg = attrs
            .get("type")
            .is_some_and(|kind| kind.eq_ignore_ascii_case("image/svg+xml"))
            || url.path().to_ascii_lowercase().ends_with(".svg");
        let size = attrs
            .get("sizes")
            .and_then(|sizes| largest_size(sizes))
            .unwrap_or(if apple {
                180
            } else if svg {
                512
            } else {
                32
            });
        candidates.push(Candidate {
            url,
            size,
            svg,
            maskable: false,
        });
    }
    (candidates, manifest)
}

/// `"16x16 32x32"` → 32; `"any"` → `None`.
fn largest_size(sizes: &str) -> Option<u32> {
    sizes
        .split_ascii_whitespace()
        .filter_map(|size| {
            let size = size.to_ascii_lowercase();
            size.split_once('x')?.0.parse::<u32>().ok()
        })
        .max()
}

/// Атрибуты всех тегов `<link>` фрагмента.
fn link_tags(html: &str) -> Vec<HashMap<String, String>> {
    // Нижний регистр ASCII не меняет длину, так что смещения совпадают.
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(position) = lower[from..].find("<link") {
        let start = from + position + "<link".len();
        let Some(length) = lower[start..].find('>') else {
            break;
        };
        out.push(parse_attrs(&html[start..start + length]));
        from = start + length;
    }
    out
}

fn parse_attrs(text: &str) -> HashMap<String, String> {
    let bytes = text.as_bytes();
    let mut attrs = HashMap::new();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        let name_start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'='
            && bytes[i] != b'/'
        {
            i += 1;
        }
        if i == name_start {
            i += 1;
            continue;
        }
        let name = text[name_start..i].to_ascii_lowercase();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let quote = bytes[i];
                i += 1;
                let value_start = i;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                value = text[value_start..i].to_string();
                i += 1;
            } else {
                let value_start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                value = text[value_start..i].to_string();
            }
        }
        attrs.entry(name).or_insert(value);
    }
    attrs
}

#[derive(Deserialize)]
struct Manifest {
    #[serde(default)]
    icons: Vec<ManifestIcon>,
}

#[derive(Deserialize)]
struct ManifestIcon {
    src: String,
    #[serde(default)]
    sizes: String,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    purpose: String,
}

async fn manifest_candidates(client: &reqwest::Client, url: Url) -> Vec<Candidate> {
    let Ok((base, body)) =
        fetch(client, url, MANIFEST_LIMIT, "application/manifest+json,*/*").await
    else {
        return Vec::new();
    };
    let Ok(manifest) = serde_json::from_slice::<Manifest>(&body) else {
        return Vec::new();
    };
    manifest
        .icons
        .into_iter()
        .filter_map(|icon| {
            let purpose = icon.purpose.to_ascii_lowercase();
            // Одноцветные иконки — силуэт для системных меню, не для плитки.
            if purpose.split_ascii_whitespace().all(|p| p == "monochrome") && !purpose.is_empty() {
                return None;
            }
            let url = base.join(&icon.src).ok()?;
            let svg = icon.kind.eq_ignore_ascii_case("image/svg+xml")
                || url.path().to_ascii_lowercase().ends_with(".svg");
            Some(Candidate {
                size: largest_size(&icon.sizes).unwrap_or(if svg { 512 } else { 96 }),
                maskable: purpose.contains("maskable") && !purpose.contains("any"),
                url,
                svg,
            })
        })
        .collect()
}

/// Скачать не больше `limit` байт. Возвращает итоговый адрес (после
/// перенаправлений) — относительные ссылки считаются от него.
async fn fetch(
    client: &reqwest::Client,
    url: Url,
    limit: usize,
    accept: &str,
) -> anyhow::Result<(Url, Vec<u8>)> {
    let mut response = client
        .get(url)
        .header(reqwest::header::ACCEPT, accept)
        .send()
        .await?
        .error_for_status()?;
    let base = response.url().clone();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() >= limit {
            body.truncate(limit);
            break;
        }
    }
    Ok((base, body))
}

async fn download(client: &reqwest::Client, candidate: &Candidate) -> Option<Icon> {
    let (_, body) = fetch(
        client,
        candidate.url.clone(),
        IMAGE_LIMIT + 1,
        "image/*,*/*;q=0.5",
    )
    .await
    .ok()?;
    if body.len() > IMAGE_LIMIT {
        return None;
    }
    let (mime, size) = sniff(&body)?;
    let size = size.unwrap_or(candidate.size);
    Some(Icon {
        data: format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&body)
        ),
        size,
    })
}

/// Тип картинки по содержимому и, где просто, её размер. Заголовку сервера не
/// верим: вместо значка часто отдают HTML-страницу с кодом 200.
fn sniff(body: &[u8]) -> Option<(&'static str, Option<u32>)> {
    if body.starts_with(b"\x89PNG\r\n\x1a\n") && body.len() >= 24 {
        let width = u32::from_be_bytes(body[16..20].try_into().ok()?);
        return Some(("image/png", Some(width)));
    }
    if body.starts_with(&[0, 0, 1, 0]) && body.len() > 6 {
        let count = usize::from(u16::from_le_bytes([body[4], body[5]]));
        let largest = (0..count)
            .filter_map(|i| body.get(6 + i * 16))
            .map(|&width| if width == 0 { 256 } else { u32::from(width) })
            .max();
        return Some(("image/x-icon", largest));
    }
    if body.starts_with(b"GIF8") && body.len() > 10 {
        return Some((
            "image/gif",
            Some(u32::from(u16::from_le_bytes([body[6], body[7]]))),
        ));
    }
    if body.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(("image/jpeg", None));
    }
    if body.len() > 12 && &body[..4] == b"RIFF" && &body[8..12] == b"WEBP" {
        return Some(("image/webp", None));
    }
    let start = String::from_utf8_lossy(&body[..body.len().min(1024)]).to_ascii_lowercase();
    if start.contains("<svg") {
        return Some(("image/svg+xml", Some(512)));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_large_icons_from_links() {
        let base = Url::parse("https://example.com/news/").unwrap();
        let html = r#"<html><head>
            <link rel="icon" href="/favicon-16.png" sizes="16x16">
            <LINK REL='apple-touch-icon' HREF="touch.png">
            <link rel="icon" type="image/svg+xml" href="https://cdn.example.com/logo.svg"/>
            <link rel=manifest href=/site.webmanifest>
            <link rel="stylesheet" href="/app.css">
            </head><body><link rel="icon" href="/late.png"></body></html>"#;
        let (candidates, manifest) = candidates_from_html(html, &base);
        let urls: Vec<String> = candidates.iter().map(|c| c.url.to_string()).collect();
        assert_eq!(
            urls,
            [
                "https://example.com/favicon-16.png",
                "https://example.com/news/touch.png",
                "https://cdn.example.com/logo.svg",
            ]
        );
        assert_eq!(candidates[1].size, 180);
        assert!(candidates[2].svg);
        assert_eq!(
            manifest.unwrap().as_str(),
            "https://example.com/site.webmanifest"
        );

        let mut sorted = candidates.clone();
        sorted.sort_by_key(|c| std::cmp::Reverse(score(c)));
        assert_eq!(sorted[0].url.path(), "/news/touch.png");
    }

    #[test]
    fn sizes_attribute_takes_the_largest() {
        assert_eq!(largest_size("16x16 32x32 192X192"), Some(192));
        assert_eq!(largest_size("any"), None);
    }

    #[test]
    fn sniffs_real_images_only() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&180u32.to_be_bytes());
        png.extend_from_slice(&180u32.to_be_bytes());
        assert_eq!(sniff(&png), Some(("image/png", Some(180))));

        let ico = [
            0, 0, 1, 0, 2, 0, 16, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(sniff(&ico), Some(("image/x-icon", Some(256))));

        assert_eq!(sniff(b"<!doctype html><title>404</title>"), None);
        assert_eq!(
            sniff(b"<?xml version=\"1.0\"?><svg viewBox='0 0 1 1'/>").map(|s| s.0),
            Some("image/svg+xml")
        );
    }
}
