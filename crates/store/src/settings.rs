//! Настройки — ключ → JSON.
//!
//! Значения по умолчанию здесь не живут: их знает интерфейс (он же рисует
//! страницу настроек) и Rust в тех немногих местах, где настройка влияет на
//! движок. В базе лежит только то, что пользователь поменял.

use rusqlite::OptionalExtension;
use serde_json::{Map, Value};

use crate::Store;

impl Store {
    pub fn settings(&self) -> anyhow::Result<Map<String, Value>> {
        self.with(|db| {
            let mut stmt = db.prepare("SELECT key, value FROM settings")?;
            let rows: Vec<(String, String)> = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows
                .into_iter()
                // Строку, которую не разобрать, пропускаем: пусть настройка
                // вернётся к умолчанию, чем браузер не откроется вовсе.
                .filter_map(|(key, value)| Some((key, serde_json::from_str(&value).ok()?)))
                .collect())
        })
    }

    pub fn setting(&self, key: &str) -> anyhow::Result<Option<Value>> {
        let raw: Option<String> = self.with(|db| {
            db.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
        })?;
        Ok(raw.and_then(|value| serde_json::from_str(&value).ok()))
    }

    pub fn setting_str(&self, key: &str) -> Option<String> {
        match self.setting(key).ok().flatten()? {
            Value::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn setting_bool(&self, key: &str, default: bool) -> bool {
        match self.setting(key).ok().flatten() {
            Some(Value::Bool(value)) => value,
            _ => default,
        }
    }

    pub fn set_setting(&self, key: &str, value: &Value) -> anyhow::Result<()> {
        let raw = serde_json::to_string(value)?;
        self.with(|db| {
            db.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                rusqlite::params![key, raw],
            )
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Store;

    #[test]
    fn setting_round_trips() {
        let store = Store::memory().unwrap();
        assert!(store.setting("search_engine").unwrap().is_none());

        store
            .set_setting("search_engine", &json!("yandex"))
            .unwrap();
        store
            .set_setting("search_engine", &json!("google"))
            .unwrap();
        store.set_setting("show_home", &json!(true)).unwrap();

        assert_eq!(
            store.setting_str("search_engine").as_deref(),
            Some("google")
        );
        assert!(store.setting_bool("show_home", false));
        assert_eq!(store.settings().unwrap().len(), 2);
    }
}
