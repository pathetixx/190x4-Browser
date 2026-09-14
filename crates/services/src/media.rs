use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::{timeouts, Services};

/// Ответ `/api/yt-ext/info` как есть плюс готовые для интерфейса варианты.
///
/// Сервер отдаёт видео и аудио разными списками, а спецификацию для `start`
/// (`video:1080`, `audio:192`) собирает из них вызывающий. Собираем её здесь,
/// один раз, чтобы правило жило в одном месте, а не в каждом клиенте.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaInfo {
    #[serde(default, deserialize_with = "flexible_string")]
    pub title: String,
    #[serde(default, deserialize_with = "flexible_string")]
    pub uploader: String,
    #[serde(default, deserialize_with = "flexible_string")]
    pub duration_str: String,
    #[serde(default)]
    pub thumbnail: Option<String>,
    #[serde(default, deserialize_with = "flexible_string")]
    pub source: String,
    /// Плоский список «что скачать», уже со спецификациями.
    #[serde(default)]
    pub formats: Vec<MediaFormat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaFormat {
    /// Спецификация для `start`: `video:1080`, `audio:192`.
    pub spec: String,
    pub label: String,
    /// Оценка размера в мегабайтах — сервер точного размера не обещает.
    #[serde(default)]
    pub approx_mb: Option<f64>,
}

/// Сырой ответ сервера: видео и аудио отдельными списками.
#[derive(Deserialize)]
struct RawInfo {
    #[serde(default, deserialize_with = "flexible_string")]
    title: String,
    #[serde(default, deserialize_with = "flexible_string")]
    uploader: String,
    #[serde(default, deserialize_with = "flexible_string")]
    duration_str: String,
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(default, deserialize_with = "flexible_string")]
    source: String,
    #[serde(default)]
    video_formats: Vec<RawFormat>,
    #[serde(default)]
    audio_formats: Vec<RawFormat>,
}

#[derive(Deserialize)]
struct RawFormat {
    #[serde(default, deserialize_with = "flexible_string")]
    label: String,
    #[serde(default)]
    height: Option<i64>,
    #[serde(default)]
    bitrate: Option<i64>,
    #[serde(default)]
    approx_mb: Option<f64>,
}

impl From<RawInfo> for MediaInfo {
    fn from(raw: RawInfo) -> Self {
        let mut formats = Vec::with_capacity(raw.video_formats.len() + raw.audio_formats.len());

        // Сначала видео от лучшего качества к худшему: пользователь чаще всего
        // берёт верхний пункт.
        let mut video = raw.video_formats;
        video.sort_by_key(|format| std::cmp::Reverse(format.height.unwrap_or(0)));
        for format in video {
            let Some(height) = format.height else {
                continue;
            };
            formats.push(MediaFormat {
                spec: format!("video:{height}"),
                label: format.label,
                approx_mb: format.approx_mb,
            });
        }

        for format in raw.audio_formats {
            let Some(bitrate) = format.bitrate else {
                continue;
            };
            formats.push(MediaFormat {
                spec: format!("audio:{bitrate}"),
                label: format.label,
                approx_mb: format.approx_mb,
            });
        }

        Self {
            title: raw.title,
            uploader: raw.uploader,
            duration_str: raw.duration_str,
            thumbnail: raw.thumbnail,
            source: raw.source,
            formats,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaProgress {
    #[serde(default, deserialize_with = "flexible_string")]
    pub status: String,
    #[serde(default, deserialize_with = "flexible_string")]
    pub phase: String,
    #[serde(default, deserialize_with = "flexible_f64")]
    pub percent: f64,
    #[serde(default, deserialize_with = "flexible_i64")]
    pub downloaded: i64,
    /// Общий размер. Сервер присылает сюда пустую строку, пока размер
    /// неизвестен, — на строгом `i64` разбор падал целиком.
    #[serde(default, deserialize_with = "flexible_i64")]
    pub total: i64,
    #[serde(default, deserialize_with = "flexible_string")]
    pub file_name: String,
    #[serde(default)]
    pub error: Option<String>,
}

/// `null`, число или строка — всё превращаем в строку.
///
/// Чужой API кладёт `null` в любое поле: сначала это был `total`, потом
/// `phase`. Латать по одному полю на каждый прогон — гиблое занятие, поэтому
/// устойчивы все поля разом.
fn flexible_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::String(text) => text,
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::Bool(flag) => flag.to_string(),
        _ => String::new(),
    })
}

/// Число, строка с числом, пустая строка или null — всё превращаем в `i64`.
///
/// Чужой API не обязан быть строго типизированным, а падать целиком из-за
/// пустого поля прогресса — худшее, что может сделать клиент.
fn flexible_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::Number(number) => number.as_i64().unwrap_or(0),
        serde_json::Value::String(text) => text.trim().parse().unwrap_or(0),
        _ => 0,
    })
}

