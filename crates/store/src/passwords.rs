//! Сохранённые пароли.
//!
//! Крейт хранит только зашифрованные байты: шифрует и расшифровывает
//! приложение (DPAPI текущего пользователя Windows), а здесь о ключах ничего
//! не знают. Поэтому ни одна функция ниже не возвращает пароль открытым
//! текстом — и тесты гоняются на любой платформе.

use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::passwords_csv::{same_site, site_of};
use crate::Store;

#[derive(Debug, Clone, Serialize)]
pub struct PasswordEntry {
    pub id: i64,
    pub origin: String,
    pub username: String,
    pub created_at: i64,
    pub used_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct PasswordSecret {
    pub id: i64,
    pub origin: String,
    pub username: String,
    pub secret: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NeverSite {
    pub origin: String,
    pub added_at: i64,
}

impl Store {
    pub fn password_entries(&self) -> anyhow::Result<Vec<PasswordEntry>> {
        self.with(|db| {
            let mut stmt = db.prepare(
                "SELECT id, origin, username, created_at, used_at FROM passwords
                 ORDER BY origin, username",
            )?;
            let rows: Vec<PasswordEntry> = stmt
                .query_map([], |row| {
                    Ok(PasswordEntry {
                        id: row.get(0)?,
                        origin: row.get(1)?,
                        username: row.get(2)?,
                        created_at: row.get(3)?,
                        used_at: row.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
    }

    /// Учётки для страницы: сохранённые для этого же адреса, затем для других
    /// адресов того же сайта (`google.com` для `accounts.google.com`), свежие
    /// первыми. Автозаполнение берёт первую.
    pub fn password_secrets_for(&self, origin: &str) -> anyhow::Result<Vec<PasswordSecret>> {
        // SQL отбирает кандидатов по окончанию адреса, сайт сверяет same_site.
        let pattern = site_of(origin)
            .map(|site| {
                let site = site
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_");
                format!("https://%{site}")
            })
            .unwrap_or_default();
        self.with(|db| {
            let mut stmt = db.prepare(
                "SELECT id, origin, username, secret FROM passwords
                 WHERE origin = ?1 OR (?2 <> '' AND origin LIKE ?2 ESCAPE '\\')
                 ORDER BY origin = ?1 DESC, COALESCE(used_at, created_at) DESC, id DESC",
            )?;
            let rows: Vec<PasswordSecret> = stmt
                .query_map([origin, pattern.as_str()], |row| {
                    Ok(PasswordSecret {
                        id: row.get(0)?,
                        origin: row.get(1)?,
                        username: row.get(2)?,
                        secret: row.get(3)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows
                .into_iter()
                .filter(|secret| same_site(origin, &secret.origin))
                .collect())
        })
    }

    pub fn password_secret(&self, id: i64) -> anyhow::Result<Option<(PasswordEntry, Vec<u8>)>> {
        self.with(|db| {
            db.query_row(
                "SELECT id, origin, username, created_at, used_at, secret FROM passwords WHERE id = ?1",
                [id],
                |row| {
                    Ok((
                        PasswordEntry {
                            id: row.get(0)?,
                            origin: row.get(1)?,
                            username: row.get(2)?,
                            created_at: row.get(3)?,
                            used_at: row.get(4)?,
                        },
                        row.get(5)?,
                    ))
                },
            )
            .optional()
        })
    }

    /// Сохранить или обновить пароль для пары «сайт + логин».
    pub fn save_password(
        &self,
        origin: &str,
        username: &str,
        secret: &[u8],
    ) -> anyhow::Result<i64> {
        let now = crate::history::now_secs();
        self.with(|db| {
            db.execute(
                "INSERT INTO passwords (origin, username, secret, created_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(origin, username) DO UPDATE SET secret = excluded.secret",
                rusqlite::params![origin, username, secret, now],
            )?;
            db.query_row(
                "SELECT id FROM passwords WHERE origin = ?1 AND username = ?2",
                rusqlite::params![origin, username],
                |row| row.get(0),
            )
        })
    }

    /// Поменять логин и, если передан, пароль. Совпадение с другой учёткой того
    /// же сайта — ошибка: тихо склеивать две записи нельзя.
    pub fn update_password(
        &self,
        id: i64,
        username: &str,
        secret: Option<&[u8]>,
    ) -> anyhow::Result<()> {
        self.with(|db| match secret {
            Some(secret) => db.execute(
                "UPDATE passwords SET username = ?2, secret = ?3 WHERE id = ?1",
                rusqlite::params![id, username, secret],
            ),
            None => db.execute(
                "UPDATE passwords SET username = ?2 WHERE id = ?1",
                rusqlite::params![id, username],
            ),
        })
        .map_err(|err| {
            if err.to_string().contains("UNIQUE") {
                anyhow::anyhow!("для этого сайта такой логин уже сохранён")
            } else {
                err
            }
        })?;
        Ok(())
    }

    pub fn delete_password(&self, id: i64) -> anyhow::Result<()> {
        self.with(|db| db.execute("DELETE FROM passwords WHERE id = ?1", [id]))?;
        Ok(())
    }

    pub fn clear_passwords(&self) -> anyhow::Result<()> {
        self.with(|db| db.execute("DELETE FROM passwords", []))?;
        Ok(())
    }

    /// Учётку только что использовали — поднимаем её наверх списка сайта.
    pub fn touch_password(&self, id: i64) -> anyhow::Result<()> {
        let now = crate::history::now_secs();
        self.with(|db| {
            db.execute(
                "UPDATE passwords SET used_at = ?2 WHERE id = ?1",
                rusqlite::params![id, now],
            )
        })?;
        Ok(())
    }

    /// «Никогда для этого сайта».
    pub fn never_save_password(&self, origin: &str) -> anyhow::Result<()> {
        let now = crate::history::now_secs();
        self.with(|db| {
            db.execute(
                "INSERT OR IGNORE INTO password_never (origin, added_at) VALUES (?1, ?2)",
                rusqlite::params![origin, now],
            )
        })?;
        Ok(())
    }

    pub fn is_password_never(&self, origin: &str) -> anyhow::Result<bool> {
        self.with(|db| {
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM password_never WHERE origin = ?1)",
                [origin],
                |row| row.get(0),
            )
        })
    }

    pub fn password_never_sites(&self) -> anyhow::Result<Vec<NeverSite>> {
        self.with(|db| {
            let mut stmt =
                db.prepare("SELECT origin, added_at FROM password_never ORDER BY origin")?;
            let rows: Vec<NeverSite> = stmt
                .query_map([], |row| {
                    Ok(NeverSite {
                        origin: row.get(0)?,
                        added_at: row.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
    }

    pub fn forget_password_never(&self, origin: &str) -> anyhow::Result<()> {
        self.with(|db| db.execute("DELETE FROM password_never WHERE origin = ?1", [origin]))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;

    #[test]
    fn password_is_upserted_per_login() {
        let store = Store::memory().unwrap();
        let first = store
            .save_password("https://habr.com", "me", b"one")
            .unwrap();
        let again = store
            .save_password("https://habr.com", "me", b"two")
            .unwrap();
        assert_eq!(first, again);

        let other = store
            .save_password("https://habr.com", "work", b"three")
            .unwrap();
        store.touch_password(other).unwrap();

        let secrets = store.password_secrets_for("https://habr.com").unwrap();
        assert_eq!(secrets.len(), 2);
        assert_eq!(secrets[0].username, "work");
        assert_eq!(
            store.password_secret(first).unwrap().unwrap().1,
            b"two".to_vec()
        );

        assert!(store.update_password(other, "me", None).is_err());
    }

    #[test]
    fn site_logins_follow_exact_ones() {
        let store = Store::memory().unwrap();
        store
            .save_password("https://google.com", "me@gmail.com", b"site")
            .unwrap();
        store
            .save_password("https://accounts.google.com", "work@gmail.com", b"exact")
            .unwrap();
        store
            .save_password("https://evilgoogle.com", "bad", b"no")
            .unwrap();
        store
            .save_password("http://google.com", "plain", b"no")
            .unwrap();

        let secrets = store
            .password_secrets_for("https://accounts.google.com")
            .unwrap();
        let names: Vec<&str> = secrets.iter().map(|s| s.username.as_str()).collect();
        assert_eq!(names, ["work@gmail.com", "me@gmail.com"]);
        assert_eq!(secrets[1].origin, "https://google.com");
        assert!(store
            .password_secrets_for("https://mail.proton.me")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn never_list_is_remembered() {
        let store = Store::memory().unwrap();
        store.never_save_password("https://bank.example").unwrap();
        store.never_save_password("https://bank.example").unwrap();
        assert!(store.is_password_never("https://bank.example").unwrap());
        assert_eq!(store.password_never_sites().unwrap().len(), 1);
        store.forget_password_never("https://bank.example").unwrap();
        assert!(!store.is_password_never("https://bank.example").unwrap());
    }
}
