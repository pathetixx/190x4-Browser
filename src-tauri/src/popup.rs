//! Всплывающие окна chrome-а: меню, пузырь загрузок, правка закладки,
//! расширение-загрузчик, предложение сохранить пароль, окна страниц.
//!
//! # Почему отдельное окно, а не HTML-слой
//!
//! Страница — нативная поверхность поверх chrome-вебвью. Любой HTML-попап,
//! заехавший на её область, окажется под ней. Спрятать страницу на время
//! попапа (overlay-режим палитры) для меню и пузырей нельзя: страница
//! исчезала бы ровно тогда, когда пользователь с ней работает. Поэтому
//! попап — собственное маленькое окно, принадлежащее главному: так же
//! устроены пузыри в самом Chrome.
//!
//! Окно одно **на каждое окно браузера** и создаётся заранее: первый показ
//! меню не должен ждать запуска вебвью, а меню второго окна не должно
//! выпрыгивать поверх первого.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::Value;
use tauri::window::Color;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

/// Окно, которое просит страница: alert, запрос разрешения, вход на сайт.
pub const DIALOG: &str = "dialog";

/// Номер показа попапа. Закрытие меню и открытие следующего попапа (меню →
/// «Закрыть браузер?») идут разными путями, и без номера запоздавшее «меню
/// закрыто» принималось за закрытие нового попапа: подтверждение отвечало
/// «нет» само, меню страницы переставало выполнять команды.
static NEXT_SEQ: AtomicU64 = AtomicU64::new(1);

/// Ярлык попапа своего окна браузера.
pub fn label_for(owner: &str) -> String {
    format!("popup--{owner}")
}

/// Чей это попап: ярлык окна браузера, которому он принадлежит.
pub fn owner_of(label: &str) -> Option<&str> {
    label.strip_prefix("popup--")
}

/// Состояние попапов всех окон.
#[derive(Default)]
pub struct Popup(Mutex<HashMap<String, State>>);

impl Popup {
    fn with<R>(&self, owner: &str, f: impl FnOnce(&mut State) -> R) -> R {
        let mut map = self.0.lock();
        f(map.entry(owner.to_string()).or_default())
    }

    fn forget(&self, owner: &str) {
        self.0.lock().remove(owner);
    }
}

