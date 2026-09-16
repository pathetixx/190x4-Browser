//! Сторож главного потока.
//!
//! Всё окно браузера живёт на одном потоке: цикл сообщений, COM вкладок и
//! синхронные команды интерфейса. Если он чем-то занят, «зависает» весь
//! интерфейс — а по логу этого не видно: зависший поток ничего не пишет.
//!
//! Сторож четыре раза в секунду ставит в очередь главного потока пустую задачу.
//! Не выполнилась за секунду — главный поток занят: сторож на мгновение
//! приостанавливает его, снимает стек и пишет в `browser.log`, где он стоит.
//! Когда поток отпустит, в лог уходит, сколько длилось зависание.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::AppHandle;

/// Как часто проверять главный поток.
const TICK: Duration = Duration::from_millis(250);
/// Задержка, после которой эпизод попадает в лог.
const SLOW: Duration = Duration::from_millis(400);
/// Задержка, после которой снимается стек.
const HUNG: Duration = Duration::from_millis(1000);
/// Повторный снимок стека, если поток так и не отпустило.
const AGAIN: Duration = Duration::from_secs(5);

pub fn spawn(app: AppHandle) {
    #[cfg(windows)]
    let main = stack::MainThread::current();

    // Проверка самого сторожа: `BROWSER190X4_WATCHDOG_SELFTEST=1` через пять
    // секунд после старта занимает главный поток на полторы секунды — в логе
    // должен появиться стек с `self_test`.
    if std::env::var_os("BROWSER190X4_WATCHDOG_SELFTEST").is_some() {
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(5));
            let _ = app.run_on_main_thread(self_test);
        });
    }

    std::thread::Builder::new()
        .name("main-thread-watchdog".into())
        .spawn(move || {
            let answered = Arc::new(AtomicU64::new(0));
            let mut sent = 0u64;
            loop {
                sent += 1;
                let mark = answered.clone();
                let ping = sent;
                let asked = Instant::now();
                if app
                    .run_on_main_thread(move || mark.store(ping, Ordering::Relaxed))
                    .is_err()
                {
                    // Цикл сообщений закончился — приложение выходит.
                    return;
                }

                let mut snapshots = 0u32;
                let mut next_snapshot = HUNG;
                while answered.load(Ordering::Relaxed) < ping {
                    std::thread::sleep(Duration::from_millis(50));
                    let waited = asked.elapsed();
                    if waited >= next_snapshot {
                        snapshots += 1;
                        next_snapshot = waited + AGAIN;
                        #[cfg(windows)]
                        {
                            let frames =
                                main.as_ref().map(|main| main.capture()).unwrap_or_default();
                            tracing::warn!(
                                ms = waited.as_millis() as u64,
                                snapshot = snapshots,
                                "главный поток не отвечает, стек:\n{}",
                                stack::describe(&frames)
                            );
                        }
                        #[cfg(not(windows))]
                        tracing::warn!(ms = waited.as_millis() as u64, "главный поток не отвечает");
                    }
                }

                let waited = asked.elapsed();
                if waited >= SLOW {
                    tracing::warn!(ms = waited.as_millis() as u64, "главный поток был занят");
                }
                std::thread::sleep(TICK);
            }
        })
        .expect("поток сторожа создаётся всегда");
}

#[inline(never)]
fn self_test() {
    tracing::info!("проверка сторожа: главный поток занят на 1,5 с");
    std::thread::sleep(Duration::from_millis(1500));
}

#[cfg(windows)]
mod stack {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Diagnostics::Debug::{
        GetThreadContext, RtlLookupFunctionEntry, RtlVirtualUnwind, SymFromAddrW,
        SymGetLineFromAddrW64, SymInitializeW, SymSetOptions, CONTEXT, CONTEXT_FULL_AMD64,
        IMAGEHLP_LINEW64, SYMBOL_INFOW, SYMOPT_DEFERRED_LOADS, SYMOPT_LOAD_LINES, SYMOPT_UNDNAME,
        UNW_FLAG_NHANDLER,
    };
    use windows::Win32::System::LibraryLoader::{
        GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
        GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentThreadId, OpenThread, ResumeThread, SuspendThread,
        THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION, THREAD_SUSPEND_RESUME,
    };
    use windows_core::PCWSTR;

    const DEPTH: usize = 48;

    /// Контекст x64 требует выравнивания по 16 байтам, иначе `GetThreadContext`
    /// отвечает ошибкой.
    #[repr(C, align(16))]
    struct Aligned(CONTEXT);

    pub struct MainThread(HANDLE);

    // Хэндл потока — просто число, им можно пользоваться из любого потока.
    unsafe impl Send for MainThread {}

    impl MainThread {
        /// Хэндл текущего потока — звать с главного.
        pub fn current() -> Option<Self> {
            let handle = unsafe {
                OpenThread(
                    THREAD_GET_CONTEXT | THREAD_SUSPEND_RESUME | THREAD_QUERY_INFORMATION,
                    false,
                    GetCurrentThreadId(),
                )
            };
            match handle {
                Ok(handle) => Some(Self(handle)),
                Err(err) => {
                    tracing::warn!(%err, "сторож не получил хэндл главного потока");
                    None
                }
            }
        }

