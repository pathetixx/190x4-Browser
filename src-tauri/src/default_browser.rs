//! Браузер по умолчанию.
//!
//! Назначить себя браузером программа в Windows не может: выбор хранится в
//! `UserChoice` под хешем, который пишет только сама система. Программа
//! регистрируется — описывает в реестре, какие ссылки и файлы умеет открывать, —
//! и открывает свою страницу в параметрах, где выбор подтверждает пользователь.
//! Регистрация — в HKCU, без прав администратора: браузер и ставится для
//! одного пользователя.
//!
//! Регистрирует установщик (`--register-browser`, `windows/hooks.nsh`) и кнопка
//! в настройках — с текущим путём к exe. Удаление программы регистрацию снимает
//! (`--unregister-browser`).

use std::io;

use serde::Serialize;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegDeleteTreeW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::UI::Shell::{
    ApplicationAssociationRegistration, IApplicationAssociationRegistration, SHChangeNotify,
    AL_EFFECTIVE, AT_URLPROTOCOL, SHCNE_ASSOCCHANGED, SHCNF_IDLIST,
};

use crate::launch::SINGLE_ARGUMENT;

/// Под этим именем браузер виден в «Приложениях по умолчанию».
const APP_NAME: &str = "190x4 Browser";
/// ProgID не может начинаться с цифры и содержать знаки, кроме точки.
const PROG_ID: &str = "Browser190x4HTML";
const DESCRIPTION: &str = "Браузер 190x4";
const URL_SCHEMES: [&str; 2] = ["http", "https"];
const FILE_TYPES: [&str; 8] = [
    ".htm", ".html", ".shtml", ".xht", ".xhtml", ".svg", ".webp", ".pdf",
];
/// Ветка HKCU, куда пишется регистрация. Тесты пишут в свою.
const ROOT: &str = "Software";

const REGISTER: &str = "--register-browser";
const UNREGISTER: &str = "--unregister-browser";

#[derive(Serialize)]
pub struct DefaultBrowser {
    is_default: bool,
}

#[tauri::command]
pub async fn default_browser_state() -> DefaultBrowser {
    let is_default = tauri::async_runtime::spawn_blocking(is_default)
        .await
        .unwrap_or(false);
    DefaultBrowser { is_default }
}

/// Регистрирует браузер и открывает его страницу в «Приложениях по умолчанию»:
/// кнопка вверху назначает его для ссылок и файлов разом.
#[tauri::command]
pub async fn default_browser_set() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        let exe = std::env::current_exe().map_err(|err| err.to_string())?;
        register(&exe.to_string_lossy())
            .map_err(|err| format!("Браузер не зарегистрирован в Windows: {err}"))?;
        let page = format!(
            "ms-settings:defaultapps?registeredAppUser={}",
            APP_NAME.replace(' ', "%20")
        );
        tauri_plugin_opener::open_url(page, None::<&str>).map_err(|err| err.to_string())
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Ключ установщика. `Some` — процесс запущен только ради регистрации, это код
/// выхода; окно не создаётся.
pub fn installer_flag() -> Option<i32> {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let [flag] = args.as_slice() else {
        return None;
    };
    let result = match flag.as_str() {
        REGISTER => std::env::current_exe().and_then(|exe| register(&exe.to_string_lossy())),
        UNREGISTER => unregister(),
        _ => return None,
    };
    Some(if result.is_ok() { 0 } else { 1 })
}

fn register(exe: &str) -> io::Result<()> {
    register_at(ROOT, exe)?;
    notify_shell();
    Ok(())
}

fn unregister() -> io::Result<()> {
    unregister_at(ROOT)?;
    notify_shell();
    Ok(())
}

/// Ссылки открывает этот браузер — и http, и https.
fn is_default() -> bool {
    URL_SCHEMES.iter().all(|scheme| {
        current_default(scheme).is_some_and(|prog_id| prog_id.eq_ignore_ascii_case(PROG_ID))
    })
}

