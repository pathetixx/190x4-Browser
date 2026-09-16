use serde::{Deserialize, Serialize};

use crate::Store;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTab {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub active: bool,
    /// Закреплённая вкладка: она открывается первой и без крестика.
    #[serde(default)]
    pub pinned: bool,
}

impl Store {
    /// Сохранить раскладку вкладок одного окна.
    ///
    /// Пишется на каждое заметное изменение (открытие, закрытие, навигация) и
    /// ещё раз при закрытии окна, поэтому всё в одной транзакции: на десяти
    /// вкладках это один fsync, а не десять, и оборванная запись не оставляет
    /// окно с наполовину стёртой сессией.
    pub fn save_session(&self, window: i64, tabs: &[SessionTab]) -> anyhow::Result<()> {
        self.with(|db| {
            let tx = db.unchecked_transaction()?;
            tx.execute("DELETE FROM session_windows WHERE window = ?1", [window])?;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO session_windows (window, position, url, title, active, pinned)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )?;
                for (index, tab) in tabs.iter().enumerate() {
                    stmt.execute(rusqlite::params![
                        window,
                        index as i64,
                        tab.url,
                        tab.title,
                        tab.active as i64,
                        tab.pinned as i64,
                    ])?;
                }
            }
            tx.commit()
        })
    }

    /// Что открыть при старте окна. Пустой список означает «показать новую вкладку».
    pub fn restore_session(&self, window: i64) -> anyhow::Result<Vec<SessionTab>> {
        self.with(|db| {
            let mut stmt = db.prepare(
                "SELECT url, title, active, pinned FROM session_windows
                 WHERE window = ?1 ORDER BY position",
            )?;
            let rows: Vec<SessionTab> = stmt
                .query_map([window], |row| {
                    Ok(SessionTab {
                        url: row.get(0)?,
                        title: row.get(1)?,
                        active: row.get::<_, i64>(2)? == 1,
                        pinned: row.get::<_, i64>(3)? == 1,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
    }

    /// Номера окон, которые были открыты в прошлый раз, — по порядку.
    pub fn session_windows(&self) -> anyhow::Result<Vec<i64>> {
        self.with(|db| {
            let mut stmt =
                db.prepare("SELECT DISTINCT window FROM session_windows ORDER BY window")?;
            let rows: Vec<i64> = stmt
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
    }

    /// Забыть сессию окна — например, когда окно закрыли, а браузер остался.
    pub fn forget_session(&self, window: i64) -> anyhow::Result<()> {
        self.with(|db| db.execute("DELETE FROM session_windows WHERE window = ?1", [window]))?;
        Ok(())
    }

    /// Сессии окон, которых больше нет: при выходе остаются только живые окна.
    pub fn keep_sessions(&self, windows: &[i64]) -> anyhow::Result<()> {
        let list = windows
            .iter()
            .map(|window| window.to_string())
            .collect::<Vec<_>>()
            .join(",");
        self.with(|db| {
            db.execute(
                &format!("DELETE FROM session_windows WHERE window NOT IN ({list})"),
                [],
            )
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{SessionTab, Store};

    fn tab(url: &str, active: bool) -> SessionTab {
        SessionTab {
            url: url.into(),
            title: String::new(),
            active,
            pinned: false,
        }
    }

    #[test]
    fn session_round_trip_keeps_order_and_active() {
        let store = Store::memory().unwrap();
        store
            .save_session(
                0,
                &[
                    tab("https://a.example/", false),
                    tab("https://b.example/", true),
                ],
            )
            .unwrap();

        let restored = store.restore_session(0).unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].url, "https://a.example/");
        assert!(restored[1].active);
    }

    #[test]
    fn windows_keep_their_own_tabs() {
        let store = Store::memory().unwrap();
        store
            .save_session(0, &[tab("https://a.example/", true)])
            .unwrap();
        store
            .save_session(1, &[tab("https://b.example/", true)])
            .unwrap();

        assert_eq!(store.session_windows().unwrap(), vec![0, 1]);
        assert_eq!(
            store.restore_session(1).unwrap()[0].url,
            "https://b.example/"
        );

        store.forget_session(1).unwrap();
        assert_eq!(store.session_windows().unwrap(), vec![0]);
        assert!(store.restore_session(1).unwrap().is_empty());
    }

    #[test]
    fn saving_replaces_previous_session() {
        let store = Store::memory().unwrap();
        store
            .save_session(0, &[tab("https://a.example/", true)])
            .unwrap();
        store
            .save_session(0, &[tab("https://b.example/", true)])
            .unwrap();

        let restored = store.restore_session(0).unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].url, "https://b.example/");
    }

    #[test]
    fn pinned_flag_survives() {
        let store = Store::memory().unwrap();
        let mut pinned = tab("https://a.example/", true);
        pinned.pinned = true;
        store.save_session(0, &[pinned]).unwrap();
        assert!(store.restore_session(0).unwrap()[0].pinned);
    }

    #[test]
    fn closed_windows_are_dropped_on_exit() {
        let store = Store::memory().unwrap();
        store
            .save_session(0, &[tab("https://a.example/", true)])
            .unwrap();
        store
            .save_session(3, &[tab("https://c.example/", true)])
            .unwrap();
        store.keep_sessions(&[0]).unwrap();
        assert_eq!(store.session_windows().unwrap(), vec![0]);
    }
}
