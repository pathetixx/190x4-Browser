use std::sync::Arc;

use adblock::lists::{FilterSet, ParseOptions};
use adblock::request::Request;
use adblock::resources::{PermissionMask, Resource};
use adblock::Engine;
use arc_swap::ArcSwap;

use crate::cosmetic::Cosmetics;
use crate::stats::Stats;

/// Текст списка и доверие к нему.
pub struct FilterList {
    pub text: String,
    pub trusted: bool,
}

/// Права доверенного списка. Скриптлеты `trusted-*` помечены в ресурсах тем же
/// битом, и из недоверенного списка движок их не встраивает.
const TRUSTED: PermissionMask = PermissionMask::from_bits(0b0000_0001);

/// Тип ресурса в терминах фильтр-списков (`$script`, `$image`, `$xhr`, …).
///
/// Маппится 1:1 из `COREWEBVIEW2_WEB_RESOURCE_CONTEXT`; перевод живёт в
/// крейте `browser190x4-webview`, чтобы этот крейт собирался и тестировался на Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Document,
    Subdocument,
    Stylesheet,
    Image,
    Media,
    Font,
    Script,
    Xhr,
    Fetch,
    Websocket,
    Ping,
    Other,
}

impl ResourceKind {
    /// Строка, которую понимает adblock-rust.
    pub fn as_str(self) -> &'static str {
        match self {
            ResourceKind::Document => "document",
            ResourceKind::Subdocument => "sub_frame",
            ResourceKind::Stylesheet => "stylesheet",
            ResourceKind::Image => "image",
            ResourceKind::Media => "media",
            ResourceKind::Font => "font",
            ResourceKind::Script => "script",
            ResourceKind::Xhr => "xhr",
            ResourceKind::Fetch => "fetch",
            ResourceKind::Websocket => "websocket",
            ResourceKind::Ping => "ping",
            ResourceKind::Other => "other",
        }
    }
}

/// Что делать с запросом.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Пропустить как есть.
    Allow,
    /// Ответить синтетическим 403 с пустым телом (`Response` создаёт вызывающий).
    Block,
    /// Пропустить, но с урезанным URL — сработало `$removeparam`/redirect-правило.
    Rewrite(String),
}

/// Точка входа горячего пути.
pub struct Guard {
    engine: ArcSwap<Engine>,
    stats: Stats,
    enabled: ArcSwap<bool>,
}

impl Guard {
    /// Пустой фильтр: всё разрешено. Браузер стартует с ним и не ждёт списки.
    pub fn empty() -> Self {
        Self {
            engine: ArcSwap::from_pointee(Engine::default()),
            stats: Stats::default(),
            enabled: ArcSwap::from_pointee(true),
        }
    }

    /// Собрать движок из сырых текстов списков. Дорого (сотни мс на easylist);
    /// звать только с фонового потока.
    ///
    /// Берёт `Vec<String>` по значению: `FilterSet::add_filter_list` требует
    /// владения текстом, и лишний `clone` здесь — это лишние мегабайты.
    pub fn build(lists: Vec<FilterList>, resources: Vec<Resource>) -> Engine {
        let mut set = FilterSet::new(false);
        for list in lists {
            let permissions = if list.trusted {
                TRUSTED
            } else {
                PermissionMask::default()
            };
            set.add_filter_list(
                list.text,
                ParseOptions {
                    permissions,
                    ..ParseOptions::default()
                },
            );
        }
        let mut engine = Engine::new_with_filter_set(set);
        engine.use_resources(resources);
        engine
    }

    /// Ресурсы скриптлетов из `resources.json`.
    pub fn parse_resources(json: &str) -> anyhow::Result<Vec<Resource>> {
        Ok(serde_json::from_str(json)?)
    }

    /// Косметика документа по адресу: что скрыть и какие скриптлеты запустить.
    /// Пусто, если фильтр выключен. Звать на навигацию, не на каждый запрос.
    pub fn cosmetics(&self, url: &str) -> Cosmetics {
        if !**self.enabled.load() {
            return Cosmetics::default();
        }
        let resources = self.engine.load().url_cosmetic_resources(url);
        let mut hide: Vec<String> = resources.hide_selectors.into_iter().collect();
        hide.sort_unstable();
        Cosmetics {
            hide,
            script: resources.injected_script,
        }
    }

