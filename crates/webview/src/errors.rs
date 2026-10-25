//! Свои страницы ошибок: «сайт не открывается» и «страница заблокирована».
//!
//! Страницу движка (она на языке системы, с чужим оформлением и советами про
//! Edge) заменяем своей прямо в документе: `ExecuteScript` переписывает
//! содержимое, адрес в строке и запись в истории остаются прежними — так же
//! ведёт себя Chrome, и «назад» не упирается в страницу ошибки. Страницу
//! «заблокировано» отдаёт сам фильтр телом ответа: до документа сайта дело
//! не доходит.

use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_WEB_ERROR_STATUS, COREWEBVIEW2_WEB_ERROR_STATUS_CANNOT_CONNECT,
    COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_COMMON_NAME_IS_INCORRECT,
    COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_EXPIRED,
    COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_IS_INVALID,
    COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_REVOKED,
    COREWEBVIEW2_WEB_ERROR_STATUS_CLIENT_CERTIFICATE_CONTAINS_ERRORS,
    COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_ABORTED,
    COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_RESET, COREWEBVIEW2_WEB_ERROR_STATUS_DISCONNECTED,
    COREWEBVIEW2_WEB_ERROR_STATUS_ERROR_HTTP_INVALID_SERVER_RESPONSE,
    COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED,
    COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED,
    COREWEBVIEW2_WEB_ERROR_STATUS_REDIRECT_FAILED,
    COREWEBVIEW2_WEB_ERROR_STATUS_SERVER_UNREACHABLE, COREWEBVIEW2_WEB_ERROR_STATUS_TIMEOUT,
    COREWEBVIEW2_WEB_ERROR_STATUS_UNEXPECTED_ERROR,
    COREWEBVIEW2_WEB_ERROR_STATUS_VALID_AUTHENTICATION_CREDENTIALS_REQUIRED,
    COREWEBVIEW2_WEB_ERROR_STATUS_VALID_PROXY_AUTHENTICATION_REQUIRED,
};

/// Заголовок и объяснение по коду ошибки движка.
///
/// `None` — показывать нечего: навигацию отменили мы сами (переход ушёл в
/// загрузку, поверх лёг новый адрес), и документ трогать нельзя.
fn describe(status: COREWEBVIEW2_WEB_ERROR_STATUS) -> Option<(&'static str, &'static str)> {
    let text = match status {
        COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED => return None,
        COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED => (
            "Сайт не найден",
            "Проверьте адрес. Если он верный, сайт мог переехать или его имя не отвечает.",
        ),
        COREWEBVIEW2_WEB_ERROR_STATUS_CANNOT_CONNECT
        | COREWEBVIEW2_WEB_ERROR_STATUS_SERVER_UNREACHABLE => (
            "Сайт не отвечает",
            "Он может быть выключен или недоступен из вашей сети.",
        ),
        COREWEBVIEW2_WEB_ERROR_STATUS_TIMEOUT => (
            "Сайт слишком долго не отвечал",
            "Попробуйте ещё раз: соединение могло оборваться по дороге.",
        ),
        COREWEBVIEW2_WEB_ERROR_STATUS_DISCONNECTED => (
            "Нет подключения к интернету",
            "Проверьте Wi-Fi, кабель или подключение через прокси.",
        ),
        COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_ABORTED
        | COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_RESET => (
            "Соединение оборвалось",
            "Сайт или сеть между вами разорвали связь. Обновите страницу.",
        ),
        COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_COMMON_NAME_IS_INCORRECT
        | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_EXPIRED
        | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_IS_INVALID
        | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_REVOKED
        | COREWEBVIEW2_WEB_ERROR_STATUS_CLIENT_CERTIFICATE_CONTAINS_ERRORS => (
            "Подключение не защищено",
            "Сертификат сайта не в порядке: срок истёк, он отозван или выписан на другое имя. Вводить данные нельзя.",
        ),
        COREWEBVIEW2_WEB_ERROR_STATUS_ERROR_HTTP_INVALID_SERVER_RESPONSE => (
            "Сайт ответил непонятно",
            "Ответ сервера не похож на страницу. Попробуйте обновить.",
        ),
        COREWEBVIEW2_WEB_ERROR_STATUS_REDIRECT_FAILED => (
            "Слишком много перенаправлений",
            "Сайт пересылает страницу по кругу. Обычно помогает очистка файлов cookie этого сайта.",
        ),
        COREWEBVIEW2_WEB_ERROR_STATUS_VALID_AUTHENTICATION_CREDENTIALS_REQUIRED => (
            "Нужен вход",
            "Сайт просит имя и пароль, но вход не удался.",
        ),
        COREWEBVIEW2_WEB_ERROR_STATUS_VALID_PROXY_AUTHENTICATION_REQUIRED => (
            "Прокси просит вход",
            "Проверьте имя и пароль прокси-сервера в настройках Windows.",
        ),
        COREWEBVIEW2_WEB_ERROR_STATUS_UNEXPECTED_ERROR => (
            "Страница не открылась",
            "Движок сообщил об ошибке, но не сказал какой. Попробуйте обновить.",
        ),
        _ => (
            "Страница не открылась",
            "Попробуйте обновить её или проверьте подключение.",
        ),
    };
    Some(text)
}

