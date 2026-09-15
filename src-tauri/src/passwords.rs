//! Менеджер паролей: решения на стороне Rust.
//!
//! Страница (скрипт `crates/webview/src/inject/passwords.js`) сообщает только
//! факты: «здесь форма входа», «отправили логин и пароль», «форма исчезла».
//! Всё остальное — здесь:
//!
//! * **от чьего имени пришло сообщение**, решает адрес документа по данным
//!   движка, а не текст сообщения — чужой сайт не может записать пароль на
//!   имя банка;
//! * **предложение сохранить** появляется только после подтверждения входа:
//!   навигация или исчезнувшая форма. Неверный пароль, после которого форма
//!   осталась на месте, сохранять не предлагаем;
//! * **пароль в открытом виде** не попадает ни в базу (DPAPI), ни в интерфейс
//!   браузера — туда уходят только сайт и логин;
//! * **учётки сайта** — сохранённые для этого адреса и для других адресов того
//!   же сайта по https (`google.com` на `accounts.google.com`), свой адрес первым.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use browser190x4_store::passwords_csv::{origin_of, same_site};
use browser190x4_webview::{TabId, PAGES_HOST};
use parking_lot::Mutex;
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::{with_host_later, App};
use crate::vault;

/// Отправленный логин ждёт подтверждения входа не дольше этого.
const CANDIDATE_TTL: Duration = Duration::from_secs(60);

struct Candidate {
    origin: String,
    username: String,
    password: String,
    at: Instant,
    /// Учётка того же сайта с этим логином, но другим паролем: «Обновить»
    /// меняет её, а не заводит копию под новым адресом.
    replaces: Option<i64>,
}

#[derive(Default)]
pub struct Passwords {
    /// Отправлено, но вход ещё не подтверждён.
    candidates: Mutex<HashMap<u32, Candidate>>,
    /// Показано предложение сохранить, ждём ответа пользователя.
    offers: Mutex<HashMap<u32, Candidate>>,
    /// Когда вкладка последний раз начала навигацию. Сообщение страницы и
    /// начало навигации приходят разными путями, и порядок между ними не
    /// гарантирован: логин может приехать уже после ухода со страницы.
    navigations: Mutex<HashMap<u32, Instant>>,
}

/// Навигация раньше отправленного логина на столько — всё ещё «вход по нему».
const NAVIGATION_SLACK: Duration = Duration::from_millis(1200);

#[derive(Deserialize)]
#[serde(tag = "evt")]
enum PageEvent {
    #[serde(rename = "password_submit")]
    Submit {
        #[serde(default)]
        username: String,
        password: String,
    },
    #[serde(rename = "password_commit")]
    Commit,
    #[serde(rename = "password_form")]
    Form,
}

/// Сообщение со страницы. `true` — оно наше и дальше не идёт.
pub fn handle_message(app: &AppHandle, tab: u32, source: &str, payload: &str) -> bool {
    let Ok(event) = serde_json::from_str::<PageEvent>(payload) else {
        return false;
    };
    tracing::debug!(tab, %source, "сообщение менеджера паролей");
    let Some(origin) = origin_of(source).filter(|origin| !origin.ends_with(PAGES_HOST)) else {
        return true;
    };
    let state = app.state::<App>();

    match event {
        PageEvent::Submit { username, password } => {
            if password.is_empty() || password.len() > 1024 || username.len() > 512 {
                return true;
            }
            state.passwords.candidates.lock().insert(
                tab,
                Candidate {
                    origin,
                    username,
                    password,
                    at: Instant::now(),
                    replaces: None,
                },
            );
            let navigated = state
                .passwords
                .navigations
                .lock()
                .get(&tab)
                .is_some_and(|at| at.elapsed() < NAVIGATION_SLACK);
            if navigated {
                commit(app, tab);
            }
        }
        PageEvent::Commit => commit(app, tab),
        PageEvent::Form => {
            let app = app.clone();
            std::thread::spawn(move || offer_accounts(&app, tab, &origin));
        }
    }
    true
}

/// Вкладка ушла на другой документ — если перед этим отправили логин, вход,
/// скорее всего, удался.
pub fn on_navigation(app: &AppHandle, tab: u32) {
    app.state::<App>()
        .passwords
        .navigations
        .lock()
        .insert(tab, Instant::now());
    commit(app, tab);
}

pub fn forget_tab(app: &AppHandle, tab: u32) {
    let state = app.state::<App>();
    state.passwords.candidates.lock().remove(&tab);
    state.passwords.offers.lock().remove(&tab);
    state.passwords.navigations.lock().remove(&tab);
}

fn commit(app: &AppHandle, tab: u32) {
    let state = app.state::<App>();
    let Some(candidate) = state.passwords.candidates.lock().remove(&tab) else {
        return;
    };
    if candidate.at.elapsed() > CANDIDATE_TTL {
        return;
    }
    tracing::debug!(tab, origin = %candidate.origin, "вход подтверждён, решаем про пароль");

    let app = app.clone();
    std::thread::spawn(move || {
        let state = app.state::<App>();
        let store = &state.store;
        if !store.setting_bool("passwords_offer", true)
            || store.is_password_never(&candidate.origin).unwrap_or(false)
        {
            return;
        }

        let existing = store
            .password_secrets_for(&candidate.origin)
            .unwrap_or_default()
            .into_iter()
            .find(|saved| saved.username == candidate.username);

        let mut candidate = candidate;
        let update = match existing {
            Some(saved) => match vault::reveal(&saved.secret) {
                Ok(password) if password == candidate.password => {
                    let _ = store.touch_password(saved.id);
                    return;
                }
                _ => {
                    candidate.replaces = Some(saved.id);
                    true
                }
            },
            None => false,
        };

        let payload = serde_json::json!({
            "tab": tab,
            "origin": candidate.origin,
            "username": candidate.username,
            "update": update,
        });
        {
            // Логин приходит дважды — при отправке формы и при уходе со
            // страницы: то же предложение второй раз не показываем.
            let mut offers = state.passwords.offers.lock();
            if offers.get(&tab).is_some_and(|offered| {
                offered.origin == candidate.origin
                    && offered.username == candidate.username
                    && offered.password == candidate.password
            }) {
                return;
            }
            offers.insert(tab, candidate);
        }
        tracing::debug!(tab, update, "предлагаем сохранить пароль");
        let _ = app.emit_to("chrome", "password-offer", payload);
    });
}

