//! Косметика документа: стили скрытия, процедурные правила и скриптлеты,
//! которые встраиваются в страницу до её собственных скриптов.

use std::sync::LazyLock;

/// Что сделать с документом.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Cosmetics {
    /// CSS-селекторы элементов, которые прячутся.
    pub hide: Vec<String>,
    /// Правила `:style()`, которые выражаются чистым CSS: селектор и объявления.
    pub styles: Vec<(String, String)>,
    /// Процедурные правила (`:has-text`, `:upward`, `:remove()`…) — JSON движка;
    /// их выполняет скрипт страницы.
    pub procedural: Vec<String>,
    /// Искать общие правила по классам и id страницы: у неё нет `$generichide`.
    pub generic: bool,
    /// Скриптлеты вместе с зависимостями — готовый JavaScript.
    pub script: String,
}

impl Cosmetics {
    pub fn is_empty(&self) -> bool {
        self.hide.is_empty()
            && self.styles.is_empty()
            && self.procedural.is_empty()
            && !self.generic
            && self.script.trim().is_empty()
    }
}

/// Исполнитель косметики на странице: стили, процедурные правила и сбор
/// классов и id для общих правил. Без строк-комментариев — они нужны в
/// исходнике, а не в каждом документе.
static RUNTIME: LazyLock<String> =
    LazyLock::new(|| crate::script::strip_comment_lines(include_str!("cosmetic.js")));

/// Хост адреса http(s) в нижнем регистре — с ним сверяется `location.hostname`.
pub fn document_host(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = match host.strip_prefix('[') {
        Some(ipv6) => format!("[{}]", ipv6.split(']').next()?),
        None => host.split(':').next()?.to_string(),
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Скрипт документа для WebView2 или `None`, если делать нечего.
///
/// Скрипт встраивается при создании документа во все его фреймы, а правила
/// посчитаны для адреса вкладки — поэтому он сверяет хост. Стиль скрытия
/// ставится, как только у документа появляется корневой элемент, остальное
/// делает исполнитель (`cosmetic.js`). Скриптлеты с нулевым символом не
/// встраиваются: WebView2 обрезал бы на нём весь скрипт.
pub fn document_script(host: &str, cosmetics: &Cosmetics) -> Option<String> {
    if cosmetics.is_empty() {
        return None;
    }
    let script = if cosmetics.script.contains('\0') {
        ""
    } else {
        cosmetics.script.as_str()
    };
    let mut css: String = cosmetics
        .hide
        .iter()
        .map(|selector| format!("{selector}{{display:none!important}}\n"))
        .collect();
    for (selector, style) in &cosmetics.styles {
        css.push_str(&format!("{selector}{{{style}}}\n"));
    }
    let procedural: Vec<serde_json::Value> = cosmetics
        .procedural
        .iter()
        .filter_map(|filter| serde_json::from_str(filter).ok())
        .collect();
    let config = serde_json::json!({
        "host": host,
        "css": css,
        "procedural": procedural,
        "generic": cosmetics.generic,
    })
    .to_string();
    let host = serde_json::to_string(host).ok()?;
    let runtime = RUNTIME.replace("__X4_CONFIG__", &config);
    Some(format!(
        r#"(() => {{
if (location.hostname !== {host}) return;
{script}
}})();
{runtime}
"#
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_is_taken_from_http_urls() {
        assert_eq!(
            document_host("https://www.YouTube.com/watch?v=1").as_deref(),
            Some("www.youtube.com")
        );
        assert_eq!(
            document_host("http://user@example.com:8080/a").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            document_host("https://[::1]:443/").as_deref(),
            Some("[::1]")
        );
        assert_eq!(document_host("about:blank"), None);
        assert_eq!(document_host("https:///path"), None);
    }

    #[test]
    fn nothing_to_do_gives_no_script() {
        assert_eq!(document_script("example.com", &Cosmetics::default()), None);
    }

    #[test]
    fn script_checks_host_and_hides_selectors() {
        let cosmetics = Cosmetics {
            hide: vec![".promo".into()],
            styles: vec![(".banner".into(), "height: 0".into())],
            procedural: vec![
                r#"{"selector":[{"type":"css-selector","arg":"div"},{"type":"has-text","arg":"Реклама"}]}"#.into(),
            ],
            generic: true,
            script: "mark();".into(),
        };
        let script = document_script("example.com", &cosmetics).unwrap();
        assert!(script.contains(r#"location.hostname !== "example.com""#));
        assert!(script.contains("mark();"));
        assert!(script.contains(r#".promo{display:none!important}"#));
        assert!(script.contains(r#".banner{height: 0}"#));
        assert!(script.contains(r#""type":"has-text""#));
        assert!(script.contains(r#""generic":true"#));
        assert!(!script.contains("__X4_CONFIG__"));
        assert!(!script.contains('\0'));
    }

    #[test]
    fn generic_rules_alone_need_a_script() {
        let cosmetics = Cosmetics {
            generic: true,
            ..Cosmetics::default()
        };
        assert!(!cosmetics.is_empty());
        assert!(document_script("example.com", &cosmetics).is_some());
    }

    #[test]
    fn scriptlets_with_nul_are_dropped_but_css_stays() {
        let cosmetics = Cosmetics {
            hide: vec![".promo".into()],
            script: "bad(\"\0\");".into(),
            ..Cosmetics::default()
        };
        let script = document_script("example.com", &cosmetics).unwrap();
        assert!(!script.contains("bad("));
        assert!(script.contains(".promo"));
    }
}