/// Оформление страниц ошибок — общее для скрипта и для страницы блокировки.
const STYLE: &str = r#":root { color-scheme: dark light; }
body { margin: 0; min-height: 100vh; display: grid; place-items: center; background: #0f0f12; color: #f1f1f4;
  font: 15px/1.5 "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif; }
@media (prefers-color-scheme: light) { body { background: #f6f6f8; color: #18181b; } }
.box190x4 { box-sizing: border-box; width: min(560px, 100% - 48px); padding: 32px 0; }
.mark190x4 { width: 44px; height: 44px; display: grid; place-items: center;
  clip-path: polygon(0 0, calc(100% - 10px) 0, 100% 10px, 100% 100%, 10px 100%, 0 calc(100% - 10px));
  background: rgba(222, 87, 114, 0.14); color: #de5772; font-size: 15px; font-weight: 700; letter-spacing: 0.5px; }
h1 { margin: 20px 0 8px; font-size: 24px; font-weight: 650; }
p { margin: 0 0 6px; opacity: 0.78; }
.site190x4 { opacity: 0.95; font-weight: 600; }
.url190x4 { margin-top: 14px; font: 12px/1.4 "JetBrains Mono", ui-monospace, monospace; opacity: 0.5; word-break: break-all; }
button { margin-top: 22px; padding: 9px 18px; border: 0; background: #de5772; color: #fff;
  clip-path: polygon(0 0, calc(100% - 7px) 0, 100% 7px, 100% 100%, 7px 100%, 0 calc(100% - 7px));
  font: inherit; font-weight: 600; cursor: pointer; }
button:hover { filter: brightness(1.08); }
button:focus-visible { outline: 2px solid currentColor; outline-offset: -5px; }
.more190x4 { margin-top: 26px; }
.more190x4 summary { cursor: pointer; opacity: 0.7; user-select: none; }
.more190x4 p { margin-top: 10px; }
.quiet190x4 { margin-top: 12px; background: transparent; color: inherit; border: 1px solid rgba(222, 87, 114, 0.55); }"#;

/// Текст под «Подробнее» у ошибки сертификата.
const PROCEED_HINT: &str = "Если это ваш роутер или сервер в своей сети и вы уверены, что это он, \
                            сайт можно открыть без проверки сертификата. Всё, что вы на нём \
                            введёте, могут перехватить.";

/// Тексты страницы ошибки. `more` — под «Подробнее», рядом с «Всё равно
/// перейти»; `kind` — класс страницы, по нему её узнают повторные скрипты.
struct Page<'a> {
    title: &'a str,
    hint: &'a str,
    more: &'a str,
    kind: &'a str,
}

/// Текст под «Подробнее» у сайта, который SmartScreen отметил опасным.
const SMARTSCREEN_MORE: &str = "SmartScreen — проверка Microsoft. Другие браузеры сверяются с \
                                другими базами и могут этот сайт не блокировать. Если вы ему \
                                доверяете, его можно открыть. Проверку для всех сайтов \
                                выключает настройка SmartScreen в разделе «Конфиденциальность».";

/// Скрипт, который собирает страницу из элементов, а не строкой HTML: у
/// страницы ошибки сертификата движок требует Trusted Types, и `innerHTML`
/// там запрещён — своя страница на ней просто не вставала. Тексты — данные
/// (`textContent`), разметкой они не становятся. `guard` — проверка перед
/// заменой, `proceed` — что делает «Всё равно перейти» (пусто — кнопки нет).
fn page_script(page: Page<'_>, url: &str, guard: &str, proceed: &str) -> String {
    let data = serde_json::json!({
        "title": page.title,
        "host": short_host(url),
        "hint": page.hint,
        "url": url,
        "style": STYLE,
        "more": page.more,
        "kind": page.kind,
    })
    // `<` — escape-последовательностью: адрес со страницы не закроет ничего,
    // куда бы скрипт ни попал.
    .to_string()
    .replace('<', "\\u003c");
    format!(
        r#"(() => {{
{guard}
const data = {data};
const make = (tag, className, text) => {{
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}};
const style = make("style", null, data.style);
const head = make("head");
head.append(style);
const box = make("div", ("box190x4 " + data.kind).trim());
box.append(make("div", "mark190x4", "190"), make("h1", null, data.title), make("p", "site190x4", data.host),
  make("p", null, data.hint), make("div", "url190x4", data.url));
const retry = make("button", null, "Обновить");
retry.type = "button";
retry.id = "retry190x4";
retry.addEventListener("click", () => location.reload());
box.append(retry);
const proceed = {proceed};
if (proceed) {{
  const more = make("details", "more190x4");
  const go = make("button", "quiet190x4", "Всё равно перейти (небезопасно)");
  go.type = "button";
  go.id = "proceed190x4";
  go.addEventListener("click", proceed);
  more.append(make("summary", null, "Подробнее"), make("p", null, data.more), go);
  box.append(more);
}}
const body = make("body");
body.append(box);
document.documentElement.replaceChildren(head, body);
document.title = data.title;
}})();
"#
    )
}

