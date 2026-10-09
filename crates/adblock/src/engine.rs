use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// Регистрируемый домен хоста (eTLD+1): `news.mail.ru` → `mail.ru`,
/// `user.github.io` → `user.github.io`. Свой и чужой сайт Brave различает так же
/// (`SameDomainOrHost` с частными суффиксами).
pub fn registrable_domain(host: &str) -> &str {
    psl::domain_str(host).unwrap_or(host)
}

/// Запрос к тому же сайту, что и страница: одинаковый регистрируемый домен.
fn same_site(url: &str, page: &str) -> bool {
    match (host_slice(url), host_slice(page)) {
        (Some(url), Some(page)) => {
            registrable_domain(url).eq_ignore_ascii_case(registrable_domain(page))
        }
        _ => false,
    }
}

/// YouTube Brave блокирует всегда как в агрессивном режиме: реклама там идёт
/// с собственных адресов сайта.
fn always_aggressive(page: &str) -> bool {
    host_slice(page)
        .is_some_and(|host| registrable_domain(host).eq_ignore_ascii_case("youtube.com"))
}

/// Поисковики, на которых правила скрытия из списков в обычном режиме не
/// применяются, — как `kVettedSearchEngines` у Brave: их выдачу списки ломают
/// чаще, чем находят рекламу. Сравнивается имя домена без суффикса.
const VETTED_SEARCH_ENGINES: &[&str] = &[
    "duckduckgo",
    "qwant",
    "bing",
    "startpage",
    "google",
    "yandex",
    "ecosia",
    "brave",
];

fn is_vetted_search_engine(url: &str) -> bool {
    let Some(host) = host_slice(url) else {
        return false;
    };
    let domain = registrable_domain(host);
    let suffix = psl::suffix_str(domain).unwrap_or("");
    let name = domain
        .strip_suffix(suffix)
        .and_then(|name| name.strip_suffix('.'))
        .unwrap_or(domain);
    VETTED_SEARCH_ENGINES.contains(&name)
}

/// Текст списка и доверие к нему.
pub struct FilterList {
    pub text: String,
    pub trusted: bool,
    /// Защита своего содержимого сайта (`first_party_protections` в каталоге
    /// Brave): правила списка не закрывают запросы страницы к её же сайту, а
    /// правила скрытия проверяются, не своё ли содержимое сайта они прячут.
    /// Список без неё — региональные, First Party, cookie, свои правила —
    /// работает всегда и сразу.
    pub protections: bool,
    /// Список рекламы, а не слежки: его правила домена целиком (`||host^`)
    /// закрывают и окна, которые открывает страница, и переходы на этот домен
    /// — как «строгая блокировка» uBlock Origin. У EasyPrivacy и подобных —
    /// нет: ссылка на домен счётчика не реклама.
    pub ads: bool,
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
    /// Ответить заглушкой правила `$redirect=` (`noop.js`, пустой VAST, `google-ima.js`):
    /// скрипт страницы получает то, что ждал, и не уходит в ветку «блокировщик найден».
    Redirect { mime: String, body: Vec<u8> },
}

/// Собранный фильтр: два движка списков, как у Brave, и движок окон, которые
/// открывают страницы.
///
/// Основной (`main`, `default_engine` у Brave) — списки с защитой своего
/// содержимого сайта: в обычном режиме их правила не трогают запросы к самому
/// сайту, а их скрытие проверяет исполнитель (`cosmetic.js`). Дополнительный
/// (`additional`, `additional_filters_engine`) — остальные списки и свои
/// правила: закрывают и прячут всегда, его исключения действуют и на
/// совпадения основного.
///
/// Правила `$popup` («реклама, открывающаяся новой вкладкой») adblock-rust не
/// понимает и выбрасывает целиком — а в EasyList и RU AdList их тысячи. Поэтому
/// они собираются отдельно, переписанные в правила документа
/// ([`popup_rule`]), и спрашиваются только об адресе нового окна.
#[derive(Default)]
pub struct Engines {
    main: Engine,
    popups: Engine,
    /// Только домены рекламных сетей целиком ([`host_rule`]): на них вкладку
    /// не уводит даже скрипт из обработчика щелчка.
    ad_hosts: Engine,
    additional: Engine,
}

