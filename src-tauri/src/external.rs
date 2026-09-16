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
    let Some(scheme) = scheme_of(uri) else {
        tracing::info!("ссылка на приложение отклонена");
        return;
    };
    let state = app.state::<App>();
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
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Win32::UI::Shell::{
        AssocQueryStringW, ASSOCF_IS_PROTOCOL, ASSOCSTR_FRIENDLYAPPNAME,
    };

    let scheme = HSTRING::from(scheme);
    let mut buffer = [0u16; 512];
    let mut len = buffer.len() as u32;
    let result = unsafe {
        AssocQueryStringW(
            ASSOCF_IS_PROTOCOL,
            ASSOCSTR_FRIENDLYAPPNAME,
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
    let name = String::from_utf16_lossy(&buffer[..end]);
    let name = name.trim_end_matches('\0').trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Открыть ссылку приложением Windows. Отдельный поток: обработчик схемы бывает
/// COM-сервером и может думать долго, а окно браузера ждать не должно.
fn launch(uri: &str) {
    // Кавычки — чтобы приложение получило адрес одним аргументом.
    let quoted = format!("\"{}\"", uri.replace('"', "%22"));
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
