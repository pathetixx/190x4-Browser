//! Окна, которые просит страница: alert, confirm, prompt, «Покинуть сайт?»,
//! запросы разрешений, вход по паролю сайта и запуск приложения по ссылке.
//!
//! Окна движка — чужая деталь: свой вид, свои кнопки и никаких правил браузера.
//! Движок отдаёт запрос и ждёт ответа под отсрочкой, окно рисует всплывающее
//! окно chrome-а, ответ приходит в [`crate::Tab::dialog_done`] с тем же номером.
//! Запуск приложения движок отменяет сразу: приложение, если пользователь
//! согласится, запускает сам браузер (`src-tauri/src/external.rs`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2BasicAuthenticationRequestedEventArgs, ICoreWebView2Deferral,
    ICoreWebView2PermissionRequestedEventArgs, ICoreWebView2PermissionRequestedEventArgs3,
    ICoreWebView2PermissionSettingCollectionView, ICoreWebView2Profile4,
    ICoreWebView2ScriptDialogOpeningEventArgs, ICoreWebView2_10, ICoreWebView2_18,
    COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_AUTOPLAY,
    COREWEBVIEW2_PERMISSION_KIND_CAMERA, COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ,
    COREWEBVIEW2_PERMISSION_KIND_FILE_READ_WRITE, COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION,
    COREWEBVIEW2_PERMISSION_KIND_LOCAL_FONTS, COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
    COREWEBVIEW2_PERMISSION_KIND_MIDI_SYSTEM_EXCLUSIVE_MESSAGES,
    COREWEBVIEW2_PERMISSION_KIND_MULTIPLE_AUTOMATIC_DOWNLOADS,
    COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS, COREWEBVIEW2_PERMISSION_KIND_OTHER_SENSORS,
    COREWEBVIEW2_PERMISSION_KIND_UNKNOWN_PERMISSION,
    COREWEBVIEW2_PERMISSION_KIND_WINDOW_MANAGEMENT, COREWEBVIEW2_PERMISSION_STATE_ALLOW,
    COREWEBVIEW2_PERMISSION_STATE_DEFAULT, COREWEBVIEW2_PERMISSION_STATE_DENY,
    COREWEBVIEW2_SCRIPT_DIALOG_KIND_ALERT, COREWEBVIEW2_SCRIPT_DIALOG_KIND_BEFOREUNLOAD,
    COREWEBVIEW2_SCRIPT_DIALOG_KIND_CONFIRM, COREWEBVIEW2_SCRIPT_DIALOG_KIND_PROMPT,
};
use webview2_com::{
    take_pwstr, BasicAuthenticationRequestedEventHandler,
    GetNonDefaultPermissionSettingsCompletedHandler, LaunchingExternalUriSchemeEventHandler,
    PermissionRequestedEventHandler, ScriptDialogOpeningEventHandler,
    SetPermissionStateCompletedHandler,
};
use windows_core::{Interface, BOOL, HSTRING, PWSTR};

use crate::tab::{EventSink, TabEvent};

/// Текст окна страницы длиннее этого не нужен: окно не для чтения, а
/// бесконечная строка подвесила бы попап.
const TEXT_LIMIT: usize = 10_000;

/// Разрешения, о которых браузер спрашивает сам. Имя уходит в интерфейс и в
/// настройки. Вид, которого здесь нет (движок новее браузера), остаётся окну
/// движка: объяснить пользователю, о чём просит сайт, браузеру нечем.
const PERMISSIONS: [(COREWEBVIEW2_PERMISSION_KIND, &str); 12] = [
    (COREWEBVIEW2_PERMISSION_KIND_CAMERA, "camera"),
    (COREWEBVIEW2_PERMISSION_KIND_MICROPHONE, "microphone"),
    (COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION, "geolocation"),
    (COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS, "notifications"),
    (COREWEBVIEW2_PERMISSION_KIND_OTHER_SENSORS, "sensors"),
    (COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ, "clipboard"),
    (
        COREWEBVIEW2_PERMISSION_KIND_MULTIPLE_AUTOMATIC_DOWNLOADS,
        "downloads",
    ),
    (COREWEBVIEW2_PERMISSION_KIND_FILE_READ_WRITE, "files"),
    (COREWEBVIEW2_PERMISSION_KIND_AUTOPLAY, "autoplay"),
    (COREWEBVIEW2_PERMISSION_KIND_LOCAL_FONTS, "fonts"),
    (
        COREWEBVIEW2_PERMISSION_KIND_MIDI_SYSTEM_EXCLUSIVE_MESSAGES,
        "midi",
    ),
    (COREWEBVIEW2_PERMISSION_KIND_WINDOW_MANAGEMENT, "windows"),
];