/// Точка входа горячего пути.
pub struct Guard {
    engine: ArcSwap<Engines>,
    stats: Stats,
    enabled: ArcSwap<bool>,
    /// Сайты, на которых пользователь выключил блокировку (ключи [`site_key`]).
    exempt: ArcSwap<HashSet<String>>,
    /// Номер правки фильтра: растёт с каждой заменой движка, включением и
    /// выключением и сменой исключений. По нему устаревают кэш ниже и скрипты
    /// косметики, встроенные во вкладки ([`Guard::revision`]).
    generation: AtomicU64,
    /// Исключения общих правил и `$generichide` по адресу документа: страница
    /// спрашивает общие правила много раз, а собирать косметику сайта заново
    /// ради одних исключений дорого.
    generic_cache: parking_lot::Mutex<HashMap<String, (u64, Arc<GenericContext>)>>,
    /// Общие сложные селекторы (`##div[id^="ad-"]`), которые движок кладёт в
    /// косметику любого сайта, — по номеру правки. По ним видно, есть ли у
    /// фрейма свои правила ([`Guard::frame_cosmetics`]).
    generic_selectors: parking_lot::Mutex<Option<(u64, Arc<HashSet<String>>)>>,
    /// Агрессивная блокировка, как у Brave: закрывать и запросы к самому
    /// сайту, а правила скрытия из списков применять сразу, без проверки,
    /// своё ли это содержимое сайта.
    aggressive: AtomicBool,
}

/// Что решили оба движка о запросе.
struct Verdict {
    block: bool,
    redirect: Option<String>,
}

/// Общие правила по классам и id страницы.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GenericHide {
    /// Правила списков: в обычном режиме их проверяет исполнитель.
    pub hide: Vec<String>,
    /// Свои правила: прячутся сразу.
    pub force: Vec<String>,
}

