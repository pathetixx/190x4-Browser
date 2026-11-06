//! Буфер обмена для меню правки интерфейса (`ui/js/edit-menu.js`).
//!
//! Интерфейс сам вставить текст не может: `navigator.clipboard.readText`
//! спрашивает разрешение окном движка. А копировать из меню, пока фокус у
//! всплывающего окна, `navigator.clipboard.writeText` не даёт. Поэтому текст
//! читает и пишет браузер, через Windows. Команды доступны только окнам
//! интерфейса, вкладки до них не достают.

#[tauri::command(async)]
pub fn clipboard_read() -> Result<String, String> {
    read().map_err(|err| format!("{err:#}"))
}

#[tauri::command(async)]
pub fn clipboard_write(text: String) -> Result<(), String> {
    write(&text).map_err(|err| format!("{err:#}"))
}

/// Текст в UTF-16 (`CF_UNICODETEXT`).
#[cfg(windows)]
const UNICODE_TEXT: u32 = 13;

/// Открыть буфер, выполнить `f` и закрыть. Другая программа держит буфер
/// мгновения — несколько попыток, а не ошибка с первого раза.
#[cfg(windows)]
fn with_clipboard<R>(f: impl FnOnce() -> anyhow::Result<R>) -> anyhow::Result<R> {
    use windows::Win32::System::DataExchange::{CloseClipboard, OpenClipboard};

    let mut opened = false;
    for _ in 0..10 {
        if unsafe { OpenClipboard(None) }.is_ok() {
            opened = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    anyhow::ensure!(opened, "буфер обмена занят другой программой");
    let result = f();
    unsafe {
        let _ = CloseClipboard();
    }
    result
}

#[cfg(windows)]
fn read() -> anyhow::Result<String> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::GetClipboardData;
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

    with_clipboard(|| unsafe {
        // Текста в буфере нет (картинка, файлы) — вставлять нечего.
        let Ok(handle) = GetClipboardData(UNICODE_TEXT) else {
            return Ok(String::new());
        };
        let memory = HGLOBAL(handle.0);
        let data = GlobalLock(memory) as *const u16;
        if data.is_null() {
            return Ok(String::new());
        }
        let units = std::slice::from_raw_parts(data, GlobalSize(memory) / 2);
        let end = units
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(units.len());
        let text = String::from_utf16_lossy(&units[..end]);
        let _ = GlobalUnlock(memory);
        Ok(text)
    })
}

#[cfg(windows)]
fn write(text: &str) -> anyhow::Result<()> {
    use windows::Win32::Foundation::{GlobalFree, HANDLE};
    use windows::Win32::System::DataExchange::{EmptyClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    let mut units: Vec<u16> = text.encode_utf16().collect();
    units.push(0);
    with_clipboard(|| unsafe {
        EmptyClipboard()?;
        let memory = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2)?;
        let data = GlobalLock(memory) as *mut u16;
        if data.is_null() {
            let _ = GlobalFree(Some(memory));
            anyhow::bail!("память под буфер обмена не выделена");
        }
        std::ptr::copy_nonoverlapping(units.as_ptr(), data, units.len());
        let _ = GlobalUnlock(memory);
        // Память теперь принадлежит буферу; освобождать её — только если он
        // её не взял.
        if let Err(err) = SetClipboardData(UNICODE_TEXT, Some(HANDLE(memory.0))) {
            let _ = GlobalFree(Some(memory));
            return Err(err.into());
        }
        Ok(())
    })
}

#[cfg(not(windows))]
fn read() -> anyhow::Result<String> {
    anyhow::bail!("буфер обмена есть только в Windows-сборке")
}

#[cfg(not(windows))]
fn write(_text: &str) -> anyhow::Result<()> {
    anyhow::bail!("буфер обмена есть только в Windows-сборке")
}
