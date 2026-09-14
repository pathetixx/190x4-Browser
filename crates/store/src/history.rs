use serde::Serialize;

use crate::Store;

#[derive(Debug, Clone, Serialize)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,
    pub host: String,
    pub visits: u32,
    pub visited_at: i64,
}

/// Совпадение для адресной строки: та же запись плюс вес, по которому
/// подсказки сортируются.
#[derive(Debug, Clone, Serialize)]
pub struct HistoryHit {
    #[serde(flatten)]
    pub entry: HistoryEntry,
    pub score: f64,
}

impl Store {
    /// Записать посещение. Повторный визит на тот же URL не плодит строки, а
    /// увеличивает счётчик — иначе подсказки забиваются одним сайтом.
    pub fn record_visit(&self, url: &str, title: &str) -> anyhow::Result<()> {
        if !is_recordable(url) {
            return Ok(());
        }
        let host = host_of(url);
        let now = now_secs();
        self.with(|db| {
            db.execute(
                r#"
                INSERT INTO history (url, title, host, visits, visited_at)
                VALUES (?1, ?2, ?3, 1, ?4)
                ON CONFLICT(url) DO UPDATE SET
                    -- Пустым заголовком затирать уже известный не даём:
                    -- NavigationCompleted иногда приходит раньше DocumentTitleChanged.
                    title      = CASE WHEN excluded.title = '' THEN history.title ELSE excluded.title END,
                    visits     = history.visits + 1,
                    visited_at = excluded.visited_at
                "#,
                rusqlite::params![url, title, host, now],
            )
        })?;
        Ok(())
    }

    pub fn recent_history(&self, limit: u32) -> anyhow::Result<Vec<HistoryEntry>> {
        self.with(|db| {
            let mut stmt = db.prepare(
                "SELECT url, title, host, visits, visited_at FROM history
                 ORDER BY visited_at DESC LIMIT ?1",
            )?;
            let rows: Vec<HistoryEntry> = stmt
                .query_map([limit], row_to_entry)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
    }

    /// Поиск для адресной строки.
    ///
    /// Вес — «частота против свежести»: часто посещаемый сайт не должен
    /// проваливаться под вчерашнюю случайную ссылку, но и месячной давности
    /// рекорд не должен держаться вечно.
    pub fn search_history(&self, query: &str, limit: u32) -> anyhow::Result<Vec<HistoryHit>> {
        let needle = format!("%{}%", query.trim().to_lowercase());
        let now = now_secs();

        self.with(|db| {
            let mut stmt = db.prepare(
                r#"
                SELECT url, title, host, visits, visited_at
                FROM history
                WHERE lower(url) LIKE ?1 OR lower(title) LIKE ?1
                ORDER BY visited_at DESC
                LIMIT 200
                "#,
            )?;
            let entries: Vec<HistoryEntry> = stmt
                .query_map([&needle], row_to_entry)?
                .collect::<rusqlite::Result<_>>()?;

            let mut hits: Vec<HistoryHit> = entries
                .into_iter()
                .map(|entry| {
                    let age_days = ((now - entry.visited_at) as f64 / 86_400.0).max(0.0);
                    let score = (entry.visits as f64).sqrt() / (1.0 + age_days / 7.0);
                    HistoryHit { entry, score }
                })
                .collect();

            hits.sort_by(|a, b| b.score.total_cmp(&a.score));
            hits.truncate(limit as usize);
            Ok(hits)
        })
    }

    pub fn forget_history(&self, url: &str) -> anyhow::Result<()> {
        self.with(|db| db.execute("DELETE FROM history WHERE url = ?1", [url]))?;
        Ok(())
    }

    pub fn clear_history(&self) -> anyhow::Result<()> {
        self.with(|db| db.execute("DELETE FROM history", []))?;
        Ok(())
    }
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
    Ok(HistoryEntry {
        url: row.get(0)?,
        title: row.get(1)?,
        host: row.get(2)?,
        visits: row.get(3)?,
        visited_at: row.get(4)?,
    })
}

/// Что в историю не попадает.
///
/// Встроенные страницы и служебные схемы — шум: пользователь ищет сайты, а не
/// свою же новую вкладку.
fn is_recordable(url: &str) -> bool {
    !(url.is_empty()
        || url.starts_with("about:")
        || url.starts_with("data:")
        || url.starts_with("blob:")
        || url.contains("190x4-pages.invalid"))
}

fn host_of(url: &str) -> String {
    url.split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or("").to_string())
        .unwrap_or_default()
}

pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use crate::Store;

    #[test]
    fn repeat_visit_bumps_counter() {
        let store = Store::memory().unwrap();
        store.record_visit("https://habr.com/", "Хабр").unwrap();
        store.record_visit("https://habr.com/", "Хабр").unwrap();

        let recent = store.recent_history(10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].visits, 2);
    }

    #[test]
    fn empty_title_does_not_overwrite() {
        let store = Store::memory().unwrap();
        store.record_visit("https://habr.com/", "Хабр").unwrap();
        store.record_visit("https://habr.com/", "").unwrap();

        assert_eq!(store.recent_history(1).unwrap()[0].title, "Хабр");
    }

    #[test]
    fn internal_pages_are_not_recorded() {
        let store = Store::memory().unwrap();
        store
            .record_visit("http://190x4-pages.invalid/newtab.html", "Новая вкладка")
            .unwrap();
        store.record_visit("about:blank", "").unwrap();

        assert!(store.recent_history(10).unwrap().is_empty());
    }

    #[test]
    fn search_prefers_frequent_over_stale() {
        let store = Store::memory().unwrap();
        store
            .record_visit("https://habr.com/rare", "Редкая")
            .unwrap();
        for _ in 0..5 {
            store
                .record_visit("https://habr.com/often", "Частая")
                .unwrap();
        }

        let hits = store.search_history("habr", 5).unwrap();
        assert_eq!(hits[0].entry.url, "https://habr.com/often");
    }

    #[test]
    fn host_is_extracted() {
        let store = Store::memory().unwrap();
        store
            .record_visit("https://www.kinopoisk.ru/film/1318972/", "Дюна")
            .unwrap();
        assert_eq!(store.recent_history(1).unwrap()[0].host, "www.kinopoisk.ru");
    }
}
