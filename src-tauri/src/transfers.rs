//! Загрузки на стороне приложения: запись в базу, события для интерфейса,
//! действия над файлом.
//!
//! Движок сообщает о загрузке своим номером операции (`key`), интерфейс знает
//! загрузку по номеру записи в базе. Связка между ними живёт здесь — так
//! список загрузок переживает и закрытие вкладки, и перезапуск браузера.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use browser190x4_store::{DownloadKind, DownloadState};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;

use crate::state::{with_host, with_host_later, App};

/// В базу — не чаще раза в секунду: интерфейс получает прогресс событиями, а
/// диску хватает редких записей.
const WRITE_EVERY: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct Transfers {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    by_key: HashMap<u64, Entry>,
    key_by_id: HashMap<i64, u64>,
    /// Загрузки через загрузчик видео: номер записи → задание на сервере.
    media_jobs: HashMap<i64, String>,
}

struct Entry {
    id: i64,
    written: Instant,
}

/// Событие `download` для интерфейса. Одинаковое для обычных загрузок и
/// загрузчика видео — список загрузок у браузера один.
#[derive(Debug, Clone, Serialize)]
pub struct DownloadEvent {
    pub id: i64,
    pub kind: DownloadKind,
    pub phase: String,
    pub url: String,
    pub path: String,
    pub bytes: i64,
    pub total: Option<i64>,
    pub error: String,
}

impl Transfers {
    pub fn register_media(&self, id: i64, job: String) {
        self.inner.lock().media_jobs.insert(id, job);
    }

    pub fn finish_media(&self, id: i64) {
        self.inner.lock().media_jobs.remove(&id);
    }

    fn media_job(&self, id: i64) -> Option<String> {
        self.inner.lock().media_jobs.get(&id).cloned()
    }
}

/// Событие движка о загрузке.
#[allow(clippy::too_many_arguments)]
pub fn on_engine_event(
    app: &AppHandle,
    key: u64,
    phase: &str,
    url: &str,
    path: &str,
    bytes: i64,
    total: Option<i64>,
    error: &str,
) {
    let state = app.state::<App>();
    let store = &state.store;

    let id = if phase == "started" {
        let id = match store.start_download(DownloadKind::Web, url, path, total) {
            Ok(id) => id,
            Err(err) => {
                tracing::warn!(%err, "загрузка не записана в базу");
                return;
            }
        };
        let mut inner = state.transfers.inner.lock();
        inner.by_key.insert(
            key,
            Entry {
                id,
                written: Instant::now(),
            },
        );
        inner.key_by_id.insert(id, key);
        id
    } else {
        let mut inner = state.transfers.inner.lock();
        let Some(entry) = inner.by_key.get_mut(&key) else {
            tracing::debug!(key, phase, "событие загрузки без записи");
            return;
        };
        let id = entry.id;
        match phase {
            "progress" => {
                if entry.written.elapsed() >= WRITE_EVERY {
                    entry.written = Instant::now();
                    let _ = store.update_download(id, bytes, DownloadState::Running);
                }
            }
            "paused" => {
                let _ = store.update_download(id, bytes, DownloadState::Paused);
            }
            "done" | "cancelled" => {
                let final_state = if phase == "done" {
                    DownloadState::Done
                } else {
                    DownloadState::Cancelled
                };
                let _ = store.finish_download(id, bytes, final_state, "");
                inner.by_key.remove(&key);
                inner.key_by_id.remove(&id);
            }
            _ => {
                // Оборванную загрузку оставляем в связке: её можно продолжить.
                let _ = store.finish_download(id, bytes, DownloadState::Failed, error);
            }
        }
        id
    };

    if phase != "progress" {
        tracing::debug!(key, id, phase, "загрузка");
    }
    let _ = app.emit(
        "download",
        DownloadEvent {
            id,
            kind: DownloadKind::Web,
            phase: phase.to_string(),
            url: url.to_string(),
            path: path.to_string(),
            bytes,
            total,
            error: error.to_string(),
        },
    );
}