        /// Адреса возврата главного потока. Пока поток приостановлен, здесь
        /// нельзя ни выделять память, ни писать в лог: он мог остановиться,
        /// держа замок кучи, и сторож повис бы на нём же.
        pub fn capture(&self) -> Vec<u64> {
            let mut frames = [0u64; DEPTH];
            let mut count = 0usize;
            unsafe {
                if SuspendThread(self.0) == u32::MAX {
                    return Vec::new();
                }
                let mut context = Aligned(std::mem::zeroed());
                context.0.ContextFlags = CONTEXT_FULL_AMD64;
                if GetThreadContext(self.0, &mut context.0).is_ok() {
                    let context = &mut context.0;
                    while count < DEPTH && context.Rip != 0 {
                        frames[count] = context.Rip;
                        count += 1;
                        let mut image_base = 0u64;
                        let entry = RtlLookupFunctionEntry(context.Rip, &mut image_base, None);
                        if entry.is_null() {
                            // Функция без таблицы раскрутки (лист): адрес возврата
                            // лежит прямо на вершине стека.
                            if context.Rsp == 0 {
                                break;
                            }
                            context.Rip = *(context.Rsp as *const u64);
                            context.Rsp += 8;
                        } else {
                            let mut handler_data = std::ptr::null_mut();
                            let mut establisher = 0u64;
                            RtlVirtualUnwind(
                                UNW_FLAG_NHANDLER,
                                image_base,
                                context.Rip,
                                entry,
                                context,
                                &mut handler_data,
                                &mut establisher,
                                None,
                            );
                        }
                    }
                }
                ResumeThread(self.0);
            }
            frames[..count].to_vec()
        }
    }

    impl Drop for MainThread {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    /// Стек текстом: функция и строка, если рядом лежит PDB, иначе модуль и
    /// смещение — по ним стек разбирается позже с PDB из сборки.
    pub fn describe(frames: &[u64]) -> String {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| unsafe {
            SymSetOptions(SYMOPT_UNDNAME | SYMOPT_DEFERRED_LOADS | SYMOPT_LOAD_LINES);
            // PDB ищется и в папке программы: путь из сборки на машине
            // пользователя не существует, а exe в пробе ещё и переименован.
            let folder = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()))
                .map(|dir| windows_core::HSTRING::from(dir.as_os_str()))
                .unwrap_or_default();
            let _ = SymInitializeW(GetCurrentProcess(), PCWSTR(folder.as_ptr()), true);
        });

        if frames.is_empty() {
            return "  (стек не снят)".into();
        }
        frames
            .iter()
            .enumerate()
            .map(|(index, address)| format!("  #{index:02} {}", frame(*address)))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn frame(address: u64) -> String {
        let module = module_of(address);
        let symbol = symbol_of(address);
        match symbol {
            Some(symbol) => format!("{symbol} [{module}]"),
            None => format!("{address:#x} [{module}]"),
        }
    }

    fn module_of(address: u64) -> String {
        unsafe {
            let mut module = windows::Win32::Foundation::HMODULE::default();
            if GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                PCWSTR(address as *const u16),
                &mut module,
            )
            .is_err()
            {
                return "?".into();
            }
            let mut name = [0u16; 260];
            let length = GetModuleFileNameW(Some(module), &mut name) as usize;
            let path = String::from_utf16_lossy(&name[..length]);
            let file = path.rsplit('\\').next().unwrap_or(&path).to_string();
            format!("{file}+{:#x}", address - module.0 as u64)
        }
    }

    fn symbol_of(address: u64) -> Option<String> {
        const NAME: usize = 512;
        // SYMBOL_INFOW с местом под имя сразу за структурой.
        #[repr(C)]
        struct Buffer {
            info: SYMBOL_INFOW,
            name: [u16; NAME],
        }
        unsafe {
            let mut buffer: Buffer = std::mem::zeroed();
            buffer.info.SizeOfStruct = std::mem::size_of::<SYMBOL_INFOW>() as u32;
            buffer.info.MaxNameLen = NAME as u32;
            let mut displacement = 0u64;
            SymFromAddrW(
                GetCurrentProcess(),
                address,
                Some(&mut displacement as *mut u64),
                &mut buffer.info,
            )
            .ok()?;
            let length = (buffer.info.NameLen as usize).min(NAME);
            let name_ptr = buffer.info.Name.as_ptr();
            let name = String::from_utf16_lossy(std::slice::from_raw_parts(name_ptr, length));

            let mut line: IMAGEHLP_LINEW64 = std::mem::zeroed();
            line.SizeOfStruct = std::mem::size_of::<IMAGEHLP_LINEW64>() as u32;
            let mut column = 0u32;
            let place =
                if SymGetLineFromAddrW64(GetCurrentProcess(), address, &mut column, &mut line)
                    .is_ok()
                    && !line.FileName.is_null()
                {
                    let file = line.FileName.to_string().unwrap_or_default();
                    let short = file
                        .rsplit(['\\', '/'])
                        .take(2)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<Vec<_>>()
                        .join("/");
                    format!(" at {short}:{}", line.LineNumber)
                } else {
                    String::new()
                };
            Some(format!("{name}{place}"))
        }
    }
}
