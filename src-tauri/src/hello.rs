//! Подтверждение личности Windows Hello.
//!
//! Показать сохранённый пароль, скопировать его или выгрузить все пароли в
//! файл — действия, ради которых и лезут в чужой незалоченный компьютер.
//! Chrome и Edge на них спрашивают вход Windows; спрашиваем и мы.
//!
//! Если Hello в системе не настроен (нет ни лица, ни отпечатка, ни PIN),
//! браузер спрашивает пароль Windows — как Chrome. Пропускает без вопроса он
//! только учётную запись Windows без пароля: проверять там нечего, а запирать
//! пользователя в его же паролях мы не имеем права.

use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// Подтверждение действует минуту: Chrome ведёт себя так же, иначе показать
/// три пароля подряд — это три запроса Hello.
const GRACE: Duration = Duration::from_secs(60);

static CONFIRMED_AT: Mutex<Option<Instant>> = Mutex::new(None);

/// Подтверждение с памятью на минуту. `false` — пользователь не подтвердил.
pub fn confirm_recent(window: Option<&tauri::WebviewWindow>, reason: &str) -> anyhow::Result<bool> {
    if CONFIRMED_AT.lock().is_some_and(|at| at.elapsed() < GRACE) {
        return Ok(true);
    }
    let ok = confirm(window, reason)?;
    if ok {
        *CONFIRMED_AT.lock() = Some(Instant::now());
    }
    Ok(ok)
}

#[cfg(windows)]
pub fn confirm(window: Option<&tauri::WebviewWindow>, reason: &str) -> anyhow::Result<bool> {
    use windows::core::{factory, HSTRING};
    use windows::Security::Credentials::UI::{
        UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability,
    };
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::WinRT::IUserConsentVerifierInterop;

    let available = UserConsentVerifier::CheckAvailabilityAsync()?.get()?;
    if available != UserConsentVerifierAvailability::Available {
        tracing::debug!(
            ?available,
            "Windows Hello недоступен — спрашиваем пароль Windows"
        );
        return confirm_with_password(window, reason);
    }

    let message = HSTRING::from(reason);
    let hwnd = window
        .and_then(|window| window.hwnd().ok())
        .unwrap_or(HWND(std::ptr::null_mut()));

    // Окно обязательно: без него запрос уходит в никуда на десктопе.
    let result = if hwnd.0.is_null() {
        UserConsentVerifier::RequestVerificationAsync(&message)?.get()?
    } else {
        let interop = factory::<UserConsentVerifier, IUserConsentVerifierInterop>()?;
        let operation: windows_future::IAsyncOperation<UserConsentVerificationResult> =
            unsafe { interop.RequestVerificationForWindowAsync(hwnd, &message)? };
        operation.get()?
    };

    Ok(result == UserConsentVerificationResult::Verified)
}

