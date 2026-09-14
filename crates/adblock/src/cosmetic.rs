//! Косметика документа: стили скрытия и скриптлеты, которые встраиваются в
//! страницу до её собственных скриптов.

/// Что сделать с документом.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Cosmetics {
    /// CSS-селекторы элементов, которые прячутся.
    pub hide: Vec<String>,
    /// Скриптлеты вместе с зависимостями — готовый JavaScript.
    pub script: String,
}

impl Cosmetics {
    pub fn is_empty(&self) -> bool {
        self.hide.is_empty() && self.script.trim().is_empty()
    }
}

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
/// ставится, как только у документа появляется корневой элемент. Скриптлеты с
/// нулевым символом не встраиваются: WebView2 обрезал бы на нём весь скрипт.
pub fn document_script(host: &str, cosmetics: &Cosmetics) -> Option<String> {
    if cosmetics.is_empty() {
        return None;
    }
    let script = if cosmetics.script.contains('\0') {
        ""
    } else {
        cosmetics.script.as_str()
    };
    let css: String = cosmetics
        .hide
        .iter()
        .map(|selector| format!("{selector}{{display:none!important}}\n"))
        .collect();
    let host = serde_json::to_string(host).ok()?;
    let css = serde_json::to_string(&css).ok()?;
    Some(format!(
        r#"(() => {{
if (location.hostname !== {host}) return;
{script}
const hideCss190x4 = {css};
if (!hideCss190x4) return;
const hide190x4 = () => {{
  const style = document.createElement("style");
  style.textContent = hideCss190x4;
  (document.head || document.documentElement).append(style);
}};
if (document.documentElement) hide190x4();
else new MutationObserver((_, observer) => {{
  if (!document.documentElement) return;
  observer.disconnect();
  hide190x4();
}}).observe(document, {{ childList: true }});
}})();
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
            script: "mark();".into(),
        };
        let script = document_script("example.com", &cosmetics).unwrap();
        assert!(script.contains(r#"location.hostname !== "example.com""#));
        assert!(script.contains("mark();"));
        assert!(script.contains(".promo{display:none!important}"));
        assert!(!script.contains('\0'));
    }

    #[test]
    fn scriptlets_with_nul_are_dropped_but_css_stays() {
        let cosmetics = Cosmetics {
            hide: vec![".promo".into()],
            script: "bad(\"\0\");".into(),
        };
        let script = document_script("example.com", &cosmetics).unwrap();
        assert!(!script.contains("bad("));
        assert!(script.contains(".promo"));
    }
}
