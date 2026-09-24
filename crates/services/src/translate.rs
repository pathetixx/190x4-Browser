use serde::{Deserialize, Serialize};

use crate::{timeouts, Services};

/// Самый длинный текст для перевода, в знаках. Столько же пускает окно
/// переводчика: перевод — интерактивное действие, а не пакетная обработка.
pub const MAX_TEXT: usize = 5000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Translation {
    /// Готовый перевод.
    pub result: String,
    /// Язык оригинала, как его определила модель. `null`, если не определился.
    #[serde(default)]
    pub detected: Option<String>,
}

#[derive(Serialize)]
struct TranslateRequest<'a> {
    text: &'a str,
    source_lang: &'a str,
    target_lang: &'a str,
}

impl Services {
    /// Перевести текст.
    ///
    /// `source_lang` — язык оригинала или `auto`: тогда его определяет модель
    /// на сервере и возвращает, что распознала, чтобы интерфейс мог это
    /// показать. Языки — названиями («Русский», «English»): так их понимает
    /// модель.
    pub async fn translate(
        &self,
        text: &str,
        source_lang: &str,
        target_lang: &str,
    ) -> anyhow::Result<Translation> {
        anyhow::ensure!(
            self.config.translate_enabled(),
            "переводчик не настроен: нет ключа в services.json"
        );

        let trimmed = text.trim();
        anyhow::ensure!(!trimmed.is_empty(), "нечего переводить");
        anyhow::ensure!(
            trimmed.chars().count() <= MAX_TEXT,
            "текст длиннее {MAX_TEXT} знаков — переведите его по частям"
        );
        let source_lang = language(source_lang).unwrap_or("auto");
        let target_lang =
            language(target_lang).ok_or_else(|| anyhow::anyhow!("не выбран язык перевода"))?;

        let response = self
            .http
            .post(format!("{}/api/translate", self.config.base_url))
            .header("X-Translate-Key", &self.config.translate_key)
            .json(&TranslateRequest {
                text: trimmed,
                source_lang,
                target_lang,
            })
            .timeout(timeouts::TRANSLATE)
            .send()
            .await
            .map_err(|err| {
                anyhow::anyhow!("перевод не удался: {}", Services::network_error(&err))
            })?;

        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            // Сервер кладёт причину в {"error": …}; текст показываем как есть,
            // он написан для человека.
            let reason = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|value| {
                    value
                        .get("error")
                        .and_then(|e| e.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| format!("HTTP {status}"));
            anyhow::bail!("перевод не удался: {reason}");
        }

        Ok(serde_json::from_str(&body)?)
    }
}

/// Название языка из интерфейса: непустое и короткое. Это текст для модели, а
/// не код, поэтому проверяется только форма.
fn language(name: &str) -> Option<&str> {
    let name = name.trim();
    (!name.is_empty() && name.chars().count() <= 40 && !name.eq_ignore_ascii_case("auto"))
        .then_some(name)
}

#[cfg(test)]
mod tests {
    use super::language;

    #[test]
    fn language_names_are_checked_for_shape() {
        assert_eq!(language(" English "), Some("English"));
        assert_eq!(language("中文"), Some("中文"));
        assert_eq!(language("auto"), None);
        assert_eq!(language(""), None);
        assert_eq!(language(&"x".repeat(41)), None);
    }
}
