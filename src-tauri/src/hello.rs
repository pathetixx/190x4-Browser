//! Подтверждение личности Windows Hello.
//!
//! Показать сохранённый пароль, скопировать его или выгрузить все пароли в
//! файл — действия, ради которых и лезут в чужой незалоченный компьютер.
//! Chrome и Edge на них спрашивают вход Windows; спрашиваем и мы.
//!
//! Если Hello в системе не настроен (нет ни лица, ни отпечатка, ни PIN),
//! спрашивать нечем — тогда действие проходит как раньше: запирать
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
            "Windows Hello недоступен — подтверждение пропущено"
        );
        return Ok(true);
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

#[cfg(not(windows))]
pub fn confirm(_window: Option<&tauri::WebviewWindow>, _reason: &str) -> anyhow::Result<bool> {
    Ok(true)
}
