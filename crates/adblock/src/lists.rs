use serde::{Deserialize, Serialize};

/// Откуда взялся список.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ListSource {
    /// Вшит в бандл — работает на первом запуске без сети.
    Bundled(String),
    /// Приходит обновлением фильтров в папку профиля `filters`: до первого
    /// обновления такого списка просто нет.
    Downloaded(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSpec {
    pub id: String,
    pub title: String,
    pub source: ListSource,
    pub enabled: bool,
    /// Доверенному списку разрешены скриптлеты, которые подменяют ответы сети
    /// и трогают cookies (`trusted-*`).
    #[serde(default)]
    pub trusted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscriptions {
    pub lists: Vec<ListSpec>,
}

impl Default for Subscriptions {
    /// Стартовый набор: EasyList + EasyPrivacy + русский RU AdList — вшиты;
    /// расширенные фильтры с косметикой и скриптлетами (в том числе против
    /// рекламы в видео) приходят обновлением фильтров.
    /// Русский список обязателен — без него Яндекс/VK/Дзен показывают всё.
    fn default() -> Self {
        let bundled = |id: &str, title: &str, file: &str| ListSpec {
            id: id.into(),
            title: title.into(),
            source: ListSource::Bundled(file.into()),
            enabled: true,
            trusted: false,
        };
        let downloaded = |id: &str, title: &str, file: &str| ListSpec {
            id: id.into(),
            title: title.into(),
            source: ListSource::Downloaded(file.into()),
            enabled: true,
            trusted: true,
        };
        Self {
            lists: vec![
                bundled("easylist", "EasyList", "easylist.txt"),
                bundled("easyprivacy", "EasyPrivacy", "easyprivacy.txt"),
                bundled("ruadlist", "RU AdList", "ruadlist.txt"),
                downloaded("extended", "Расширенные фильтры", "ubo-filters.txt"),
                downloaded("quick-fixes", "Быстрые исправления", "ubo-quick-fixes.txt"),
                downloaded("privacy", "Защита от слежки", "ubo-privacy.txt"),
                downloaded("unbreak", "Исправления поломок сайтов", "ubo-unbreak.txt"),
            ],
        }
    }
}
