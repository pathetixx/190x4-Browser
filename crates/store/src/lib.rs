//! Постоянное состояние браузера: история, закладки, сессия, загрузки,
//! настройки и пароли.
//!
//! Одна база SQLite в профиле пользователя. Доступ через `Mutex`: запись идёт
//! из Tauri-команд и из событий вкладок, читает интерфейс — конкуренция
//! низкая, а связываться с пулом соединений ради десятка запросов в секунду
//! незачем.
//!
//! Крейт намеренно не знает ни про COM, ни про Tauri, ни про шифрование: его
//! тесты гоняются на любой платформе, в памяти (`Store::memory`).

mod bookmarks;
pub mod bookmarks_html;
mod downloads;
pub(crate) mod history;
mod passwords;
pub mod passwords_csv;
mod schema;
mod session;
mod settings;

use std::path::Path;

use parking_lot::Mutex;
use rusqlite::Connection;

pub use bookmarks::{is_root as is_root_bookmark, BookmarkKind, BookmarkNode, ImportReport};
pub use downloads::{Download, DownloadKind, DownloadState};
pub use history::{HistoryEntry, HistoryHit, Visit};
pub use passwords::{NeverSite, PasswordEntry, PasswordSecret};
pub use schema::{BAR_FOLDER, OTHER_FOLDER};
pub use session::{SessionGroup, SessionTab};

pub struct Store {
    db: Mutex<Connection>,
}

impl Store {
    /// Открыть базу в файле профиля, создав схему при первом запуске.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let db = Connection::open(path)?;
        Self::prepare(db)
    }

    /// База в памяти — для тестов.
    pub fn memory() -> anyhow::Result<Self> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(db: Connection) -> anyhow::Result<Self> {
        // WAL: интерфейс читает историю, пока вкладки пишут визиты; без него
        // читатель и писатель блокируют друг друга.
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        // Без внешних ключей удаление папки закладок оставило бы сирот.
        db.pragma_update(None, "foreign_keys", "ON")?;
        schema::migrate(&db)?;
        Ok(Self { db: Mutex::new(db) })
    }

    pub(crate) fn with<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> anyhow::Result<T> {
        let db = self.db.lock();
        Ok(f(&db)?)
    }
}
