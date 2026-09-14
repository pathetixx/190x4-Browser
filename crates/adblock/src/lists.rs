use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Откуда взялся список.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ListSource {
    /// Вшит в бандл — работает на первом запуске без сети.
    Bundled(String),
    /// Скачивается и кэшируется в `%LOCALAPPDATA%\190x4 Browser\lists`.
    Remote { url: String, cache: PathBuf },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSpec {
    pub id: String,
    pub title: String,
    pub source: ListSource,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscriptions {
    pub lists: Vec<ListSpec>,
}

impl Default for Subscriptions {
    /// Стартовый набор: EasyList + EasyPrivacy + русский RU AdList.
    /// Русский список обязателен — без него Яндекс/VK/Дзен показывают всё.
    fn default() -> Self {
        Self {
            lists: vec![
                ListSpec {
                    id: "easylist".into(),
                    title: "EasyList".into(),
                    source: ListSource::Bundled("easylist.txt".into()),
                    enabled: true,
                },
                ListSpec {
                    id: "easyprivacy".into(),
                    title: "EasyPrivacy".into(),
                    source: ListSource::Bundled("easyprivacy.txt".into()),
                    enabled: true,
                },
                ListSpec {
                    id: "ruadlist".into(),
                    title: "RU AdList".into(),
                    source: ListSource::Bundled("ruadlist.txt".into()),
                    enabled: true,
                },
            ],
        }
    }
}
