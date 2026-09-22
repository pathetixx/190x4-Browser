//! С чем браузер запустили: ссылка из другой программы, `.html` из проводника,
//! адрес в командной строке.
//!
//! Браузер работает одним процессом. Первый запуск берёт адреса из своей
//! командной строки; следующий передаёт свою первому через плагин
//! single-instance и завершается, а окно браузера выходит на передний план.
//! Адреса копятся в очереди [`Launch`], chrome забирает их командой
//! [`launch_take`]: после восстановления сессии и по событию `launch`.

use std::collections::HashMap;
use std::path::Path;

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager, State, Url};

/// Ключ из команды открытия в реестре (`default_browser.rs`): всё после него —
/// один адрес, даже если командная строка разрезала его на части. Ссылка с
/// кавычками и пробелами не превратится в ключи браузера.
pub const SINGLE_ARGUMENT: &str = "--single-argument";

/// Мьютекс первого процесса. Имя своё: мьютекс плагина single-instance занимать
/// нельзя, иначе плагин примет первый процесс за второй.
const PRIMARY_MUTEX: &str = "Local\\pw.x190x4.browser-primary";

/// Очереди адресов: общая (её заберёт первое спросившее окно) и адресные —
/// для окон, которые открыли специально под ссылку.
#[derive(Default)]
pub struct Launch {
    common: Mutex<Vec<String>>,
    windows: Mutex<HashMap<String, Vec<String>>>,
    /// Вкладки, которые переедут в окно живыми, когда его интерфейс будет
    /// готов: окно открыли, вытащив вкладку из строки другого окна.
    adopt: Mutex<HashMap<String, Vec<u32>>>,
}

impl Launch {
    pub fn push(&self, targets: Vec<String>) {
        self.common.lock().extend(targets);
    }

    /// Адреса для конкретного окна: его интерфейс ещё грузится и заберёт их сам.
    pub fn push_for(&self, label: &str, targets: Vec<String>) {
        self.windows
            .lock()
            .entry(label.to_string())
            .or_default()
            .extend(targets);
    }

    /// Вкладка переедет в окно `label`, когда его интерфейс её попросит.
    pub fn push_adopt(&self, label: &str, tab: u32) {
        self.adopt
            .lock()
            .entry(label.to_string())
            .or_default()
            .push(tab);
    }

    fn take(&self, label: &str) -> Vec<String> {
        let mut found = self.windows.lock().remove(label).unwrap_or_default();
        found.extend(std::mem::take(&mut *self.common.lock()));
        found
    }
}

#[tauri::command]
pub fn launch_take(window: tauri::Window, launch: State<'_, Launch>) -> Vec<String> {
    launch.take(window.label())
}

/// Вкладки, которые ждут этого окна (см. [`Launch::push_adopt`]).
#[tauri::command]
pub fn launch_adopt_take(window: tauri::Window, launch: State<'_, Launch>) -> Vec<u32> {
    let mut adopt = launch.adopt.lock();
    adopt.remove(window.label()).unwrap_or_default()
}

/// Браузер уже запущен? Второй процесс живёт до настройки плагинов, и профиль
/// ему трогать нельзя: лог открывается с обрезкой, а незавершённые загрузки при
/// старте помечаются прерванными.
pub fn already_running() -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;

    let name: Vec<u16> = PRIMARY_MUTEX.encode_utf16().chain([0]).collect();
    unsafe {
        // Хэндл не закрываем: мьютекс живёт, пока жив процесс.
        match CreateMutexW(None, false, PCWSTR(name.as_ptr())) {
            Ok(_) => GetLastError() == ERROR_ALREADY_EXISTS,
            Err(_) => false,
        }
    }
}

/// Второй процесс запустил пользователь или программа на переднем плане, и
/// вывести окно вперёд может только он. Передаём это право первому — иначе
/// Windows вместо окна мигнёт кнопкой на панели задач.
pub fn allow_foreground() {
    use windows::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};

    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
}

/// Адреса из командной строки этого процесса.
pub fn from_command_line() -> Vec<String> {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let cwd = std::env::current_dir().unwrap_or_default();
    targets(&args, &cwd, " ")
}

