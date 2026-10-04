//! Менеджер паролей: решения на стороне Rust.
//!
//! Страница (скрипт `crates/webview/src/inject/passwords.js`) сообщает только
//! факты: «здесь форма входа», «отправили логин и пароль», «форма исчезла»,
//! «человек встал в поле логина». Всё остальное — здесь:
//!
//! * **от чьего имени пришло сообщение**, решает адрес документа по данным
//!   движка, а не текст сообщения — чужой сайт не может записать пароль на
//!   имя банка;
//! * **учётку выбирает человек в окне браузера**, а не на странице. Канал
//!   `chrome.webview` открыт любому скрипту документа, поэтому список учёток
//!   странице не уходит вовсе, а «выбор» со страницы не принимается: иначе
//!   чужой скрипт или рекламный фрейм получал бы пароль одним сообщением;
//! * **фрейму — только учётки его сайта**: рекламный фрейм на странице банка
//!   учёток банка не увидит;
//! * **предложение сохранить** появляется только после подтверждения входа:
//!   навигация или исчезнувшая форма. Неверный пароль, после которого форма
//!   осталась на месте, сохранять не предлагаем;
//! * **пароль в открытом виде** не попадает ни в базу (DPAPI), ни в интерфейс
//!   браузера — туда уходят только сайт и логин;
//! * **учётки сайта** — сохранённые для этого адреса и для других адресов того
//!   же сайта по https (`google.com` на `accounts.google.com`), свой адрес первым.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use browser190x4_store::passwords_csv::{origin_of, shared_sign_in_sites};
use browser190x4_webview::{TabId, PAGES_HOST};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::state::{later, App};
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

/// Учётка в списке окна браузера: логин и адрес, без пароля.
#[derive(Clone, Serialize)]
struct Account {
    id: i64,
    username: String,
    origin: String,
}

/// Учётки, найденные для документа: подставить можно только одну из них.
struct PageOffer {
    origin: String,
    accounts: Vec<Account>,
    /// Показывать список, как только человек встаёт в поле (`passwords_autofill`);
    /// без этого — только по стрелке вниз в поле.
    suggest: bool,
}

/// Поле логина в документе: прямоугольник от левого верхнего угла его окна, в
/// CSS-пикселях.
#[derive(Clone, Copy, Deserialize, Serialize)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Rect {
    /// Прямоугольник от недоверенной страницы: конечные числа в разумных пределах.
    fn sane(self) -> Option<Self> {
        const LIMIT: f64 = 100_000.0;
        let ok = |value: f64| value.is_finite() && value.abs() <= LIMIT;
        (ok(self.x) && ok(self.y) && ok(self.width) && ok(self.height)).then_some(Self {
            width: self.width.max(0.0),
            height: self.height.max(0.0),
            ..self
        })
    }
}

/// Человек встал в поле раньше, чем нашлись учётки документа: список покажем,
/// когда они найдутся, если человек всё ещё там.
struct PendingMenu {
    window: String,
    rect: Option<Rect>,
    keyboard: bool,
    at: Instant,
}

/// Просьба показать список дольше этого не ждёт учёток.
const MENU_WAIT: Duration = Duration::from_secs(3);

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
    /// Учётки по вкладке и фрейму.
    pages: Mutex<HashMap<(u32, Option<u32>), PageOffer>>,
    /// Просьбы показать список, которые ждут учёток документа.
    menus: Mutex<HashMap<(u32, Option<u32>), PendingMenu>>,
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

    /// Документ вкладки или фрейма сменился: его учётки и просьбы — в прошлом.
    fn forget_document(&self, tab: u32, frame: Option<u32>) {
        let gone =
            |(id, slot): &(u32, Option<u32>)| *id == tab && (frame.is_none() || *slot == frame);
        self.pages.lock().retain(|key, _| !gone(key));
        self.menus.lock().retain(|key, _| !gone(key));
        self.asked.lock().retain(|key, _| !gone(key));
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
    Form,
    /// Человек встал в поле логина или пароля (`keyboard` — стрелкой вниз):
    /// показать учётки списком окна браузера.
    #[serde(rename = "password_menu")]
    Menu {
        #[serde(default)]
        rect: Option<Rect>,
        #[serde(default)]
        keyboard: bool,
    },
    /// Человек печатает в поле, щёлкнул мимо или страница ушла — список не нужен.
    #[serde(rename = "password_menu_close")]
    MenuClose,
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
        PageEvent::Form => {
            // Сообщения страницы — недоверенный поток: сайт может слать их в
            // цикле. Поэтому работа уходит в пул задач, а не в новый поток на
            // каждое сообщение, и одна и та же форма не опрашивается чаще
            // раза в секунду.
            if !state.passwords.allow_form(tab, frame) {
                return true;
            }
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || offer_accounts(&app, tab, frame, &origin));
        }
        PageEvent::Menu { rect, keyboard } => {
            let rect = rect.and_then(Rect::sane);
            let offer = state
                .passwords
                .pages
                .lock()
                .get(&(tab, frame))
                .filter(|offer| offer.origin == origin)
                .map(|offer| (offer.accounts.clone(), offer.suggest));
            match offer {
                Some((accounts, suggest)) => {
                    if keyboard || suggest {
                        show_menu(app, window, tab, frame, &origin, &accounts, rect, keyboard);
                    }
                }
                // Учётки документа ещё ищутся: список покажем, когда найдутся.
                None => {
                    state.passwords.menus.lock().insert(
                        (tab, frame),
                        PendingMenu {
                            window: window.to_string(),
                            rect,
                            keyboard,
                            at: Instant::now(),
                        },
                    );
                }
            }
        }
        PageEvent::MenuClose => {
            state.passwords.menus.lock().remove(&(tab, frame));
            let _ = app.emit_to(
                window,
                "password-menu-close",
                serde_json::json!({ "tab": tab, "frame": frame }),
            );
        }
    }
    true
}

