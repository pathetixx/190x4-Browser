use std::sync::Arc;

use adblock::lists::{FilterSet, ParseOptions};
use adblock::request::Request;
use adblock::Engine;
use arc_swap::ArcSwap;

use crate::stats::Stats;

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
    pub fn build(lists: Vec<String>) -> Engine {
        let mut set = FilterSet::new(false);
        for raw in lists {
            set.add_filter_list(raw, ParseOptions::default());
        }
        Engine::new_with_filter_set(set)
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

    fn guard_with(rules: &str) -> Guard {
        let guard = Guard::empty();
        guard.swap(Guard::build(vec![rules.to_string()]));
        guard
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
