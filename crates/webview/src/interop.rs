//! Мост между Tauri-вебвью (chrome, #0) и нашими вкладками.

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2Controller, ICoreWebView2Environment, ICoreWebView2_2,
};
use windows_core::Interface;

/// Достать Environment, в котором Tauri создал chrome-вебвью.
///
/// Это единственный способ гарантировать, что вкладки окажутся в том же
/// browser-процессе, что и chrome: Environment несёт в себе и путь к user data
/// folder, и browser arguments, и флаги. Создавать второй Environment
/// «с теми же параметрами» — не то же самое: любое расхождение в аргументах
/// (а Tauri свои добавляет) разведёт нас по двум процессам браузера, и вся
/// экономия памяти из замеров исчезнет.
///
/// Вызывать строго с UI-потока, внутри `with_webview`.
pub fn environment_of(
    controller: &ICoreWebView2Controller,
) -> windows_core::Result<ICoreWebView2Environment> {
    unsafe {
        let core = controller.CoreWebView2()?;
        let core2: ICoreWebView2_2 = core.cast()?;
        core2.Environment()
    }
}