    /// Подменить движок целиком. Читатели, которые уже внутри `check`,
    /// дочитывают старый Arc и не блокируются — в этом весь смысл ArcSwap.
    pub fn swap(&self, engine: Engine) {
        self.engine.store(Arc::new(engine));
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(Arc::new(on));
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Горячий путь. Никаких своих локов, никакого I/O, никакого логирования.
    ///
    /// `url` — запрашиваемый ресурс, `source_url` — документ вкладки (нужен
    /// для `$third-party` и исключений по домену), `method` — HTTP-метод
    /// (правила с `$method=` без него не работают).
    pub fn check(&self, url: &str, source_url: &str, kind: ResourceKind, method: &str) -> Decision {
        if !**self.enabled.load() {
            return Decision::Allow;
        }

        let started = std::time::Instant::now();
        let engine = self.engine.load();

        let Ok(request) = Request::new(url, source_url, kind.as_str(), method) else {
            // Невалидный URL (blob:, data:, кривая схема) — не наше дело.
            return Decision::Allow;
        };

        let result = engine.check_network_request(&request);
        let decision = if result.should_block() {
            Decision::Block
        } else if let Some(rewritten) = result.rewritten_url {
            Decision::Rewrite(rewritten)
        } else {
            Decision::Allow
        };

        self.stats.record(started.elapsed(), &decision);
        decision
    }
}

impl Default for Guard {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(text: &str, trusted: bool) -> FilterList {
        FilterList {
            text: text.to_string(),
            trusted,
        }
    }

    fn guard_with(rules: &str) -> Guard {
        let guard = Guard::empty();
        guard.swap(Guard::build(vec![list(rules, false)], Vec::new()));
        guard
    }

    fn b64(input: &str) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in input.as_bytes().chunks(3) {
            let bytes = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    fn scriptlet_guard(trusted: bool) -> Guard {
        let json = serde_json::json!([
            {"name": "mark.js", "aliases": [], "kind": {"mime": "application/javascript"},
             "content": b64("function mark(value) { window.__mark = value; }"), "dependencies": [], "permission": 0},
            {"name": "trusted-mark.js", "aliases": [], "kind": {"mime": "application/javascript"},
             "content": b64("function trustedMark(value) { window.__trusted = value; }"), "dependencies": [], "permission": 1},
        ])
        .to_string();
        let resources = Guard::parse_resources(&json).unwrap();
        let guard = Guard::empty();
        guard.swap(Guard::build(
            vec![list(
                "example.com##+js(mark, 1)\nexample.com##+js(trusted-mark, 2)",
                trusted,
            )],
            resources,
        ));
        guard
    }

    #[test]
    fn hides_elements_by_cosmetic_rule() {
        let guard = guard_with("example.com##.promo");
        assert_eq!(
            guard.cosmetics("https://example.com/page").hide,
            vec![".promo".to_string()]
        );
        assert!(guard.cosmetics("https://other.example/").hide.is_empty());
    }

    #[test]
    fn trusted_scriptlets_need_a_trusted_list() {
        let plain = scriptlet_guard(false)
            .cosmetics("https://example.com/")
            .script;
        assert!(plain.contains("function mark"));
        assert!(!plain.contains("function trustedMark"));

        let trusted = scriptlet_guard(true)
            .cosmetics("https://example.com/")
            .script;
        assert!(trusted.contains("function mark"));
        assert!(trusted.contains("function trustedMark"));
    }

    #[test]
    fn disabled_guard_has_no_cosmetics() {
        let guard = guard_with("example.com##.promo");
        guard.set_enabled(false);
        assert!(guard.cosmetics("https://example.com/").is_empty());
    }

    #[test]
    fn blocks_by_network_rule() {
        let guard = guard_with("||ads.example.com^");
        assert_eq!(
            guard.check(
                "https://ads.example.com/banner.js",
                "https://news.example/",
                ResourceKind::Script,
                "GET"
            ),
            Decision::Block
        );
    }

    #[test]
    fn passes_unrelated_request() {
        let guard = guard_with("||ads.example.com^");
        assert_eq!(
            guard.check(
                "https://cdn.example/app.js",
                "https://news.example/",
                ResourceKind::Script,
                "GET"
            ),
            Decision::Allow
        );
    }

    #[test]
    fn disabled_guard_is_transparent() {
        let guard = guard_with("||ads.example.com^");
        guard.set_enabled(false);
        assert_eq!(
            guard.check(
                "https://ads.example.com/banner.js",
                "https://news.example/",
                ResourceKind::Script,
                "GET"
            ),
            Decision::Allow
        );
    }

    #[test]
    fn broken_url_does_not_panic() {
        let guard = guard_with("||ads.example.com^");
        assert_eq!(
            guard.check(
                "not a url",
                "https://news.example/",
                ResourceKind::Other,
                "GET"
            ),
            Decision::Allow
        );
    }
}