fn permission_name(kind: COREWEBVIEW2_PERMISSION_KIND) -> Option<&'static str> {
    PERMISSIONS
        .iter()
        .find(|(known, _)| *known == kind)
        .map(|(_, name)| *name)
}

fn permission_kind(name: &str) -> Option<COREWEBVIEW2_PERMISSION_KIND> {
    PERMISSIONS
        .iter()
        .find(|(_, known)| *known == name)
        .map(|(kind, _)| *kind)
}

/// О чём просит страница. Все строки — со страницы или о ней: показывать как
/// текст, никогда как разметку.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DialogRequest {
    /// alert, confirm, prompt или beforeunload («Покинуть сайт?»).
    Script {
        kind: &'static str,
        /// Документ, который просит окно (может быть фреймом).
        url: String,
        message: String,
        default_text: String,
    },
    Permission {
        permission: &'static str,
        url: String,
        user_initiated: bool,
    },
    /// Сайт или прокси требует имя и пароль (HTTP Basic, NTLM).
    Auth { url: String },
    /// Ссылка на приложение: tg:, mailto: и прочие схемы Windows. Движок её уже
    /// отменил, ответа не ждёт.
    External {
        uri: String,
        /// Origin страницы, с которой пришла ссылка; пустой — неизвестен.
        origin: String,
        user_initiated: bool,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DialogAction {
    /// ОК, «Покинуть», «Войти», «Открыть».
    Accept,
    /// Отмена, Escape, крестик.
    #[default]
    Cancel,
    /// Разрешение: навсегда для сайта.
    Allow,
    /// Разрешение: только в этот раз.
    AllowOnce,
    /// Разрешение: запретить навсегда.
    Deny,
}

/// Ответ пользователя. Поля, которые окну не нужны, остаются пустыми.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct DialogAnswer {
    pub action: DialogAction,
    /// Текст ответа prompt.
    pub text: String,
    pub username: String,
    pub password: String,
    /// «Всегда разрешать» для запуска приложения.
    pub remember: bool,
    /// «Запретить странице показывать новые окна».
    pub suppress: bool,
}

/// Разрешение сайта, сохранённое в профиле движка.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PermissionSetting {
    pub permission: &'static str,
    pub origin: String,
    pub allowed: bool,
}

enum Pending {
    Script {
        args: ICoreWebView2ScriptDialogOpeningEventArgs,
        deferral: ICoreWebView2Deferral,
        leave: bool,
    },
    Permission {
        args: ICoreWebView2PermissionRequestedEventArgs,
        deferral: ICoreWebView2Deferral,
    },
    Auth {
        args: ICoreWebView2BasicAuthenticationRequestedEventArgs,
        deferral: ICoreWebView2Deferral,
    },
}

impl Pending {
    /// Записать ответ и отпустить движок. `Complete` — в любом случае, даже если
    /// сам ответ не записался: иначе страница так и висела бы на окне.
    fn resolve(self, answer: &DialogAnswer) -> windows_core::Result<()> {
        let applied = self.apply(answer);
        let deferral = match &self {
            Pending::Script { deferral, .. }
            | Pending::Permission { deferral, .. }
            | Pending::Auth { deferral, .. } => deferral,
        };
        let completed = unsafe { deferral.Complete() };
        applied.and(completed)
    }

    fn apply(&self, answer: &DialogAnswer) -> windows_core::Result<()> {
        unsafe {
            match self {
                Pending::Script { args, .. } => {
                    if answer.action == DialogAction::Accept {
                        // Текст нужен только prompt; остальные окна его не читают.
                        args.SetResultText(&HSTRING::from(limit(&answer.text)))?;
                        args.Accept()?;
                    }
                    Ok(())
                }
                Pending::Permission { args, .. } => {
                    let (state, save) = match answer.action {
                        DialogAction::Allow => (COREWEBVIEW2_PERMISSION_STATE_ALLOW, true),
                        DialogAction::AllowOnce => (COREWEBVIEW2_PERMISSION_STATE_ALLOW, false),
                        DialogAction::Deny => (COREWEBVIEW2_PERMISSION_STATE_DENY, true),
                        // Окно закрыли, не ответив: отказ только в этот раз.
                        // DEFAULT здесь нельзя — движок показал бы своё окно.
                        DialogAction::Accept | DialogAction::Cancel => {
                            (COREWEBVIEW2_PERMISSION_STATE_DENY, false)
                        }
                    };
                    if let Ok(args3) = args.cast::<ICoreWebView2PermissionRequestedEventArgs3>() {
                        args3.SetSavesInProfile(save)?;
                    }
                    args.SetState(state)
                }
                Pending::Auth { args, .. } => {
                    // Пустые имя и пароль движок понимает как «ответа нет» и
                    // показывает своё окно — такой ответ считаем отменой.
                    let empty = answer.username.is_empty() && answer.password.is_empty();
                    if answer.action == DialogAction::Accept && !empty {
                        let response = args.Response()?;
                        response.SetUserName(&HSTRING::from(answer.username.as_str()))?;
                        response.SetPassword(&HSTRING::from(answer.password.as_str()))
                    } else {
                        args.SetCancel(true)
                    }
                }
            }
        }
    }

