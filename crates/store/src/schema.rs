use rusqlite::Connection;

/// Версия схемы хранится в `user_version`: это дешевле отдельной таблицы и
/// не требует запроса при каждом старте.
const VERSION: i64 = 2;

/// Корневые папки закладок. Номера фиксированы: интерфейс и импорт ссылаются
/// на них напрямую, а переименовать или удалить их нельзя.
pub const BAR_FOLDER: i64 = 1;
pub const OTHER_FOLDER: i64 = 2;

pub fn migrate(db: &Connection) -> rusqlite::Result<()> {
    let current: i64 = db.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if current >= VERSION {
        return Ok(());
    }
    if current < 1 {
        db.execute_batch(V1)?;
        db.pragma_update(None, "user_version", 1)?;
    }
    if current < 2 {
        // Версия ставится внутри той же транзакции: иначе падение между
        // COMMIT и PRAGMA оставило бы базу, на которой миграция не повторяется.
        db.execute_batch(V2)?;
    }
    Ok(())
}

const V1: &str = r#"
    CREATE TABLE IF NOT EXISTS history (
        id         INTEGER PRIMARY KEY,
        url        TEXT NOT NULL,
        title      TEXT NOT NULL DEFAULT '',
        host       TEXT NOT NULL DEFAULT '',
        visits     INTEGER NOT NULL DEFAULT 1,
        visited_at INTEGER NOT NULL
    );
    CREATE UNIQUE INDEX IF NOT EXISTS history_url ON history(url);
    CREATE INDEX IF NOT EXISTS history_visited ON history(visited_at DESC);
    CREATE INDEX IF NOT EXISTS history_host ON history(host);

    CREATE TABLE IF NOT EXISTS bookmarks (
        id       INTEGER PRIMARY KEY,
        url      TEXT NOT NULL,
        title    TEXT NOT NULL DEFAULT '',
        added_at INTEGER NOT NULL,
        position INTEGER NOT NULL DEFAULT 0
    );
    CREATE UNIQUE INDEX IF NOT EXISTS bookmarks_url ON bookmarks(url);

    CREATE TABLE IF NOT EXISTS session (
        position INTEGER PRIMARY KEY,
        url      TEXT NOT NULL,
        title    TEXT NOT NULL DEFAULT '',
        active   INTEGER NOT NULL DEFAULT 0
    );

    CREATE TABLE IF NOT EXISTS downloads (
        id           INTEGER PRIMARY KEY,
        url          TEXT NOT NULL,
        path         TEXT NOT NULL,
        state        TEXT NOT NULL,
        bytes        INTEGER NOT NULL DEFAULT 0,
        total_bytes  INTEGER,
        started_at   INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS downloads_started ON downloads(started_at DESC);
"#;

/// Вторая версия: закладки становятся деревом (панель, «Другие», папки),
/// появляются настройки и пароли.
///
/// Плоская таблица закладок держала уникальный индекс по адресу — у папок
/// адреса нет, а одна и та же страница законно лежит в двух папках. Поэтому
/// не ALTER, а новая таблица с переносом.
const V2: &str = r#"
    BEGIN;

    CREATE TABLE bookmark_nodes (
        id        INTEGER PRIMARY KEY,
        parent_id INTEGER REFERENCES bookmark_nodes(id) ON DELETE CASCADE,
        kind      TEXT NOT NULL,
        title     TEXT NOT NULL DEFAULT '',
        url       TEXT,
        icon      TEXT NOT NULL DEFAULT '',
        position  INTEGER NOT NULL DEFAULT 0,
        added_at  INTEGER NOT NULL
    );
    CREATE INDEX bookmark_nodes_parent ON bookmark_nodes(parent_id, position);
    CREATE INDEX bookmark_nodes_url ON bookmark_nodes(url);

    INSERT INTO bookmark_nodes (id, parent_id, kind, title, position, added_at) VALUES
        (1, NULL, 'folder', 'Панель закладок', 0, CAST(strftime('%s', 'now') AS INTEGER)),
        (2, NULL, 'folder', 'Другие закладки', 1, CAST(strftime('%s', 'now') AS INTEGER));

    INSERT INTO bookmark_nodes (parent_id, kind, title, url, position, added_at)
        SELECT 1, 'url', title, url, ROW_NUMBER() OVER (ORDER BY position) - 1, added_at
        FROM bookmarks;
    DROP TABLE bookmarks;

    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );

    CREATE TABLE passwords (
        id         INTEGER PRIMARY KEY,
        origin     TEXT NOT NULL,
        username   TEXT NOT NULL DEFAULT '',
        secret     BLOB NOT NULL,
        created_at INTEGER NOT NULL,
        used_at    INTEGER
    );
    CREATE UNIQUE INDEX passwords_login ON passwords(origin, username);

    CREATE TABLE password_never (
        origin   TEXT PRIMARY KEY,
        added_at INTEGER NOT NULL
    );

    ALTER TABLE downloads ADD COLUMN kind TEXT NOT NULL DEFAULT 'web';
    ALTER TABLE downloads ADD COLUMN error TEXT NOT NULL DEFAULT '';

    PRAGMA user_version = 2;
    COMMIT;
"#;

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn flat_bookmarks_move_to_the_bar() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(super::V1).unwrap();
        db.pragma_update(None, "user_version", 1).unwrap();
        db.execute(
            "INSERT INTO bookmarks (url, title, added_at, position) VALUES ('https://habr.com/', 'Хабр', 1, 5)",
            [],
        )
        .unwrap();

        super::migrate(&db).unwrap();

        let (parent, title): (i64, String) = db
            .query_row(
                "SELECT parent_id, title FROM bookmark_nodes WHERE url = 'https://habr.com/'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(parent, super::BAR_FOLDER);
        assert_eq!(title, "Хабр");
    }
}