/// Список учёток у поля — окну браузера. У документа вкладки он встаёт под
/// полем (`rect`); у фрейма место поля в окне неизвестно, и список встаёт у
/// курсора (`point`, CSS-пиксели окна).
#[allow(clippy::too_many_arguments)]
fn show_menu(
    app: &AppHandle,
    window: &str,
    tab: u32,
    frame: Option<u32>,
    origin: &str,
    accounts: &[Account],
    rect: Option<Rect>,
    keyboard: bool,
) {
    if accounts.is_empty() {
        return;
    }
    let point = if frame.is_some() || rect.is_none() {
        cursor_point(app, window)
    } else {
        None
    };
    let _ = app.emit_to(
        window,
        "password-menu",
        serde_json::json!({
            "tab": tab,
            "frame": frame,
            "origin": origin,
            "accounts": accounts,
            "rect": if frame.is_none() { rect } else { None },
            "point": point,
            "focus": keyboard,
        }),
    );
}

/// Курсор в CSS-пикселях окна браузера.
fn cursor_point(app: &AppHandle, window: &str) -> Option<serde_json::Value> {
    let window = app.get_webview_window(window)?;
    let cursor = window.cursor_position().ok()?;
    let origin = window.inner_position().ok()?;
    let scale = window.scale_factor().ok()?;
    Some(serde_json::json!({
        "x": (cursor.x - f64::from(origin.x)) / scale,
        "y": (cursor.y - f64::from(origin.y)) / scale,
    }))
}

/// Вкладка ушла на другой документ — если перед этим отправили логин, вход,
/// скорее всего, удался. Учётки прежнего документа и его фреймов больше не
/// подставляются.
pub fn on_navigation(app: &AppHandle, tab: u32) {
    let state = app.state::<App>();
    state
        .passwords
        .navigations
        .lock()
        .insert(tab, Instant::now());
    state.passwords.forget_document(tab, None);
    commit(app, tab);
}

/// Фрейм вкладки ушёл на другой адрес: учётки, найденные для прежнего, в него
/// уже не уйдут.
pub fn on_frame_navigation(app: &AppHandle, tab: u32, frame: u32) {
    app.state::<App>()
        .passwords
        .forget_document(tab, Some(frame));
}