    /// Окно, после ответа на которое клавиатура возвращается странице.
    fn takes_focus(&self) -> bool {
        !matches!(self, Pending::Permission { .. })
    }
}

/// Окна одной вкладки.
#[derive(Default)]
pub(crate) struct DialogState {
    next: Cell<u64>,
    pending: RefCell<HashMap<u64, Pending>>,
    /// «Запретить странице показывать новые окна» — до следующей навигации.
    suppressed: Cell<bool>,
}

pub(crate) type Dialogs = Rc<DialogState>;

impl DialogState {
    fn issue(&self) -> u64 {
        let token = self.next.get().wrapping_add(1);
        self.next.set(token);
        token
    }

    fn hold(&self, pending: Pending) -> u64 {
        let token = self.issue();
        self.pending.borrow_mut().insert(token, pending);
        token
    }
}

fn limit(text: &str) -> String {
    text.chars().take(TEXT_LIMIT).collect()
}

fn read(get: impl FnOnce(*mut PWSTR) -> windows_core::Result<()>) -> windows_core::Result<String> {
    let mut raw = PWSTR::null();
    get(&mut raw)?;
    Ok(take_pwstr(raw))
}

fn read_flag(
    get: impl FnOnce(*mut BOOL) -> windows_core::Result<()>,
) -> windows_core::Result<bool> {
    let mut value = BOOL::default();
    get(&mut value)?;
    Ok(value.as_bool())
}

/// Перехватить окна вкладки. Что не удалось перехватить, остаётся окнам движка.
pub(crate) fn wire(id: u32, core: &ICoreWebView2, sink: EventSink, dialogs: Dialogs) {
    if let Err(err) = wire_script(id, core, sink.clone(), dialogs.clone()) {
        tracing::warn!(%err, "окна страницы не перехвачены");
    }
    if let Err(err) = wire_permissions(id, core, sink.clone(), dialogs.clone()) {
        tracing::warn!(%err, "запросы разрешений не перехвачены");
    }
    match core.cast::<ICoreWebView2_10>() {
        Ok(core10) => {
            if let Err(err) = wire_auth(id, &core10, sink.clone(), dialogs.clone()) {
                tracing::warn!(%err, "вход на сайт по паролю не перехвачен");
            }
        }
        Err(_) => tracing::warn!("движок не отдаёт вход на сайт по паролю"),
    }
    match core.cast::<ICoreWebView2_18>() {
        Ok(core18) => {
            if let Err(err) = wire_external(id, &core18, sink, dialogs) {
                tracing::warn!(%err, "запуск приложений по ссылке не перехвачен");
            }
        }
        Err(_) => tracing::warn!("движок не отдаёт запуск приложений по ссылке"),
    }
}

