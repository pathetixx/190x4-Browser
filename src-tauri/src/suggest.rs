//! Подсказки поисковой системы для адресной строки.
//!
//! Запрос делает Rust, а не страница интерфейса: у chrome-вебвью нет сетевого
//! фильтра, и обращение к поисковику оттуда шло бы мимо всех правил. Ответы
//! разбираются в один список строк — формат у каждого поисковика свой, а
//! интерфейсу нужен просто список.
//!
//! Подсказки — это отправка набранного на сторону: в приватном окне их нет, и
//! выключить их можно настройкой `search_suggest`.

use std::time::Duration;

use serde_json::Value;

/// Интерактивная подсказка: ждать дольше бессмысленно — пользователь уже
/// дописал запрос.
const TIMEOUT: Duration = Duration::from_millis(1500);
const LIMIT: usize = 6;

fn endpoint(engine: &str, query: &str) -> String {
    let query = urlencoding(query);
    match engine {
        "yandex" => format!("https://suggest.yandex.ru/suggest-ff.cgi?part={query}"),
        "google" => {
            format!("https://suggestqueries.google.com/complete/search?client=firefox&q={query}")
        }
        "bing" => format!("https://api.bing.com/osjson.aspx?query={query}"),
        _ => format!("https://duckduckgo.com/ac/?type=list&q={query}"),
    }
}

/// Подсказки поисковика или пустой список, если он не ответил.
pub async fn fetch(client: &reqwest::Client, engine: &str, query: &str) -> Vec<String> {
    let query = query.trim();
    if query.is_empty() || query.chars().count() > 200 {
        return Vec::new();
    }
    let request = client
        .get(endpoint(engine, query))
        .timeout(TIMEOUT)
        .header(reqwest::header::ACCEPT, "application/json,text/javascript");
    let Ok(response) = request.send().await else {
        return Vec::new();
    };
    let Ok(text) = response.text().await else {
        return Vec::new();
    };
    parse(&text, query)
}

/// Разбор ответа. У всех четырёх поисковиков форма своя, но сводится к двум:
/// OpenSearch (`["запрос", ["подсказка", …]]`) и список объектов с `phrase`.
fn parse(body: &str, query: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let mut out = Vec::new();

    match &value {
        // ["запрос", ["подсказка", ...]] — Яндекс, Google, Bing.
        Value::Array(items) => {
            if let Some(Value::Array(list)) = items.get(1) {
                collect(list, &mut out);
            } else {
                // [{"phrase": "..."}] — DuckDuckGo.
                collect(items, &mut out);
            }
        }
        _ => return Vec::new(),
    }

    let lower = query.to_lowercase();
    out.retain(|item| !item.is_empty() && item.to_lowercase() != lower);
    out.truncate(LIMIT);
    out
}

fn collect(items: &[Value], out: &mut Vec<String>) {
    for item in items {
        let text = match item {
            Value::String(text) => text.clone(),
            Value::Object(map) => match map.get("phrase").or_else(|| map.get("text")) {
                Some(Value::String(text)) => text.clone(),
                _ => continue,
            },
            _ => continue,
        };
        let text: String = text.chars().take(200).collect();
        if !out.contains(&text) {
            out.push(text);
        }
    }
}

fn urlencoding(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opensearch_and_duckduckgo_shapes_are_read() {
        let opensearch = r#"["хабр",["хабр","хабр карьера","хабр"]]"#;
        assert_eq!(
            parse(opensearch, "хабр"),
            vec!["хабр карьера".to_string()],
            "свой же запрос и повторы в подсказках не нужны"
        );

        let ddg = r#"[{"phrase":"погода москва"},{"phrase":"погода спб"}]"#;
        assert_eq!(parse(ddg, "погода"), ["погода москва", "погода спб"]);

        assert!(parse("не json", "x").is_empty());
        assert!(parse("{}", "x").is_empty());
    }

    #[test]
    fn endpoints_are_per_engine() {
        assert!(endpoint("yandex", "а б").contains("suggest.yandex.ru"));
        assert!(endpoint("yandex", "а б").contains("%20"));
        assert!(endpoint("unknown", "x").contains("duckduckgo.com"));
    }
}
