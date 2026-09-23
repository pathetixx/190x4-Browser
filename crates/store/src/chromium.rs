//! Импорт из браузеров на Chromium: Chrome, Edge, Яндекс Браузер, Brave,
//! Vivaldi, Opera.
//!
//! Закладки лежат в профиле файлом `Bookmarks` (JSON), история — базой SQLite
//! `History`; время у обоих — микросекунды с 1601 года, как в Windows. Модуль
//! не знает, где профили лежат: их ищет приложение, здесь — разбор и запись в
//! свою базу. Пароли так не перенести: Chrome и Edge шифруют их ключом,
//! привязанным к своему exe, — для них остаётся экспорт в CSV.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::bookmarks_html::{FolderRole, ImportNode};
use crate::history::{host_of, is_recordable, searchable};
use crate::Store;

/// Микросекунды от 1601-01-01 до 1970-01-01.
const EPOCH_OFFSET: i64 = 11_644_473_600_000_000;

fn unix_secs(time: i64) -> Option<i64> {
    (time > EPOCH_OFFSET).then(|| (time - EPOCH_OFFSET) / 1_000_000)
}

/// Закладки из файла `Bookmarks`: панель, «Другие закладки» и закладки с
/// телефона — отдельной папкой.
pub fn parse_bookmarks(json: &str) -> anyhow::Result<Vec<ImportNode>> {
    let root: serde_json::Value = serde_json::from_str(json)?;
    let roots = root
        .get("roots")
        .ok_or_else(|| anyhow::anyhow!("в файле нет закладок"))?;
    let mut out = Vec::new();
    for (key, role, title) in [
        ("bookmark_bar", FolderRole::Toolbar, "Панель закладок"),
        ("other", FolderRole::Other, "Другие закладки"),
        ("synced", FolderRole::Plain, "Закладки с телефона"),
    ] {
        let Some(folder) = roots.get(key) else {
            continue;
        };
        let children = children_of(folder);
        if children.is_empty() {
            continue;
        }
        out.push(ImportNode::Folder {
            title: title.to_string(),
            role,
            added_at: added_at(folder),
            children,
        });
    }
    Ok(out)
}

fn children_of(folder: &serde_json::Value) -> Vec<ImportNode> {
    folder
        .get("children")
        .and_then(|children| children.as_array())
        .map(|items| items.iter().filter_map(node).collect())
        .unwrap_or_default()
}

fn node(value: &serde_json::Value) -> Option<ImportNode> {
    let title = value
        .get("name")
        .and_then(|name| name.as_str())
        .unwrap_or_default()
        .to_string();
    match value.get("type")?.as_str()? {
        "url" => Some(ImportNode::Link {
            title,
            url: value.get("url")?.as_str()?.to_string(),
            icon: String::new(),
            added_at: added_at(value),
        }),
        "folder" => Some(ImportNode::Folder {
            title,
            role: FolderRole::Plain,
            added_at: added_at(value),
            children: children_of(value),
        }),
        _ => None,
    }
}

fn added_at(value: &serde_json::Value) -> Option<i64> {
    let time = value.get("date_added")?.as_str()?.parse::<i64>().ok()?;
    unix_secs(time)
}

/// Адрес из истории другого браузера: сколько раз на нём были и когда в
/// последний раз.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedPage {
    pub url: String,
    pub title: String,
    pub visits: i64,
    pub visited_at: i64,
}

/// Одно посещение — строка страницы истории.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedVisit {
    pub url: String,
    pub title: String,
    pub visited_at: i64,
}

/// Прочитать историю из копии базы `History`: адреса, где были не раньше
/// `since` (unix-секунды), и не больше `limit` последних посещений. Переходы
/// во фреймах страниц в историю не попадают — Chrome их тоже не показывает.
pub fn read_history(
    path: &Path,
    since: i64,
    limit: u32,
) -> anyhow::Result<(Vec<ImportedPage>, Vec<ImportedVisit>)> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let from = since.saturating_mul(1_000_000).saturating_add(EPOCH_OFFSET);
    Ok((visited_pages(&db, from)?, visits_since(&db, from, limit)?))
}

