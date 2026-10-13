//! Кем вкладка представляется сайтам: Edge, как движок есть, или Chrome.
//!
//! Движок для сайтов — Edge: в строке браузера `Edg/…`, в Client Hints (заголовки
//! `Sec-CH-UA*`, `navigator.userAgentData`) — бренд «Microsoft Edge WebView2».
//! Режим Chrome подменяет и то и другое командой протокола отладки
//! `Emulation.setUserAgentOverride`: строка — та же, без `Edg/…`, Client Hints —
//! настоящие значения движка (их присылает интерфейс, см. [`set_engine_hints`]),
//! а бренды — как у Chrome той же версии: три, с заглушкой и порядком по
//! алгоритму Chromium, Google Chrome — с версией Chromium.
//!
//! Подмена живёт во вкладке и ставится до навигации: при создании вкладки и в
//! `NavigationStarting`, если сайт нового адреса представляется иначе. Первый
//! запрос документа такой навигации ещё уходит с прежним видом, скрипты
//! страницы видят уже новый. Фреймам чужих сайтов — они живут в своих
//! процессах — вкладка ставит ту же подмену сама, до их скриптов
//! (`tab::attach_frames`): иначе страница была бы Chrome, а фрейм в ней — Edge,
//! и проверки защиты от ботов шли бы по кругу. Выдаёт движок и PlayReady: у
//! Chrome его нет.

use parking_lot::RwLock;
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Identity {
    Edge,
    Chrome,
}

impl Identity {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "edge" => Some(Self::Edge),
            "chrome" => Some(Self::Chrome),
            _ => None,
        }
    }
}

struct Config {
    default: Identity,
    /// Сайты со своим видом: домен и его поддомены.
    sites: Vec<(String, Identity)>,
    /// `getHighEntropyValues` движка: бренды, версии, платформа.
    hints: Option<Value>,
    /// Строка браузера движка без подмены — с неё снимается `Edg/…`.
    engine_agent: Option<String>,
    /// Растёт с каждой сменой: вкладки по нему видят, что подмену пора обновить.
    generation: u64,
}

static CONFIG: RwLock<Config> = RwLock::new(Config {
    default: Identity::Edge,
    sites: Vec::new(),
    hints: None,
    engine_agent: None,
    generation: 0,
});

/// Вид для всех сайтов и исключения. Открытым вкладкам — [`crate::TabHost::apply_identity`].
pub fn set_identity(default: Identity, sites: Vec<(String, Identity)>) {
    let mut config = CONFIG.write();
    config.default = default;
    config.sites = sites;
    config.generation += 1;
}

/// Настоящие Client Hints движка — `navigator.userAgentData.getHighEntropyValues`.
pub fn set_engine_hints(hints: Value) {
    let mut config = CONFIG.write();
    config.hints = Some(hints);
    config.generation += 1;
}

/// Строка браузера движка. Запоминается первая: у вкладок она одна и та же.
pub(crate) fn remember_engine_agent(agent: &str) {
    let mut config = CONFIG.write();
    if config.engine_agent.is_none() && !agent.is_empty() {
        config.engine_agent = Some(agent.to_string());
    }
}

pub(crate) fn generation() -> u64 {
    CONFIG.read().generation
}

/// Кем представляться на этом адресе. Адреса не сайтов (новая вкладка,
/// `about:blank`) — как для всех: следующая навигация уйдёт уже с ним.
pub(crate) fn for_url(url: &str) -> Identity {
    let config = CONFIG.read();
    let host = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .map(|authority| authority.rsplit('@').next().unwrap_or(authority))
        .map(|host| host.split(':').next().unwrap_or(host).to_ascii_lowercase());
    if let Some(host) = host {
        for (site, identity) in &config.sites {
            if host == *site || host.ends_with(&format!(".{site}")) {
                return *identity;
            }
        }
    }
    config.default
}

/// Параметры `Emulation.setUserAgentOverride` для вида. Edge — пустая строка:
/// подмена снята, движок отвечает сам. `None` — строка движка ещё неизвестна.
pub(crate) fn override_params(identity: Identity) -> Option<String> {
    match identity {
        Identity::Edge => Some(json!({ "userAgent": "" }).to_string()),
        Identity::Chrome => {
            let config = CONFIG.read();
            let agent = chrome_agent(config.engine_agent.as_deref()?);
            let metadata = chrome_metadata(config.hints.as_ref(), &agent);
            Some(json!({ "userAgent": agent, "userAgentMetadata": metadata }).to_string())
        }
    }
}

