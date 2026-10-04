//! Ссылки на приложения: tg:, mailto:, zoommtg: и другие схемы, которые
//! открывает не браузер, а программа на компьютере.
//!
//! Движок такую ссылку отменяет и сообщает о ней (`crates/webview/src/dialogs.rs`).
//! Дальше решает браузер: спросить пользователя или открыть сразу, если сайт
//! уже получил разрешение «всегда». Открывает Windows по ассоциации схемы, как
//! в Chrome: адрес в кавычках через `ShellExecuteW`.

use std::collections::HashMap;

use browser190x4_store::Store;
use browser190x4_webview::{DialogAction, DialogAnswer};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::App;

/// Настройка со списком разрешений «всегда открывать».
pub const ALLOWED_SETTING: &str = "external_apps_allowed";

/// Схемы, которые не открываются никогда: они запускают системные обработчики
/// или выполняют код. Список Chrome и схемы, через которые атаковали Windows.
const DENIED: &[&str] = &[
    "afp",
    "data",
    "disk",
    "disks",
    "file",
    "hcp",
    "ie.http",
    "javascript",
    "mk",
    "ms-appinstaller",
    "ms-cxh",
    "ms-cxh-full",
    "ms-help",
    "ms-msdt",
    "ms-officecmd",
    "nntp",
    "res",
    "search",
    "search-ms",
    "shell",
    "vbscript",
    "view-source",
    "vnd.ms.radio",
];

/// Длиннее адрес в Windows не передают: ShellExecute его обрезает или падает.
const MAX_URI: usize = 2048;

/// Сайт, которому разрешено открывать приложение без вопроса.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedApp {
    pub origin: String,
    pub scheme: String,
    /// Название приложения на момент разрешения — для списка в настройках.
    #[serde(default)]
    pub app: String,
}

/// Ссылки, о которых спросили пользователя: (вкладка, номер окна) → ссылка.
/// Окно браузера отвечает только номером — адрес со страницы обратно не ходит.
#[derive(Default)]
pub struct Offers(Mutex<HashMap<(u32, u64), Offer>>);

struct Offer {
    uri: String,
    scheme: String,
    origin: String,
    app: String,
}

/// Схема ссылки, если такую ссылку можно отдать приложению.
fn scheme_of(uri: &str) -> Option<String> {
    if uri.len() > MAX_URI || uri.chars().any(char::is_control) {
        return None;
    }
    let (scheme, _) = uri.split_once(':')?;
    let scheme = scheme.to_ascii_lowercase();
    let mut chars = scheme.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    let web = matches!(scheme.as_str(), "http" | "https");
    (valid && !web && !DENIED.contains(&scheme.as_str())).then_some(scheme)
}

/// Origin от движка: «null» и прочие непрозрачные значат «сайт неизвестен».
fn clean_origin(origin: &str) -> &str {
    if origin.starts_with("https://") || origin.starts_with("http://") {
        origin
    } else {
        ""
    }
}

/// «Всегда разрешать» — только сайтам по https: адрес без шифрования подделает
/// любой, кто сидит в той же сети.
fn can_remember(origin: &str) -> bool {
    origin.starts_with("https://")
}

