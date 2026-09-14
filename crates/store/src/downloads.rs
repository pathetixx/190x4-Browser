use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Running,
    Paused,
    Done,
    Failed,
    Cancelled,
}

impl DownloadState {
    pub fn as_str(self) -> &'static str {
        match self {
            DownloadState::Running => "running",
            DownloadState::Paused => "paused",
            DownloadState::Done => "done",
            DownloadState::Failed => "failed",
            DownloadState::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "paused" => DownloadState::Paused,
            "done" => DownloadState::Done,
            "failed" => DownloadState::Failed,
            "cancelled" => DownloadState::Cancelled,
            _ => DownloadState::Running,
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, DownloadState::Running | DownloadState::Paused)
    }
}

/// Откуда загрузка: обычная из страницы или через загрузчик видео.
/// Действия у них разные — отменять серверное задание нужно на сервере.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadKind {
    Web,
    Media,
}

impl DownloadKind {
    fn as_str(self) -> &'static str {
        match self {
            DownloadKind::Web => "web",
            DownloadKind::Media => "media",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Download {
    pub id: i64,
    pub kind: DownloadKind,
    pub url: String,
    pub path: String,
    pub state: DownloadState,
    pub bytes: i64,
    pub total_bytes: Option<i64>,
    pub started_at: i64,
    pub error: String,
}

const COLUMNS: &str = "id, kind, url, path, state, bytes, total_bytes, started_at, error";

fn row_to_download(row: &rusqlite::Row<'_>) -> rusqlite::Result<Download> {
    let kind: String = row.get(1)?;
    Ok(Download {
        id: row.get(0)?,
        kind: if kind == "media" {
            DownloadKind::Media
        } else {
            DownloadKind::Web
        },
        url: row.get(2)?,
        path: row.get(3)?,
        state: DownloadState::parse(&row.get::<_, String>(4)?),
        bytes: row.get(5)?,
        total_bytes: row.get(6)?,
        started_at: row.get(7)?,
        error: row.get(8)?,
    })
}

impl Store {
    /// Завести запись о начатой загрузке; id нужен, чтобы потом обновлять прогресс.
    pub fn start_download(
        &self,
        kind: DownloadKind,
        url: &str,
        path: &str,
        total: Option<i64>,
    ) -> anyhow::Result<i64> {
        let now = crate::history::now_secs();
        self.with(|db| {
            db.execute(
                "INSERT INTO downloads (kind, url, path, state, bytes, total_bytes, started_at)
                 VALUES (?1, ?2, ?3, 'running', 0, ?4, ?5)",
                rusqlite::params![kind.as_str(), url, path, total, now],
            )?;
            Ok(db.last_insert_rowid())
        })
    }

    pub fn update_download(&self, id: i64, bytes: i64, state: DownloadState) -> anyhow::Result<()> {
        self.with(|db| {
            db.execute(
                "UPDATE downloads SET bytes = ?2, state = ?3 WHERE id = ?1",
                rusqlite::params![id, bytes, state.as_str()],
            )
        })?;
        Ok(())
    }

    /// Итог загрузки. Причину ошибки храним: «Ошибка» без объяснения в списке
    /// загрузок бесполезна.
    pub fn finish_download(
        &self,
        id: i64,
        bytes: i64,
        state: DownloadState,
        error: &str,
    ) -> anyhow::Result<()> {
        self.with(|db| {
            db.execute(
                "UPDATE downloads SET bytes = ?2, state = ?3, error = ?4 WHERE id = ?1",
                rusqlite::params![id, bytes, state.as_str(), error],
            )
        })?;
        Ok(())
    }

    /// Путь и размер уточняются по ходу: имя файла у загрузчика видео
    /// известно только после разбора ссылки на сервере.
    pub fn set_download_target(
        &self,
        id: i64,
        path: &str,
        total: Option<i64>,
    ) -> anyhow::Result<()> {
        self.with(|db| {
            db.execute(
                "UPDATE downloads SET path = ?2, total_bytes = COALESCE(?3, total_bytes) WHERE id = ?1",
                rusqlite::params![id, path, total],
            )
        })?;
        Ok(())
    }

    pub fn download(&self, id: i64) -> anyhow::Result<Option<Download>> {
        self.with(|db| {
            db.query_row(
                &format!("SELECT {COLUMNS} FROM downloads WHERE id = ?1"),
                [id],
                row_to_download,
            )
            .optional()
        })
    }

    pub fn downloads(&self, limit: u32) -> anyhow::Result<Vec<Download>> {
        self.with(|db| {
            let mut stmt = db.prepare(&format!(
                "SELECT {COLUMNS} FROM downloads ORDER BY started_at DESC, id DESC LIMIT ?1"
            ))?;
            let rows: Vec<Download> = stmt
                .query_map([limit], row_to_download)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
    }

    /// Убрать из списка. Файл на диске остаётся.
    pub fn remove_download(&self, id: i64) -> anyhow::Result<()> {
        self.with(|db| db.execute("DELETE FROM downloads WHERE id = ?1", [id]))?;
        Ok(())
    }

    /// Очистить список от завершённых. Идущие загрузки не трогаем.
    pub fn clear_downloads(&self) -> anyhow::Result<()> {
        self.with(|db| {
            db.execute(
                "DELETE FROM downloads WHERE state NOT IN ('running', 'paused')",
                [],
            )
        })?;
        Ok(())
    }

    /// Загрузки, которые шли в момент закрытия браузера, больше не идут.
    /// Без этого после перезапуска они вечно висели бы с прогрессом.
    pub fn fail_interrupted_downloads(&self) -> anyhow::Result<()> {
        self.with(|db| {
            db.execute(
                "UPDATE downloads SET state = 'failed', error = 'shutdown'
                 WHERE state IN ('running', 'paused')",
                [],
            )
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{DownloadKind, DownloadState, Store};

    #[test]
    fn download_progress_is_tracked() {
        let store = Store::memory().unwrap();
        let id = store
            .start_download(
                DownloadKind::Web,
                "https://example/file.zip",
                "C:/downloads/file.zip",
                Some(1000),
            )
            .unwrap();

        store
            .update_download(id, 512, DownloadState::Running)
            .unwrap();
        store
            .finish_download(id, 1000, DownloadState::Done, "")
            .unwrap();

        let all = store.downloads(10).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].bytes, 1000);
        assert_eq!(all[0].state, DownloadState::Done);
    }

    #[test]
    fn restart_fails_unfinished_and_clear_keeps_them() {
        let store = Store::memory().unwrap();
        let running = store
            .start_download(DownloadKind::Media, "https://youtu.be/x", "", None)
            .unwrap();
        let done = store
            .start_download(DownloadKind::Web, "https://example/a", "C:/a", None)
            .unwrap();
        store
            .finish_download(done, 1, DownloadState::Done, "")
            .unwrap();

        store.clear_downloads().unwrap();
        assert_eq!(store.downloads(10).unwrap().len(), 1);

        store.fail_interrupted_downloads().unwrap();
        let left = store.download(running).unwrap().unwrap();
        assert_eq!(left.state, DownloadState::Failed);
        assert_eq!(left.error, "shutdown");
    }
}
