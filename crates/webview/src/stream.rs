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

#[cfg(test)]
mod tests {
    #[test]
    fn empty_body_is_allowed() {
        // Проверяем только то, что вызов не паникует на пустом срезе: сам
        // поток живёт в COM и на CI без движка не читается.
        let _ = super::from_bytes(b"");
    }
}
