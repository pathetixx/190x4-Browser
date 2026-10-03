use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use adblock::cosmetic_filter_cache::ProceduralOrActionFilter;
use adblock::lists::{FilterSet, ParseOptions};
use adblock::request::Request;
use adblock::resources::{PermissionMask, Resource};
use adblock::Engine;
use arc_swap::ArcSwap;

use crate::cosmetic::{document_host, Cosmetics};
use crate::stats::Stats;

/// Ключ сайта для исключений: хост без `www.`. Исключение действует и на
/// поддомены: выключенная блокировка на `youtube.com` выключает её и на
/// `m.youtube.com`.
pub fn site_key(url: &str) -> Option<String> {
    let host = document_host(url)?;
    Some(host.strip_prefix("www.").unwrap_or(&host).to_string())
}

/// Хост адреса без аллокаций — для горячего пути. Регистр не трогаем: адрес
/// документа приходит от движка уже нормализованным.
fn host_slice(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    if host.starts_with('[') {
        return host.split(']').next().map(|ipv6| &host[..ipv6.len() + 1]);
    }
    host.split(':').next()
}

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

/// Собранный фильтр: движок запросов и косметики и движок окон, которые
/// открывают страницы.
///
/// Правила `$popup` («реклама, открывающаяся новой вкладкой») adblock-rust не
/// понимает и выбрасывает целиком — а в EasyList и RU AdList их тысячи. Поэтому
/// они собираются отдельно, переписанные в правила документа
/// ([`popup_rule`]), и спрашиваются только об адресе нового окна.
#[derive(Default)]
pub struct Engines {
    main: Engine,
    popups: Engine,
}

/// Точка входа горячего пути.
pub struct Guard {
    engine: ArcSwap<Engines>,
    stats: Stats,
    enabled: ArcSwap<bool>,
    /// Сайты, на которых пользователь выключил блокировку (ключи [`site_key`]).
    exempt: ArcSwap<HashSet<String>>,
    /// Номер движка: растёт с каждой заменой, по нему устаревает кэш ниже.
    generation: AtomicU64,
    /// Исключения общих правил и `$generichide` по адресу документа: страница
    /// спрашивает общие правила много раз, а собирать косметику сайта заново
    /// ради одних исключений дорого.
    generic_cache: parking_lot::Mutex<HashMap<String, (u64, Arc<GenericContext>)>>,
}

/// Что нужно общим правилам по классам и id на странице.
struct GenericContext {
    exceptions: HashSet<String>,
    generichide: bool,
}

impl Guard {
    /// Пустой фильтр: всё разрешено. Браузер стартует с ним и не ждёт списки.
    pub fn empty() -> Self {
        Self {
            engine: ArcSwap::from_pointee(Engines::default()),
            stats: Stats::default(),
            enabled: ArcSwap::from_pointee(true),
            exempt: ArcSwap::from_pointee(HashSet::new()),
            generation: AtomicU64::new(0),
            generic_cache: parking_lot::Mutex::new(HashMap::new()),
        }
    }

    /// Заменить список сайтов без блокировки.
    pub fn set_exempt_sites(&self, sites: impl IntoIterator<Item = String>) {
        self.exempt.store(Arc::new(sites.into_iter().collect()));
    }

    /// Выключена ли блокировка для документа по этому адресу. Сверяются хост и
    /// все его родительские домены; пустой список — одна загрузка указателя.
    pub fn is_exempt(&self, document_url: &str) -> bool {
        let exempt = self.exempt.load();
        if exempt.is_empty() {
            return false;
        }
        let Some(mut host) = host_slice(document_url) else {
            return false;
        };
        loop {
            if exempt.contains(host) {
                return true;
            }
            match host.find('.') {
                Some(dot) => host = &host[dot + 1..],
                None => return false,
            }
        }
    }