/// Ответ на предложение: `save`, `never` или `dismiss`.
pub fn answer(app: &AppHandle, tab: u32, action: &str) -> anyhow::Result<()> {
    let state = app.state::<App>();
    let Some(candidate) = state.passwords.offers.lock().remove(&tab) else {
        return Ok(());
    };
    match action {
        "save" => {
            let secret = vault::protect(&candidate.password)?;
            let id = match candidate.replaces {
                Some(id) => {
                    state
                        .store
                        .update_password(id, &candidate.username, Some(&secret))?;
                    id
                }
                None => {
                    state
                        .store
                        .save_password(&candidate.origin, &candidate.username, &secret)?
                }
            };
            state.store.touch_password(id)?;
            let _ = app.emit("passwords", ());
        }
        "never" => state.store.never_save_password(&candidate.origin)?,
        _ => {}
    }
    Ok(())
}

/// На странице есть форма входа: показать ключ в адресной строке и, если
/// разрешено, заполнить свежей учёткой.
fn offer_accounts(app: &AppHandle, tab: u32, origin: &str) {
    let state = app.state::<App>();
    let accounts = state.store.password_secrets_for(origin).unwrap_or_default();
    tracing::debug!(tab, %origin, accounts = accounts.len(), "форма входа");
    if accounts.is_empty() {
        return;
    }

    let _ = app.emit_to(
        "chrome",
        "password-site",
        serde_json::json!({
            "tab": tab,
            "origin": origin,
            "accounts": accounts
                .iter()
                .map(|account| {
                    serde_json::json!({
                        "id": account.id,
                        "username": account.username,
                        "origin": account.origin,
                    })
                })
                .collect::<Vec<_>>(),
        }),
    );

    if state.store.setting_bool("passwords_autofill", true) {
        let first = &accounts[0];
        match vault::reveal(&first.secret) {
            Ok(password) => fill_tab(app, tab, &first.origin, &first.username, password),
            Err(err) => tracing::warn!(%err, "пароль не расшифрован"),
        }
    }
}

/// Заполнить форму выбранной учёткой (ключ в адресной строке → логин).
pub fn fill(app: &AppHandle, tab: u32, id: i64) -> anyhow::Result<()> {
    let state = app.state::<App>();
    let (entry, secret) = state
        .store
        .password_secret(id)?
        .ok_or_else(|| anyhow::anyhow!("пароль удалён"))?;
    let password = vault::reveal(&secret)?;
    fill_tab(app, tab, &entry.origin, &entry.username, password);
    Ok(())
}

/// `saved` — адрес, для которого сохранена учётка: документ вкладки должен
/// быть тем же сайтом.
fn fill_tab(app: &AppHandle, tab: u32, saved: &str, username: &str, password: String) {
    let saved = saved.to_string();
    let username = username.to_string();
    with_host_later(app, move |host| {
        host.with_tab(TabId(tab), |view| {
            // Проверяем адрес документа прямо перед отправкой: пока мы ходили
            // в базу, вкладка могла уйти на другой сайт.
            let Some(origin) = origin_of(&view.source_url()) else {
                return;
            };
            if !same_site(&origin, &saved) {
                tracing::debug!(tab, %origin, "вкладка ушла на другой сайт, форму не заполняем");
                return;
            }
            let message = serde_json::json!({
                "cmd": "password_fill",
                "origin": origin,
                "username": username,
                "password": password,
            });
            match view.post(&message.to_string()) {
                Ok(()) => tracing::debug!(tab, %origin, "учётка отправлена в форму"),
                Err(err) => tracing::warn!(%err, "форма не заполнена"),
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::PageEvent;

    #[test]
    fn page_messages_are_parsed_strictly() {
        let submit: PageEvent = serde_json::from_str(
            r#"{"evt":"password_submit","username":"tomsmith","password":"SuperSecretPassword!"}"#,
        )
        .unwrap();
        assert!(
            matches!(submit, PageEvent::Submit { ref username, ref password } if username == "tomsmith" && password == "SuperSecretPassword!")
        );
        assert!(matches!(
            serde_json::from_str::<PageEvent>(r#"{"evt":"password_commit"}"#).unwrap(),
            PageEvent::Commit
        ));
        assert!(matches!(
            serde_json::from_str::<PageEvent>(r#"{"evt":"password_form"}"#).unwrap(),
            PageEvent::Form
        ));

        // Чужие сообщения — не наши: новая вкладка шлёт строку, загрузчик — media_found.
        assert!(serde_json::from_str::<PageEvent>(r#""{\"evt\":\"navigate\"}""#).is_err());
        assert!(serde_json::from_str::<PageEvent>(r#"{"evt":"media_found","url":"x"}"#).is_err());
        assert!(serde_json::from_str::<PageEvent>(r#"{"evt":"password_submit"}"#).is_err());
    }
}