#[derive(Default)]
struct State {
    /// Что рисовать: окно могло ещё не загрузиться к моменту первого вызова.
    pending: Option<Value>,
    width: f64,
    /// Где кончается окно браузера — ниже попап не растёт.
    max_height: f64,
    /// Вид и место последнего показа: тот же попап на том же месте
    /// перерисовывается без переезда окна.
    placement: Option<(String, i32, i32, i64)>,
    /// Меню у точки щелчка (`align = "point"`): высота точки в окне браузера.
    point: Option<f64>,
    /// Окно страницы и где было окно браузера, когда его показали. Такое окно
    /// не закрывается от потери фокуса и едет вместе с окном браузера:
    /// страница ждёт ответа.
    sticky: Option<PhysicalPosition<i32>>,
    /// Номер текущего показа (`NEXT_SEQ`). Тот же попап на том же месте
    /// (подсказки на каждую клавишу) номер не меняет.
    seq: u64,
    /// Показ, который закрыли раньше, чем попап успел появиться: подсказки
    /// адресной строки, по которым уже нажали Enter.
    cancelled: Option<u64>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Anchor {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Создать окно попапа для окна браузера, если его ещё нет.
pub fn ensure(app: &AppHandle, owner: &str) -> tauri::Result<WebviewWindow> {
    let label = label_for(owner);
    if let Some(window) = app.get_webview_window(&label) {
        return Ok(window);
    }
    let main = app
        .get_webview_window(owner)
        .ok_or(tauri::Error::WindowNotFound)?;

    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("popup.html".into()))
        .additional_browser_args(crate::browser_args())
        .title("190x4")
        .decorations(false)
        .resizable(false)
        .skip_taskbar(true)
        .shadow(true)
        .visible(false)
        .focused(false)
        .inner_size(320.0, 120.0)
        // Цвет фона до первой отрисовки: без него окно мелькает белым.
        .background_color(Color(16, 16, 20, 255))
        .parent(&main)?
        .build()?;
    // Сочетания движка (F5, Ctrl+F) и SmartScreen попапу не нужны — как окну
    // браузера.
    #[cfg(windows)]
    window.with_webview(|platform| {
        browser190x4_webview::tab::configure_interface(&platform.controller());
    })?;

    let handle = app.clone();
    let closing = window.clone();
    let owner = owner.to_string();
    window.on_window_event(move |event| {
        if let WindowEvent::Focused(false) = event {
            // Окно страницы ждёт ответа: щелчок мимо его не закрывает.
            let sticky = handle
                .state::<crate::state::App>()
                .popup
                .with(&owner, |state| state.sticky.is_some());
            if !sticky {
                hide_window(&handle, &owner, &closing);
            }
        }
    });
    Ok(window)
}

/// Окно браузера закрылось — его попап больше не нужен.
pub fn destroy(app: &AppHandle, owner: &str) {
    if let Some(window) = app.get_webview_window(&label_for(owner)) {
        let _ = window.destroy();
    }
    app.state::<crate::state::App>().popup.forget(owner);
}

/// Видимость попапа ведём сами, через Win32, а не через `show()`/`hide()`
/// Tauri: подсказкам адресной строки нужно появиться, не забирая фокус
/// (`SW_SHOWNOACTIVATE`), а Tauri такого показа не умеет. Смешивать нельзя:
/// tao помнит свой флаг видимости и молча пропустил бы следующий `show()`.
#[cfg(windows)]
fn native_visible(window: &WebviewWindow) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
    window
        .hwnd()
        .map(|hwnd| unsafe { IsWindowVisible(hwnd).as_bool() })
        .unwrap_or(false)
}

