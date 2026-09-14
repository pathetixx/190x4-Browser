//! Обновление расширенных фильтров: списки и ресурсы скриптлетов, которые
//! собирает workflow `filters.yml`.
//!
//! Источник — GitLab, резерв — GitHub, как у обновлений браузера. Файлы
//! сверяются с `manifest.json` по размеру и SHA-256 и записываются атомарно;
//! манифест — последним, поэтому оборванное обновление не выглядит готовым.
//! После обновления фильтр пересобирается.

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
    let manifest: Manifest = serde_json::from_str(&manifest_text).context("manifest.json")?;

    let local = std::fs::read_to_string(dir.join("manifest.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Manifest>(&text).ok());
    if let Some(local) = &local {
        if local.generated_at == manifest.generated_at {
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

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let tmp = path.with_extension("download");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_value() {
        assert_eq!(
            sha256_hex(b"190x4"),
            "32847f0ba1aad1ae04528b5a7284bd93397bd3779884f889b522e58503910f00"
        );
    }
}