fn visited_pages(db: &Connection, from: i64) -> rusqlite::Result<Vec<ImportedPage>> {
    // `hidden` — адреса фреймов и перенаправлений; в старых базах его нет.
    let sql = if db.prepare("SELECT hidden FROM urls LIMIT 0").is_ok() {
        "SELECT url, title, visit_count, last_visit_time FROM urls
         WHERE hidden = 0 AND last_visit_time >= ?1"
    } else {
        "SELECT url, title, visit_count, last_visit_time FROM urls
         WHERE last_visit_time >= ?1"
    };
    let mut stmt = db.prepare(sql)?;
    let rows = stmt.query_map([from], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    let mut pages = Vec::new();
    for row in rows {
        let (url, title, visits, last) = row?;
        if let Some(visited_at) = unix_secs(last) {
            pages.push(ImportedPage {
                url,
                title,
                visits: visits.max(1),
                visited_at,
            });
        }
    }
    Ok(pages)
}

fn visits_since(db: &Connection, from: i64, limit: u32) -> rusqlite::Result<Vec<ImportedVisit>> {
    let mut stmt = db.prepare(
        "SELECT urls.url, urls.title, visits.visit_time FROM visits
         JOIN urls ON urls.id = visits.url
         WHERE visits.visit_time >= ?1 AND (visits.transition & 255) NOT IN (3, 4)
         ORDER BY visits.visit_time DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![from, limit], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut visits = Vec::new();
    for row in rows {
        let (url, title, time) = row?;
        if let Some(visited_at) = unix_secs(time) {
            visits.push(ImportedVisit {
                url,
                title,
                visited_at,
            });
        }
    }
    Ok(visits)
}

impl Store {
    /// Записать историю другого браузера. Повторный импорт той же истории не
    /// удваивает ни счётчики, ни посещения. Возвращает, сколько адресов и
    /// посещений записано.
    pub fn import_history(
        &self,
        pages: &[ImportedPage],
        visits: &[ImportedVisit],
    ) -> anyhow::Result<(usize, usize)> {
        self.with(|db| {
            let tx = db.unchecked_transaction()?;
            let mut written_pages = 0;
            {
                let mut upsert = tx.prepare(
                    "INSERT INTO history (url, title, host, search, visits, visited_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(url) DO UPDATE SET
                         title = CASE WHEN history.title = ''
                             THEN excluded.title ELSE history.title END,
                         search = CASE WHEN history.title = ''
                             THEN excluded.search ELSE history.search END,
                         visits = MAX(history.visits, excluded.visits),
                         visited_at = MAX(history.visited_at, excluded.visited_at)",
                )?;
                for page in pages.iter().filter(|page| is_recordable(&page.url)) {
                    upsert.execute(rusqlite::params![
                        page.url,
                        page.title,
                        host_of(&page.url),
                        searchable(&page.title, &page.url),
                        page.visits,
                        page.visited_at,
                    ])?;
                    written_pages += 1;
                }
            }
            let mut written_visits = 0;
            {
                let mut insert = tx.prepare(
                    "INSERT INTO visits (url, title, host, search, visited_at)
                     SELECT ?1, ?2, ?3, ?4, ?5
                     WHERE NOT EXISTS (SELECT 1 FROM visits WHERE url = ?1 AND visited_at = ?5)",
                )?;
                for visit in visits.iter().filter(|visit| is_recordable(&visit.url)) {
                    written_visits += insert.execute(rusqlite::params![
                        visit.url,
                        visit.title,
                        host_of(&visit.url),
                        searchable(&visit.title, &visit.url),
                        visit.visited_at,
                    ])?;
                }
            }
            tx.commit()?;
            Ok((written_pages, written_visits))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2024-01-01 00:00:00 UTC во времени Chromium.
    const JAN_2024: i64 = 1_704_067_200 * 1_000_000 + EPOCH_OFFSET;

    fn folder(node: &ImportNode) -> (&str, FolderRole, &[ImportNode]) {
        match node {
            ImportNode::Folder {
                title,
                role,
                children,
                ..
            } => (title.as_str(), *role, children.as_slice()),
            ImportNode::Link { .. } => panic!("ожидалась папка"),
        }
    }

    #[test]
    fn bookmarks_keep_bar_other_and_nested_folders() {
        let json = serde_json::json!({
            "roots": {
                "bookmark_bar": {"type": "folder", "name": "Bookmarks bar", "children": [
                    {"type": "url", "name": "Хабр", "url": "https://habr.com/", "date_added": JAN_2024.to_string()},
                    {"type": "folder", "name": "Работа", "children": [
                        {"type": "url", "name": "GitHub", "url": "https://github.com/"}
                    ]}
                ]},
                "other": {"type": "folder", "name": "Other", "children": [
                    {"type": "url", "name": "Кинопоиск", "url": "https://kinopoisk.ru/"}
                ]},
                "synced": {"type": "folder", "name": "Mobile", "children": []}
            }
        })
        .to_string();
        let nodes = parse_bookmarks(&json).unwrap();
        assert_eq!(nodes.len(), 2);
        let (_, role, bar) = folder(&nodes[0]);
        assert_eq!(role, FolderRole::Toolbar);
        assert_eq!(
            bar[0],
            ImportNode::Link {
                title: "Хабр".into(),
                url: "https://habr.com/".into(),
                icon: String::new(),
                added_at: Some(1_704_067_200),
            }
        );
        let (title, _, nested) = folder(&bar[1]);
        assert_eq!((title, nested.len()), ("Работа", 1));

        let store = Store::memory().unwrap();
        let report = store.import_bookmarks(&nodes).unwrap();
        assert_eq!(report.links, 3);
    }

    #[test]
    fn history_imports_pages_and_visits_once() {
        let dir = std::env::temp_dir().join(format!("190x4-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("History");
        let _ = std::fs::remove_file(&path);
        {
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute_batch(
                "CREATE TABLE urls (id INTEGER PRIMARY KEY, url TEXT, title TEXT,
                     visit_count INTEGER, typed_count INTEGER, last_visit_time INTEGER,
                     hidden INTEGER);
                 CREATE TABLE visits (id INTEGER PRIMARY KEY, url INTEGER, visit_time INTEGER,
                     transition INTEGER);",
            )
            .unwrap();
            let hour = 3_600 * 1_000_000;
            db.execute(
                "INSERT INTO urls VALUES (1, 'https://habr.com/', 'Хабр', 5, 0, ?1, 0),
                                        (2, 'https://ads.example/frame', '', 1, 0, ?1, 1)",
                [JAN_2024 + hour],
            )
            .unwrap();
            db.execute(
                "INSERT INTO visits VALUES (1, 1, ?1, 1), (2, 1, ?2, 1), (3, 2, ?2, 3)",
                [JAN_2024, JAN_2024 + hour],
            )
            .unwrap();
        }

        let (pages, visits) = read_history(&path, 1_704_067_200, 100).unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].visits, 5);
        assert_eq!(visits.len(), 2);

        let store = Store::memory().unwrap();
        assert_eq!(store.import_history(&pages, &visits).unwrap(), (1, 2));
        assert_eq!(store.import_history(&pages, &visits).unwrap(), (1, 0));
        let hits = store.search_history("хабр", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.visits, 5);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
