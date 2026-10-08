//! Обновление расширенных фильтров: списки и ресурсы скриптлетов, которые
//! собирает workflow `filters.yml`.
//!
//! Источник — GitLab, резерв — GitHub, как у обновлений браузера. Манифест
//! подписан тем же ключом minisign, что и обновления (`manifest.json.sig`):
//! скриптлеты из канала выполняются на каждом сайте, и без подписи доступ к
//! пакету GitLab или релизу на GitHub значил бы свой код на всех сайтах у всех
//! пользователей. Старый манифест не принимается — откатить фильтры, подсунув
//! прежнюю выкладку, нельзя. Файлы сверяются с манифестом по размеру и SHA-256
//! и записываются атомарно; манифест — последним, поэтому оборванное
//! обновление не выглядит готовым. После обновления фильтр пересобирается.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};

use crate::state::App;

const SOURCES: [&str; 2] = [
    "https://gitlab.com/api/v4/projects/86438976/packages/generic/filters/stable/",
    "https://github.com/pathetixx/190x4-Browser/releases/download/filters/",
];

/// Открытый ключ подписи — тот же, что у обновлений браузера (`plugins.updater.pubkey`
/// в `tauri.conf.json`; тест ниже следит, чтобы они не разошлись): base64 от
/// файла открытого ключа minisign.
const PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEVFNUE4NUMwQzk1NkMxOUEKUldTYXdWYkp3SVZhN2hnYm1mUWJOZlpnRk9zY3NrVkR6WTNjRlN5MHB5K2t5d3FRZnBGL0NVejgK";

/// Первое обновление — не в момент запуска: сначала поднимаются вкладки.
const FIRST: Duration = Duration::from_secs(30);
const EVERY: Duration = Duration::from_secs(12 * 60 * 60);

#[derive(Deserialize)]
struct Manifest {
    generated_at: String,
    files: BTreeMap<String, FileInfo>,
}