fn wire_script(
    id: u32,
    core: &ICoreWebView2,
    sink: EventSink,
    dialogs: Dialogs,
) -> windows_core::Result<()> {
    let mut token = 0i64;
    unsafe {
        // Иначе движок покажет своё окно, не дожидаясь обработчика.
        core.Settings()?.SetAreDefaultScriptDialogsEnabled(false)?;
        core.add_ScriptDialogOpening(
            &ScriptDialogOpeningEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_SCRIPT_DIALOG_KIND_ALERT;
                args.Kind(&mut kind)?;
                let leave = kind == COREWEBVIEW2_SCRIPT_DIALOG_KIND_BEFOREUNLOAD;
                // Странице, которой окна запретили, ответ до следующей навигации
                // приходит сразу: alert закрыт, confirm и prompt отменены, уйти
                // со страницы можно.
                if dialogs.suppressed.get() {
                    if leave {
                        args.Accept()?;
                    }
                    return Ok(());
                }
                let request = DialogRequest::Script {
                    kind: match kind {
                        COREWEBVIEW2_SCRIPT_DIALOG_KIND_CONFIRM => "confirm",
                        COREWEBVIEW2_SCRIPT_DIALOG_KIND_PROMPT => "prompt",
                        COREWEBVIEW2_SCRIPT_DIALOG_KIND_BEFOREUNLOAD => "beforeunload",
                        _ => "alert",
                    },
                    url: read(|out| args.Uri(out))?,
                    message: limit(&read(|out| args.Message(out))?),
                    default_text: limit(&read(|out| args.DefaultText(out))?),
                };
                let deferral = args.GetDeferral()?;
                let token = dialogs.hold(Pending::Script {
                    args,
                    deferral,
                    leave,
                });
                sink(TabEvent::Dialog { id, token, request });
                Ok(())
            })),
            &mut token,
        )
    }
}

fn wire_permissions(
    id: u32,
    core: &ICoreWebView2,
    sink: EventSink,
    dialogs: Dialogs,
) -> windows_core::Result<()> {
    let mut token = 0i64;
    unsafe {
        core.add_PermissionRequested(
            &PermissionRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_PERMISSION_KIND_UNKNOWN_PERMISSION;
                args.PermissionKind(&mut kind)?;
                let Some(permission) = permission_name(kind) else {
                    return Ok(());
                };
                let request = DialogRequest::Permission {
                    permission,
                    url: read(|out| args.Uri(out))?,
                    user_initiated: read_flag(|out| args.IsUserInitiated(out))?,
                };
                let deferral = args.GetDeferral()?;
                let token = dialogs.hold(Pending::Permission { args, deferral });
                sink(TabEvent::Dialog { id, token, request });
                Ok(())
            })),
            &mut token,
        )
    }
}

fn wire_auth(
    id: u32,
    core10: &ICoreWebView2_10,
    sink: EventSink,
    dialogs: Dialogs,
) -> windows_core::Result<()> {
    let mut token = 0i64;
    unsafe {
        core10.add_BasicAuthenticationRequested(
            &BasicAuthenticationRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let request = DialogRequest::Auth {
                    url: read(|out| args.Uri(out))?,
                };
                let deferral = args.GetDeferral()?;
                let token = dialogs.hold(Pending::Auth { args, deferral });
                sink(TabEvent::Dialog { id, token, request });
                Ok(())
            })),
            &mut token,
        )
    }
}

fn wire_external(
    id: u32,
    core18: &ICoreWebView2_18,
    sink: EventSink,
    dialogs: Dialogs,
) -> windows_core::Result<()> {
    let mut token = 0i64;
    unsafe {
        core18.add_LaunchingExternalUriScheme(
            &LaunchingExternalUriSchemeEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                // Окно движка не нужно ни в каком случае: приложение, если
                // пользователь согласится, запускает сам браузер.
                args.SetCancel(true)?;
                let request = DialogRequest::External {
                    uri: read(|out| args.Uri(out))?,
                    origin: read(|out| args.InitiatingOrigin(out))?,
                    user_initiated: read_flag(|out| args.IsUserInitiated(out))?,
                };
                sink(TabEvent::Dialog {
                    id,
                    token: dialogs.issue(),
                    request,
                });
                Ok(())
            })),
            &mut token,
        )
    }
}

/// Ответ пользователя. `Some(true)` — окно было и клавиатуру можно вернуть
/// странице, `None` — окна с таким номером уже нет.
pub(crate) fn answer(
    dialogs: &DialogState,
    token: u64,
    answer: &DialogAnswer,
) -> windows_core::Result<Option<bool>> {
    let Some(pending) = dialogs.pending.borrow_mut().remove(&token) else {
        return Ok(None);
    };
    if answer.suppress {
        dialogs.suppressed.set(true);
    }
    let focus = pending.takes_focus();
    pending.resolve(answer)?;
    Ok(Some(focus))
}