/// Подтверждение паролем Windows, когда Hello не настроен: системное окно
/// входа с плиткой текущего пользователя, пароль сверяет сама Windows
/// (`LogonUserW`). Три попытки, отмена — «не подтверждено».
#[cfg(windows)]
fn confirm_with_password(
    window: Option<&tauri::WebviewWindow>,
    reason: &str,
) -> anyhow::Result<bool> {
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_ACCOUNT_RESTRICTION, ERROR_CANCELLED, ERROR_LOGON_FAILURE, HANDLE, HWND,
    };
    use windows::Win32::Graphics::Gdi::HBITMAP;
    use windows::Win32::Security::Credentials::{
        CredUIPromptForWindowsCredentialsW, CredUnPackAuthenticationBufferW,
        CREDUIWIN_ENUMERATE_CURRENT_USER, CREDUI_INFOW, CRED_PACK_FLAGS,
    };
    use windows::Win32::Security::{
        LogonUserW, LOGON32_LOGON_INTERACTIVE, LOGON32_PROVIDER_DEFAULT,
    };
    use windows::Win32::System::Com::CoTaskMemFree;

    /// Стереть пароль из памяти: обычная запись нулями после последнего чтения
    /// компилятор вправе выбросить.
    fn wipe(buffer: &mut [u16]) {
        for unit in buffer.iter_mut() {
            unsafe { std::ptr::write_volatile(unit, 0) };
        }
    }

    let hwnd = window
        .and_then(|window| window.hwnd().ok())
        .unwrap_or(HWND(std::ptr::null_mut()));
    let caption = HSTRING::from("190x4 Browser");
    let message = HSTRING::from(format!("{reason}: подтвердите паролем Windows"));
    let info = CREDUI_INFOW {
        cbSize: std::mem::size_of::<CREDUI_INFOW>() as u32,
        hwndParent: hwnd,
        pszMessageText: PCWSTR(message.as_ptr()),
        pszCaptionText: PCWSTR(caption.as_ptr()),
        hbmBanner: HBITMAP::default(),
    };

    let mut error = 0u32;
    for _ in 0..3 {
        let mut package = 0u32;
        let mut packed: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut packed_size = 0u32;
        let code = unsafe {
            CredUIPromptForWindowsCredentialsW(
                Some(std::ptr::addr_of!(info)),
                error,
                &mut package,
                None,
                0,
                &mut packed,
                &mut packed_size,
                None,
                CREDUIWIN_ENUMERATE_CURRENT_USER,
            )
        };
        if code == ERROR_CANCELLED.0 {
            return Ok(false);
        }
        if code != 0 || packed.is_null() {
            // Окно входа не открылось — пароль не показываем: пропускать без
            // проверки на любой ошибке значило бы, что её можно вызвать нарочно.
            tracing::warn!(code, "окно входа Windows не открылось");
            anyhow::bail!("окно входа Windows не открылось — попробуйте ещё раз");
        }

        let mut user = vec![0u16; 514];
        let mut domain = vec![0u16; 338];
        let mut password = vec![0u16; 514];
        let (mut user_len, mut domain_len, mut password_len) = (
            user.len() as u32,
            domain.len() as u32,
            password.len() as u32,
        );
        let unpacked = unsafe {
            let result = CredUnPackAuthenticationBufferW(
                CRED_PACK_FLAGS(0),
                packed,
                packed_size,
                Some(PWSTR(user.as_mut_ptr())),
                &mut user_len,
                Some(PWSTR(domain.as_mut_ptr())),
                Some(std::ptr::addr_of_mut!(domain_len)),
                Some(PWSTR(password.as_mut_ptr())),
                &mut password_len,
            );
            std::ptr::write_bytes(packed.cast::<u8>(), 0, packed_size as usize);
            CoTaskMemFree(Some(packed.cast_const()));
            result
        };
        if let Err(err) = unpacked {
            wipe(&mut password);
            tracing::warn!(%err, "ответ окна входа Windows не прочитан");
            return Ok(false);
        }

        // «ДОМЕН\имя» или «MicrosoftAccount\почта» — домен отдельно; «почта» — как есть.
        let end = user
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(user.len());
        let name = String::from_utf16_lossy(&user[..end]);
        let (account_domain, account) = match name.split_once('\\') {
            Some((domain, account)) => (Some(domain.to_string()), account.to_string()),
            None => (None, name.clone()),
        };
        let blank = password.first() == Some(&0);
        let mut token = HANDLE::default();
        let logon = unsafe {
            let domain = account_domain.as_deref().map(HSTRING::from);
            LogonUserW(
                &HSTRING::from(account.as_str()),
                domain
                    .as_ref()
                    .map_or(PCWSTR::null(), |domain| PCWSTR(domain.as_ptr())),
                PCWSTR(password.as_ptr()),
                LOGON32_LOGON_INTERACTIVE,
                LOGON32_PROVIDER_DEFAULT,
                &mut token,
            )
        };
        wipe(&mut password);
        wipe(&mut domain);
        match logon {
            // Окно входа может принять и чужую учётную запись этого компьютера:
            // пароли открывает только вход того, чьи они, — сверяем SID.
            Ok(()) => {
                let same = token_sid(token)
                    .is_some_and(|sid| current_user_sid().is_some_and(|own| own == sid));
                unsafe {
                    let _ = CloseHandle(token);
                }
                if same {
                    return Ok(true);
                }
                tracing::warn!("в окне входа — другая учётная запись Windows");
                error = ERROR_LOGON_FAILURE.0;
            }
            // Учётная запись без пароля: Windows не пускает с пустым паролем
            // через этот вход, а проверять больше нечего — если это она сама.
            Err(err)
                if blank
                    && err.code() == ERROR_ACCOUNT_RESTRICTION.to_hresult()
                    && is_current_account(account_domain.as_deref(), &account) =>
            {
                return Ok(true);
            }
            Err(err) => {
                tracing::debug!(%err, "пароль Windows не подошёл");
                error = ERROR_LOGON_FAILURE.0;
            }
        }
    }
    Ok(false)
}

/// SID учётной записи токена — байтами.
#[cfg(windows)]
fn token_sid(token: windows::Win32::Foundation::HANDLE) -> Option<Vec<u8>> {
    use windows::Win32::Security::{GetLengthSid, GetTokenInformation, TokenUser, TOKEN_USER};

    unsafe {
        let mut size = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut size);
        if size == 0 {
            return None;
        }
        // Буфер под TOKEN_USER — с выравниванием указателя.
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            size,
            &mut size,
        )
        .ok()?;
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        let sid = user.User.Sid;
        if sid.0.is_null() {
            return None;
        }
        let length = GetLengthSid(sid) as usize;
        Some(std::slice::from_raw_parts(sid.0 as *const u8, length).to_vec())
    }
}

/// SID пользователя, от чьего имени работает браузер.
#[cfg(windows)]
fn current_user_sid() -> Option<Vec<u8>> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::TOKEN_QUERY;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let sid = token_sid(token);
        let _ = CloseHandle(token);
        sid
    }
}

/// Имя из окна входа — учётная запись, от чьего имени работает браузер. Для
/// записи без пароля: у неё нет токена, по которому сверить SID.
#[cfg(windows)]
fn is_current_account(domain: Option<&str>, account: &str) -> bool {
    let same = |name: &str, var: &str| {
        std::env::var(var).is_ok_and(|own| own.to_lowercase() == name.to_lowercase())
    };
    same(account, "USERNAME") && domain.is_none_or(|domain| same(domain, "USERDOMAIN"))
}

#[cfg(not(windows))]
pub fn confirm(_window: Option<&tauri::WebviewWindow>, _reason: &str) -> anyhow::Result<bool> {
    Ok(true)
}