/// Скрипт, который рисует страницу ошибки в уже загруженном документе.
///
/// Только поверх страницы ошибки движка (`chrome-error://`). Неуспешной движок
/// считает и навигацию, после которой на экране осталась прежняя страница
/// (переход отменили, ссылка ушла в приложение), и ответ сайта 4xx со своим
/// содержимым — их документ не наш, и заменять его нельзя.
///
/// Сайт, который SmartScreen отметил опасным, движок закрывает своей страницей
/// предупреждения — тоже `chrome-error://`, и код ошибки у неё общий. Её
/// узнаёт [`smartscreen_script`], он идёт первым.
pub fn error_script(status: COREWEBVIEW2_WEB_ERROR_STATUS, url: &str) -> Option<String> {
    let (title, hint) = describe(status)?;
    let page = page_script(
        Page {
            title,
            hint,
            more: "",
            kind: "",
        },
        url,
        r#"if (!location.href.startsWith("chrome-error://")) return;
if (document.querySelector(".box190x4")) return;"#,
        "null",
    );
    Some(format!("{}\n{page}", smartscreen_script(url)))
}

/// Своя страница вместо предупреждения SmartScreen «Это небезопасный сайт».
///
/// Прежде здесь рисовалась обычная «Страница не открылась», и сайт, который
/// SmartScreen считает опасным, выглядел как обрыв связи. Предупреждение
/// движок может поставить и позже конца навигации, поэтому скрипт повторяют
/// (`Tab`): он заменяет и уже нарисованную общую страницу, но не себя.
/// Обход через общий выключатель SmartScreen затронул бы другие сайты.
pub fn smartscreen_script(url: &str) -> String {
    page_script(
        Page {
            title: "Сайт отмечен как опасный",
            hint: "Microsoft Defender SmartScreen считает, что этот сайт может красть данные \
                   или заражать компьютер. Вернитесь на безопасную страницу.",
            more: SMARTSCREEN_MORE,
            kind: "smartscreen190x4",
        },
        url,
        r#"if (!location.href.startsWith("chrome-error://") || document.querySelector(".smartscreen190x4")) return;
let strings = null;
try {
  strings = typeof loadTimeData !== "undefined" && loadTimeData.valueExists("smartScreenPageTitle") ? loadTimeData : null;
} catch (_) {}
if (!strings) return;"#,
        "null",
    )
}

