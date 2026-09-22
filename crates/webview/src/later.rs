//! Отложенное действие на главном потоке: таймер Windows без окна.
//!
//! Нужен там, где движку надо дать кадр-другой: вкладка, которую только что
//! показали, рисует первый кадр не сразу, и прежнюю прячем чуть позже, чтобы
//! между ними не мелькнул пустой фон.

use std::cell::RefCell;
use std::collections::HashMap;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

type Task = Box<dyn FnOnce()>;

thread_local! {
    static TASKS: RefCell<HashMap<usize, Task>> = RefCell::new(HashMap::new());
}

/// Выполнить `task` через `ms` миллисекунд на этом же (главном) потоке.
pub fn after(ms: u32, task: impl FnOnce() + 'static) {
    let id = unsafe { SetTimer(None, 0, ms, Some(fire)) };
    if id == 0 {
        // Таймер не завёлся — лучше сразу, чем никогда.
        task();
        return;
    }
    TASKS.with(|tasks| tasks.borrow_mut().insert(id, Box::new(task)));
}

unsafe extern "system" fn fire(_: HWND, _: u32, id: usize, _: u32) {
    unsafe {
        let _ = KillTimer(None, id);
    }
    let task = TASKS.with(|tasks| tasks.borrow_mut().remove(&id));
    if let Some(task) = task {
        task();
    }
}
