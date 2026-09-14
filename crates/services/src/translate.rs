use serde::{Deserialize, Serialize};

use crate::{timeouts, Services};

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
    /// Язык оригинала не угадываем сами — «auto» разбирает модель на сервере;
    /// она же возвращает, что распознала, чтобы интерфейс мог это показать.
    pub async fn translate(&self, text: &str, target_lang: &str) -> anyhow::Result<Translation> {
        anyhow::ensure!(
            self.config.translate_enabled(),
            "переводчик не настроен: нет ключа в services.json"
        );

        let trimmed = text.trim();
        anyhow::ensure!(!trimmed.is_empty(), "нечего переводить");

        let response = self
            .http
            .post(format!("{}/api/translate", self.config.base_url))
            .header("X-Translate-Key", &self.config.translate_key)
            .json(&TranslateRequest {
                text: trimmed,
                source_lang: "auto",
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