    /// Собрать движок из сырых текстов списков. Дорого (сотни мс на easylist);
    /// звать только с фонового потока.
    ///
    /// Берёт `Vec<String>` по значению: `FilterSet::add_filter_list` требует
    /// владения текстом, и лишний `clone` здесь — это лишние мегабайты.
    pub fn build(lists: Vec<FilterList>, resources: Vec<Resource>) -> Engines {
        let mut set = FilterSet::new(false);
        let mut popup_rules = Vec::new();
        for list in lists {
            popup_rules.extend(list.text.lines().filter_map(popup_rule));
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
        let mut main = Engine::new_with_filter_set(set);
        main.use_resources(resources);
        let mut popups = FilterSet::new(false);
        popups.add_filter_list(popup_rules.join("\n"), ParseOptions::default());
        Engines {
            main,
            popups: Engine::new_with_filter_set(popups),
        }
    }

    /// Собранный движок в байтах. Следующий запуск поднимает его через
    /// [`Guard::restore`], не разбирая списки заново: разбор сотен тысяч
    /// правил стоит сотен миллисекунд процессора на каждом старте.
    ///
    /// Формат: длина снимка основного движка (u32, little-endian), он сам, за
    /// ним снимок движка окон.
    pub fn snapshot(engines: &Engines) -> Vec<u8> {
        let main = engines.main.serialize();
        let popups = engines.popups.serialize();
        let mut bytes = Vec::with_capacity(4 + main.len() + popups.len());
        bytes.extend_from_slice(&(main.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&main);
        bytes.extend_from_slice(&popups);
        bytes
    }

    /// Движок из снимка [`Guard::snapshot`]. Ресурсы скриптлетов в снимок не
    /// входят — их кладут заново. `None` — снимок испорчен или записан другой
    /// версией движка: тогда списки собираются как обычно.
    pub fn restore(bytes: &[u8], resources: Vec<Resource>) -> Option<Engines> {
        let (length, rest) = bytes.split_first_chunk::<4>()?;
        let length = usize::try_from(u32::from_le_bytes(*length)).ok()?;
        if length > rest.len() {
            return None;
        }
        let (main_bytes, popup_bytes) = rest.split_at(length);
        let mut main = Engine::default();
        main.deserialize(main_bytes).ok()?;
        main.use_resources(resources);
        let mut popups = Engine::default();
        popups.deserialize(popup_bytes).ok()?;
        Some(Engines { main, popups })
    }

    /// Ресурсы скриптлетов из `resources.json`.
    pub fn parse_resources(json: &str) -> anyhow::Result<Vec<Resource>> {
        Ok(serde_json::from_str(json)?)
    }

    /// Косметика документа по адресу: что скрыть, какие процедурные правила
    /// выполнить и какие скриптлеты запустить. Пусто, если фильтр выключен.
    /// Звать на навигацию, не на каждый запрос.
    ///
    /// Процедурное правило, которое выражается чистым CSS (`:style()` на
    /// обычном селекторе), уходит в стиль: так оно действует и на элементы,
    /// появившиеся позже, без работы скрипта.
    pub fn cosmetics(&self, url: &str) -> Cosmetics {
        if !self.filters(url) {
            return Cosmetics::default();
        }
        let resources = self.engine.load().main.url_cosmetic_resources(url);
        let mut hide: Vec<String> = resources.hide_selectors.into_iter().collect();
        let mut styles = Vec::new();
        let mut procedural = Vec::new();
        for raw in resources.procedural_actions {
            let Ok(filter) = serde_json::from_str::<ProceduralOrActionFilter>(&raw) else {
                continue;
            };
            match (filter.as_css(), &filter.action) {
                (Some((selector, _)), None) => hide.push(selector),
                (Some(css), Some(_)) => styles.push(css),
                (None, _) => procedural.push(raw),
            }
        }
        hide.sort_unstable();
        styles.sort_unstable();
        procedural.sort_unstable();
        Cosmetics {
            hide,
            styles,
            procedural,
            generic: !resources.generichide,
            script: resources.injected_script,
        }
    }

    /// Общие правила скрытия для классов и id, которые нашлись на странице:
    /// селекторы, которые ей нужно спрятать. Пусто, если фильтр на странице
    /// выключен или у неё `$generichide`. Звать не с главного потока.
    pub fn generic_hide(
        &self,
        document_url: &str,
        classes: &[String],
        ids: &[String],
    ) -> Vec<String> {
        if !self.filters(document_url) {
            return Vec::new();
        }
        // Номер — до движка: замена между ними оставит в кэше устаревший номер,
        // и исключения соберутся заново, а не наоборот.
        let generation = self.generation.load(Ordering::Acquire);
        let engine = self.engine.load();
        let cached = self
            .generic_cache
            .lock()
            .get(document_url)
            .filter(|(at, _)| *at == generation)
            .map(|(_, context)| context.clone());
        let context = match cached {
            Some(context) => context,
            None => {
                let resources = engine.main.url_cosmetic_resources(document_url);
                let context = Arc::new(GenericContext {
                    exceptions: resources.exceptions,
                    generichide: resources.generichide,
                });
                let mut cache = self.generic_cache.lock();
                if cache.len() >= 64 {
                    cache.clear();
                }
                cache.insert(document_url.to_string(), (generation, context.clone()));
                context
            }
        };
        if context.generichide {
            return Vec::new();
        }
        engine
            .main
            .hidden_class_id_selectors(classes, ids, &context.exceptions)
    }

    /// Подменить движок целиком. Читатели, которые уже внутри `check`,
    /// дочитывают старый Arc и не блокируются — в этом весь смысл ArcSwap.
    pub fn swap(&self, engines: Engines) {
        self.engine.store(Arc::new(engines));
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(Arc::new(on));
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Фильтрует ли браузер запросы документа по этому адресу: блокировка
    /// включена и сайт не в исключениях. Нет — запрос можно не разбирать вовсе.
    pub fn filters(&self, document_url: &str) -> bool {
        **self.enabled.load() && !self.is_exempt(document_url)
    }

    /// Горячий путь. Никаких своих локов, никакого I/O, никакого логирования.
    ///
    /// `url` — запрашиваемый ресурс, `source_url` — документ вкладки (нужен
    /// для `$third-party` и исключений по домену), `method` — HTTP-метод
    /// (правила с `$method=` без него не работают).
    pub fn check(&self, url: &str, source_url: &str, kind: ResourceKind, method: &str) -> Decision {
        // Исключение сайта считается по документу вкладки: на выключенном сайте
        // проходят и его собственные запросы, и запросы встроенных в него фреймов.
        if !self.filters(source_url) {
            return Decision::Allow;
        }

        let started = std::time::Instant::now();
        let engine = self.engine.load();

        let Ok(request) = Request::new(url, source_url, kind.as_str(), method) else {
            // Невалидный URL (blob:, data:, кривая схема) — не наше дело.
            return Decision::Allow;
        };

        let result = engine.main.check_network_request(&request);
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

impl Guard {
    /// Окно, которое страница `opener_url` открывает по адресу `url`, —
    /// реклама: его ловит правило `$popup` или блокировка самого документа
    /// (`$document`, `$all`). Звать на открытие окна и на переходы в нём, пока
    /// оно ещё «окно страницы» — так ловится и реклама, уходящая на рекламный
    /// адрес через пустую страницу или переадресацию.
    pub fn blocks_popup(&self, url: &str, opener_url: &str) -> bool {
        if !self.filters(opener_url) {
            return false;
        }
        let Ok(request) = Request::new(url, opener_url, "document", "GET") else {
            return false;
        };
        let engines = self.engine.load();
        engines
            .popups
            .check_network_request(&request)
            .should_block()
            || engines.main.check_network_request(&request).should_block()
    }
}

/// Типы ресурсов в опциях правила: у правила окна они не нужны, оно станет
/// правилом документа.
const TYPE_OPTIONS: &[&str] = &[
    "popup",
    "popunder",
    "document",
    "doc",
    "all",
    "script",
    "image",
    "stylesheet",
    "css",
    "xmlhttprequest",
    "xhr",
    "subdocument",
    "frame",
    "media",
    "font",
    "object",
    "object-subrequest",
    "other",
    "ping",
    "beacon",
    "websocket",
    "inline-script",
    "inline-font",
];

/// Правило `$popup` из списка — как правило документа для движка окон:
/// `||ads.example^$popup,3p` → `||ads.example^$document,3p`. Остальные строки
/// (и `$popunder`, где рекламой становится сама страница) — `None`.
fn popup_rule(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('!') || line.starts_with('[') {
        return None;
    }
    let (pattern, options) = line.rsplit_once('$')?;
    if !options.split(',').any(|option| option == "popup") {
        return None;
    }
    let mut rule = format!("{pattern}$document");
    for option in options.split(',') {
        let name = option.trim_start_matches('~');
        let name = name.split('=').next().unwrap_or(name);
        if !TYPE_OPTIONS.contains(&name) {
            rule.push(',');
            rule.push_str(option);
        }
    }
    Some(rule)
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
    fn procedural_rules_reach_the_page() {
        let guard = guard_with(
            "example.com##div:has-text(Реклама)\n\
             example.com##.x:style(height: 0)\n\
             example.com##.y:remove()\n\
             example.com##.z:has(> .ad)",
        );
        let cosmetics = guard.cosmetics("https://example.com/");
        assert!(cosmetics
            .procedural
            .iter()
            .any(|rule| rule.contains(r#""type":"has-text""#) && rule.contains("Реклама")));
        assert!(cosmetics
            .procedural
            .iter()
            .any(|rule| rule.contains(r#""type":"remove""#)));
        assert_eq!(cosmetics.styles.len(), 1);
        assert_eq!(cosmetics.styles[0].0, ".x");
        // `:has()` браузер понимает сам — это обычный стиль скрытия.
        assert!(cosmetics
            .hide
            .iter()
            .any(|selector| selector.contains(":has(")));
        assert!(cosmetics.generic);
    }

    #[test]
    fn generic_rules_follow_page_classes() {
        let guard = guard_with("##.ad-banner\n###sidebar-ads\nnews.example#@#.ad-banner");
        assert_eq!(
            guard.generic_hide(
                "https://other.example/",
                &["ad-banner".into(), "content".into()],
                &["sidebar-ads".into()]
            ),
            vec![".ad-banner".to_string(), "#sidebar-ads".to_string()]
        );
        assert!(guard
            .generic_hide("https://news.example/", &["ad-banner".into()], &[])
            .is_empty());
        guard.set_exempt_sites(["other.example".to_string()]);
        assert!(guard
            .generic_hide("https://other.example/", &["ad-banner".into()], &[])
            .is_empty());
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
        assert!(!guard.filters("https://news.example/"));
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
    fn site_key_drops_www_and_port() {
        assert_eq!(
            site_key("https://www.YouTube.com:443/watch?v=1").as_deref(),
            Some("youtube.com")
        );
        assert_eq!(
            site_key("http://m.example.com/").as_deref(),
            Some("m.example.com")
        );
        assert_eq!(site_key("about:blank"), None);
    }

    #[test]
    fn exempt_site_and_subdomains_pass() {
        let guard = guard_with("||ads.example.com^\nnews.example##.promo");
        guard.set_exempt_sites(["news.example".to_string()]);
        for page in [
            "https://news.example/",
            "https://m.news.example/a",
            "http://[::1]:8080/",
        ] {
            let expected = if page.contains("news.example") {
                Decision::Allow
            } else {
                Decision::Block
            };
            assert_eq!(
                guard.check(
                    "https://ads.example.com/b.js",
                    page,
                    ResourceKind::Script,
                    "GET"
                ),
                expected,
                "{page}"
            );
        }
        assert!(guard.cosmetics("https://news.example/").is_empty());
        assert!(!guard.is_exempt("https://othernews.example/"));
        assert!(!guard.filters("https://m.news.example/"));
        assert!(guard.filters("https://othernews.example/"));

        guard.set_exempt_sites(Vec::new());
        assert_eq!(
            guard.check(
                "https://ads.example.com/b.js",
                "https://news.example/",
                ResourceKind::Script,
                "GET"
            ),
            Decision::Block
        );
    }

    #[test]
    fn snapshot_restores_rules_and_scriptlets() {
        let json = serde_json::json!([
            {"name": "mark.js", "aliases": [], "kind": {"mime": "application/javascript"},
             "content": b64("function mark(value) { window.__mark = value; }"), "dependencies": [], "permission": 0},
        ])
        .to_string();
        let resources = Guard::parse_resources(&json).unwrap();
        let engine = Guard::build(
            vec![list(
                "||ads.example.com^\nexample.com##.promo\nexample.com##+js(mark, 1)\n||pop.example^$popup",
                false,
            )],
            resources,
        );
        let bytes = Guard::snapshot(&engine);

        let resources = Guard::parse_resources(&json).unwrap();
        assert!(Guard::restore(&bytes[..3], Vec::new()).is_none());
        let restored = Guard::restore(&bytes, resources).unwrap();
        let guard = Guard::empty();
        guard.swap(restored);
        assert_eq!(
            guard.check(
                "https://ads.example.com/banner.js",
                "https://news.example/",
                ResourceKind::Script,
                "GET"
            ),
            Decision::Block
        );
        let cosmetics = guard.cosmetics("https://example.com/");
        assert_eq!(cosmetics.hide, vec![".promo".to_string()]);
        assert!(cosmetics.script.contains("function mark"));
        assert!(guard.blocks_popup("https://pop.example/", "https://news.example/"));

        assert!(Guard::restore(b"not an engine", Vec::new()).is_none());
    }

    #[test]
    fn popup_rules_become_document_rules() {
        assert_eq!(
            popup_rule("||ads.example^$popup,third-party").as_deref(),
            Some("||ads.example^$document,third-party")
        );
        assert_eq!(
            popup_rule("@@||ok.example^$popup,domain=site.example").as_deref(),
            Some("@@||ok.example^$document,domain=site.example")
        );
        assert_eq!(
            popup_rule("||x.example^$script,popup").as_deref(),
            Some("||x.example^$document")
        );
        assert_eq!(popup_rule("||x.example^$popunder"), None);
        assert_eq!(popup_rule("||x.example^$script"), None);
        assert_eq!(popup_rule("example.com##.popup"), None);
        assert_eq!(popup_rule("! $popup"), None);
    }

    #[test]
    fn ad_popups_are_blocked() {
        let guard = guard_with(
            "||ads.example^$popup\n\
             $popup,third-party,domain=player.example\n\
             @@||ok.example^$popup,domain=player.example\n\
             ||malware.example^$document\n\
             ||tracker.example^",
        );
        let page = "https://news.example/";
        assert!(guard.blocks_popup("https://ads.example/click?id=1", page));
        assert!(guard.blocks_popup("https://malware.example/", page));
        // Обычное правило запроса не про окна: ссылка туда открывается.
        assert!(!guard.blocks_popup("https://tracker.example/", page));
        assert!(!guard.blocks_popup("https://wiki.example/", page));
        // Плеер, который открывает чужие окна, — только его окна.
        let player = "https://player.example/watch/1";
        assert!(guard.blocks_popup("https://any.example/", player));
        assert!(!guard.blocks_popup("https://ok.example/", player));
        assert!(!guard.blocks_popup("https://player.example/next", player));
        // Сайт в исключениях — как с выключенной блокировкой.
        guard.set_exempt_sites(["news.example".to_string()]);
        assert!(!guard.blocks_popup("https://ads.example/click?id=1", page));
        // Запросы правило окна не трогает.
        assert_eq!(
            guard.check(
                "https://ads.example/a.js",
                "https://other.example/",
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