pub fn allowed(store: &Store) -> Vec<AllowedApp> {
    store
        .setting(ALLOWED_SETTING)
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

fn is_allowed(list: &[AllowedApp], origin: &str, scheme: &str) -> bool {
    list.iter()
        .any(|entry| entry.origin == origin && entry.scheme == scheme)
}

fn remember(app: &AppHandle, store: &Store, offer: &Offer) {
    let mut list = allowed(store);
    if is_allowed(&list, &offer.origin, &offer.scheme) {
        return;
    }
    list.push(AllowedApp {
        origin: offer.origin.clone(),
        scheme: offer.scheme.clone(),
        app: offer.app.clone(),
    });
    let value = serde_json::to_value(&list).unwrap_or(Value::Null);
    match store.set_setting(ALLOWED_SETTING, &value) {
        Ok(()) => {
            let _ = app.emit(
                "settings",
                serde_json::json!({ "key": ALLOWED_SETTING, "value": value }),
            );
        }
        Err(err) => tracing::warn!(%err, "разрешение открывать приложение не сохранено"),
    }
}

/// Страница открывает ссылку на приложение.
pub fn on_request(
    app: &AppHandle,
    window: &str,
    tab: u32,
    token: u64,
    uri: &str,
    origin: &str,
    user_initiated: bool,
) {
    // Событие пришло на главный поток, а решение читает базу и реестр.
    let (app, window, uri, origin) = (
        app.clone(),
        window.to_string(),
        uri.to_string(),
        origin.to_string(),
    );
    tauri::async_runtime::spawn_blocking(move || {
        decide(&app, &window, tab, token, &uri, &origin, user_initiated)
    });
}

fn decide(
    app: &AppHandle,
    window: &str,
    tab: u32,
    token: u64,
    uri: &str,
    origin: &str,
    user_initiated: bool,
) {
    let Some(scheme) = scheme_of(uri) else {
        tracing::info!("ссылка на приложение отклонена");
        return;
    };
    let state = app.state::<App>();
    // Приложения для схемы нет: Windows предложила бы искать его в магазине, а
    // сайт мог бы звать это окно сколько угодно. Как в Chrome — ничего не
    // открываем, только говорим, почему.
    if !registered(&scheme) {
        tracing::info!(%scheme, "для ссылки нет приложения");
        if user_initiated {
            let _ = app.emit_to(
                window,
                "notice",
                serde_json::json!({
                    "id": tab,
                    "text": format!("Для ссылок «{scheme}:» на компьютере нет приложения"),
                }),
            );
        }
        return;
    }
    // Вопрос этой вкладке уже на экране: страница, открывающая ссылку на
    // приложение в цикле, иначе засыпала бы человека одинаковыми окнами.
    // Щелчок человека по ссылке другой схемы — новый вопрос.
    let pending = state
        .external
        .0
        .lock()
        .iter()
        .filter(|((owner, _), _)| *owner == tab)
        .map(|(_, offer)| offer.scheme.clone())
        .collect::<Vec<_>>();
    if !pending.is_empty() && (!user_initiated || pending.contains(&scheme)) {
        tracing::debug!(%scheme, "вопрос о приложении уже открыт");
        return;
    }
    let origin = clean_origin(origin);
    // «Всегда» не распространяется на ссылки, которые страница открывает сама,
    // без щелчка: иначе сайт запускал бы приложение при каждой загрузке.
    if user_initiated && !origin.is_empty() && is_allowed(&allowed(&state.store), origin, &scheme) {
        launch(uri);
        return;
    }
    let name = app_name(&scheme).unwrap_or_default();
    let payload = serde_json::json!({
        "kind": "dialog",
        "id": tab,
        "token": token,
        "request": {
            "type": "external",
            "scheme": scheme,
            "origin": origin,
            "app": name,
            "remember": can_remember(origin),
        },
    });
    state.external.0.lock().insert(
        (tab, token),
        Offer {
            uri: uri.to_string(),
            scheme,
            origin: origin.to_string(),
            app: name,
        },
    );
    let _ = app.emit_to(window, "tab", payload);
}

/// Ответ на окно. `false` — окно не про приложение, ответ нужен движку.
pub fn answer(app: &AppHandle, state: &App, tab: u32, token: u64, answer: &DialogAnswer) -> bool {
    let Some(offer) = state.external.0.lock().remove(&(tab, token)) else {
        return false;
    };
    if answer.action == DialogAction::Accept {
        if answer.remember && can_remember(&offer.origin) {
            remember(app, &state.store, &offer);
        }
        launch(&offer.uri);
    }
    true
}

/// Вкладку закрыли — её вопросы больше никому не нужны.
pub fn forget_tab(state: &App, tab: u32) {
    state
        .external
        .0
        .lock()
        .retain(|(owner, _), _| *owner != tab);
}

/// Название приложения, которое Windows открывает для схемы.
fn app_name(scheme: &str) -> Option<String> {
    use windows::Win32::UI::Shell::ASSOCSTR_FRIENDLYAPPNAME;
    association(scheme, ASSOCSTR_FRIENDLYAPPNAME)
}

/// Есть ли в Windows приложение для схемы: команда запуска (обычная
/// программа), приложение из магазина (`AppUserModelID`) или хотя бы имя.
fn registered(scheme: &str) -> bool {
    use windows::Win32::UI::Shell::{ASSOCSTR_APPID, ASSOCSTR_COMMAND};
    association(scheme, ASSOCSTR_COMMAND).is_some()
        || association(scheme, ASSOCSTR_APPID).is_some()
        || app_name(scheme).is_some()
}

/// Сведения Windows о схеме ссылки: `None` — их нет.
fn association(scheme: &str, what: windows::Win32::UI::Shell::ASSOCSTR) -> Option<String> {
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Win32::UI::Shell::{AssocQueryStringW, ASSOCF_IS_PROTOCOL};

    let scheme = HSTRING::from(scheme);
    let mut buffer = [0u16; 1024];
    let mut len = buffer.len() as u32;
    let result = unsafe {
        AssocQueryStringW(
            ASSOCF_IS_PROTOCOL,
            what,
            &scheme,
            PCWSTR::null(),
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut len,
        )
    };
    if result.is_err() {
        return None;
    }
    // Длина — вместе с завершающим нулём.
    let end = (len as usize).min(buffer.len());
    let value = String::from_utf16_lossy(&buffer[..end]);
    let value = value.trim_end_matches('\0').trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Адрес для приложения — как его экранирует Chrome
/// (`EscapeExternalHandlerValue`): буквы, цифры, служебные символы адреса и
/// готовые `%XX` остаются, всё остальное — `%XX`. Пробел и кавычка внутри
/// адреса иначе разбили бы его на несколько аргументов у приложения, которое
/// зарегистрировано без кавычек вокруг `%1`.
fn escape_for_app(uri: &str) -> String {
    use std::fmt::Write as _;

    const KEEP: &[u8] = b";/?:@&=+$,!'()*-._~#[]";
    let bytes = uri.as_bytes();
    let mut out = String::with_capacity(uri.len());
    for (index, &byte) in bytes.iter().enumerate() {
        let escaped = byte == b'%'
            && bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
            && bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit);
        if byte.is_ascii_alphanumeric() || KEEP.contains(&byte) || escaped {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// Открыть ссылку приложением Windows. Отдельный поток: обработчик схемы бывает
/// COM-сервером и может думать долго, а окно браузера ждать не должно.
fn launch(uri: &str) {
    // Кавычки — чтобы приложение получило адрес одним аргументом.
    let quoted = format!("\"{}\"", escape_for_app(uri));
    std::thread::spawn(move || unsafe {
        use windows::core::{w, HSTRING, PCWSTR};
        use windows::Win32::System::Com::{
            CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
        };
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let result = ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(quoted),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        if result.0 as isize <= 32 {
            tracing::warn!(
                code = result.0 as isize,
                "приложение по ссылке не запустилось"
            );
        }
        if com.is_ok() {
            CoUninitialize();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_links_pass_and_dangerous_schemes_do_not() {
        assert_eq!(
            scheme_of("tg://resolve?domain=durov").as_deref(),
            Some("tg")
        );
        assert_eq!(scheme_of("MailTo:me@190x4.pw").as_deref(), Some("mailto"));
        assert_eq!(
            scheme_of("ms-settings:display").as_deref(),
            Some("ms-settings")
        );
        assert_eq!(scheme_of("https://t.me/durov"), None);
        assert_eq!(scheme_of("javascript:alert(1)"), None);
        assert_eq!(scheme_of("MS-MSDT:/id PCWDiagnostic"), None);
        assert_eq!(scheme_of("search-ms:query=x"), None);
        assert_eq!(scheme_of("1tg://x"), None);
        assert_eq!(scheme_of("no-colon"), None);
        assert_eq!(scheme_of(&format!("tg://{}", "a".repeat(MAX_URI))), None);
        assert_eq!(scheme_of("tg://x\ny"), None);
    }

    #[test]
    fn addresses_for_apps_are_escaped_like_chrome() {
        assert_eq!(
            escape_for_app("tg://resolve?domain=durov&start=1"),
            "tg://resolve?domain=durov&start=1"
        );
        // Пробел и кавычка больше не разбивают адрес на аргументы.
        assert_eq!(
            escape_for_app("app:x --flag \"y\""),
            "app:x%20--flag%20%22y%22"
        );
        // Готовые escape-последовательности остаются, одинокий % — нет.
        assert_eq!(escape_for_app("app:a%20b%zz"), "app:a%20b%25zz");
        assert_eq!(escape_for_app("app:я"), "app:%D1%8F");
        assert_eq!(escape_for_app("app:a|b^c<d>"), "app:a%7Cb%5Ec%3Cd%3E");
    }

    #[test]
    fn remembered_only_for_known_secure_sites() {
        assert_eq!(clean_origin("null"), "");
        assert_eq!(clean_origin("https://t.me"), "https://t.me");
        assert!(can_remember("https://t.me"));
        assert!(!can_remember("http://t.me"));
        assert!(!can_remember(""));
        let list = [AllowedApp {
            origin: "https://t.me".into(),
            scheme: "tg".into(),
            app: String::new(),
        }];
        assert!(is_allowed(&list, "https://t.me", "tg"));
        assert!(!is_allowed(&list, "https://t.me", "mailto"));
        assert!(!is_allowed(&list, "https://evil.example", "tg"));
    }
}