/// Событие загрузчика видео — в тот же поток, что и обычные загрузки.
#[allow(clippy::too_many_arguments)]
pub fn emit_media(
    app: &AppHandle,
    id: i64,
    phase: &str,
    url: &str,
    path: &str,
    bytes: i64,
    total: Option<i64>,
    error: &str,
) {
    let _ = app.emit(
        "download",
        DownloadEvent {
            id,
            kind: DownloadKind::Media,
            phase: phase.to_string(),
            url: url.to_string(),
            path: path.to_string(),
            bytes,
            total,
            error: error.to_string(),
        },
    );
}

/// «Всегда спрашивать, куда сохранить»: системный диалог, ответ — в движок.
pub fn ask_target(app: &AppHandle, key: u64, suggested: String) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let suggested = PathBuf::from(suggested);
        let mut dialog = app.dialog().file().set_title("Сохранить как");
        if let Some(name) = suggested.file_name() {
            dialog = dialog.set_file_name(name.to_string_lossy());
        }
        if let Some(dir) = suggested.parent() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(main) = app.get_webview_window("chrome") {
            dialog = dialog.set_parent(&main);
        }
        let target = dialog
            .blocking_save_file()
            .and_then(|path| path.into_path().ok());
        with_host_later(&app, move |host| {
            if let Err(err) = host.download_answer(key, target) {
                tracing::warn!(%err, "ответ на загрузку не принят");
            }
        });
    });
}

/// Действие из списка загрузок.
pub async fn control(app: &AppHandle, id: i64, action: &str) -> anyhow::Result<()> {
    let state = app.state::<App>();
    let download = state
        .store
        .download(id)?
        .ok_or_else(|| anyhow::anyhow!("загрузки нет в списке"))?;
    let path = PathBuf::from(&download.path);

    match action {
        "open" => {
            anyhow::ensure!(path.is_file(), "файл удалён или перемещён");
            tauri_plugin_opener::open_path(&path, None::<&str>)?;
        }
        "show" => {
            if path.exists() {
                tauri_plugin_opener::reveal_item_in_dir(&path)?;
            } else if let Some(dir) = path.parent().filter(|dir| dir.is_dir()) {
                tauri_plugin_opener::open_path(dir, None::<&str>)?;
            } else {
                anyhow::bail!("файл удалён или перемещён");
            }
        }
        "cancel" if download.kind == DownloadKind::Media => {
            let job = state
                .transfers
                .media_job(id)
                .ok_or_else(|| anyhow::anyhow!("загрузка уже завершена"))?;
            state.services.media_cancel(&job).await?;
        }
        "cancel" | "pause" | "resume" => {
            anyhow::ensure!(
                download.kind == DownloadKind::Web,
                "загрузку видео можно только отменить"
            );
            let key = state
                .transfers
                .inner
                .lock()
                .key_by_id
                .get(&id)
                .copied()
                .ok_or_else(|| {
                    anyhow::anyhow!("эту загрузку уже не продолжить — начните заново")
                })?;
            let action = action.to_string();
            with_host(app, move |host| host.download_control(key, &action))
                .map_err(anyhow::Error::msg)??;
        }
        "retry" => {
            anyhow::ensure!(
                download.kind == DownloadKind::Web,
                "видео скачайте заново через загрузчик"
            );
            // Ссылка на файл отдаётся с Content-Disposition: attachment, и
            // навигация на неё не меняет страницу, а просто начинает загрузку.
            let url = download.url.clone();
            state.store.remove_download(id)?;
            with_host(app, move |host| {
                let active = host.active_id();
                active.and_then(|tab| host.with_tab(tab, |view| view.navigate(&url).ok()))
            })
            .map_err(anyhow::Error::msg)?
            .ok_or_else(|| anyhow::anyhow!("откройте любую вкладку и повторите"))?;
        }
        "remove" => {
            anyhow::ensure!(!download.state.is_active(), "сначала отмените загрузку");
            state.store.remove_download(id)?;
        }
        "delete" => {
            anyhow::ensure!(!download.state.is_active(), "сначала отмените загрузку");
            if path.is_file() {
                std::fs::remove_file(&path)?;
            }
            state.store.remove_download(id)?;
        }
        other => anyhow::bail!("неизвестное действие {other}"),
    }

    let _ = app.emit("downloads", ());
    Ok(())
}