/// ProgID, который сейчас открывает схему. Спрашиваем систему, а не читаем
/// `UserChoice`: новые сборки Windows хранят выбор не только там.
fn current_default(scheme: &str) -> Option<String> {
    let scheme = wide(scheme);
    unsafe {
        let com = CoInitializeEx(None, COINIT_MULTITHREADED);
        let found = {
            let registration: windows::core::Result<IApplicationAssociationRegistration> =
                CoCreateInstance(
                    &ApplicationAssociationRegistration,
                    None,
                    CLSCTX_INPROC_SERVER,
                );
            registration
                .and_then(|registration| {
                    registration.QueryCurrentDefault(
                        PCWSTR(scheme.as_ptr()),
                        AT_URLPROTOCOL,
                        AL_EFFECTIVE,
                    )
                })
                .ok()
                .map(|raw| {
                    let prog_id = raw.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(raw.0 as *const _));
                    prog_id
                })
        };
        if com.is_ok() {
            CoUninitialize();
        }
        found
    }
}

/// Проводник и «Открыть с помощью» перечитывают сопоставления.
fn notify_shell() {
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) }
}

fn class_key(root: &str) -> String {
    format!(r"{root}\Classes\{PROG_ID}")
}

fn client_key(root: &str) -> String {
    format!(r"{root}\Clients\StartMenuInternet\{APP_NAME}")
}

/// Регистрация целиком: ключ от HKCU, имя значения (пустое — значение самого
/// ключа) и строка.
fn entries(root: &str, exe: &str) -> Vec<(String, &'static str, String)> {
    let icon = format!("{exe},0");
    let class = class_key(root);
    let client = client_key(root);
    let caps = format!(r"{client}\Capabilities");

    let mut list = vec![
        (class.clone(), "", "Документ 190x4 Browser".to_string()),
        (class.clone(), "URL Protocol", String::new()),
        (
            format!(r"{class}\Application"),
            "ApplicationName",
            APP_NAME.to_string(),
        ),
        (
            format!(r"{class}\Application"),
            "ApplicationIcon",
            icon.clone(),
        ),
        (
            format!(r"{class}\Application"),
            "ApplicationDescription",
            DESCRIPTION.to_string(),
        ),
        (format!(r"{class}\DefaultIcon"), "", icon.clone()),
        (
            format!(r"{class}\shell\open\command"),
            "",
            format!("\"{exe}\" {SINGLE_ARGUMENT} \"%1\""),
        ),
        (client.clone(), "", APP_NAME.to_string()),
        (format!(r"{client}\DefaultIcon"), "", icon.clone()),
        (
            format!(r"{client}\shell\open\command"),
            "",
            format!("\"{exe}\""),
        ),
        (caps.clone(), "ApplicationName", APP_NAME.to_string()),
        (
            caps.clone(),
            "ApplicationDescription",
            DESCRIPTION.to_string(),
        ),
        (caps.clone(), "ApplicationIcon", icon),
        (
            format!(r"{caps}\StartMenu"),
            "StartMenuInternet",
            APP_NAME.to_string(),
        ),
    ];
    for scheme in URL_SCHEMES {
        list.push((
            format!(r"{caps}\URLAssociations"),
            scheme,
            PROG_ID.to_string(),
        ));
    }
    for ext in FILE_TYPES {
        list.push((
            format!(r"{caps}\FileAssociations"),
            ext,
            PROG_ID.to_string(),
        ));
        // «Открыть с помощью» в проводнике.
        list.push((
            format!(r"{root}\Classes\{ext}\OpenWithProgids"),
            PROG_ID,
            String::new(),
        ));
    }
    list.push((format!(r"{root}\RegisteredApplications"), APP_NAME, caps));
    list
}

fn register_at(root: &str, exe: &str) -> io::Result<()> {
    for (path, name, value) in entries(root, exe) {
        Key::create(&path)?.set(name, &value)?;
    }
    Ok(())
}

/// Свои ключи удаляются целиком, в чужих (`RegisteredApplications`,
/// `OpenWithProgids`) — только свои значения. Снимать можно и повторно.
fn unregister_at(root: &str) -> io::Result<()> {
    let own = [class_key(root), client_key(root)];
    for tree in &own {
        delete_tree(tree)?;
    }
    for (path, name, _) in entries(root, "") {
        if !own.iter().any(|tree| path.starts_with(tree.as_str())) {
            delete_value(&path, name)?;
        }
    }
    Ok(())
}