impl GenericHide {
    pub fn is_empty(&self) -> bool {
        self.hide.is_empty() && self.force.is_empty()
    }
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
            generic_selectors: parking_lot::Mutex::new(None),
            aggressive: AtomicBool::new(false),
        }
    }

    /// Включить или выключить агрессивную блокировку.
    pub fn set_aggressive(&self, on: bool) {
        self.aggressive.store(on, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Блокирует ли фильтр агрессивно документ по этому адресу.
    fn aggressive_on(&self, page: &str) -> bool {
        self.aggressive.load(Ordering::Relaxed) || always_aggressive(page)
    }

    /// Заменить список сайтов без блокировки.
    pub fn set_exempt_sites(&self, sites: impl IntoIterator<Item = String>) {
        self.exempt.store(Arc::new(sites.into_iter().collect()));
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Номер правки фильтра. Сменился — всё, что посчитано по прежнему фильтру
    /// (скрипты косметики во вкладках), надо считать заново.
    pub fn revision(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
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
        let mut protected = FilterSet::new(false);
        let mut forced = FilterSet::new(false);
        let mut popup_rules = Vec::new();
        let mut host_rules = Vec::new();
        for list in lists {
            popup_rules.extend(list.text.lines().filter_map(popup_rule));
            if list.ads {
                host_rules.extend(list.text.lines().filter_map(host_rule));
            }
            let permissions = if list.trusted {
                TRUSTED
            } else {
                PermissionMask::default()
            };
            let set = if list.protections {
                &mut protected
            } else {
                &mut forced
            };
            set.add_filter_list(
                list.text,
                ParseOptions {
                    permissions,
                    ..ParseOptions::default()
                },
            );
        }
        let mut main = Engine::new_with_filter_set(protected);
        main.use_resources(resources.clone());
        let mut additional = Engine::new_with_filter_set(forced);
        additional.use_resources(resources);
        let hosts = host_rules.join("\n");
        let mut popups = FilterSet::new(false);
        popups.add_filter_list(popup_rules.join("\n"), ParseOptions::default());
        popups.add_filter_list(hosts.clone(), ParseOptions::default());
        let mut ad_hosts = FilterSet::new(false);
        ad_hosts.add_filter_list(hosts, ParseOptions::default());
        Engines {
            main,
            popups: Engine::new_with_filter_set(popups),
            ad_hosts: Engine::new_with_filter_set(ad_hosts),
            additional,
        }
    }

    /// Собранный движок в байтах. Следующий запуск поднимает его через
    /// [`Guard::restore`], не разбирая списки заново: разбор сотен тысяч
    /// правил стоит сотен миллисекунд процессора на каждом старте.
    ///
    /// Формат: снимки основного движка, движка окон, доменов рекламных сетей и
    /// дополнительного подряд,
    /// перед каждым — его длина (u32, little-endian).
    pub fn snapshot(engines: &Engines) -> Vec<u8> {
        let parts = [
            engines.main.serialize(),
            engines.popups.serialize(),
            engines.ad_hosts.serialize(),
            engines.additional.serialize(),
        ];
        let mut bytes = Vec::with_capacity(parts.iter().map(|part| part.len() + 4).sum());
        for part in parts {
            bytes.extend_from_slice(&(part.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&part);
        }
        bytes
    }

    /// Движок из снимка [`Guard::snapshot`]. Ресурсы скриптлетов в снимок не
    /// входят — их кладут заново. `None` — снимок испорчен или записан другой
    /// версией движка: тогда списки собираются как обычно.
    pub fn restore(bytes: &[u8], resources: Vec<Resource>) -> Option<Engines> {
        let mut rest = bytes;
        let mut next = || -> Option<Engine> {
            let (length, tail) = rest.split_first_chunk::<4>()?;
            let length = usize::try_from(u32::from_le_bytes(*length)).ok()?;
            if length > tail.len() {
                return None;
            }
            let (part, tail) = tail.split_at(length);
            rest = tail;
            let mut engine = Engine::default();
            engine.deserialize(part).ok()?;
            Some(engine)
        };
        let mut main = next()?;
        let popups = next()?;
        let ad_hosts = next()?;
        let mut additional = next()?;
        if !rest.is_empty() {
            return None;
        }
        main.use_resources(resources.clone());
        additional.use_resources(resources);
        Some(Engines {
            main,
            popups,
            ad_hosts,
            additional,
        })
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
    /// появившиеся позже, без работы скрипта. Правила скрытия списков в
    /// обычном режиме исполнитель проверяет, своё ли содержимое сайта они
    /// прячут (как Brave); на поисковиках их нет вовсе. Свои правила — всегда.
    pub fn cosmetics(&self, url: &str) -> Cosmetics {
        if !self.filters(url) {
            return Cosmetics::default();
        }
        let aggressive = self.aggressive_on(url);
        let engines = self.engine.load();
        let resources = engines.main.url_cosmetic_resources(url);
        let own = engines.additional.url_cosmetic_resources(url);
        let mut hide: Vec<String> = if aggressive || !is_vetted_search_engine(url) {
            resources.hide_selectors.into_iter().collect()
        } else {
            Vec::new()
        };
        let mut force: Vec<String> = own.hide_selectors.into_iter().collect();
        // Скриптлеты — из обоих: JS Fixes RU AdList, например, в дополнительном.
        let mut script = resources.injected_script;
        if !own.injected_script.trim().is_empty() {
            script.push('\n');
            script.push_str(&own.injected_script);
        }
        let generic = !(resources.generichide || own.generichide);
        let mut styles = Vec::new();
        let mut procedural = Vec::new();
        for raw in resources
            .procedural_actions
            .into_iter()
            .chain(own.procedural_actions)
        {
            let Ok(filter) = serde_json::from_str::<ProceduralOrActionFilter>(&raw) else {
                continue;
            };
            match (filter.as_css(), &filter.action) {
                (Some((selector, _)), None) => force.push(selector),
                (Some(css), Some(_)) => styles.push(css),
                (None, _) => procedural.push(raw),
            }
        }
        hide.sort_unstable();
        force.sort_unstable();
        force.dedup();
        styles.sort_unstable();
        procedural.sort_unstable();
        Cosmetics {
            hide,
            force,
            styles,
            procedural,
            generic,
            script,
            site: host_slice(url)
                .map(|host| registrable_domain(host).to_ascii_lowercase())
                .unwrap_or_default(),
            aggressive,
            popups: false,
        }
    }

    /// Косметика фрейма другого сайта (плеер, встроенный на страницу) или
    /// `None`, если своего у его сайта ничего нет: ни правил скрытия сверх
    /// общих, ни процедурных правил, ни скриптлетов. Таким фреймам отдельный
    /// скрипт не нужен — общие правила там почти ничего не находят, а каждый
    /// скрипт вкладки разбирается в каждом её документе.
    pub fn frame_cosmetics(&self, url: &str, page_url: &str) -> Option<Cosmetics> {
        if !self.filters(page_url) {
            return None;
        }
        let cosmetics = self.cosmetics(url);
        let generic = self.generic_selectors();
        let specific = cosmetics
            .hide
            .iter()
            .any(|selector| !generic.contains(selector));
        let rules = specific
            || !cosmetics.force.is_empty()
            || !cosmetics.styles.is_empty()
            || !cosmetics.procedural.is_empty()
            || !cosmetics.script.trim().is_empty();
        // Фрейм чужого сайта скрипт получает всегда — ради окон (`popups`).
        let popups = !same_site(url, page_url);
        if rules {
            Some(Cosmetics {
                popups,
                ..cosmetics
            })
        } else if popups {
            Some(Cosmetics {
                popups,
                site: cosmetics.site,
                ..Cosmetics::default()
            })
        } else {
            None
        }
    }

    /// Общие сложные селекторы текущего движка: косметика адреса, на который не
    /// пишут ни правил сайта, ни исключений.
    fn generic_selectors(&self) -> Arc<HashSet<String>> {
        let generation = self.generation.load(Ordering::Acquire);
        if let Some((at, selectors)) = &*self.generic_selectors.lock() {
            if *at == generation {
                return selectors.clone();
            }
        }
        let selectors: Arc<HashSet<String>> = Arc::new(
            self.engine
                .load()
                .main
                .url_cosmetic_resources("https://generic.invalid/")
                .hide_selectors,
        );
        *self.generic_selectors.lock() = Some((generation, selectors.clone()));
        selectors
    }

    /// Общие правила скрытия для классов и id, которые нашлись на странице:
    /// селекторы списков (их проверяет исполнитель) и своих правил (прячутся
    /// сразу). Пусто, если фильтр на странице выключен или у неё
    /// `$generichide`. Звать не с главного потока.
    pub fn generic_hide(
        &self,
        document_url: &str,
        classes: &[String],
        ids: &[String],
    ) -> GenericHide {
        if !self.filters(document_url) {
            return GenericHide::default();
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
                let own = engine.additional.url_cosmetic_resources(document_url);
                let mut exceptions = resources.exceptions;
                exceptions.extend(own.exceptions);
                let context = Arc::new(GenericContext {
                    exceptions,
                    generichide: resources.generichide || own.generichide,
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
            return GenericHide::default();
        }
        let lists = self.aggressive_on(document_url) || !is_vetted_search_engine(document_url);
        GenericHide {
            hide: if lists {
                engine
                    .main
                    .hidden_class_id_selectors(classes, ids, &context.exceptions)
            } else {
                Vec::new()
            },
            force: engine
                .additional
                .hidden_class_id_selectors(classes, ids, &context.exceptions),
        }
    }

    /// Подменить движок целиком. Читатели, которые уже внутри `check`,
    /// дочитывают старый Arc и не блокируются — в этом весь смысл ArcSwap.
    pub fn swap(&self, engines: Engines) {
        self.engine.store(Arc::new(engines));
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(Arc::new(on));
        self.generation.fetch_add(1, Ordering::AcqRel);
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
        let Ok(request) = Request::new(url, source_url, kind.as_str(), method) else {
            // Невалидный URL (blob:, data:, кривая схема) — не наше дело.
            return Decision::Allow;
        };
        let verdict = self.verdict(&request, url, source_url, kind);
        let decision = if verdict.block {
            // Заглушка есть только у правил `$redirect=`/`$redirect-rule=`, и
            // только если её ресурс пришёл с каналом фильтров.
            match verdict.redirect.as_deref().and_then(decode_data_url) {
                Some((mime, body)) => Decision::Redirect { mime, body },
                None => Decision::Block,
            }
        } else {
            Decision::Allow
        };

        self.stats.record(started.elapsed(), &decision);
        decision
    }
}

impl Guard {
    /// Закроет ли фильтр такой запрос — тот же ответ, что у [`Guard::check`],
    /// но без счёта в статистике: вопрос заранее, сам запрос ещё придёт.
    pub fn blocks(&self, url: &str, source_url: &str, kind: ResourceKind) -> bool {
        if !self.filters(source_url) {
            return false;
        }
        let Ok(request) = Request::new(url, source_url, kind.as_str(), "GET") else {
            return false;
        };
        self.verdict(&request, url, source_url, kind).block
    }

    /// Решение по запросу — как `AdBlockEngineWrapper::ShouldStartRequest` у
    /// Brave. Основной движок в обычном режиме запросы к своему сайту не
    /// закрывает (его исключение при этом в силе); `$removeparam` его не
    /// применяется. Дополнительный спрашивается всегда и знает, что основной
    /// уже нашёл: его исключение снимает блокировку основного.
    ///
    /// Главный документ вкладки «своим» не бывает: у Brave инициатор перехода —
    /// прежняя страница или ссылка, открывшая окно, а не сам адрес. Переход на
    /// рекламный домен закрывается своей страницей, как «строгая блокировка»
    /// uBlock Origin.
    fn verdict(
        &self,
        request: &Request,
        url: &str,
        source_url: &str,
        kind: ResourceKind,
    ) -> Verdict {
        let engines = self.engine.load();
        let mut first = engines.main.check_network_request(request);
        if kind != ResourceKind::Document
            && !self.aggressive_on(source_url)
            && same_site(url, source_url)
        {
            first.filter = None;
            first.important = false;
            first.redirect = None;
        } else if first.important {
            return Verdict {
                block: true,
                redirect: first.redirect,
            };
        }
        let second = engines.additional.check_network_request_subset(
            request,
            first.filter.is_some(),
            first.exception.is_some(),
        );
        let important = second.important;
        let matched = first.filter.is_some() || second.filter.is_some();
        let exception = first.exception.is_some() || second.exception.is_some();
        Verdict {
            block: important || (matched && !exception),
            redirect: second.redirect.or(first.redirect),
        }
    }

    /// Окно, которое страница `opener_url` открывает по адресу `url`, —
    /// реклама: его ловит правило `$popup` или `$all` либо правило рекламного
    /// домена целиком (`||host^` из списков рекламы, [`host_rule`]). Основной движок здесь не
    /// спрашивается: у adblock-rust обычное правило (`||tracker.example^`)
    /// действует и на документ, и ссылка на любой домен из EasyPrivacy
    /// закрывалась бы молча. Звать на открытие окна и на переходы в нём, пока
    /// оно ещё «окно страницы» — так ловится и реклама, уходящая на рекламный
    /// адрес через пустую страницу или переадресацию.
    pub fn blocks_popup(&self, url: &str, opener_url: &str) -> bool {
        if !self.filters(opener_url) {
            return false;
        }
        let Ok(request) = Request::new(url, opener_url, "document", "GET") else {
            return false;
        };
        self.engine
            .load()
            .popups
            .check_network_request(&request)
            .should_block()
    }

    /// Адрес `url` — домен рекламной сети целиком (`||popads.net^`,
    /// `||adsterra.com^$third-party` от страницы `from_url`). На такой домен
    /// страница не уводит вкладку даже из обработчика щелчка: так реклама
    /// подменяет плеер, по которому щёлкнули. Правила `$popup` сюда не входят —
    /// они про окна, а ссылки сайта по щелчку должны работать.
    pub fn blocks_ad_host(&self, url: &str, from_url: &str) -> bool {
        if !self.filters(from_url) {
            return false;
        }
        let Ok(request) = Request::new(url, from_url, "document", "GET") else {
            return false;
        };
        self.engine
            .load()
            .ad_hosts
            .check_network_request(&request)
            .should_block()
    }
}

/// Правило домена целиком из списка рекламы (`||popads.net^`, `@@||ok.example^`,
/// `||adsterra.com^$third-party`) — как правило документа для движка окон. Так
/// uBlock Origin решает, что переход на домен — реклама («строгая блокировка»);
/// `$third-party` здесь считается от страницы, открывшей окно, а `$badfilter`
/// отменяет такое же правило другого списка. Правила с путём или другими
/// опциями не участвуют: они про ресурсы страниц, а не про окна.
fn host_rule(line: &str) -> Option<String> {
    let line = line.trim();
    let (exception, rest) = match line.strip_prefix("@@") {
        Some(rest) => (true, rest),
        None => (false, line),
    };
    let (pattern, options) = rest.split_once('$').unwrap_or((rest, ""));
    let (mut party, mut bad) = ("", "");
    for option in options.split(',').filter(|o| !o.is_empty()) {
        match option {
            "third-party" | "3p" => party = ",third-party",
            "badfilter" => bad = ",badfilter",
            _ => return None,
        }
    }
    let host = pattern.strip_prefix("||")?.strip_suffix('^')?;
    let plain = !host.is_empty()
        && host.contains('.')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    plain.then(|| {
        format!(
            "{}||{host}^$document{party}{bad}",
            if exception { "@@" } else { "" }
        )
    })
}

/// Заглушка движка — `data:<mime>;base64,<тело>` — в тип и байты ответа.
fn decode_data_url(url: &str) -> Option<(String, Vec<u8>)> {
    use base64::Engine as _;

    let (mime, data) = url.strip_prefix("data:")?.split_once(";base64,")?;
    let body = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    Some((mime.to_string(), body))
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

/// Правило `$popup` (и `$all`, который в uBlock Origin включает окна) из
/// списка — как правило документа для движка окон: `||ads.example^$popup,3p` →
/// `||ads.example^$document,3p`. Остальные строки (и `$popunder`, где рекламой
/// становится сама страница) — `None`.
fn popup_rule(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('!') || line.starts_with('[') {
        return None;
    }
    let (pattern, options) = line.rsplit_once('$')?;
    if !options
        .split(',')
        .any(|option| option == "popup" || option == "all")
    {
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
            protections: true,
            ads: true,
        }
    }

    /// Список без защиты своего содержимого сайта: региональный, свои правила.
    fn forced(text: &str) -> FilterList {
        FilterList {
            text: text.to_string(),
            trusted: false,
            protections: false,
            ads: false,
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
            guard
                .generic_hide(
                    "https://other.example/",
                    &["ad-banner".into(), "content".into()],
                    &["sidebar-ads".into()]
                )
                .hide,
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
            ),
            forced("example.com##.mine")],
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
        assert_eq!(cosmetics.force, vec![".mine".to_string()]);
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
        assert_eq!(
            popup_rule("||x.example^$all").as_deref(),
            Some("||x.example^$document")
        );
        assert_eq!(popup_rule("||x.example^$popunder"), None);
        assert_eq!(popup_rule("||x.example^$script"), None);
        assert_eq!(popup_rule("example.com##.popup"), None);
        assert_eq!(popup_rule("! $popup"), None);
    }

    #[test]
    fn ad_popups_are_blocked() {
        let guard = Guard::empty();
        guard.swap(Guard::build(
            vec![
                list(
                    "||ads.example^$popup\n\
                     $popup,third-party,domain=player.example\n\
                     @@||ok.example^$popup,domain=player.example\n\
                     ||malware.example^$all\n\
                     ||phishing.example^$document",
                    false,
                ),
                // Список слежки: его домены — не реклама для окон.
                FilterList {
                    ads: false,
                    ..list("||tracker.example^", false)
                },
            ],
            Vec::new(),
        ));
        let page = "https://news.example/";
        assert!(guard.blocks_popup("https://ads.example/click?id=1", page));
        assert!(guard.blocks_popup("https://malware.example/", page));
        // Правила запроса и документа не про окна: ссылка туда открывается
        // (а документ, если правило про него, закроет своя страница).
        assert!(!guard.blocks_popup("https://tracker.example/", page));
        assert!(!guard.blocks_popup("https://phishing.example/", page));
        assert!(!guard.blocks_popup("https://wiki.example/", page));
        // Плеер, который открывает чужие окна, — только его окна.
        let player = "https://player.example/watch/1";
        assert!(guard.blocks_popup("https://any.example/", player));
        // Ссылки плеера по щелчку — не домены рекламных сетей.
        assert!(!guard.blocks_ad_host("https://any.example/", player));
        assert!(!guard.blocks_ad_host("https://ads.example/", page));
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
    fn redirect_rules_answer_with_a_stub() {
        let json = serde_json::json!([
            {"name": "noop.js", "aliases": ["noopjs"], "kind": {"mime": "application/javascript"},
             "content": b64("(function(){})();"), "dependencies": [], "permission": 0},
            {"name": "noop-vast4.xml", "aliases": [], "kind": {"mime": "text/xml"},
             "content": b64("<VAST version=\"4.0\"></VAST>"), "dependencies": [], "permission": 0},
        ])
        .to_string();
        let guard = Guard::empty();
        guard.swap(Guard::build(
            vec![list(
                "||ads.example^$script,redirect=noopjs\n\
                 ||vast.example^$xhr,redirect=noop-vast4.xml\n\
                 ||nostub.example^$script,redirect=missing.js",
                false,
            )],
            Guard::parse_resources(&json).unwrap(),
        ));
        let page = "https://news.example/";
        assert_eq!(
            guard.check(
                "https://ads.example/a.js",
                page,
                ResourceKind::Script,
                "GET"
            ),
            Decision::Redirect {
                mime: "application/javascript".into(),
                body: b"(function(){})();".to_vec(),
            }
        );
        assert!(matches!(
            guard.check("https://vast.example/v?x=1", page, ResourceKind::Xhr, "GET"),
            Decision::Redirect { ref mime, .. } if mime == "text/xml"
        ));
        // Ресурса заглушки нет — запрос просто закрывается.
        assert_eq!(
            guard.check(
                "https://nostub.example/a.js",
                page,
                ResourceKind::Script,
                "GET"
            ),
            Decision::Block
        );
    }

    #[test]
    fn frames_get_own_rules_and_window_guard() {
        let guard = guard_with(
            "##div[id^=\"ad-\"]\n\
             player.example##.overlay-ad\n\
             proc.example##div:has-text(Реклама)",
        );
        let page = "https://news.example/";
        assert!(guard
            .frame_cosmetics("https://player.example/embed/1", page)
            .is_some());
        assert!(guard
            .frame_cosmetics("https://proc.example/", page)
            .is_some());
        assert!(guard
            .frame_cosmetics("https://player.example/embed/1", page)
            .is_some_and(|c| c.popups));
        // Только общие правила: чужому фрейму — лишь защита окон, без
        // косметики; своему — ничего.
        let plain = guard
            .frame_cosmetics("https://plain.example/", page)
            .unwrap();
        assert!(plain.popups && plain.hide.is_empty() && !plain.generic);
        assert_eq!(plain.site, "plain.example");
        assert!(guard
            .frame_cosmetics("https://cdn.news.example/embed", page)
            .is_none());
        // Страница без блокировки — и её фреймы без косметики.
        guard.set_exempt_sites(["news.example".to_string()]);
        assert!(guard
            .frame_cosmetics("https://player.example/embed/1", page)
            .is_none());
    }

    #[test]
    fn standard_mode_leaves_first_party_requests_alone() {
        let guard = guard_with("||ads.mail.example^\n||tracker.example^\n/banner/*");
        let page = "https://news.mail.example/feed";
        // Свой сайт (тот же регистрируемый домен) — как в обычном режиме Brave.
        for url in [
            "https://ads.mail.example/a.js",
            "https://mail.example/banner/1.png",
        ] {
            assert_eq!(
                guard.check(url, page, ResourceKind::Script, "GET"),
                Decision::Allow,
                "{url}"
            );
            assert!(!guard.blocks(url, page, ResourceKind::Subdocument), "{url}");
        }
        // Чужой — закрывается.
        assert_eq!(
            guard.check(
                "https://tracker.example/t.js",
                page,
                ResourceKind::Script,
                "GET"
            ),
            Decision::Block
        );
        // Агрессивный режим закрывает и свой.
        guard.set_aggressive(true);
        assert_eq!(
            guard.check(
                "https://ads.mail.example/a.js",
                page,
                ResourceKind::Script,
                "GET"
            ),
            Decision::Block
        );
    }

    #[test]
    fn youtube_is_always_aggressive() {
        let guard = guard_with("||youtube.com/pagead/");
        assert_eq!(
            guard.check(
                "https://www.youtube.com/pagead/viewthroughconversion/1",
                "https://www.youtube.com/watch?v=1",
                ResourceKind::Xhr,
                "GET"
            ),
            Decision::Block
        );
    }

    #[test]
    fn search_engines_keep_list_hiding_off_in_standard_mode() {
        let guard = guard_with("yandex.ru##.serp-ad\ndzen.ru##.feed-ad");
        assert!(guard
            .cosmetics("https://yandex.ru/search/?text=1")
            .hide
            .is_empty());
        assert_eq!(
            guard.cosmetics("https://dzen.ru/").hide,
            vec![".feed-ad".to_string()]
        );
        guard.set_aggressive(true);
        assert_eq!(
            guard.cosmetics("https://yandex.ru/search/?text=1").hide,
            vec![".serp-ad".to_string()]
        );
        assert!(guard.cosmetics("https://yandex.ru/").aggressive);
    }

    #[test]
    fn user_rules_always_hide() {
        let guard = Guard::empty();
        guard.swap(Guard::build(
            vec![
                list("##.list-ad", false),
                forced("dzen.ru##.mine\n##.everywhere"),
            ],
            Vec::new(),
        ));
        let cosmetics = guard.cosmetics("https://dzen.ru/");
        assert_eq!(cosmetics.force, vec![".mine".to_string()]);
        assert_eq!(cosmetics.site, "dzen.ru");
        let found = guard.generic_hide(
            "https://dzen.ru/",
            &["list-ad".into(), "everywhere".into()],
            &[],
        );
        assert_eq!(found.hide, vec![".list-ad".to_string()]);
        assert_eq!(found.force, vec![".everywhere".to_string()]);
    }

    #[test]
    fn ad_hosts_close_windows_and_navigations() {
        let guard = Guard::empty();
        guard.swap(Guard::build(
            vec![
                list(
                    "||popads.example^\n||adsterra.example^$third-party\n||adserver.example/path^\n||cdn.example^$script\n@@||ok.popads.example^",
                    false,
                ),
                list("||exo.example^\n||exo.example^$badfilter\n||exo.example^$3p", false),
                FilterList {
                    ads: false,
                    ..list("||counter.example^", false)
                },
            ],
            Vec::new(),
        ));
        let page = "https://video.example/watch/1";
        // Домен рекламы целиком — окно не открывается; `$third-party` — от
        // страницы, открывшей окно.
        assert!(guard.blocks_popup("https://popads.example/go?z=1", page));
        assert!(guard.blocks_popup("https://net.adsterra.example/", page));
        assert!(!guard.blocks_popup("https://adsterra.example/next", "https://adsterra.example/"));
        assert!(guard.blocks_popup("https://www.popads.example/", page));
        // Домены сетей уводят вкладку и по щелчку — переход отменяется.
        assert!(guard.blocks_ad_host("https://popads.example/go", page));
        assert!(guard.blocks_ad_host("https://net.adsterra.example/", page));
        assert!(!guard.blocks_ad_host("https://ok.popads.example/", page));
        assert!(!guard.blocks_ad_host("https://counter.example/", page));
        // `$badfilter` снимает правило целиком, остаётся только `$3p`.
        assert!(guard.blocks_popup("https://exo.example/", page));
        assert!(!guard.blocks_popup("https://exo.example/next", "https://exo.example/"));
        // Исключение, правило с путём или типом и домен из списка слежки — открываются.
        assert!(!guard.blocks_popup("https://ok.popads.example/", page));
        assert!(!guard.blocks_popup("https://adserver.example/path", page));
        assert!(!guard.blocks_popup("https://cdn.example/", page));
        assert!(!guard.blocks_popup("https://counter.example/", page));
        // Переход вкладки на рекламный домен закрывается своей страницей, хотя
        // адрес документа и есть «страница»: главный документ своим не бывает.
        let ad = "https://popads.example/landing";
        assert_eq!(
            guard.check(ad, ad, ResourceKind::Document, "GET"),
            Decision::Block
        );
    }

    #[test]
    fn host_rules_are_plain_domains_only() {
        assert_eq!(
            host_rule("||ads.example^").as_deref(),
            Some("||ads.example^$document")
        );
        assert_eq!(
            host_rule("@@||ok.example^").as_deref(),
            Some("@@||ok.example^$document")
        );
        assert_eq!(
            host_rule("||adsterra.example^$third-party").as_deref(),
            Some("||adsterra.example^$document,third-party")
        );
        assert_eq!(
            host_rule("||ads.example^$badfilter").as_deref(),
            Some("||ads.example^$document,badfilter")
        );
        assert_eq!(host_rule("||ads.example^$doc,badfilter"), None);
        assert_eq!(host_rule("||ads.example^$script"), None);
        assert_eq!(host_rule("||ads.example/x^"), None);
        assert_eq!(host_rule("||localhost^"), None);
        assert_eq!(host_rule("example.com##.ad"), None);
    }

    #[test]
    fn additional_lists_work_like_brave() {
        let guard = Guard::empty();
        guard.swap(Guard::build(
            vec![
                list("||ads.example^\n||cdn.example^", false),
                forced(
                    "@@||cdn.example^$domain=news.example\n\
                     ||promo.news.example^\n\
                     news.example##.tgb-wrapper",
                ),
            ],
            Vec::new(),
        ));
        let page = "https://news.example/";
        // Исключение дополнительного снимает блокировку основного.
        assert_eq!(
            guard.check(
                "https://cdn.example/a.js",
                page,
                ResourceKind::Script,
                "GET"
            ),
            Decision::Allow
        );
        assert_eq!(
            guard.check(
                "https://ads.example/a.js",
                page,
                ResourceKind::Script,
                "GET"
            ),
            Decision::Block
        );
        // Дополнительный закрывает и запросы к самому сайту.
        assert_eq!(
            guard.check(
                "https://promo.news.example/b.js",
                page,
                ResourceKind::Script,
                "GET"
            ),
            Decision::Block
        );
        // Его скрытие — сразу, без проверки исполнителем.
        let cosmetics = guard.cosmetics(page);
        assert_eq!(cosmetics.force, vec![".tgb-wrapper".to_string()]);
        assert!(cosmetics.hide.is_empty());
    }

    #[test]
    fn registrable_domains_follow_the_suffix_list() {
        assert_eq!(registrable_domain("news.mail.ru"), "mail.ru");
        assert_eq!(registrable_domain("user.github.io"), "user.github.io");
        assert_eq!(registrable_domain("a.b.co.uk"), "b.co.uk");
        assert!(same_site("https://r.mail.ru/x", "https://e.mail.ru/inbox"));
        assert!(!same_site("https://mradx.net/x", "https://mail.ru/"));
    }

    #[test]
    fn revision_follows_every_filter_change() {
        let guard = Guard::empty();
        let start = guard.revision();
        guard.swap(Engines::default());
        guard.set_enabled(false);
        guard.set_exempt_sites(["news.example".to_string()]);
        guard.set_aggressive(true);
        assert_eq!(guard.revision(), start + 4);
    }

    #[test]
    fn data_urls_are_decoded() {
        assert_eq!(
            decode_data_url("data:text/plain;base64,MTkweDQ="),
            Some(("text/plain".to_string(), b"190x4".to_vec()))
        );
        assert_eq!(decode_data_url("data:text/plain,190x4"), None);
        assert_eq!(decode_data_url("https://example.com/"), None);
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