/// Началась навигация: окна прежней страницы больше никто не ждёт. Отвечаем за
/// них отказом и возвращаем номера, чтобы chrome их убрал. «Покинуть сайт?» не
/// трогаем: навигация, ради которой оно показано, начнётся только после ответа.
pub(crate) fn navigation_started(dialogs: &DialogState) -> Vec<u64> {
    dialogs.suppressed.set(false);
    let stale: Vec<(u64, Pending)> = {
        let mut pending = dialogs.pending.borrow_mut();
        let tokens: Vec<u64> = pending
            .iter()
            .filter(|(_, item)| !matches!(item, Pending::Script { leave: true, .. }))
            .map(|(token, _)| *token)
            .collect();
        tokens
            .into_iter()
            .filter_map(|token| pending.remove(&token).map(|item| (token, item)))
            .collect()
    };
    // Движок отпускаем уже без заёма: ответ может сразу поднять новое событие.
    stale
        .into_iter()
        .map(|(token, item)| {
            if let Err(err) = item.resolve(&DialogAnswer::default()) {
                tracing::debug!(%err, "окно прежней страницы уже закрыто");
            }
            token
        })
        .collect()
}

/// Разрешения, которые пользователь дал или запретил сайтам.
pub(crate) fn permission_settings(
    profile: &ICoreWebView2Profile4,
    done: impl FnOnce(Vec<PermissionSetting>) + 'static,
) -> windows_core::Result<()> {
    let done = RefCell::new(Some(done));
    unsafe {
        profile.GetNonDefaultPermissionSettings(
            &GetNonDefaultPermissionSettingsCompletedHandler::create(Box::new(
                move |code, view| {
                    let list = code
                        .and_then(|()| read_settings(view))
                        .unwrap_or_else(|err| {
                            tracing::warn!(%err, "разрешения сайтов не прочитаны");
                            Vec::new()
                        });
                    if let Some(done) = done.borrow_mut().take() {
                        done(list);
                    }
                    Ok(())
                },
            )),
        )
    }
}

fn read_settings(
    view: Option<ICoreWebView2PermissionSettingCollectionView>,
) -> windows_core::Result<Vec<PermissionSetting>> {
    let Some(view) = view else {
        return Ok(Vec::new());
    };
    let mut count = 0u32;
    unsafe { view.Count(&mut count)? };
    let mut out = Vec::new();
    for index in 0..count {
        unsafe {
            let setting = view.GetValueAtIndex(index)?;
            let mut kind = COREWEBVIEW2_PERMISSION_KIND_UNKNOWN_PERMISSION;
            setting.PermissionKind(&mut kind)?;
            let mut state = COREWEBVIEW2_PERMISSION_STATE_DEFAULT;
            setting.PermissionState(&mut state)?;
            let Some(permission) = permission_name(kind) else {
                continue;
            };
            if state == COREWEBVIEW2_PERMISSION_STATE_DEFAULT {
                continue;
            }
            out.push(PermissionSetting {
                permission,
                origin: read(|out| setting.PermissionOrigin(out))?,
                allowed: state == COREWEBVIEW2_PERMISSION_STATE_ALLOW,
            });
        }
    }
    out.sort_by(|a, b| (&a.origin, a.permission).cmp(&(&b.origin, b.permission)));
    Ok(out)
}

/// Забыть решение: при следующем запросе сайт снова спросит.
pub(crate) fn permission_reset(
    profile: &ICoreWebView2Profile4,
    permission: &str,
    origin: &str,
    done: impl FnOnce(bool) + 'static,
) -> anyhow::Result<()> {
    let kind = permission_kind(permission)
        .ok_or_else(|| anyhow::anyhow!("неизвестное разрешение {permission}"))?;
    let done = RefCell::new(Some(done));
    unsafe {
        profile.SetPermissionState(
            kind,
            &HSTRING::from(origin),
            COREWEBVIEW2_PERMISSION_STATE_DEFAULT,
            &SetPermissionStateCompletedHandler::create(Box::new(move |code| {
                if let Err(err) = &code {
                    tracing::warn!(%err, "разрешение сайта не сброшено");
                }
                if let Some(done) = done.borrow_mut().take() {
                    done(code.is_ok());
                }
                Ok(())
            })),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_names_round_trip() {
        for (kind, name) in PERMISSIONS {
            assert_eq!(permission_name(kind), Some(name));
            assert_eq!(permission_kind(name), Some(kind));
        }
        assert_eq!(
            permission_name(COREWEBVIEW2_PERMISSION_KIND_UNKNOWN_PERMISSION),
            None
        );
    }

    #[test]
    fn answer_defaults_to_cancel() {
        let answer: DialogAnswer = serde_json::from_str(r#"{"action":"allow_once"}"#).unwrap();
        assert_eq!(answer.action, DialogAction::AllowOnce);
        assert!(answer.text.is_empty() && !answer.remember && !answer.suppress);
        assert_eq!(DialogAnswer::default().action, DialogAction::Cancel);
    }
}
