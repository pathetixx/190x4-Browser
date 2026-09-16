//! История: сводка по адресу и полный список посещений.
//!
//! `history` — по строке на адрес: сколько раз были и когда в последний раз.
//! По ней работают подсказки адресной строки, и ранжирование считается прямо
//! в SQL. `visits` — каждое посещение отдельной строкой: без них страница
//! истории не покажет «сегодня в 14:20 и вчера в 9:00», как это делает Chrome.

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

/// Одно посещение — строка страницы истории.
#[derive(Debug, Clone, Serialize)]
pub struct Visit {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub host: String,
    pub visited_at: i64,
}

impl Store {
    /// Записать посещение. Повторный визит на тот же URL не плодит строки в
    /// сводке, а увеличивает счётчик — иначе подсказки забиваются одним
    /// сайтом; в полном списке он остаётся отдельной строкой.
    pub fn record_visit(&self, url: &str, title: &str) -> anyhow::Result<()> {
        if !is_recordable(url) {
            return Ok(());
        }
        let host = host_of(url);
        let now = now_secs();
        self.with(|db| {
            let tx = db.unchecked_transaction()?;
            tx.execute(
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
            )?;
            // Перезагрузка страницы в ту же секунду — это одно посещение.
            let repeat: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM visits WHERE url = ?1 AND visited_at >= ?2)",
                rusqlite::params![url, now - 1],
                |row| row.get(0),
            )?;
            if repeat {
                tx.execute(
                    "UPDATE visits SET title = CASE WHEN ?2 = '' THEN title ELSE ?2 END
                     WHERE url = ?1 AND visited_at >= ?3",
                    rusqlite::params![url, title, now - 1],
                )?;
            } else {
                tx.execute(
                    "INSERT INTO visits (url, title, host, visited_at) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![url, title, host, now],
                )?;
            }
            tx.commit()
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

    /// Страница истории: посещения по времени, новые первыми. `before` —
    /// метка времени, с которой продолжать (для подгрузки), `query` —
    /// поиск по адресу и заголовку.
    pub fn history_visits(
        &self,
        query: &str,
        before: Option<i64>,
        limit: u32,
    ) -> anyhow::Result<Vec<Visit>> {
        let needle = like_pattern(query);
        let before = before.unwrap_or(i64::MAX);
        self.with(|db| {
            let mut stmt = db.prepare(
                r#"
                SELECT id, url, title, host, visited_at FROM visits
                WHERE visited_at <= ?2
                  AND (?1 = '' OR lower(url) LIKE ?1 ESCAPE '\' OR lower(title) LIKE ?1 ESCAPE '\')
                ORDER BY visited_at DESC, id DESC
                LIMIT ?3
                "#,
            )?;
            let rows: Vec<Visit> = stmt
                .query_map(rusqlite::params![needle, before, limit], |row| {
                    Ok(Visit {
                        id: row.get(0)?,
                        url: row.get(1)?,
                        title: row.get(2)?,
                        host: row.get(3)?,
                        visited_at: row.get(4)?,
                    })
                })?
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
        let needle = like_pattern(query);
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        let now = now_secs();

        self.with(|db| {
            let mut stmt = db.prepare(
                r#"
                SELECT url, title, host, visits, visited_at
                FROM history
                WHERE lower(url) LIKE ?1 ESCAPE '\' OR lower(title) LIKE ?1 ESCAPE '\'
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
        self.with(|db| {
            let tx = db.unchecked_transaction()?;
            tx.execute("DELETE FROM history WHERE url = ?1", [url])?;
            tx.execute("DELETE FROM visits WHERE url = ?1", [url])?;
            tx.commit()
        })?;
        Ok(())
    }

    /// Убрать одно посещение со страницы истории; сводка остаётся, пока есть
    /// хотя бы одно другое посещение этого адреса.
    pub fn forget_visit(&self, id: i64) -> anyhow::Result<()> {
        self.with(|db| {
            let tx = db.unchecked_transaction()?;
            let url: Option<String> = tx
                .query_row("SELECT url FROM visits WHERE id = ?1", [id], |row| {
                    row.get(0)
                })
                .ok();
            tx.execute("DELETE FROM visits WHERE id = ?1", [id])?;
            if let Some(url) = url {
                let left: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM visits WHERE url = ?1",
                    [&url],
                    |row| row.get(0),
                )?;
                if left == 0 {
                    tx.execute("DELETE FROM history WHERE url = ?1", [&url])?;
                } else {
                    tx.execute(
                        "UPDATE history SET visits = ?2,
                         visited_at = (SELECT MAX(visited_at) FROM visits WHERE url = ?1)
                         WHERE url = ?1",
                        rusqlite::params![&url, left],
                    )?;
                }
            }
            tx.commit()
        })?;
        Ok(())
    }

    pub fn clear_history(&self) -> anyhow::Result<()> {
        self.with(|db| {
            let tx = db.unchecked_transaction()?;
            tx.execute("DELETE FROM history", [])?;
            tx.execute("DELETE FROM visits", [])?;
            tx.commit()
        })?;
        Ok(())
    }

    /// Удалить историю за последнее время: «час», «сутки», «неделя».
    pub fn clear_history_since(&self, since: i64) -> anyhow::Result<()> {
        self.with(|db| {
            let tx = db.unchecked_transaction()?;
            tx.execute("DELETE FROM visits WHERE visited_at >= ?1", [since])?;
            // Сводка пересобирается по тому, что осталось.
            tx.execute(
                "DELETE FROM history WHERE url NOT IN (SELECT url FROM visits)",
                [],
            )?;
            tx.execute(
                "UPDATE history SET
                    visits = (SELECT COUNT(*) FROM visits WHERE visits.url = history.url),
                    visited_at = (SELECT MAX(visited_at) FROM visits WHERE visits.url = history.url)",
                [],
            )?;
            tx.commit()
        })?;
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

/// Шаблон для LIKE: свои `%` и `_` пользователь ищет как обычные знаки, а не
/// как подстановочные — иначе запрос «100%» находил бы всё подряд.
fn like_pattern(query: &str) -> String {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return String::new();
    }
    let escaped = query
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
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
        assert!(store.history_visits("", None, 10).unwrap().is_empty());
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
    fn search_takes_percent_literally() {
        let store = Store::memory().unwrap();
        store
            .record_visit("https://shop.example/sale", "Скидки")
            .unwrap();
        store
            .record_visit("https://shop.example/100%25", "Скидка 100%")
            .unwrap();

        let hits = store.search_history("100%", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].entry.title.contains("100%"));
        assert!(store.search_history("   ", 5).unwrap().is_empty());
    }

    #[test]
    fn every_visit_is_listed() {
        let store = Store::memory().unwrap();
        store.record_visit("https://a.example/", "А").unwrap();
        store.record_visit("https://b.example/", "Б").unwrap();

        let visits = store.history_visits("", None, 10).unwrap();
        assert_eq!(visits.len(), 2);
        assert_eq!(visits[0].url, "https://b.example/");

        let found = store.history_visits("а.example", None, 10).unwrap();
        assert_eq!(found.len(), 1);

        store.forget_visit(visits[0].id).unwrap();
        assert_eq!(store.history_visits("", None, 10).unwrap().len(), 1);
        assert_eq!(store.recent_history(10).unwrap().len(), 1);
    }

    #[test]
    fn host_is_extracted() {
        let store = Store::memory().unwrap();
        store
            .record_visit("https://www.kinopoisk.ru/film/1318972/", "Дюна")
            .unwrap();
        assert_eq!(store.recent_history(1).unwrap()[0].host, "www.kinopoisk.ru");
    }

    #[test]
    fn clearing_a_period_keeps_older_visits() {
        let store = Store::memory().unwrap();
        store
            .record_visit("https://old.example/", "Старая")
            .unwrap();
        let now = super::now_secs();
        store
            .with(|db| {
                db.execute("UPDATE visits SET visited_at = ?1", [now - 7200])?;
                db.execute("UPDATE history SET visited_at = ?1", [now - 7200])
            })
            .unwrap();
        store.record_visit("https://new.example/", "Новая").unwrap();

        store.clear_history_since(now - 3600).unwrap();
        let left = store.history_visits("", None, 10).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].url, "https://old.example/");
    }
}
