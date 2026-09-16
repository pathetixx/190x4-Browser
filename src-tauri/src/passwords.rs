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

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use browser190x4_store::passwords_csv::{host_of, origin_of, same_site, shared_sign_in_sites};
use browser190x4_webview::{TabId, PAGES_HOST};
use parking_lot::Mutex;
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::{later, with_tab, App};
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

/// Учётки, показанные документу списком: выбрать можно только из них.
struct PageOffer {
    origin: String,
    ids: Vec<i64>,
}

#[derive(Default)]
pub struct Passwords {
    /// Окно каждой вкладки: предложение сохранить пароль должно прийти в то
    /// окно, где эта вкладка живёт, а не в первое попавшееся.
    windows: Mutex<HashMap<u32, String>>,
    /// Когда вкладка (или её фрейм) последний раз спрашивала учётки: страница
    /// может слать это сообщение в цикле, а нам хватает одного раза в секунду.
    asked: Mutex<HashMap<(u32, Option<u32>), Instant>>,
    /// Отправлено, но вход ещё не подтверждён.
    candidates: Mutex<HashMap<u32, Candidate>>,
    /// Показано предложение сохранить, ждём ответа пользователя.
    offers: Mutex<HashMap<u32, Candidate>>,
    /// Списки учёток по вкладке и фрейму.
    pages: Mutex<HashMap<(u32, Option<u32>), PageOffer>>,
    /// Когда вкладка последний раз начала навигацию. Сообщение страницы и
    /// начало навигации приходят разными путями, и порядок между ними не
    /// гарантирован: логин может приехать уже после ухода со страницы.
    navigations: Mutex<HashMap<u32, Instant>>,
}

/// Навигация раньше отправленного логина на столько — всё ещё «вход по нему».
const NAVIGATION_SLACK: Duration = Duration::from_millis(1200);

/// Чаще этого одна и та же форма учётки не запрашивает.
const ASK_EVERY: Duration = Duration::from_secs(1);

impl Passwords {
    /// Запомнить, в каком окне живёт вкладка.
    fn remember_window(&self, tab: u32, window: &str) {
        self.windows.lock().insert(tab, window.to_string());
    }

    /// Окно вкладки; если оно уже закрыто — первое окно браузера.
    fn window_of(&self, tab: u32) -> String {
        self.windows
            .lock()
            .get(&tab)
            .cloned()
            .unwrap_or_else(|| crate::browser_windows::FIRST.to_string())
    }

    /// Пустить ли запрос учёток от этой формы: защита от страницы, которая
    /// шлёт `password_form` в цикле.
    fn allow_form(&self, tab: u32, frame: Option<u32>) -> bool {
        let mut asked = self.asked.lock();
        let now = Instant::now();
        match asked.get(&(tab, frame)) {
            Some(at) if now.duration_since(*at) < ASK_EVERY => false,
            _ => {
                asked.insert((tab, frame), now);
                true
            }
        }
    }
}

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
    Form {
        /// Страница похожа на вход: единственную учётку можно подставить сразу.
        #[serde(default)]
        confident: bool,
    },
    #[serde(rename = "password_pick")]
    Pick { id: i64 },
    #[serde(rename = "password_manage")]
    Manage,
}