/// Строка Chrome: строка движка без `Edg/…` (у Chrome этого слова нет).
fn chrome_agent(engine: &str) -> String {
    engine
        .split(' ')
        .filter(|token| !token.starts_with("Edg/") && !token.starts_with("EdgA/"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Client Hints Chrome из настоящих значений движка: платформа и архитектура —
/// движка, бренды — как у Chrome той же версии ([`chrome_brands`]), полная
/// версия Chrome — версия Chromium.
fn chrome_metadata(hints: Option<&Value>, agent: &str) -> Value {
    let major = agent
        .split("Chrome/")
        .nth(1)
        .and_then(|rest| rest.split('.').next())
        .unwrap_or("0")
        .to_string();
    let field = |name: &str| hints.and_then(|hints| hints.get(name));
    let text = |name: &str, default: &str| {
        field(name)
            .and_then(Value::as_str)
            .unwrap_or(default)
            .to_string()
    };

    let chromium_full = field("fullVersionList")
        .and_then(Value::as_array)
        .and_then(|list| {
            list.iter()
                .find(|item| item.get("brand").and_then(Value::as_str) == Some("Chromium"))
        })
        .and_then(|item| item.get("version").and_then(Value::as_str))
        .map(str::to_string)
        .unwrap_or_else(|| format!("{major}.0.0.0"));

    json!({
        "brands": chrome_brands(&major, &major, false),
        "fullVersionList": chrome_brands(&major, &chromium_full, true),
        "fullVersion": chromium_full,
        "platform": text("platform", "Windows"),
        "platformVersion": text("platformVersion", "10.0.0"),
        "architecture": text("architecture", "x86"),
        "model": text("model", ""),
        "mobile": false,
        "bitness": text("bitness", "64"),
        "wow64": field("wow64").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// Бренды Chrome этой версии — так, как их строит сам Chromium
/// (`GenerateBrandVersionList`, `components/embedder_support/user_agent_utils.cc`):
/// ровно три — бренд-заглушка, Chromium и Google Chrome, — а порядок и
/// заглушка зависят от основной версии. Бренды движка переименовывать нельзя:
/// у WebView2 их два («Microsoft Edge» и «Microsoft Edge WebView2»), и Chrome
/// выходил дважды — защита от ботов на такое не ведётся.
fn chrome_brands(major: &str, version: &str, full: bool) -> Value {
    const CHARS: [&str; 11] = [" ", "(", ":", "-", ".", "/", ")", ";", "=", "?", "_"];
    const VERSIONS: [&str; 3] = ["8", "99", "24"];
    const ORDERS: [[usize; 3]; 6] = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let seed: usize = major.parse().unwrap_or(0);
    let grease_brand = format!(
        "Not{}A{}Brand",
        CHARS[seed % CHARS.len()],
        CHARS[(seed + 1) % CHARS.len()]
    );
    let grease_version = VERSIONS[seed % VERSIONS.len()];
    let grease_version = if full {
        format!("{grease_version}.0.0.0")
    } else {
        grease_version.to_string()
    };
    let order = ORDERS[seed % ORDERS.len()];
    let mut list = vec![Value::Null; 3];
    list[order[0]] = json!({ "brand": grease_brand, "version": grease_version });
    list[order[1]] = json!({ "brand": "Chromium", "version": version });
    list[order[2]] = json!({ "brand": "Google Chrome", "version": version });
    Value::Array(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EDGE: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36 Edg/146.0.0.0";

    #[test]
    fn chrome_agent_drops_the_edge_token() {
        assert_eq!(
            chrome_agent(EDGE),
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36"
        );
    }

    #[test]
    fn brands_are_chrome_with_the_chromium_version() {
        let hints = json!({
            "brands": [
                {"brand": "Microsoft Edge", "version": "154"},
                {"brand": "Not A(Brand", "version": "99"},
                {"brand": "Microsoft Edge WebView2", "version": "154"},
                {"brand": "Chromium", "version": "154"}
            ],
            "fullVersionList": [
                {"brand": "Microsoft Edge", "version": "154.0.4258.62"},
                {"brand": "Not A(Brand", "version": "99.0.0.0"},
                {"brand": "Microsoft Edge WebView2", "version": "154.0.4258.62"},
                {"brand": "Chromium", "version": "154.0.8037.98"}
            ],
            "platform": "Windows",
            "platformVersion": "19.0.0",
            "architecture": "x86",
            "bitness": "64",
            "model": "",
            "mobile": false,
            "wow64": false
        });
        let agent = EDGE.replace("146.0.0.0", "154.0.0.0");
        let metadata = chrome_metadata(Some(&hints), &chrome_agent(&agent));
        assert!(!metadata.to_string().contains("Edge"));
        // Заглушка движка той же версии совпадает с посчитанной.
        assert_eq!(
            metadata["brands"],
            json!([
                {"brand": "Chromium", "version": "154"},
                {"brand": "Google Chrome", "version": "154"},
                {"brand": "Not A(Brand", "version": "99"}
            ])
        );
        assert_eq!(
            metadata["fullVersionList"][1],
            json!({"brand": "Google Chrome", "version": "154.0.8037.98"})
        );
        assert_eq!(metadata["fullVersion"], "154.0.8037.98");
        assert_eq!(metadata["platformVersion"], "19.0.0");
    }

    #[test]
    fn brands_match_the_chrome_header() {
        // Sec-CH-UA настоящего Chrome 120:
        // "Not_A Brand";v="8", "Chromium";v="120", "Google Chrome";v="120".
        assert_eq!(
            chrome_brands("120", "120", false),
            json!([
                {"brand": "Not_A Brand", "version": "8"},
                {"brand": "Chromium", "version": "120"},
                {"brand": "Google Chrome", "version": "120"}
            ])
        );
    }

    #[test]
    fn without_hints_the_brands_look_like_chrome() {
        let metadata = chrome_metadata(None, &chrome_agent(EDGE));
        let brands: Vec<&str> = metadata["brands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["brand"].as_str().unwrap())
            .collect();
        // Для 146 — как у WebView2 той же версии: Chromium, Not-A.Brand, бренд.
        assert_eq!(brands, ["Chromium", "Not-A.Brand", "Google Chrome"]);
        assert_eq!(metadata["fullVersion"], "146.0.0.0");
    }
}
