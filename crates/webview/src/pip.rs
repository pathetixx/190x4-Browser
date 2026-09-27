//! Окно мини-плеера: маленькое окно поверх всех, куда на время переезжает
//! вкладка с видео (`TabHost::pip_open`).
//!
//! Своё окно, а не «картинка в картинке» движка: у той только пауза, а в
//! мини-плеере нужны перемотка и громкость. Вкладка переезжает сюда живой, как
//! между окнами браузера (`SetParentWindow`), а видео на всё окно и кнопки
//! поверх него делает скрипт страницы (`inject/pip.js`).
//!
//! Рамки нет: по краю — тонкая кромка шириной [`EDGE`], за неё окно тянут, а
//! внутри неё лежит страница. Двигают окно за видео: страница просит об этом
//! сообщением, и перенос начинает [`start_drag`].

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Once;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_DONOTROUND,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, GetStockObject, MonitorFromWindow, ScreenToClient, BLACK_BRUSH, HBRUSH,
    MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, LoadCursorW, PostMessageW,
    RegisterClassW, ShowWindow, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTLEFT, HTRIGHT,
    HTTOP, HTTOPLEFT, HTTOPRIGHT, IDC_ARROW, MINMAXINFO, SIZE_MINIMIZED, SW_SHOWNOACTIVATE,
    WM_CLOSE, WM_GETMINMAXINFO, WM_NCCALCSIZE, WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_SIZE, WNDCLASSW,
    WS_CLIPCHILDREN, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_THICKFRAME,
};
use windows_core::{w, PCWSTR};

const CLASS_NAME: PCWSTR = w!("Browser190x4MiniPlayer");

/// Кромка окна, за которую его тянут, — в пикселях при 100%.
pub(crate) const EDGE: i32 = 4;
/// Мини-плеер при открытии и самый маленький — при 100%.
const SIZE: (i32, i32) = (480, 270);
const MIN_SIZE: (i32, i32) = (240, 135);
/// Отступ от угла рабочей области экрана.
const MARGIN: i32 = 24;

/// Что окно сообщает хосту вкладки.
pub(crate) enum PipSignal {
    /// Размер сменился: страница встаёт в новую клиентскую область.
    Resized(RECT),
    /// Окно закрыли (Alt+F4): вкладка возвращается на место.
    Close,
}

type Handler = Rc<dyn Fn(PipSignal)>;

thread_local! {
    static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
}

/// Создать окно мини-плеера у правого нижнего угла экрана окна `near`.
/// Окно показывается без фокуса: смотреть видео не значит отдать ему клавиатуру.
pub(crate) fn create(near: HWND, on_signal: Handler) -> windows_core::Result<HWND> {
    static REGISTER: Once = Once::new();
    let instance = unsafe { GetModuleHandleW(None)? };
    REGISTER.call_once(|| {
        let class = WNDCLASSW {
            hInstance: instance.into(),
            lpszClassName: CLASS_NAME,
            lpfnWndProc: Some(wndproc),
            hbrBackground: HBRUSH(unsafe { GetStockObject(BLACK_BRUSH) }.0),
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
            ..Default::default()
        };
        unsafe { RegisterClassW(&class) };
    });

    let scale = |value: i32| value * dpi_of(near) as i32 / 96;
    let (width, height) = (scale(SIZE.0), scale(SIZE.1));
    let work = work_area(near);
    let x = work.right - width - scale(MARGIN);
    let y = work.bottom - height - scale(MARGIN);

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            CLASS_NAME,
            w!("190x4 · мини-плеер"),
            WS_POPUP | WS_THICKFRAME | WS_CLIPCHILDREN,
            x,
            y,
            width,
            height,
            None,
            None,
            Some(instance.into()),
            None,
        )?
    };
    HANDLER.with(|handler| *handler.borrow_mut() = Some(on_signal));
    unsafe {
        // Рамки нет, а тень у окна остаётся; углы прямые, как у всего 190x4.
        let _ = DwmExtendFrameIntoClientArea(
            hwnd,
            &MARGINS {
                cxLeftWidth: 0,
                cxRightWidth: 0,
                cyTopHeight: 1,
                cyBottomHeight: 0,
            },
        );
        let corners = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            std::ptr::from_ref(&corners).cast(),
            std::mem::size_of_val(&corners) as u32,
        );
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    Ok(hwnd)
}