#[derive(Deserialize)]
struct FileInfo {
    size: u64,
    sha256: String,
}

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST).await;
        loop {
            match update().await {
                Ok(true) => {
                    tracing::info!("расширенные фильтры обновлены");
                    let state = app.state::<App>();
                    crate::rebuild_filter(state.guard.clone(), state.store.clone(), app.clone());
                }
                Ok(false) => {}
                Err(err) => tracing::warn!(%err, "расширенные фильтры не обновлены"),
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

/// `true` — файлы поменялись.
async fn update() -> anyhow::Result<bool> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(90))
        .user_agent(concat!("190x4-browser/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let dir = crate::filters_dir();
    std::fs::create_dir_all(&dir)?;

    let mut failure = None;
    for base in SOURCES {
        match update_from(&client, base, &dir).await {
            Ok(changed) => return Ok(changed),
            Err(err) => {
                tracing::debug!(source = base, %err, "источник фильтров недоступен");
                failure = Some(err);
            }
        }
    }
    Err(failure.unwrap_or_else(|| anyhow::anyhow!("нет источников фильтров")))
}

async fn update_from(client: &reqwest::Client, base: &str, dir: &Path) -> anyhow::Result<bool> {
    let manifest_text = client
        .get(format!("{base}manifest.json"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let signature = client
        .get(format!("{base}manifest.json.sig"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    verify(manifest_text.as_bytes(), &signature, PUBLIC_KEY).context("подпись manifest.json")?;
    let manifest: Manifest = serde_json::from_str(&manifest_text).context("manifest.json")?;

    let local = std::fs::read_to_string(dir.join("manifest.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Manifest>(&text).ok());
    if let Some(local) = &local {
        // Время выкладки — ISO 8601 в UTC одной длины: строки сравниваются как
        // время. Та же или более старая выкладка — обновлять нечего.
        if manifest.generated_at <= local.generated_at {
            return Ok(false);
        }
    }

    // Сначала всё скачать и проверить, потом писать: частично обновлённые
    // списки хуже прежних целиком.
    let mut staged = Vec::new();
    for (name, info) in &manifest.files {
        if name.is_empty()
            || name.starts_with('.')
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        {
            bail!("недопустимое имя файла в манифесте: {name}");
        }
        if let Ok(current) = std::fs::read(dir.join(name)) {
            if current.len() as u64 == info.size && sha256_hex(&current) == info.sha256 {
                continue;
            }
        }
        let bytes = client
            .get(format!("{base}{name}"))
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        if bytes.len() as u64 != info.size || sha256_hex(&bytes) != info.sha256 {
            bail!("{name}: не совпали размер или контрольная сумма");
        }
        staged.push((name.clone(), bytes));
    }

    for (name, bytes) in &staged {
        write_atomic(&dir.join(name), bytes)?;
    }
    write_atomic(&dir.join("manifest.json"), manifest_text.as_bytes())?;
    Ok(true)
}

/// Подпись minisign в формате Tauri: и ключ, и подпись — base64 от текста
/// файлов minisign. Так же проверяет обновления плагин updater.
fn verify(data: &[u8], signature: &str, public_key: &str) -> anyhow::Result<()> {
    use base64::Engine as _;
    use minisign_verify::{PublicKey, Signature};

    let text = |value: &str| -> anyhow::Result<String> {
        let bytes = base64::engine::general_purpose::STANDARD.decode(value.trim())?;
        Ok(String::from_utf8(bytes)?)
    };
    let key = PublicKey::decode(&text(public_key)?)?;
    let signature = Signature::decode(&text(signature)?)?;
    key.verify(data, &signature, true)?;
    Ok(())
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    // Имя временного файла — с суффиксом, а не с подменённым расширением:
    // `list.txt` и `list.json` иначе спорили бы за один и тот же `list.download`.
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    let tmp = path.with_file_name(name);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ключ и подпись, сделанные для теста (Ed25519 с BLAKE2b, как подписывает
    /// `tauri signer sign`).
    const TEST_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXkgMDEyMzQ1Njc4OUFCQ0RFRgpSV1FCSTBWbmlhdk43d09oQjcvenpoQytIWERkR09kTHdKbG41Tll3bTZVTlh4M2NobVFTVlRHNAo=";
    const TEST_SIGNATURE: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRQkkwVm5pYXZON3pYYTN6SnFodnNjNzZzUzF0b1F6bHVTanBYVXo5L1NJNDgrbEdCYnhkM0ZLcSttL1V6ekJzQXVDanZxWGpyR1U5enBvL0hRYUx6Y1cwOHVVRE0wWndrPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkxMDgzODIwCWZpbGU6bWFuaWZlc3QuanNvbgo2TXNFTXh0NEJQekRpOXcyc3B4elgrSHB5WnhHVHZVSUk4bWJIT1NSS0MyU3hyWUtBR1FvdVZQVEtKdjJUQnhnWEkzbnNOdWlxbVpieU9sSE1RVjdDdz09Cg==";
    const TEST_DATA: &str = r#"{"generated_at":"2026-10-04T03:17:00.000Z","files":{}}"#;

    #[test]
    fn signed_manifest_passes_and_changed_one_does_not() {
        assert!(verify(TEST_DATA.as_bytes(), TEST_SIGNATURE, TEST_KEY).is_ok());
        let changed = TEST_DATA.replace("2026", "2027");
        assert!(verify(changed.as_bytes(), TEST_SIGNATURE, TEST_KEY).is_err());
        // Чужой ключ — тот, которым подписаны настоящие выкладки.
        assert!(verify(TEST_DATA.as_bytes(), TEST_SIGNATURE, PUBLIC_KEY).is_err());
        assert!(verify(TEST_DATA.as_bytes(), "не base64", TEST_KEY).is_err());
    }

    #[test]
    fn filters_key_is_the_updater_key() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(config["plugins"]["updater"]["pubkey"], PUBLIC_KEY);
    }

    #[test]
    fn sha256_matches_known_value() {
        assert_eq!(
            sha256_hex(b"190x4"),
            "32847f0ba1aad1ae04528b5a7284bd93397bd3779884f889b522e58503910f00"
        );
    }
}