fn flexible_f64<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::Number(number) => number.as_f64().unwrap_or(0.0),
        serde_json::Value::String(text) => text.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    })
}

impl MediaProgress {
    pub fn finished(&self) -> bool {
        matches!(self.status.as_str(), "done" | "error" | "cancelled")
    }

    pub fn failed(&self) -> bool {
        matches!(self.status.as_str(), "error" | "cancelled")
    }
}

#[derive(Serialize)]
struct UrlRequest<'a> {
    url: &'a str,
}

#[derive(Serialize)]
struct StartRequest<'a> {
    url: &'a str,
    format: &'a str,
}

#[derive(Deserialize)]
struct StartResponse {
    job_id: String,
}

impl Services {
    /// Что за ссылка и что с неё можно скачать.
    pub async fn media_info(&self, url: &str) -> anyhow::Result<MediaInfo> {
        anyhow::ensure!(
            self.config.media_enabled(),
            "загрузчик не настроен: нет ключа в services.json"
        );

        let response = self
            .http
            .post(format!("{}/api/yt-ext/info", self.config.base_url))
            .header("X-YT-Ext-Key", &self.config.media_key)
            .json(&UrlRequest { url })
            .timeout(timeouts::MEDIA_INFO)
            .send()
            .await
            .map_err(|err| {
                anyhow::anyhow!(
                    "не удалось разобрать ссылку: {}",
                    Services::network_error(&err)
                )
            })?;

        let raw: RawInfo = self.decode(response, "не удалось разобрать ссылку").await?;
        Ok(raw.into())
    }

    /// Поставить загрузку в очередь. Возвращает идентификатор задания.
    pub async fn media_start(&self, url: &str, format: &str) -> anyhow::Result<String> {
        anyhow::ensure!(self.config.media_enabled(), "загрузчик не настроен");

        let response = self
            .http
            .post(format!("{}/api/yt-ext/start", self.config.base_url))
            .header("X-YT-Ext-Key", &self.config.media_key)
            .json(&StartRequest { url, format })
            .timeout(timeouts::MEDIA_CONTROL)
            .send()
            .await
            .map_err(|err| {
                anyhow::anyhow!("загрузка не началась: {}", Services::network_error(&err))
            })?;

        let started: StartResponse = self.decode(response, "загрузка не началась").await?;
        Ok(started.job_id)
    }

    pub async fn media_progress(&self, job_id: &str) -> anyhow::Result<MediaProgress> {
        let response = self
            .http
            .get(format!("{}/api/yt-ext/progress", self.config.base_url))
            .query(&[("job_id", job_id)])
            .header("X-YT-Ext-Key", &self.config.media_key)
            .timeout(timeouts::MEDIA_CONTROL)
            .send()
            .await?;

        self.decode(response, "статус загрузки недоступен").await
    }

    /// Отменить задание на сервере: yt-dlp там гасится, задание помечается
    /// ошибкой, и следующий опрос прогресса это увидит.
    pub async fn media_cancel(&self, job_id: &str) -> anyhow::Result<()> {
        #[derive(serde::Serialize)]
        struct CancelRequest<'a> {
            job_id: &'a str,
        }

        self.http
            .post(format!("{}/api/yt/cancel", self.config.base_url))
            .header("X-YT-Ext-Key", &self.config.media_key)
            .json(&CancelRequest { job_id })
            .timeout(timeouts::MEDIA_CONTROL)
            .send()
            .await
            .map_err(|err| {
                anyhow::anyhow!("загрузка не отменилась: {}", Services::network_error(&err))
            })?
            .error_for_status()?;
        Ok(())
    }

    /// Забрать готовый файл на диск.
    ///
    /// Пишем потоком: ролик на пару гигабайт не должен оказаться в памяти
    /// целиком. Имя берём то, что отдал сервер.
    pub async fn media_fetch(
        &self,
        job_id: &str,
        dir: &Path,
        file_name: &str,
    ) -> anyhow::Result<PathBuf> {
        let response = self
            .http
            .get(format!("{}/api/yt-ext/file", self.config.base_url))
            .query(&[("job_id", job_id)])
            .header("X-YT-Ext-Key", &self.config.media_key)
            .timeout(timeouts::MEDIA_FETCH)
            .send()
            .await?
            .error_for_status()?;

        tokio::fs::create_dir_all(dir).await?;
        let target = unique_path(dir, file_name);

        let mut file = tokio::fs::File::create(&target).await?;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            file.write_all(&chunk?).await?;
        }
        file.flush().await?;

        Ok(target)
    }

    /// Человеческий текст вместо служебного кода сервера.
    ///
    /// Коды приходят как есть (`session_busy`, `unsupported_source`), и
    /// показывать их пользователю нельзя: это внутренний словарь yt-dlp-бэка.
    fn explain(code: &str) -> String {
        match code.trim() {
            "session_busy" => "Уже качается другой файл — дождитесь, пока он закончится".into(),
            "unsupported_source" => "Этот сайт загрузчик не поддерживает".into(),
            "disk_full" => "На сервере кончилось место, попробуйте позже".into(),
            "bad_url" => "Ссылка не похожа на страницу с видео".into(),
            "unauthorized" => "Загрузчик не авторизован: проверьте ключ в services.json".into(),
            "rate_limited" => "Слишком часто — подождите минуту".into(),
            other => other.to_string(),
        }
    }

    async fn decode<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
        context: &str,
    ) -> anyhow::Result<T> {
        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            let reason = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|value| {
                    value
                        .get("error")
                        .and_then(|e| e.as_str())
                        .map(Self::explain)
                })
                .unwrap_or_else(|| format!("HTTP {status}"));
            anyhow::bail!("{context}: {reason}");
        }

        Ok(serde_json::from_str(&body)?)
    }
}

