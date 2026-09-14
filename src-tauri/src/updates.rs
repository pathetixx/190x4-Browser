//! Обновления браузера — по той же схеме, что у Ninety.
//!
//! Первичный источник — GitLab Generic Packages, резерв — GitHub Releases
//! (`plugins.updater.endpoints` в `tauri.conf.json`). Подпись установщика
//! проверяет плагин. На Windows плагин запускает установщик NSIS в пассивном
//! режиме и сразу завершает браузер; установщик ставит новую версию и
//! запускает её снова.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::state::App;

/// Первая проверка — не в момент запуска: сначала поднимаются вкладки.
const FIRST_CHECK: Duration = Duration::from_secs(20);
/// Дальше — раз в шесть часов, пока браузер открыт.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Default)]
pub struct Updates {
    found: Mutex<Option<Update>>,
    installing: AtomicBool,
}

/// Что показать пользователю о найденной версии.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub current: String,
    pub notes: String,
    pub date: Option<String>,
}

#[derive(Clone, Serialize)]
struct Progress {
    phase: &'static str,
    downloaded: u64,
    total: Option<u64>,
}

fn info(update: &Update) -> UpdateInfo {
    UpdateInfo {
        version: update.version.clone(),
        current: update.current_version.clone(),
        notes: update.body.clone().unwrap_or_default(),
        date: update.date.map(|date| date.date().to_string()),
    }
}

async fn check(app: &AppHandle) -> Result<Option<UpdateInfo>, String> {
    let updater = app.updater().map_err(|err| err.to_string())?;
    let found = updater.check().await.map_err(|err| err.to_string())?;
    let info = found.as_ref().map(info);
    *app.state::<Updates>().found.lock() = found;
    if let Some(info) = &info {
        tracing::info!(version = %info.version, "доступно обновление");
        let _ = app.emit("update-available", info);
    }
    Ok(info)
}

/// Фоновая проверка, если пользователь её не выключил.
pub fn spawn_checker(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            let enabled = app.state::<App>().store.setting_bool("updates_auto", true);
            let known = app.state::<Updates>().found.lock().is_some();
            if enabled && !known {
                if let Err(err) = check(&app).await {
                    tracing::warn!(%err, "проверка обновлений не удалась");
                }
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

#[tauri::command]
pub async fn update_check(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    check(&app).await
}

/// Уже найденное обновление: окно интерфейса могло открыться позже проверки.
#[tauri::command]
pub fn update_state(state: State<'_, Updates>) -> Option<UpdateInfo> {
    state.found.lock().as_ref().map(info)
}

#[tauri::command]
pub async fn update_install(app: AppHandle) -> Result<(), String> {
    let state = app.state::<Updates>();
    if state.installing.swap(true, Ordering::SeqCst) {
        return Err("обновление уже устанавливается".into());
    }
    let taken = state.found.lock().take();
    let Some(update) = taken else {
        state.installing.store(false, Ordering::SeqCst);
        return Err("обновление не найдено — проверьте ещё раз".into());
    };

    let progress = app.clone();
    let finished = app.clone();
    let mut downloaded = 0u64;
    let result = update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk as u64;
                let _ = progress.emit(
                    "update-progress",
                    Progress {
                        phase: "download",
                        downloaded,
                        total,
                    },
                );
            },
            move || {
                let _ = finished.emit(
                    "update-progress",
                    Progress {
                        phase: "install",
                        downloaded: 0,
                        total: None,
                    },
                );
            },
        )
        .await;

    // На Windows при успехе сюда не доходит: процесс уже завершён плагином.
    if let Err(err) = &result {
        tracing::warn!(%err, "обновление не установлено");
        *state.found.lock() = Some(update);
    }
    state.installing.store(false, Ordering::SeqCst);
    result.map_err(|err| err.to_string())
}
