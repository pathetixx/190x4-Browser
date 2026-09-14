//! Всплывающие окна chrome-а: меню, пузырь загрузок, правка закладки,
//! расширение-загрузчик, предложение сохранить пароль.
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
//! Окно одно на всё и создаётся заранее: первый показ меню не должен ждать
//! запуска вебвью.

use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::Value;
use tauri::window::Color;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

pub const LABEL: &str = "popup";

#[derive(Default)]
pub struct Popup {
    /// Что рисовать: окно могло ещё не загрузиться к моменту первого вызова.
    pending: Mutex<Option<Value>>,
    width: Mutex<f64>,
    /// Где кончается окно браузера — ниже попап не растёт.
    max_height: Mutex<f64>,
    /// Вид и место последнего показа: тот же попап на том же месте
    /// перерисовывается без переезда окна.
    placement: Mutex<Option<(String, i32, i32, i64)>>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Anchor {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Создать окно попапа, если его ещё нет.
pub fn ensure(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(window) = app.get_webview_window(LABEL) {
        return Ok(window);
    }
    let main = app
        .get_webview_window("chrome")
        .ok_or(tauri::Error::WindowNotFound)?;

    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("popup.html".into()))
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

    let handle = app.clone();
    let closing = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Focused(false) = event {
            hide_window(&handle, &closing);
        }
    });
    Ok(window)
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

fn hide_window(app: &AppHandle, window: &WebviewWindow) {
    if native_visible(window) {
        native_hide(window);
        let _ = app.emit_to("chrome", "popup-closed", ());
    }
}

pub fn hide(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        hide_window(app, &window);
    }
}

/// Показать попап под элементом chrome-а. Размеры — в CSS-пикселях окна.
pub fn open(
    app: &AppHandle,
    state: &Popup,
    kind: &str,
    anchor: Anchor,
    width: f64,
    align: &str,
    payload: Value,
) -> anyhow::Result<()> {
    let main = app
        .get_webview_window("chrome")
        .ok_or_else(|| anyhow::anyhow!("нет окна браузера"))?;
    let popup = ensure(app)?;

    let scale = main.scale_factor()?;
    let origin = main.inner_position()?;
    let size = main.inner_size()?;
    let main_width = f64::from(size.width) / scale;
    let main_height = f64::from(size.height) / scale;

    let left = match align {
        "end" => anchor.x + anchor.width - width,
        "center" => anchor.x + (anchor.width - width) / 2.0,
        _ => anchor.x,
    }
    .clamp(8.0, (main_width - width - 8.0).max(8.0));
    let top = anchor.y + anchor.height + 4.0;

    *state.width.lock() = width;
    *state.max_height.lock() = (main_height - top - 12.0).max(160.0);

    let x = origin.x + (left * scale).round() as i32;
    let y = origin.y + (top * scale).round() as i32;
    let placement = (kind.to_string(), x, y, (width * 100.0).round() as i64);
    // Подсказки адресной строки приходят на каждую клавишу: если тот же попап
    // уже стоит на этом месте, меняется только содержимое. Переезд и сжатие
    // окна до черновой высоты на каждый символ заставляли его мигать.
    let reuse = native_visible(&popup) && state.placement.lock().as_ref() == Some(&placement);
    if !reuse {
        popup.set_position(PhysicalPosition::new(x, y))?;
        popup.set_size(LogicalSize::new(width, 80.0))?;
    }
    *state.placement.lock() = Some(placement);

    let message =
        serde_json::json!({ "kind": kind, "payload": payload, "width": width, "reuse": reuse });
    *state.pending.lock() = Some(message.clone());
    app.emit_to(LABEL, "popup-render", message)?;
    Ok(())
}

/// Попап отрисовал содержимое и знает свою высоту — показываем. Возвращает
/// высоту, которая досталась окну: у нижнего края экрана она меньше
/// запрошенной, и попапу нужно прокручивать содержимое, а не резать его.
///
/// `focus = false` — показать, не забирая фокус: так живут подсказки адресной
/// строки, пока пользователь печатает.
pub fn show(app: &AppHandle, state: &Popup, height: f64, focus: bool) -> anyhow::Result<f64> {
    let popup = ensure(app)?;
    let width = *state.width.lock();
    let height = height.min(*state.max_height.lock()).max(24.0);
    popup.set_size(LogicalSize::new(width, height))?;
    native_show(&popup, focus)?;
    Ok(height)
}

/// Содержимое попапа поменялось (пришёл список форматов, выросла загрузка) —
/// подогнать высоту, не показывая окно заново.
pub fn resize(app: &AppHandle, state: &Popup, height: f64) -> anyhow::Result<f64> {
    let height = height.min(*state.max_height.lock()).max(24.0);
    let Some(popup) = app.get_webview_window(LABEL) else {
        return Ok(height);
    };
    let width = *state.width.lock();
    popup.set_size(LogicalSize::new(width, height))?;
    Ok(height)
}

pub fn take_pending(state: &Popup) -> Option<Value> {
    state.pending.lock().take()
}