/// Закрыть окно мини-плеера. Страницу из него уже унесли.
pub(crate) fn destroy(hwnd: HWND) {
    HANDLER.with(|handler| handler.borrow_mut().take());
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

/// Где в окне лежит страница: вся клиентская область без кромки.
pub(crate) fn page_bounds(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rect);
    }
    inset(hwnd, rect)
}

fn inset(hwnd: HWND, rect: RECT) -> RECT {
    let edge = EDGE * dpi_of(hwnd) as i32 / 96;
    RECT {
        left: rect.left + edge,
        top: rect.top + edge,
        right: (rect.right - edge).max(rect.left + edge),
        bottom: (rect.bottom - edge).max(rect.top + edge),
    }
}

/// Начать перенос окна: кнопка мыши ещё зажата на видео. Сообщение — в
/// очередь, а не сразу: перенос крутит свой цикл сообщений, и начинать его
/// внутри обработчика события движка нельзя.
pub(crate) fn start_drag(hwnd: HWND) {
    unsafe {
        let _ = ReleaseCapture();
        let _ = PostMessageW(
            Some(hwnd),
            WM_NCLBUTTONDOWN,
            WPARAM(HTCAPTION as usize),
            LPARAM(0),
        );
    }
}

fn dpi_of(hwnd: HWND) -> u32 {
    match unsafe { GetDpiForWindow(hwnd) } {
        0 => 96,
        dpi => dpi,
    }
}

fn work_area(hwnd: HWND) -> RECT {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let _ = GetMonitorInfoW(monitor, &mut info);
    }
    info.rcWork
}

fn signal(value: PipSignal) {
    // Обработчик достаётся из ячейки до вызова: он может закрыть окно, а с
    // ним и саму ячейку.
    let handler = HANDLER.with(|handler| handler.borrow().clone());
    if let Some(handler) = handler {
        handler(value);
    }
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        // Рамки нет: вся область окна — клиентская.
        WM_NCCALCSIZE if wparam.0 != 0 => LRESULT(0),
        WM_NCHITTEST => {
            let point = POINT {
                x: (lparam.0 & 0xFFFF) as i16 as i32,
                y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
            };
            LRESULT(hit_test(hwnd, point) as isize)
        }
        WM_SIZE => {
            if wparam.0 != SIZE_MINIMIZED as usize {
                signal(PipSignal::Resized(page_bounds(hwnd)));
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            signal(PipSignal::Close);
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let info = lparam.0 as *mut MINMAXINFO;
            if !info.is_null() {
                let dpi = dpi_of(hwnd) as i32;
                unsafe {
                    (*info).ptMinTrackSize.x = MIN_SIZE.0 * dpi / 96;
                    (*info).ptMinTrackSize.y = MIN_SIZE.1 * dpi / 96;
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

/// Кромка тянет окно, внутри неё — перенос: сюда попадают только щелчки по
/// самой кромке, страница лежит внутри.
fn hit_test(hwnd: HWND, screen: POINT) -> u32 {
    let mut point = screen;
    let mut rect = RECT::default();
    unsafe {
        let _ = ScreenToClient(hwnd, &mut point);
        let _ = GetClientRect(hwnd, &mut rect);
    }
    let edge = (EDGE * dpi_of(hwnd) as i32 / 96).max(1) * 2;
    let left = point.x < rect.left + edge;
    let right = point.x >= rect.right - edge;
    let top = point.y < rect.top + edge;
    let bottom = point.y >= rect.bottom - edge;
    match (left, right, top, bottom) {
        (true, _, true, _) => HTTOPLEFT,
        (_, true, true, _) => HTTOPRIGHT,
        (true, _, _, true) => HTBOTTOMLEFT,
        (_, true, _, true) => HTBOTTOMRIGHT,
        (true, ..) => HTLEFT,
        (_, true, ..) => HTRIGHT,
        (_, _, true, _) => HTTOP,
        (.., true) => HTBOTTOM,
        _ => HTCAPTION,
    }
}
