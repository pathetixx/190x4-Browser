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
    /// Версия набора списков, в которой список появился. У того, кто уже
    /// выбирал списки в более старом наборе, новый список включается сам —
    /// иначе он остался бы выключенным молча (`enabled_lists` в браузере).
    #[serde(default)]
    pub since: u32,
}

/// Версия набора списков по умолчанию: наибольшая `since`.
pub const LISTS_VERSION: u32 = 3;

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
            since: 1,
        };
        let downloaded = |id: &str, title: &str, file: &str| ListSpec {
            id: id.into(),
            title: title.into(),
            source: ListSource::Downloaded(file.into()),
            enabled: true,
            trusted: true,
            since: 2,
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
                // Команда AdGuard правит рунет каждый день — Яндекс, Дзен,
                // Mail.ru, ВК, — в том числе их защиту от блокировщиков, а
                // RU AdList за ней не успевает. Вариант списка в синтаксисе
                // uBlock Origin, его понимает adblock-rust; скриптлеты из
                // недоверенного списка — только обычные, как в uBlock Origin.
                ListSpec {
                    trusted: false,
                    since: 3,
                    ..downloaded("adguard-russian", "AdGuard Russian", "adguard-russian.txt")
                },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_version_follows_the_newest_list() {
        let lists = Subscriptions::default().lists;
        assert_eq!(
            lists.iter().map(|spec| spec.since).max(),
            Some(LISTS_VERSION)
        );
        let mut ids: Vec<&str> = lists.iter().map(|spec| spec.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), lists.len(), "id списков не повторяются");
    }
}
