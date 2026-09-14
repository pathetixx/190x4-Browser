//! Схема сообщений chrome ↔ вкладка.
//!
//! Через `PostWebMessageAsJson` в обе стороны. Это единственный канал: у
//! вкладок нет Tauri-IPC, а значит и доступа к командам приложения. Всё, что
//! приходит из вкладки, — данные с недоверенной страницы; обрабатывать
//! соответственно (никаких путей к файлам, никаких shell-аргументов).

use serde::{Deserialize, Serialize};

/// Chrome → вкладка.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum TabCommand {
    /// Подсветить и прокрутить к совпадению (Ctrl+F).
    FindHighlight { query: String, index: u32 },
    /// Перевести выделение / всю страницу. Текст уходит на наш релей,
    /// ключ Gemini во вкладку не попадает никогда.
    TranslateApply { chunks: Vec<TranslatedChunk> },
    /// Погасить косметику (элементы, которые сеть уже не режет).
    CosmeticRules { selectors: Vec<String> },
    /// Спросить у страницы, есть ли на ней видео, которое умеет скачать yt-dlp.
    ProbeMedia,
}

/// Вкладка → chrome.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "evt", rename_all = "snake_case")]
pub enum ChromeEvent {
    /// Нашлось медиа для загрузчика: ссылка + заголовок.
    MediaFound { url: String, title: String },
    /// Пользователь выделил текст — показываем кнопку перевода.
    SelectionChanged { text: String },
    /// Результат поиска по странице.
    FindResult { total: u32, current: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslatedChunk {
    pub id: u32,
    pub text: String,
}
