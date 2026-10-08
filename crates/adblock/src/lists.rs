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
    /// Защита своего содержимого сайта, как `first_party_protections` в каталоге
    /// Brave: правила такого списка не закрывают запросы к самому сайту, а их
    /// скрытие проверяет исполнитель. У Brave она есть у основных списков
    /// (uBlock Origin, EasyList, EasyPrivacy, свои списки Brave, URLhaus) и нет
    /// у региональных, First Party, cookie и промо приложений.
    #[serde(default)]
    pub protections: bool,
    /// Версия набора списков, в которой список появился. У того, кто уже
    /// выбирал списки в более старом наборе, новый список включается сам —
    /// иначе он остался бы выключенным молча (`enabled_lists` в браузере).
    #[serde(default)]
    pub since: u32,
}

/// Версия набора списков по умолчанию: наибольшая `since`.
pub const LISTS_VERSION: u32 = 4;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscriptions {
    pub lists: Vec<ListSpec>,
}

impl Default for Subscriptions {
    /// Набор — как каталог Brave для русского языка: EasyList, EasyPrivacy и
    /// RU AdList вшиты; списки uBlock Origin и Brave, баннеры о куках, промо
    /// приложений и вредоносные сайты приходят обновлением фильтров.
    /// Русский список обязателен — без него Яндекс/VK/Дзен показывают всё.
    fn default() -> Self {
        let bundled = |id: &str, title: &str, file: &str| ListSpec {
            id: id.into(),
            title: title.into(),
            source: ListSource::Bundled(file.into()),
            enabled: true,
            trusted: false,
            protections: true,
            since: 1,
        };
        let downloaded = |id: &str, title: &str, file: &str| ListSpec {
            id: id.into(),
            title: title.into(),
            source: ListSource::Downloaded(file.into()),
            enabled: true,
            trusted: true,
            protections: true,
            since: 2,
        };
        Self {
            lists: vec![
                bundled("easylist", "EasyList", "easylist.txt"),
                bundled("easyprivacy", "EasyPrivacy", "easyprivacy.txt"),
                // Региональный список: у Brave — без защиты своего содержимого.
                ListSpec {
                    protections: false,
                    ..bundled("ruadlist", "RU AdList", "ruadlist.txt")
                },
                downloaded("extended", "Расширенные фильтры", "ubo-filters.txt"),
                downloaded("quick-fixes", "Быстрые исправления", "ubo-quick-fixes.txt"),
                downloaded("privacy", "Защита от слежки", "ubo-privacy.txt"),
                downloaded("unbreak", "Исправления поломок сайтов", "ubo-unbreak.txt"),
                // Списки Brave и те, что он включает по умолчанию. Недоверенные:
                // так их подключает и Brave.
                ListSpec {
                    trusted: false,
                    since: 4,
                    ..downloaded("brave", "Списки Brave", "brave.txt")
                },
                ListSpec {
                    trusted: false,
                    protections: false,
                    since: 4,
                    ..downloaded(
                        "brave-firstparty",
                        "Brave First Party",
                        "brave-firstparty.txt",
                    )
                },
                ListSpec {
                    trusted: false,
                    protections: false,
                    since: 4,
                    ..downloaded("cookies", "Баннеры о cookie", "cookies.txt")
                },
                ListSpec {
                    trusted: false,
                    protections: false,
                    since: 4,
                    ..downloaded("mobile-promo", "Промо приложений", "mobile-promo.txt")
                },
                ListSpec {
                    trusted: false,
                    since: 4,
                    ..downloaded("urlhaus", "Вредоносные сайты", "urlhaus.txt")
                },
                // AdGuard Russian (вариант в синтаксисе uBlock Origin) — по
                // выбору: у Brave его нет, а его правила под Яндекс и Дзен
                // бывают из тех, по которым сайт замечает блокировщик.
                ListSpec {
                    enabled: false,
                    trusted: false,
                    protections: false,
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