struct Key(HKEY);

impl Key {
    fn create(path: &str) -> io::Result<Self> {
        let path = wide(path);
        let mut key = HKEY::default();
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(path.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut key,
                None,
            )
        };
        check(status)?;
        Ok(Self(key))
    }

    fn set(&self, name: &str, value: &str) -> io::Result<()> {
        let name = wide(name);
        let data: Vec<u8> = wide(value).into_iter().flat_map(u16::to_le_bytes).collect();
        check(unsafe { RegSetValueExW(self.0, PCWSTR(name.as_ptr()), None, REG_SZ, Some(&data)) })
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn delete_tree(path: &str) -> io::Result<()> {
    let path = wide(path);
    missing_ok(unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr())) })
}

fn delete_value(path: &str, name: &str) -> io::Result<()> {
    let (path, name) = (wide(path), wide(name));
    missing_ok(unsafe {
        RegDeleteKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(path.as_ptr()),
            PCWSTR(name.as_ptr()),
        )
    })
}

fn missing_ok(status: WIN32_ERROR) -> io::Result<()> {
    if status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        check(status)
    }
}

fn check(status: WIN32_ERROR) -> io::Result<()> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status.0 as i32))
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_SZ};

    fn read(path: &str, name: &str) -> Option<String> {
        let (path, name) = (wide(path), wide(name));
        let mut buffer = [0u16; 512];
        let mut size = std::mem::size_of_val(&buffer) as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(path.as_ptr()),
                PCWSTR(name.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        (status == ERROR_SUCCESS)
            .then(|| String::from_utf16_lossy(&buffer[..(size as usize / 2).saturating_sub(1)]))
    }

    #[test]
    fn open_command_passes_the_address_as_one_argument() {
        let exe = r"C:\Users\me\AppData\Local\190x4 Browser\190x4-browser.exe";
        let list = entries(ROOT, exe);
        let command = list
            .iter()
            .find(|(path, _, _)| path.ends_with(r"Browser190x4HTML\shell\open\command"))
            .unwrap();
        assert_eq!(command.2, format!("\"{exe}\" --single-argument \"%1\""));
        assert!(list.iter().any(|(path, name, value)| {
            path == r"Software\RegisteredApplications"
                && *name == APP_NAME
                && value == r"Software\Clients\StartMenuInternet\190x4 Browser\Capabilities"
        }));
    }

    #[test]
    fn registers_and_unregisters_cleanly() {
        let root = format!(r"Software\190x4-browser-test-{}", std::process::id());
        let caps = format!(r"{root}\Clients\StartMenuInternet\190x4 Browser\Capabilities");
        register_at(&root, r"C:\190x4\190x4-browser.exe").unwrap();
        let https = read(&format!(r"{caps}\URLAssociations"), "https");
        let registered = read(&format!(r"{root}\RegisteredApplications"), APP_NAME);
        let open_with = read(&format!(r"{root}\Classes\.html\OpenWithProgids"), PROG_ID);

        unregister_at(&root).unwrap();
        // Повторное снятие — не ошибка.
        unregister_at(&root).unwrap();
        let left = (
            read(&format!(r"{root}\RegisteredApplications"), APP_NAME),
            read(&format!(r"{root}\Classes\{PROG_ID}\shell\open\command"), ""),
            read(&format!(r"{root}\Classes\.html\OpenWithProgids"), PROG_ID),
        );
        delete_tree(&root).unwrap();

        assert_eq!(https.as_deref(), Some(PROG_ID));
        assert_eq!(registered, Some(caps));
        assert_eq!(open_with.as_deref(), Some(""));
        assert_eq!(left, (None, None, None));
    }

    #[test]
    fn asks_windows_for_the_current_browser() {
        // Кто открывает http, зависит от машины; если назначен, ответ — непустой ProgID.
        assert!(current_default("http").is_none_or(|prog_id| !prog_id.is_empty()));
    }
}