/// Не затирать уже скачанное: `video.mp4` → `video (2).mp4`.
fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let safe = sanitize(file_name);
    let candidate = dir.join(&safe);
    if !candidate.exists() {
        return candidate;
    }

    let path = Path::new(&safe);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();

    for index in 2..1000 {
        let candidate = dir.join(format!("{stem} ({index}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }

    dir.join(safe)
}

/// Имя приходит от стороннего сервиса — в путь оно попадать как есть не должно.
fn sanitize(file_name: &str) -> String {
    let name = file_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("download")
        .trim();

    let cleaned: String = name
        .chars()
        .map(|c| {
            if r#"<>:"/\|?*"#.contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();

    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "download".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::{sanitize, MediaInfo, RawFormat, RawInfo};
    use crate::Services;

    #[test]
    fn progress_survives_empty_numbers() {
        // Ровно то, что присылает сервер, пока размер неизвестен.
        let raw = r#"{"status":"downloading","phase":null,"percent":"12.5","downloaded":1024,"total":"","file_name":"x.mp4","error":null}"#;
        let progress: super::MediaProgress = serde_json::from_str(raw).unwrap();

        assert_eq!(progress.total, 0);
        assert_eq!(progress.downloaded, 1024);
        assert!((progress.percent - 12.5).abs() < f64::EPSILON);
        assert!(progress.phase.is_empty());
        assert!(!progress.finished());
    }

    #[test]
    fn server_codes_become_readable() {
        assert!(Services::explain("session_busy").contains("дождитесь"));
        assert!(Services::explain("unsupported_source").contains("не поддерживает"));
        // Незнакомый код показываем как есть — лучше странный текст, чем молчание.
        assert_eq!(Services::explain("whatever"), "whatever");
    }

    #[test]
    fn formats_are_flattened_and_sorted() {
        let raw = RawInfo {
            title: "Big Buck Bunny".into(),
            uploader: "Blender".into(),
            duration_str: "10:35".into(),
            thumbnail: None,
            source: "youtube".into(),
            video_formats: vec![
                RawFormat {
                    label: "144p (mp4)".into(),
                    height: Some(144),
                    bitrate: None,
                    approx_mb: Some(14.2),
                },
                RawFormat {
                    label: "1080p (mp4)".into(),
                    height: Some(1080),
                    bitrate: None,
                    approx_mb: Some(300.0),
                },
            ],
            audio_formats: vec![RawFormat {
                label: "MP3 192k".into(),
                height: None,
                bitrate: Some(192),
                approx_mb: Some(14.9),
            }],
        };

        let info: MediaInfo = raw.into();
        let specs: Vec<&str> = info.formats.iter().map(|f| f.spec.as_str()).collect();
        assert_eq!(specs, ["video:1080", "video:144", "audio:192"]);
    }

    #[test]
    fn path_traversal_is_stripped() {
        assert_eq!(sanitize("../../windows/system32/evil.exe"), "evil.exe");
        assert_eq!(sanitize(r"..\..\evil.exe"), "evil.exe");
    }

    #[test]
    fn forbidden_characters_are_replaced() {
        assert_eq!(sanitize(r#"video<>:"|?*.mp4"#), "video_______.mp4");
    }

    #[test]
    fn empty_name_has_fallback() {
        assert_eq!(sanitize("   "), "download");
        assert_eq!(sanitize(".."), "download");
    }
}
