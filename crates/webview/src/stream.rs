//! Тело ответа для `CreateWebResourceResponse`.
//!
//! Движок принимает содержимое только `IStream`-ом. Свои страницы (блокировка
//! фильтром) маленькие и уже лежат в памяти, поэтому берём готовый
//! `SHCreateMemStream`: он сам копирует буфер и живёт, пока движок держит
//! ссылку.

use windows::Win32::System::Com::IStream;
use windows::Win32::UI::Shell::SHCreateMemStream;

/// Поток с копией байтов. `None` — Windows не выделила память: ответ уйдёт
/// пустым, это не повод ронять обработчик запроса.
pub fn from_bytes(bytes: &[u8]) -> Option<IStream> {
    unsafe { SHCreateMemStream(Some(bytes)) }
}

/// Всё содержимое потока, который отдал движок (значок сайта), — с начала и не
/// больше `limit` байт. `None` — поток не читается или длиннее предела.
pub fn read_all(stream: &IStream, limit: usize) -> Option<Vec<u8>> {
    use windows::Win32::System::Com::STREAM_SEEK_SET;

    let mut out = Vec::new();
    let mut chunk = [0u8; 8192];
    unsafe {
        let _ = stream.Seek(0, STREAM_SEEK_SET, None);
        loop {
            let mut read = 0u32;
            let code = stream.Read(
                chunk.as_mut_ptr().cast(),
                chunk.len() as u32,
                Some(&mut read),
            );
            if code.is_err() {
                return None;
            }
            if read == 0 {
                return Some(out);
            }
            out.extend_from_slice(&chunk[..read as usize]);
            if out.len() > limit {
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn empty_body_is_allowed() {
        // Проверяем только то, что вызов не паникует на пустом срезе: сам
        // поток живёт в COM и на CI без движка не читается.
        let _ = super::from_bytes(b"");
    }

    #[test]
    fn read_all_returns_what_was_written() {
        let bytes: Vec<u8> = (0..20_000u32).map(|i| i as u8).collect();
        let stream = super::from_bytes(&bytes).unwrap();
        assert_eq!(super::read_all(&stream, 64 * 1024), Some(bytes));
        assert_eq!(super::read_all(&stream, 1024), None);
    }
}
