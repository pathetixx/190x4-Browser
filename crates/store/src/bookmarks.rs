//! Закладки — дерево: две корневые папки («Панель закладок» и «Другие
//! закладки»), внутри — ссылки и папки любой глубины.
//!
//! Порядок внутри папки — поле `position`, без дыр: после любого изменения
//! папка перенумеровывается целиком. Папки маленькие, а интерфейсу так не
//! приходится гадать, куда встанет перетащенная закладка.

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use crate::bookmarks_html::{FolderRole, ImportNode};
use crate::schema::{BAR_FOLDER, OTHER_FOLDER};
use crate::Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BookmarkKind {
    Folder,
    Url,
}

impl BookmarkKind {
    fn as_str(self) -> &'static str {
        match self {
            BookmarkKind::Folder => "folder",
            BookmarkKind::Url => "url",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BookmarkNode {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub kind: BookmarkKind,
    pub title: String,
    pub url: Option<String>,
    pub icon: String,
    pub position: i64,
    pub added_at: i64,
}

/// Итог импорта — что показать пользователю.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportReport {
    pub links: usize,
    pub folders: usize,
    /// Ссылки, которые уже лежали в той же папке.
    pub skipped: usize,
}

const COLUMNS: &str = "id, parent_id, kind, title, url, icon, position, added_at";

fn row_to_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<BookmarkNode> {
    let kind: String = row.get(2)?;
    Ok(BookmarkNode {
        id: row.get(0)?,
        parent_id: row.get(1)?,
        kind: if kind == "folder" {
            BookmarkKind::Folder
        } else {
            BookmarkKind::Url
        },
        title: row.get(3)?,
        url: row.get(4)?,
        icon: row.get(5)?,
        position: row.get(6)?,
        added_at: row.get(7)?,
    })
}

pub fn is_root(id: i64) -> bool {
    id == BAR_FOLDER || id == OTHER_FOLDER
}

impl Store {
    /// Всё дерево одним списком: собрать его в JS дешевле, чем гонять
    /// по запросу на папку.
    pub fn bookmark_nodes(&self) -> anyhow::Result<Vec<BookmarkNode>> {
        self.with(|db| {
            let mut stmt = db.prepare(&format!(
                "SELECT {COLUMNS} FROM bookmark_nodes
                 ORDER BY parent_id IS NOT NULL, parent_id, position, id"
            ))?;
            let rows: Vec<BookmarkNode> = stmt
                .query_map([], row_to_node)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
    }

    pub fn bookmark(&self, id: i64) -> anyhow::Result<Option<BookmarkNode>> {
        self.with(|db| {
            db.query_row(
                &format!("SELECT {COLUMNS} FROM bookmark_nodes WHERE id = ?1"),
                [id],
                row_to_node,
            )
            .optional()
        })
    }

    /// Первая закладка на этот адрес — по ней звезда в адресной строке
    /// понимает, закрашиваться ли ей и что открывать на редактирование.
    pub fn bookmark_by_url(&self, url: &str) -> anyhow::Result<Option<BookmarkNode>> {
        self.with(|db| {
            db.query_row(
                &format!(
                    "SELECT {COLUMNS} FROM bookmark_nodes
                     WHERE kind = 'url' AND url = ?1 ORDER BY id LIMIT 1"
                ),
                [url],
                row_to_node,
            )
            .optional()
        })
    }

    pub fn is_bookmarked(&self, url: &str) -> anyhow::Result<bool> {
        Ok(self.bookmark_by_url(url)?.is_some())
    }

    pub fn add_bookmark(
        &self,
        parent: i64,
        title: &str,
        url: &str,
        icon: &str,
    ) -> anyhow::Result<i64> {
        self.ensure_folder(parent)?;
        let now = crate::history::now_secs();
        self.with(|db| insert(db, parent, BookmarkKind::Url, title, Some(url), icon, now))
    }

    pub fn add_bookmark_folder(&self, parent: i64, title: &str) -> anyhow::Result<i64> {
        self.ensure_folder(parent)?;
        let now = crate::history::now_secs();
        self.with(|db| insert(db, parent, BookmarkKind::Folder, title, None, "", now))
    }

    /// Переименовать или поменять адрес. Корневые папки не трогаем: на них
    /// ссылаются интерфейс и импорт.
    pub fn update_bookmark(&self, id: i64, title: &str, url: Option<&str>) -> anyhow::Result<()> {
        if is_root(id) {
            anyhow::bail!("корневую папку нельзя изменить");
        }
        self.with(|db| {
            db.execute(
                "UPDATE bookmark_nodes SET title = ?2, url = COALESCE(?3, url) WHERE id = ?1",
                rusqlite::params![id, title, url],
            )
        })?;
        Ok(())
    }

    /// Иконку сайта узнаём позже, чем закладку: движок сообщает её после
    /// загрузки страницы. Возвращает, изменилось ли что-нибудь.
    pub fn set_bookmark_icon(&self, url: &str, icon: &str) -> anyhow::Result<bool> {
        let changed = self.with(|db| {
            db.execute(
                "UPDATE bookmark_nodes SET icon = ?2
                 WHERE kind = 'url' AND url = ?1 AND icon <> ?2",
                rusqlite::params![url, icon],
            )
        })?;
        Ok(changed > 0)
    }

    /// Переставить узел: в другую папку и/или на другое место.
    pub fn move_bookmark(&self, id: i64, parent: i64, index: usize) -> anyhow::Result<()> {
        if is_root(id) {
            anyhow::bail!("корневую папку нельзя переместить");
        }
        self.ensure_folder(parent)?;
        let node = self
            .bookmark(id)?
            .ok_or_else(|| anyhow::anyhow!("закладки {id} нет"))?;

        // Папку нельзя положить в саму себя или в собственного потомка:
        // поднимаемся от новой родительской папки к корню.
        let mut cursor = Some(parent);
        while let Some(current) = cursor {
            if current == id {
                anyhow::bail!("папку нельзя переместить внутрь неё самой");
            }
            cursor = self.bookmark(current)?.and_then(|n| n.parent_id);
        }

        self.with(|db| {
            let tx = db.unchecked_transaction()?;
            let mut siblings = children_ids(&tx, parent)?;
            siblings.retain(|sibling| *sibling != id);
            siblings.insert(index.min(siblings.len()), id);
            tx.execute(
                "UPDATE bookmark_nodes SET parent_id = ?2 WHERE id = ?1",
                rusqlite::params![id, parent],
            )?;
            write_order(&tx, &siblings)?;
            if let Some(old) = node.parent_id.filter(|old| *old != parent) {
                renumber(&tx, old)?;
            }
            tx.commit()
        })
    }

    /// Удалить узел; у папки уходит всё содержимое.
    pub fn remove_bookmark(&self, id: i64) -> anyhow::Result<()> {
        if is_root(id) {
            anyhow::bail!("корневую папку нельзя удалить");
        }
        let Some(node) = self.bookmark(id)? else {
            return Ok(());
        };
        self.with(|db| {
            let tx = db.unchecked_transaction()?;
            tx.execute("DELETE FROM bookmark_nodes WHERE id = ?1", [id])?;
            if let Some(parent) = node.parent_id {
                renumber(&tx, parent)?;
            }
            tx.commit()
        })
    }

    /// Снять звезду: убрать все закладки на этот адрес.
    pub fn remove_bookmarks_by_url(&self, url: &str) -> anyhow::Result<()> {
        self.with(|db| {
            let tx = db.unchecked_transaction()?;
            let parents: Vec<i64> = {
                let mut stmt = tx.prepare(
                    "SELECT DISTINCT parent_id FROM bookmark_nodes
                     WHERE kind = 'url' AND url = ?1 AND parent_id IS NOT NULL",
                )?;
                let rows = stmt
                    .query_map([url], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                rows
            };
            tx.execute(
                "DELETE FROM bookmark_nodes WHERE kind = 'url' AND url = ?1",
                [url],
            )?;
            for parent in parents {
                renumber(&tx, parent)?;
            }
            tx.commit()
        })
    }

    /// Импорт из HTML (формат Netscape — его экспортируют Chrome, Edge,
    /// Firefox, Яндекс).
    ///
    /// Панель закладок из файла становится нашей панелью, если та пуста; иначе
    /// её содержимое уходит в папку «Импортировано» на панели — чужие
    /// закладки не должны перемешиваться с уже расставленными. Всё остальное
    /// ложится в «Другие закладки».
    pub fn import_bookmarks(&self, nodes: &[ImportNode]) -> anyhow::Result<ImportReport> {
        let now = crate::history::now_secs();
        self.with(|db| {
            let tx = db.unchecked_transaction()?;
            let mut report = ImportReport::default();
            let bar_empty = children_ids(&tx, BAR_FOLDER)?.is_empty();
            let mut imported_folder: Option<i64> = None;

            for node in nodes {
                match node {
                    ImportNode::Folder {
                        role: FolderRole::Toolbar,
                        children,
                        ..
                    } => {
                        let target = if bar_empty {
                            BAR_FOLDER
                        } else {
                            match imported_folder {
                                Some(id) => id,
                                None => {
                                    let id = insert(
                                        &tx,
                                        BAR_FOLDER,
                                        BookmarkKind::Folder,
                                        "Импортировано",
                                        None,
                                        "",
                                        now,
                                    )?;
                                    report.folders += 1;
                                    imported_folder = Some(id);
                                    id
                                }
                            }
                        };
                        insert_tree(&tx, target, children, now, &mut report)?;
                    }
                    ImportNode::Folder {
                        role: FolderRole::Other,
                        children,
                        ..
                    } => insert_tree(&tx, OTHER_FOLDER, children, now, &mut report)?,
                    other => insert_tree(
                        &tx,
                        OTHER_FOLDER,
                        std::slice::from_ref(other),
                        now,
                        &mut report,
                    )?,
                }
            }

            tx.commit()?;
            Ok(report)
        })
    }

    /// Дерево для экспорта: панель и «Другие закладки» по отдельности.
    pub fn bookmarks_for_export(&self) -> anyhow::Result<(Vec<ImportNode>, Vec<ImportNode>)> {
        let nodes = self.bookmark_nodes()?;
        Ok((subtree(&nodes, BAR_FOLDER), subtree(&nodes, OTHER_FOLDER)))
    }

    fn ensure_folder(&self, id: i64) -> anyhow::Result<()> {
        match self.bookmark(id)? {
            Some(node) if node.kind == BookmarkKind::Folder => Ok(()),
            Some(_) => anyhow::bail!("{id} — не папка"),
            None => anyhow::bail!("папки {id} нет"),
        }
    }
}

fn subtree(nodes: &[BookmarkNode], parent: i64) -> Vec<ImportNode> {
    let mut children: Vec<&BookmarkNode> = nodes
        .iter()
        .filter(|node| node.parent_id == Some(parent))
        .collect();
    children.sort_by_key(|node| (node.position, node.id));
    children
        .into_iter()
        .map(|node| match node.kind {
            BookmarkKind::Folder => ImportNode::Folder {
                title: node.title.clone(),
                role: FolderRole::Plain,
                added_at: Some(node.added_at),
                children: subtree(nodes, node.id),
            },
            BookmarkKind::Url => ImportNode::Link {
                title: node.title.clone(),
                url: node.url.clone().unwrap_or_default(),
                icon: node.icon.clone(),
                added_at: Some(node.added_at),
            },
        })
        .collect()
}

fn insert(
    db: &Connection,
    parent: i64,
    kind: BookmarkKind,
    title: &str,
    url: Option<&str>,
    icon: &str,
    added_at: i64,
) -> rusqlite::Result<i64> {
    let position: i64 = db.query_row(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM bookmark_nodes WHERE parent_id = ?1",
        [parent],
        |row| row.get(0),
    )?;
    db.execute(
        "INSERT INTO bookmark_nodes (parent_id, kind, title, url, icon, position, added_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![parent, kind.as_str(), title, url, icon, position, added_at],
    )?;
    Ok(db.last_insert_rowid())
}

fn insert_tree(
    db: &Connection,
    parent: i64,
    nodes: &[ImportNode],
    now: i64,
    report: &mut ImportReport,
) -> rusqlite::Result<()> {
    for node in nodes {
        match node {
            ImportNode::Link {
                title,
                url,
                icon,
                added_at,
            } => {
                let exists: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM bookmark_nodes
                     WHERE parent_id = ?1 AND kind = 'url' AND url = ?2)",
                    rusqlite::params![parent, url],
                    |row| row.get(0),
                )?;
                if exists {
                    report.skipped += 1;
                    continue;
                }
                insert(
                    db,
                    parent,
                    BookmarkKind::Url,
                    title,
                    Some(url),
                    icon,
                    added_at.unwrap_or(now),
                )?;
                report.links += 1;
            }
            ImportNode::Folder {
                title,
                children,
                added_at,
                ..
            } => {
                let id = insert(
                    db,
                    parent,
                    BookmarkKind::Folder,
                    title,
                    None,
                    "",
                    added_at.unwrap_or(now),
                )?;
                report.folders += 1;
                insert_tree(db, id, children, now, report)?;
            }
        }
    }
    Ok(())
}

fn children_ids(db: &Connection, parent: i64) -> rusqlite::Result<Vec<i64>> {
    let mut stmt =
        db.prepare("SELECT id FROM bookmark_nodes WHERE parent_id = ?1 ORDER BY position, id")?;
    let rows = stmt
        .query_map([parent], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

fn write_order(db: &Connection, ids: &[i64]) -> rusqlite::Result<()> {
    let mut stmt = db.prepare("UPDATE bookmark_nodes SET position = ?2 WHERE id = ?1")?;
    for (index, id) in ids.iter().enumerate() {
        stmt.execute(rusqlite::params![id, index as i64])?;
    }
    Ok(())
}

fn renumber(db: &Connection, parent: i64) -> rusqlite::Result<()> {
    let ids = children_ids(db, parent)?;
    write_order(db, &ids)
}

#[cfg(test)]
mod tests {
    use crate::bookmarks_html::{FolderRole, ImportNode};
    use crate::{Store, BAR_FOLDER, OTHER_FOLDER};

    fn link(title: &str, url: &str) -> ImportNode {
        ImportNode::Link {
            title: title.into(),
            url: url.into(),
            icon: String::new(),
            added_at: None,
        }
    }

    #[test]
    fn bookmark_lands_in_folder_in_order() {
        let store = Store::memory().unwrap();
        let first = store
            .add_bookmark(BAR_FOLDER, "Хабр", "https://habr.com/", "")
            .unwrap();
        let second = store
            .add_bookmark(BAR_FOLDER, "GitHub", "https://github.com/", "")
            .unwrap();

        store.move_bookmark(second, BAR_FOLDER, 0).unwrap();

        let bar: Vec<i64> = store
            .bookmark_nodes()
            .unwrap()
            .into_iter()
            .filter(|n| n.parent_id == Some(BAR_FOLDER))
            .map(|n| n.id)
            .collect();
        assert_eq!(bar, vec![second, first]);
        assert!(store.is_bookmarked("https://habr.com/").unwrap());
    }

    #[test]
    fn folder_cannot_go_inside_itself() {
        let store = Store::memory().unwrap();
        let outer = store.add_bookmark_folder(BAR_FOLDER, "Работа").unwrap();
        let inner = store.add_bookmark_folder(outer, "Проекты").unwrap();
        assert!(store.move_bookmark(outer, inner, 0).is_err());
        assert!(store.move_bookmark(BAR_FOLDER, OTHER_FOLDER, 0).is_err());
    }

    #[test]
    fn removing_folder_removes_contents() {
        let store = Store::memory().unwrap();
        let folder = store.add_bookmark_folder(BAR_FOLDER, "Чтение").unwrap();
        store
            .add_bookmark(folder, "Хабр", "https://habr.com/", "")
            .unwrap();
        store.remove_bookmark(folder).unwrap();
        assert!(!store.is_bookmarked("https://habr.com/").unwrap());
    }

    #[test]
    fn import_fills_empty_bar_and_skips_duplicates() {
        let store = Store::memory().unwrap();
        let tree = vec![
            ImportNode::Folder {
                title: "Bookmarks bar".into(),
                role: FolderRole::Toolbar,
                added_at: None,
                children: vec![
                    link("Хабр", "https://habr.com/"),
                    link("Хабр", "https://habr.com/"),
                ],
            },
            link("Кинопоиск", "https://www.kinopoisk.ru/"),
        ];

        let report = store.import_bookmarks(&tree).unwrap();
        assert_eq!(report.links, 2);
        assert_eq!(report.skipped, 1);

        let nodes = store.bookmark_nodes().unwrap();
        assert!(nodes
            .iter()
            .any(|n| n.parent_id == Some(BAR_FOLDER) && n.title == "Хабр"));
        assert!(nodes
            .iter()
            .any(|n| n.parent_id == Some(OTHER_FOLDER) && n.title == "Кинопоиск"));

        // Вторая волна: панель уже не пуста — импорт уходит в свою папку.
        store.import_bookmarks(&tree).unwrap();
        let nodes = store.bookmark_nodes().unwrap();
        assert!(nodes
            .iter()
            .any(|n| n.parent_id == Some(BAR_FOLDER) && n.title == "Импортировано"));
    }
}