/// Повторный запуск: адреса — в очередь, окно — на передний план.
pub fn on_second_instance(app: &AppHandle, args: Vec<String>, cwd: String) {
    // Первым идёт путь к exe. Плагин передаёт аргументы одной строкой через `|`
    // и режет её обратно по `|` — части адреса склеиваем тем же знаком.
    let found = targets(args.get(1..).unwrap_or_default(), Path::new(&cwd), "|");
    tracing::info!(count = found.len(), "повторный запуск браузера");
    // Адреса забирает то окно, которое сейчас впереди: в него же выходит и
    // фокус. Приватному окну чужие ссылки не отдаём: если открыты только
    // приватные окна, ссылка получает новое обычное окно.
    let Some(target) = crate::browser_windows::normal_label(app) else {
        let app = app.clone();
        // Окно создаётся не здесь: сюда попадают из обработчика сообщения
        // окна на главном потоке, а создание окна ждёт главный поток.
        tauri::async_runtime::spawn(async move {
            use crate::browser_windows::{open, WindowKind};
            let mut urls = found.into_iter();
            let first = urls.next();
            match open(&app, WindowKind::Normal, first) {
                Ok(label) => {
                    let rest: Vec<String> = urls.collect();
                    if !rest.is_empty() {
                        app.state::<Launch>().push_for(&label, rest);
                    }
                }
                Err(err) => tracing::warn!(%err, "окно для ссылки не открылось"),
            }
        });
        return;
    };
    if !found.is_empty() {
        app.state::<Launch>().push_for(&target, found);
        let _ = app.emit_to(target.as_str(), "launch", ());
    }
    if let Some(window) = app.get_webview_window(&target) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Адреса среди аргументов. `joiner` склеивает части после [`SINGLE_ARGUMENT`].
pub fn targets(args: &[String], cwd: &Path, joiner: &str) -> Vec<String> {
    if let Some(at) = args.iter().position(|arg| arg == SINGLE_ARGUMENT) {
        return target(&args[at + 1..].join(joiner), cwd)
            .into_iter()
            .collect();
    }
    args.iter()
        .filter(|arg| !arg.starts_with('-'))
        .filter_map(|arg| target(arg, cwd))
        .collect()
}

/// http(s)- и file-адрес — как есть, путь к существующему файлу — file-адресом.
/// Остальное (другие схемы, несуществующие пути) браузер не открывает.
fn target(raw: &str, cwd: &Path) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(url) = Url::parse(raw) {
        match url.scheme() {
            "http" | "https" | "file" => return Some(url.into()),
            // `C:\page.html` разбирается как адрес со схемой `c`.
            scheme if scheme.len() > 1 => return None,
            _ => {}
        }
    }
    let path = std::path::absolute(cwd.join(raw)).ok()?;
    if !path.is_file() {
        return None;
    }
    Url::from_file_path(path).ok().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn web_addresses_pass_as_is() {
        let cwd = std::env::temp_dir();
        assert_eq!(
            targets(&args(&["https://ya.ru/search?text=190x4"]), &cwd, " "),
            ["https://ya.ru/search?text=190x4"]
        );
        assert!(targets(&args(&["javascript:alert(1)", "--flag", "ya"]), &cwd, " ").is_empty());
    }

    #[test]
    fn single_argument_keeps_the_whole_address() {
        let cwd = std::env::temp_dir();
        // Кавычка в ссылке разрезала командную строку: ключ после неё не срабатывает.
        let found = targets(
            &args(&[
                SINGLE_ARGUMENT,
                "https://a.example/?q=\"",
                "--unregister-browser",
            ]),
            &cwd,
            " ",
        );
        assert_eq!(found, ["https://a.example/?q=%22%20--unregister-browser"]);
        // Плагин разрезал адрес по `|`.
        assert_eq!(
            targets(
                &args(&[SINGLE_ARGUMENT, "https://a.example/?x=1", "2"]),
                &cwd,
                "|"
            ),
            ["https://a.example/?x=1|2"]
        );
    }

    #[test]
    fn files_become_file_addresses() {
        let dir = std::env::temp_dir().join(format!("190x4-launch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let page = dir.join("страница 1.html");
        std::fs::write(&page, "<p>190x4</p>").unwrap();

        let absolute = targets(&args(&[page.to_str().unwrap()]), Path::new("C:\\"), " ");
        let relative = targets(&args(&["страница 1.html"]), &dir, " ");
        let missing = targets(&args(&["нет-такого.html"]), &dir, " ");
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(absolute.len(), 1);
        assert!(absolute[0].starts_with("file:///"));
        assert!(absolute[0].ends_with("/%D1%81%D1%82%D1%80%D0%B0%D0%BD%D0%B8%D1%86%D0%B0%201.html"));
        assert_eq!(absolute, relative);
        assert!(missing.is_empty());
    }
}
