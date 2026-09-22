//! Контейнер вкладок — собственное дочернее окно поверх chrome-вебвью.
//!
//! # Зачем
//!
//! Chrome-вебвью Tauri занимает всю клиентскую область окна. Вкладка,
//! созданная тем же родителем, оказывается в Z-порядке под ним: координаты
//! правильные, поверхность живая, а на экране — ничего. Именно это и
//! случилось на первом живом запуске.
//!
//! Управлять Z-порядком чужих дочерних окон (их создаёт wry) нельзя. Зато
//! можно создать своё: контейнер живёт в родительском HWND, поднимается
//! поверх через `HWND_TOP`, и все вкладки создаются уже внутри него. Заодно
//! упрощается всё остальное:
//!
//! * раскладка — это `SetWindowPos` одного окна, вкладки всегда занимают его
//!   целиком;
//! * overlay-режим — это `ShowWindow(SW_HIDE)` одного окна вместо обхода всех
//!   вкладок.

use std::sync::Once;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, RegisterClassW, SetWindowPos, ShowWindow, HWND_TOP,
    SWP_NOACTIVATE, SW_HIDE, SW_SHOW, WINDOW_EX_STYLE, WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN,
    WS_VISIBLE,
};
use windows_core::{w, PCWSTR};

const CLASS_NAME: PCWSTR = w!("Browser190x4TabContainer");

/// Создать контейнер внутри окна приложения и поднять его над chrome-вебвью.
pub fn create(parent: HWND) -> windows_core::Result<HWND> {
    static REGISTER: Once = Once::new();
    let instance = unsafe { GetModuleHandleW(None)? };

    REGISTER.call_once(|| {
        let class = WNDCLASSW {
            hInstance: instance.into(),
            lpszClassName: CLASS_NAME,
            lpfnWndProc: Some(wndproc),
            ..Default::default()
        };
        unsafe { RegisterClassW(&class) };
    });

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            w!(""),
            // CLIPCHILDREN: сам контейнер ничего не рисует, всё поле отдано
            // поверхности вкладки — без него при ресайзе мигает фон.
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN,
            0,
            0,
            0,
            0,
            Some(parent),
            None,
            Some(instance.into()),
            None,
        )?
    };

    raise(hwnd)?;
    Ok(hwnd)
}

/// Поднять контейнер на самый верх среди детей окна.
pub fn raise(hwnd: HWND) -> windows_core::Result<()> {
    unsafe {
        SetWindowPos(
            hwnd,
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE
                | windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE,
        )
    }
}

/// Подвинуть контейнер под область страницы (физические пиксели).
pub fn place(hwnd: HWND, x: i32, y: i32, width: i32, height: i32) -> windows_core::Result<()> {
    unsafe { SetWindowPos(hwnd, Some(HWND_TOP), x, y, width, height, SWP_NOACTIVATE) }
}

/// Убрать страницу с экрана целиком — так работает overlay-режим.
pub fn set_visible(hwnd: HWND, visible: bool) {
    unsafe {
        let _ = ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE });
    }
}

/// Окна вкладок внутри контейнера — прямые дочерние окна, по одному на
/// контроллер. Порядок — от верхнего к нижнему.
pub fn children(hwnd: HWND) -> Vec<HWND> {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindow, GW_CHILD, GW_HWNDNEXT};
    let mut out = Vec::new();
    let mut next = unsafe { GetWindow(hwnd, GW_CHILD) }.ok();
    while let Some(child) = next {
        out.push(child);
        next = unsafe { GetWindow(child, GW_HWNDNEXT) }.ok();
    }
    out
}

/// Опустить окно вкладки под остальные: показанная вкладка рисует первый
/// кадр под прежней, а не поверх неё пустым фоном.
pub fn lower(hwnd: HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{HWND_BOTTOM, SWP_NOMOVE, SWP_NOSIZE};
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_BOTTOM),
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
        );
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
