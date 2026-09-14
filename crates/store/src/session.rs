use serde::{Deserialize, Serialize};

use crate::Store;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTab {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub active: bool,
}

impl Store {
    /// Сохранить раскладку вкладок целиком.
    ///
    /// Пишется на каждое заметное изменение (открытие, закрытие, навигация),
    /// поэтому всё в одной транзакции: на десяти вкладках это один fsync,
    /// а не десять.
    pub fn save_session(&self, tabs: &[SessionTab]) -> anyhow::Result<()> {
        self.with(|db| {
            db.execute("DELETE FROM session", [])?;
            let mut stmt = db.prepare(
                "INSERT INTO session (position, url, title, active) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (index, tab) in tabs.iter().enumerate() {
                stmt.execute(rusqlite::params![
                    index as i64,
                    tab.url,
                    tab.title,
                    tab.active as i64
                ])?;
            }
            Ok(())
        })
    }

    /// Что открыть при старте. Пустой список означает «показать новую вкладку».
    pub fn restore_session(&self) -> anyhow::Result<Vec<SessionTab>> {
        self.with(|db| {
            let mut stmt =
                db.prepare("SELECT url, title, active FROM session ORDER BY position")?;
            let rows: Vec<SessionTab> = stmt
                .query_map([], |row| {
                    Ok(SessionTab {
                        url: row.get(0)?,
                        title: row.get(1)?,
                        active: row.get::<_, i64>(2)? == 1,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
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
        }
    }

    #[test]
    fn session_round_trip_keeps_order_and_active() {
        let store = Store::memory().unwrap();
        store
            .save_session(&[
                tab("https://a.example/", false),
                tab("https://b.example/", true),
            ])
            .unwrap();

        let restored = store.restore_session().unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].url, "https://a.example/");
        assert!(restored[1].active);
    }

    #[test]
    fn saving_replaces_previous_session() {
        let store = Store::memory().unwrap();
        store
            .save_session(&[tab("https://a.example/", true)])
            .unwrap();
        store
            .save_session(&[tab("https://b.example/", true)])
            .unwrap();

        let restored = store.restore_session().unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].url, "https://b.example/");
    }
}
