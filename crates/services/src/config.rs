use std::path::Path;

use serde::{Deserialize, Serialize};

/// Адрес сервисов и ключи доступа.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServicesConfig {
    #[serde(default = "default_base")]
    pub base_url: String,
    /// Ключ `X-Translate-Key`. Пустой — переводчик выключен.
    #[serde(default)]
    pub translate_key: String,
    /// Ключ `X-YT-Ext-Key`. Пустой — загрузчик выключен.
    #[serde(default)]
    pub media_key: String,
}

fn default_base() -> String {
    "https://190x4.pw".to_string()
}

impl Default for ServicesConfig {
    fn default() -> Self {
        Self {
            base_url: default_base(),
            translate_key: String::new(),
            media_key: String::new(),
        }
    }
}

impl ServicesConfig {
    /// Загрузить конфиг: сначала файл профиля, поверх — переменные окружения.
    ///
    /// Отсутствие файла не ошибка: браузер работает и без сервисов, просто
    /// переводчик и загрузчик честно говорят, что не настроены.
    pub fn load(path: &Path) -> Self {
        let mut config = match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
                tracing::warn!(%err, "services.json не разобран, берём значения по умолчанию");
                Self::default()
            }),
            Err(_) => Self::default(),
        };

        if let Ok(base) = std::env::var("BROWSER190X4_SERVICES_URL") {
            config.base_url = base;
        }
        if let Ok(key) = std::env::var("BROWSER190X4_TRANSLATE_KEY") {
            config.translate_key = key;
        }
        if let Ok(key) = std::env::var("BROWSER190X4_MEDIA_KEY") {
            config.media_key = key;
        }

        config
    }

    pub fn translate_enabled(&self) -> bool {
        !self.translate_key.is_empty()
    }

    pub fn media_enabled(&self) -> bool {
        !self.media_key.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::ServicesConfig;

    #[test]
    fn missing_file_gives_defaults() {
        let config = ServicesConfig::load(std::path::Path::new("/nonexistent/services.json"));
        assert_eq!(config.base_url, "https://190x4.pw");
        assert!(!config.translate_enabled());
        assert!(!config.media_enabled());
    }
}