/// Своя страница вместо страницы движка «Подключение не защищено».
///
/// Страницу движка (Edge, на языке системы) движок ставит сам, уже после
/// конца навигации, — поэтому она заменяется, когда её документ готов, и
/// только если это действительно она. «Всё равно перейти» под «Подробнее», как
/// в Chrome: роутер или сервер в своей сети часто работает с самоподписанным
/// сертификатом. Нажимает его механизм самого движка — решение он помнит до
/// выхода, как Chrome.
pub fn certificate_script(url: &str) -> String {
    page_script(
        Page {
            title: "Подключение не защищено",
            hint:
                "Сертификат сайта не в порядке: срок истёк, он отозван или выписан на другое имя. \
                   Кто-то может выдавать себя за этот сайт.",
            more: PROCEED_HINT,
            kind: "",
        },
        url,
        r#"if (!location.href.startsWith("chrome-error://") || document.querySelector(".box190x4")) return;
const edge = document.getElementById("proceed-link");
const controller = window.certificateErrorPageController;
if (!edge && !controller) return;"#,
        r#"() => {
  if (controller && typeof controller.proceed === "function") controller.proceed();
  else if (edge) edge.click();
}"#,
    )
}

/// Готовый документ «страница заблокирована» — телом ответа фильтра.
pub fn blocked_html(url: &str) -> String {
    format!(
        r#"<!doctype html><html lang="ru"><head><meta charset="utf-8"><title>{title}</title><style>{STYLE}</style></head><body><div class="box190x4"><div class="mark190x4">190</div><h1>{title}</h1>
<p class="site190x4">{host}</p><p>{hint}</p><div class="url190x4">{url}</div></div></body></html>"#,
        title = "Страница заблокирована",
        host = escape(&short_host(url)),
        hint = "Её адрес есть в списках блокировки. Выключить фильтр для этого сайта можно значком щита слева от адреса.",
        url = escape(url),
    )
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Хост адреса для подписи под заголовком.
fn short_host(url: &str) -> String {
    url.split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or("").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_navigation_has_no_page() {
        assert!(error_script(
            COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED,
            "https://a",
        )
        .is_none());
    }

    #[test]
    fn only_certificate_errors_offer_to_proceed() {
        let cert = certificate_script("https://192.168.1.1/");
        assert!(cert.contains("Всё равно перейти"));
        assert!(cert.contains("chrome-error://"));
        assert!(!cert.contains('\0'));
        let timeout = error_script(COREWEBVIEW2_WEB_ERROR_STATUS_TIMEOUT, "https://a/").unwrap();
        assert!(timeout.contains("const proceed = null;"));
        // Предупреждение сохраняется, обход общей защиты не предлагается.
        let smartscreen = smartscreen_script("https://a/");
        assert!(timeout.starts_with(&smartscreen));
        assert!(smartscreen.contains("smartScreenPageTitle"));
        assert!(smartscreen.contains("const proceed = null;"));
        assert!(!smartscreen.contains("smartscreen_proceed"));
        assert!(!smartscreen.contains('\0'));
    }

    #[test]
    fn error_page_replaces_only_the_engine_page() {
        let script = error_script(COREWEBVIEW2_WEB_ERROR_STATUS_TIMEOUT, "https://a/").unwrap();
        assert!(script.contains(r#"if (!location.href.startsWith("chrome-error://")) return;"#));
    }

    #[test]
    fn addresses_are_escaped() {
        let html = blocked_html("https://ads.example/?a=1&b=\"><script>alert(1)</script>");
        assert!(!html.contains("<script>alert"));
        assert!(html.contains("&lt;script&gt;"));

        let script = error_script(
            COREWEBVIEW2_WEB_ERROR_STATUS_TIMEOUT,
            "https://a/\"</script>",
        )
        .unwrap();
        assert!(!script.contains("</script>"));
        assert!(!script.contains('\0'));
    }
}