pub fn forget_tab(app: &AppHandle, tab: u32) {
    let state = app.state::<App>();
    state.passwords.candidates.lock().remove(&tab);
    state.passwords.offers.lock().remove(&tab);
    state.passwords.navigations.lock().remove(&tab);
    state.passwords.forget_document(tab, None);
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

/// На странице форма входа: найти учётки документа и запомнить их. Документ
/// списка не получает — его покажет окно браузера, когда человек встанет в
/// поле, а ключ в адресной строке появится сразу.
///
/// В списке сначала учётки своего сайта, затем сайтов с общим входом (почта
/// Mail.ru входит во фрейме VK ID). Учётки сайта вкладки фрейму чужого сайта
/// не предлагаются: так рекламный фрейм на странице банка увидел бы логины
/// банка.
fn offer_accounts(app: &AppHandle, tab: u32, frame: Option<u32>, origin: &str) {
    let state = app.state::<App>();
    let store = &state.store;
    let mut offered = store.password_secrets_for(origin).unwrap_or_default();
    offered.extend(
        store
            .password_secrets_for_sites(shared_sign_in_sites(origin))
            .unwrap_or_default(),
    );
    let mut seen = HashSet::new();
    offered.retain(|account| seen.insert(account.id));
    tracing::debug!(tab, ?frame, offered = offered.len(), "форма входа");
    if offered.is_empty() {
        state.passwords.menus.lock().remove(&(tab, frame));
        return;
    }

    let accounts: Vec<Account> = offered
        .into_iter()
        .map(|account| Account {
            id: account.id,
            username: account.username,
            origin: account.origin,
        })
        .collect();
    state.passwords.pages.lock().insert(
        (tab, frame),
        PageOffer {
            origin: origin.to_string(),
            accounts: accounts.clone(),
            suggest: store.setting_bool("passwords_autofill", true),
        },
    );

    if frame.is_none() {
        let _ = app.emit_to(
            state.passwords.window_of(tab).as_str(),
            "password-site",
            serde_json::json!({ "tab": tab, "origin": origin, "accounts": accounts }),
        );
    }

    // Человек уже стоит в поле — список сейчас, а не по второму щелчку.
    let pending = state
        .passwords
        .menus
        .lock()
        .remove(&(tab, frame))
        .filter(|menu| menu.at.elapsed() < MENU_WAIT);
    if let Some(menu) = pending {
        let suggest = store.setting_bool("passwords_autofill", true);
        if menu.keyboard || suggest {
            show_menu(
                app,
                &menu.window,
                tab,
                frame,
                origin,
                &accounts,
                menu.rect,
                menu.keyboard,
            );
        }
    }
}

/// Учётка, которую человек выбрал в окне браузера, — в документ вкладки или
/// во фрейм, которому её показали.
pub fn fill(app: &AppHandle, tab: u32, frame: Option<u32>, id: i64) -> anyhow::Result<()> {
    let origin = app
        .state::<App>()
        .passwords
        .pages
        .lock()
        .get(&(tab, frame))
        .filter(|offer| offer.accounts.iter().any(|account| account.id == id))
        .map(|offer| offer.origin.clone())
        .ok_or_else(|| anyhow::anyhow!("учётки нет среди учёток страницы"))?;
    let state = app.state::<App>();
    let (entry, secret) = state
        .store
        .password_secret(id)?
        .ok_or_else(|| anyhow::anyhow!("пароль удалён"))?;
    let password = vault::reveal(&secret)?;
    let _ = state.store.touch_password(id);
    fill_tab(app, tab, frame, origin, entry.username, password);
    Ok(())
}

/// Учётку — в документ вкладки или во фрейм. `origin` — адрес документа,
/// которому показали список: во вкладке он сверяется с документом прямо перед
/// отправкой, во фрейме — скриптом страницы (`location.origin`), а сменившийся
/// документ фрейма свои учётки уже потерял (`on_frame_navigation`).
fn fill_tab(
    app: &AppHandle,
    tab: u32,
    frame: Option<u32>,
    origin: String,
    username: String,
    password: String,
) {
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
            });
            match view.post_to(frame, &message.to_string()) {
                Ok(()) => tracing::debug!(tab, ?frame, %origin, "учётка отправлена в форму"),
                Err(err) => tracing::warn!(%err, "форма не заполнена"),
            }
            // Список забирал клавиатуру: после выбора она — у страницы, и
            // Enter сразу отправит форму.
            if view.visible() {
                view.focus();
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::{PageEvent, Rect};

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
        assert!(matches!(
            serde_json::from_str::<PageEvent>(
                r#"{"evt":"password_form","step":"login","confident":true}"#
            )
            .unwrap(),
            PageEvent::Form
        ));
        assert!(matches!(
            serde_json::from_str::<PageEvent>(
                r#"{"evt":"password_menu","rect":{"x":10,"y":20,"width":200,"height":30},"keyboard":true}"#
            )
            .unwrap(),
            PageEvent::Menu { rect: Some(_), keyboard: true }
        ));
        assert!(matches!(
            serde_json::from_str::<PageEvent>(r#"{"evt":"password_menu_close"}"#).unwrap(),
            PageEvent::MenuClose
        ));

        // Выбора учётки со страницы больше нет: такое сообщение не наше.
        assert!(serde_json::from_str::<PageEvent>(r#"{"evt":"password_pick","id":7}"#).is_err());
        assert!(serde_json::from_str::<PageEvent>(r#"{"evt":"password_manage"}"#).is_err());
        // Чужие сообщения — не наши: новая вкладка шлёт строку, загрузчик — media_found.
        assert!(serde_json::from_str::<PageEvent>(r#""{\"evt\":\"navigate\"}""#).is_err());
        assert!(serde_json::from_str::<PageEvent>(r#"{"evt":"media_found","url":"x"}"#).is_err());
        assert!(serde_json::from_str::<PageEvent>(r#"{"evt":"password_submit"}"#).is_err());
    }

    #[test]
    fn page_rectangles_are_checked() {
        let rect = |x: f64, width: f64| Rect {
            x,
            y: 0.0,
            width,
            height: 10.0,
        };
        assert!(rect(10.0, 100.0).sane().is_some());
        assert!(rect(f64::NAN, 100.0).sane().is_none());
        assert!(rect(f64::INFINITY, 100.0).sane().is_none());
        assert!(rect(1e9, 100.0).sane().is_none());
        assert_eq!(rect(10.0, -5.0).sane().map(|rect| rect.width), Some(0.0));
    }
}