#[cfg(windows)]
fn native_show(window: &WebviewWindow, focus: bool) -> anyhow::Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::{
        SetForegroundWindow, ShowWindow, SW_SHOW, SW_SHOWNOACTIVATE,
    };
    let hwnd = window.hwnd()?;
    unsafe {
        let _ = ShowWindow(hwnd, if focus { SW_SHOW } else { SW_SHOWNOACTIVATE });
        if focus {
            let _ = SetForegroundWindow(hwnd);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn native_hide(window: &WebviewWindow) {
    use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
    if let Ok(hwnd) = window.hwnd() {
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

#[cfg(not(windows))]
fn native_visible(window: &WebviewWindow) -> bool {
    window.is_visible().unwrap_or(false)
}

#[cfg(not(windows))]
fn native_show(window: &WebviewWindow, focus: bool) -> anyhow::Result<()> {
    window.show()?;
    if focus {
        window.set_focus()?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn native_hide(window: &WebviewWindow) {
    let _ = window.hide();
}

fn hide_window(app: &AppHandle, owner: &str, window: &WebviewWindow) {
    if native_visible(window) {
        native_hide(window);
        let seq = app
            .state::<crate::state::App>()
            .popup
            .with(owner, |state| state.seq);
        closed(app, owner, seq, false);
    }
}

/// Сказать окну браузера, какой показ попапа закончился. `replaced` — окно не
/// пряталось, его содержимое сменил следующий попап.
fn closed(app: &AppHandle, owner: &str, seq: u64, replaced: bool) {
    let payload = serde_json::json!({ "seq": seq, "replaced": replaced });
    let _ = app.emit_to(owner, "popup-closed", payload);
}

pub fn hide(app: &AppHandle, owner: &str) {
    if let Some(window) = app.get_webview_window(&label_for(owner)) {
        hide_window(app, owner, &window);
    }
}

/// Скрыть попап по просьбе интерфейса. `seq` — какой показ закрывают: если
/// его уже сменил следующий попап, закрывать нечего.
pub fn hide_shown(app: &AppHandle, owner: &str, seq: Option<u64>) {
    let current = app
        .state::<crate::state::App>()
        .popup
        .with(owner, |state| {
            if seq == Some(state.seq) {
                state.cancelled = seq;
            }
            state.seq
        });
    if seq.is_none_or(|seq| seq == current) {
        hide(app, owner);
    }
}

/// Окно браузера сдвинули. Меню и пузыри закрываются, а окно страницы едет
/// вместе с ним: страница ждёт ответа, и прятать окно незачем.
pub fn main_moved(app: &AppHandle, owner: &str) {
    let Some(popup) = app.get_webview_window(&label_for(owner)) else {
        return;
    };
    let state = &app.state::<crate::state::App>().popup;
    let followed = state.with(owner, |state| {
        (|| -> Option<()> {
            let now = app.get_webview_window(owner)?.inner_position().ok()?;
            let before = state.sticky.as_mut()?;
            if !native_visible(&popup) {
                return None;
            }
            let at = popup.outer_position().ok()?;
            popup
                .set_position(PhysicalPosition::new(
                    at.x + now.x - before.x,
                    at.y + now.y - before.y,
                ))
                .ok()?;
            *before = now;
            Some(())
        })()
    });
    if followed.is_none() {
        hide_window(app, owner, &popup);
    }
}

/// Показать попап под элементом chrome-а. Размеры — в CSS-пикселях окна.
/// Возвращает номер показа: по нему интерфейс узнаёт, что закрылся именно
/// этот попап, а не тот, что был на экране до него.
pub fn open(
    app: &AppHandle,
    owner: &str,
    kind: &str,
    anchor: Anchor,
    width: f64,
    align: &str,
    payload: Value,
) -> anyhow::Result<u64> {
    let main = app
        .get_webview_window(owner)
        .ok_or_else(|| anyhow::anyhow!("нет окна браузера"))?;
    let popup = ensure(app, owner)?;

    let scale = main.scale_factor()?;
    let origin = main.inner_position()?;
    let size = main.inner_size()?;
    let main_width = f64::from(size.width) / scale;
    let main_height = f64::from(size.height) / scale;

    let point = align == "point";
    let left = match align {
        "end" => anchor.x + anchor.width - width,
        "center" => anchor.x + (anchor.width - width) / 2.0,
        // Как у системных меню: у правого края раскрывается влево от курсора.
        "point" if anchor.x + width > main_width - 8.0 => anchor.x - width,
        _ => anchor.x,
    }
    .clamp(8.0, (main_width - width - 8.0).max(8.0));
    let top = if point {
        anchor.y
    } else {
        anchor.y + anchor.height + 4.0
    };

    // Окно с тенью окружено невидимой рамкой: `set_position` ставит её угол, а
    // видимое содержимое съезжало на её толщину вправо и вниз от кнопки.
    let (frame_x, frame_y) = match (popup.inner_position(), popup.outer_position()) {
        (Ok(inner), Ok(outer)) => (inner.x - outer.x, inner.y - outer.y),
        _ => (0, 0),
    };
    let x = origin.x + (left * scale).round() as i32 - frame_x;
    let y = origin.y + (top * scale).round() as i32 - frame_y;
    let placement = (kind.to_string(), x, y, (width * 100.0).round() as i64);

    let state = &app.state::<crate::state::App>().popup;
    // Подсказки адресной строки приходят на каждую клавишу: если тот же попап
    // уже стоит на этом месте, меняется только содержимое. Переезд и сжатие
    // окна до черновой высоты на каждый символ заставляли его мигать.
    let visible = native_visible(&popup);
    let (reuse, seq, replaced) = state.with(owner, |state| {
        state.width = width;
        state.max_height = if point {
            (main_height - 16.0).max(160.0)
        } else {
            (main_height - top - 12.0).max(160.0)
        };
        state.point = point.then_some(anchor.y);
        state.sticky = (kind == DIALOG).then_some(origin);
        let reuse = visible && state.placement.as_ref() == Some(&placement);
        state.placement = Some(placement);
        // Новый попап сменил тот, что был на экране: для интерфейса прежний
        // закрылся, хотя окно и не пряталось.
        let replaced = (visible && !reuse).then_some(state.seq);
        if !reuse {
            state.seq = NEXT_SEQ.fetch_add(1, Ordering::Relaxed);
        }
        state.cancelled = None;
        (reuse, state.seq, replaced)
    });
    if let Some(previous) = replaced {
        closed(app, owner, previous, true);
    }

    if !reuse {
        popup.set_position(PhysicalPosition::new(x, y))?;
        popup.set_size(LogicalSize::new(width, 80.0))?;
    }

    let message = serde_json::json!({
        "kind": kind,
        "payload": payload,
        "width": width,
        "reuse": reuse,
        "owner": owner,
        "seq": seq,
    });
    state.with(owner, |state| state.pending = Some(message.clone()));
    app.emit_to(label_for(owner).as_str(), "popup-render", message)?;
    Ok(seq)
}

/// Попап отрисовал содержимое и знает свою высоту — показываем. Возвращает
/// высоту, которая досталась окну: у нижнего края экрана она меньше
/// запрошенной, и попапу нужно прокручивать содержимое, а не резать его.
///
/// `focus = false` — показать, не забирая фокус: так живут подсказки адресной
/// строки, пока пользователь печатает. `seq` — какой показ отрисован: показ,
/// который уже сменили или закрыли, на экран не выходит.
pub fn show(
    app: &AppHandle,
    owner: &str,
    height: f64,
    focus: bool,
    seq: Option<u64>,
) -> anyhow::Result<f64> {
    let popup = ensure(app, owner)?;
    let state = &app.state::<crate::state::App>().popup;
    let (width, height, point, stale) = state.with(owner, |state| {
        (
            state.width,
            height.min(state.max_height).max(24.0),
            state.point,
            seq.is_some_and(|seq| seq != state.seq || state.cancelled == Some(seq)),
        )
    });
    if stale {
        return Ok(height);
    }
    popup.set_size(LogicalSize::new(width, height))?;
    if let Some(y) = point {
        place_at_point(app, owner, &popup, y, height)?;
    }
    native_show(&popup, focus)?;
    Ok(height)
}

/// Меню у точки щелчка: под точкой, а если снизу не помещается — над ней.
fn place_at_point(
    app: &AppHandle,
    owner: &str,
    popup: &WebviewWindow,
    y: f64,
    height: f64,
) -> anyhow::Result<()> {
    let main = app
        .get_webview_window(owner)
        .ok_or_else(|| anyhow::anyhow!("нет окна браузера"))?;
    let scale = main.scale_factor()?;
    let origin = main.inner_position()?;
    let main_height = f64::from(main.inner_size()?.height) / scale;
    let top = if y + height <= main_height - 8.0 {
        y
    } else if y - height >= 8.0 {
        y - height
    } else {
        (main_height - 8.0 - height).max(8.0)
    };
    let outer = popup.outer_position()?;
    // Невидимая рамка окна с тенью (см. `open`).
    let frame_y = popup.inner_position().map_or(0, |inner| inner.y - outer.y);
    popup.set_position(PhysicalPosition::new(
        outer.x,
        origin.y + (top * scale).round() as i32 - frame_y,
    ))?;
    Ok(())
}

/// Содержимое попапа поменялось (пришёл список форматов, выросла загрузка) —
/// подогнать высоту, не показывая окно заново.
pub fn resize(app: &AppHandle, owner: &str, height: f64) -> anyhow::Result<f64> {
    let state = &app.state::<crate::state::App>().popup;
    let (width, height, point) = state.with(owner, |state| {
        (
            state.width,
            height.min(state.max_height).max(24.0),
            state.point,
        )
    });
    let Some(popup) = app.get_webview_window(&label_for(owner)) else {
        return Ok(height);
    };
    popup.set_size(LogicalSize::new(width, height))?;
    if let Some(y) = point {
        place_at_point(app, owner, &popup, y, height)?;
    }
    Ok(height)
}

pub fn take_pending(app: &AppHandle, owner: &str) -> Option<Value> {
    app.state::<crate::state::App>()
        .popup
        .with(owner, |state| state.pending.take())
}