/// Сообщение со страницы или её фрейма (`frame`). `true` — оно наше и дальше
/// не идёт.
pub fn handle_message(
    app: &AppHandle,
    window: &str,
    tab: u32,
    frame: Option<u32>,
    source: &str,
    payload: &str,
) -> bool {
    let Ok(event) = serde_json::from_str::<PageEvent>(payload) else {
        return false;
    };
    // Приватное окно паролей не предлагает и не подставляет: его сеанс не
    // должен оставлять следов ни в базе, ни на странице.
    if app.state::<App>().windows.is_private(window) {
        return true;
    }
    app.state::<App>().passwords.remember_window(tab, window);
    tracing::debug!(tab, ?frame, %source, "сообщение менеджера паролей");
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
        PageEvent::Form { confident } => {
            // Сообщения страницы — недоверенный поток: сайт может слать их в
            // цикле. Поэтому работа уходит в пул задач, а не в новый поток на
            // каждое сообщение, и одна и та же форма не опрашивается чаще
            // раза в секунду.
            if !state.passwords.allow_form(tab, frame) {
                return true;
            }
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                offer_accounts(&app, tab, frame, &origin, confident)
            });
        }
        PageEvent::Pick { id } => {
            // Выбрать можно только учётку из списка, показанного этому документу.
            let offered = state
                .passwords
                .pages
                .lock()
                .get(&(tab, frame))
                .is_some_and(|offer| offer.origin == origin && offer.ids.contains(&id));
            if !offered {
                tracing::debug!(tab, ?frame, id, "учётки нет в списке документа");
                return true;
            }
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(err) = fill_offered(&app, tab, frame, &origin, id) {
                    tracing::warn!(%err, "учётка не подставлена");
                }
            });
        }
        PageEvent::Manage => {
            let _ = app.emit_to(
                state.passwords.window_of(tab).as_str(),
                "open-settings",
                serde_json::json!({ "section": "passwords" }),
            );
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
    state.passwords.pages.lock().retain(|(id, _), _| *id != tab);
    state.passwords.asked.lock().retain(|(id, _), _| *id != tab);
    state.passwords.windows.lock().remove(&tab);
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
    tauri::async_runtime::spawn_blocking(move || {
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
        let window = state.passwords.window_of(tab);
        let _ = app.emit_to(window.as_str(), "password-offer", payload);
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

/// На странице форма входа: список учёток — документу, ключ — в адресную
/// строку, единственную учётку своего сайта браузер подставляет сам.
///
/// В списке сначала учётки своего сайта, во фрейме — ещё и сайта вкладки (почта
/// Mail.ru входит во фрейме VK ID), затем сайтов с общим входом. Из нескольких
/// учёток выбирает человек, поэтому сам браузер подставляет только
/// единственную учётку своего сайта — и только на странице входа.
fn offer_accounts(app: &AppHandle, tab: u32, frame: Option<u32>, origin: &str, confident: bool) {
    let state = app.state::<App>();
    let store = &state.store;
    let own = store.password_secrets_for(origin).unwrap_or_default();
    let mut offered = own.clone();
    if frame.is_some() {
        if let Some(top) = tab_origin(app, tab).filter(|top| !same_site(top, origin)) {
            offered.extend(store.password_secrets_for(&top).unwrap_or_default());
        }
    }
    offered.extend(
        store
            .password_secrets_for_sites(shared_sign_in_sites(origin))
            .unwrap_or_default(),
    );
    let mut seen = HashSet::new();
    offered.retain(|account| seen.insert(account.id));
    tracing::debug!(tab, ?frame, %origin, own = own.len(), offered = offered.len(), "форма входа");
    if offered.is_empty() {
        return;
    }

    state.passwords.pages.lock().insert(
        (tab, frame),
        PageOffer {
            origin: origin.to_string(),
            ids: offered.iter().map(|account| account.id).collect(),
        },
    );
    post_to_page(
        app,
        tab,
        frame,
        serde_json::json!({
            "cmd": "password_accounts",
            "origin": origin,
            "theme": store.setting_str("theme").unwrap_or_default(),
            "accounts": offered
                .iter()
                .map(|account| {
                    serde_json::json!({
                        "id": account.id,
                        "username": account.username,
                        "host": host_of(&account.origin),
                    })
                })
                .collect::<Vec<_>>(),
        }),
    );

    if frame.is_none() {
        let _ = app.emit_to(
            state.passwords.window_of(tab).as_str(),
            "password-site",
            serde_json::json!({
                "tab": tab,
                "origin": origin,
                "accounts": offered
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
    }

    if confident && own.len() == 1 && store.setting_bool("passwords_autofill", true) {
        let account = &own[0];
        match vault::reveal(&account.secret) {
            // Сам браузер подставляет учётку только после того, как человек
            // тронул страницу: пароль не должен лежать в поле у страницы,
            // которую открыли и забыли.
            Ok(password) => fill_tab(app, tab, frame, origin, &account.username, password, true),
            Err(err) => tracing::warn!(%err, "пароль не расшифрован"),
        }
    }
}

/// Учётка из ключа в адресной строке — в документ вкладки.
pub fn fill(app: &AppHandle, tab: u32, id: i64) -> anyhow::Result<()> {
    let origin = app
        .state::<App>()
        .passwords
        .pages
        .lock()
        .get(&(tab, None))
        .filter(|offer| offer.ids.contains(&id))
        .map(|offer| offer.origin.clone())
        .ok_or_else(|| anyhow::anyhow!("учётки нет среди учёток страницы"))?;
    fill_offered(app, tab, None, &origin, id)
}

fn fill_offered(
    app: &AppHandle,
    tab: u32,
    frame: Option<u32>,
    origin: &str,
    id: i64,
) -> anyhow::Result<()> {
    let state = app.state::<App>();
    let (entry, secret) = state
        .store
        .password_secret(id)?
        .ok_or_else(|| anyhow::anyhow!("пароль удалён"))?;
    let password = vault::reveal(&secret)?;
    fill_tab(app, tab, frame, origin, &entry.username, password, false);
    Ok(())
}

/// Учётку — в документ вкладки или во фрейм. `origin` — адрес документа,
/// которому показали список: во вкладке он сверяется с документом прямо перед
/// отправкой, во фрейме — скриптом страницы (`location.origin`).
#[allow(clippy::too_many_arguments)]
fn fill_tab(
    app: &AppHandle,
    tab: u32,
    frame: Option<u32>,
    origin: &str,
    username: &str,
    password: String,
    auto: bool,
) {
    let origin = origin.to_string();
    let username = username.to_string();
    later(app, tab, move |host| {
        host.with_tab(TabId(tab), |view| {
            if frame.is_none() && origin_of(&view.source_url()).as_deref() != Some(origin.as_str())
            {
                tracing::debug!(tab, "вкладка ушла на другой адрес, форму не заполняем");
                return;
            }
            let message = serde_json::json!({
                "cmd": "password_fill",
                "origin": origin,
                "username": username,
                "password": password,
                // Автозаполнение страница придержит до первого касания.
                "auto": auto,
            });
            match view.post_to(frame, &message.to_string()) {
                Ok(()) => tracing::debug!(tab, ?frame, %origin, "учётка отправлена в форму"),
                Err(err) => tracing::warn!(%err, "форма не заполнена"),
            }
        });
    });
}

fn post_to_page(app: &AppHandle, tab: u32, frame: Option<u32>, message: serde_json::Value) {
    let json = message.to_string();
    later(app, tab, move |host| {
        host.with_tab(TabId(tab), |view| {
            if let Err(err) = view.post_to(frame, &json) {
                tracing::warn!(%err, "список учёток не отправлен");
            }
        });
    });
}

/// Адрес документа вкладки. Ждёт главный поток — звать только из фоновых.
fn tab_origin(app: &AppHandle, tab: u32) -> Option<String> {
    with_tab(app, tab, move |host| {
        host.with_tab(TabId(tab), |view| view.source_url())
    })
    .ok()
    .flatten()
    .and_then(|url| origin_of(&url))
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
            PageEvent::Form { confident: false }
        ));
        assert!(matches!(
            serde_json::from_str::<PageEvent>(
                r#"{"evt":"password_form","step":"login","confident":true}"#
            )
            .unwrap(),
            PageEvent::Form { confident: true }
        ));
        assert!(matches!(
            serde_json::from_str::<PageEvent>(r#"{"evt":"password_pick","id":7}"#).unwrap(),
            PageEvent::Pick { id: 7 }
        ));
        assert!(serde_json::from_str::<PageEvent>(r#"{"evt":"password_pick","id":"7"}"#).is_err());

        // Чужие сообщения — не наши: новая вкладка шлёт строку, загрузчик — media_found.
        assert!(serde_json::from_str::<PageEvent>(r#""{\"evt\":\"navigate\"}""#).is_err());
        assert!(serde_json::from_str::<PageEvent>(r#"{"evt":"media_found","url":"x"}"#).is_err());
        assert!(serde_json::from_str::<PageEvent>(r#"{"evt":"password_submit"}"#).is_err());
    }
}
