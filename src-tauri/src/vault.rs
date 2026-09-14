//! Шифрование паролей — DPAPI текущего пользователя Windows.
//!
//! Тот же механизм, которым Chrome и Edge закрывают свой ключ паролей:
//! расшифровать данные может только этот пользователь на этой машине, а
//! копия базы, унесённая на другой компьютер, бесполезна. Своего мастер-ключа
//! и хранилища ключей у браузера нет — их пришлось бы где-то держать.

/// Дополнительная энтропия: не секрет, но без неё любой процесс, вызвавший
/// `CryptUnprotectData` на наши байты «на пробу», получил бы пароль сразу.
#[cfg(windows)]
const ENTROPY: &[u8] = b"190x4 Browser password vault v1";

#[cfg(windows)]
pub fn protect(plain: &str) -> anyhow::Result<Vec<u8>> {
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: plain.len() as u32,
        pbData: plain.as_ptr() as *mut u8,
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: ENTROPY.len() as u32,
        pbData: ENTROPY.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();

    unsafe {
        CryptProtectData(
            &input,
            windows_core::w!("190x4 Browser"),
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )?;
        Ok(take_blob(output))
    }
}

#[cfg(windows)]
pub fn reveal(secret: &[u8]) -> anyhow::Result<String> {
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: secret.len() as u32,
        pbData: secret.as_ptr() as *mut u8,
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: ENTROPY.len() as u32,
        pbData: ENTROPY.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();

    let bytes = unsafe {
        CryptUnprotectData(
            &input,
            None,
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|_| {
            anyhow::anyhow!("пароль не расшифровывается: профиль с другой учётной записи Windows")
        })?;
        take_blob(output)
    };
    Ok(String::from_utf8(bytes)?)
}

/// Скопировать буфер, выделенный DPAPI, и освободить его.
#[cfg(windows)]
unsafe fn take_blob(blob: windows::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB) -> Vec<u8> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};

    if blob.pbData.is_null() {
        return Vec::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(blob.pbData, blob.cbData as usize) }.to_vec();
    unsafe {
        let _ = LocalFree(Some(HLOCAL(blob.pbData as *mut core::ffi::c_void)));
    }
    bytes
}

#[cfg(not(windows))]
pub fn protect(_plain: &str) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!("хранилище паролей есть только в Windows-сборке")
}

#[cfg(not(windows))]
pub fn reveal(_secret: &[u8]) -> anyhow::Result<String> {
    anyhow::bail!("хранилище паролей есть только в Windows-сборке")
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn secret_round_trips_and_is_not_plain() {
        let secret = super::protect("p@ss — пароль").unwrap();
        assert!(!secret.windows(4).any(|w| w == b"p@ss"));
        assert_eq!(super::reveal(&secret).unwrap(), "p@ss — пароль");
    }
}
