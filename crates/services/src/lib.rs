//! Клиент к сервисам 190x4: переводчик и загрузчик медиа.
//!
//! # Где живут ключи
//!
//! Не в коде и не в репозитории. Конфиг читается из
//! `%LOCALAPPDATA%\pw.x190x4.browser\services.json`, а для сборки-разработки его
//! перекрывают переменные окружения. Ключей нет и во вкладках: все запросы
//! делает Rust, страница к ним не прикасается — иначе первый же сайт с
//! доступом к нашему IPC утащил бы их в открытую.
//!
//! # Почему клиент, а не свой бэкенд
//!
//! `/api/translate` и `/api/yt-ext/*` уже работают на сервере 190x4: там один
//! ключ Gemini, один rate-limit, одни логи и разобранные грабли yt-dlp по
//! источникам. Дублировать это в браузере незачем.

mod config;
mod media;
mod translate;

pub use config::ServicesConfig;
pub use media::{MediaFormat, MediaInfo, MediaProgress};
pub use translate::{Translation, MAX_TEXT as TRANSLATE_MAX_TEXT};

use std::time::Duration;

pub struct Services {
    config: ServicesConfig,
    http: reqwest::Client,
    /// Клиент без пула соединений: каждый запрос — по новому соединению.
    /// Нужен для повтора, когда старое соединение из пула умерло молча.
    fresh: reqwest::Client,
}

impl Services {
    pub fn new(config: ServicesConfig) -> anyhow::Result<Self> {
        // Общего таймаута нет намеренно: у запросов разная природа. Перевод —
        // интерактивное действие (секунды), разбор ссылки идёт через yt-dlp и
        // честно думает до минуты, а сам файл может качаться час. Таймаут
        // ставится на каждый запрос отдельно.
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .user_agent(concat!("190x4-browser/", env!("CARGO_PKG_VERSION")))
            // Простаивающее соединение могли оборвать по дороге (VPN
            // переподключился, прокси закрыл туннель), не сказав нам: запрос в
            // него висит до таймаута. Долго держать такие незачем.
            .pool_idle_timeout(Duration::from_secs(20))
            .build()?;
        let fresh = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .user_agent(concat!("190x4-browser/", env!("CARGO_PKG_VERSION")))
            .pool_max_idle_per_host(0)
            .build()?;

        Ok(Self {
            config,
            http,
            fresh,
        })
    }

    pub fn config(&self) -> &ServicesConfig {
        &self.config
    }

    /// Сетевую ошибку пользователю показываем по-человечески: «error sending
    /// request for url» ему ничего не объясняет.
    pub(crate) fn network_error(err: &reqwest::Error) -> String {
        if err.is_timeout() {
            "сервис не ответил вовремя".into()
        } else if err.is_connect() {
            "нет связи с сервисом 190x4".into()
        } else {
            format!("сеть: {err}")
        }
    }
}

/// Сколько ждать ответа на каждый вид запроса.
pub(crate) mod timeouts {
    use std::time::Duration;

    /// Перевод: интерактивно, дольше — уже раздражает.
    pub const TRANSLATE: Duration = Duration::from_secs(25);
    /// Первая попытка перевода. Сервер отвечает за секунду; молчание дольше —
    /// почти всегда мёртвое соединение из пула, и повтор по новому быстрее,
    /// чем ждать полный таймаут.
    pub const TRANSLATE_FIRST: Duration = Duration::from_secs(8);
    /// Разбор ссылки: yt-dlp ходит на сайт и перебирает форматы.
    pub const MEDIA_INFO: Duration = Duration::from_secs(90);
    /// Постановка задания и опрос прогресса — быстрые операции.
    pub const MEDIA_CONTROL: Duration = Duration::from_secs(20);
    /// Скачивание готового файла.
    pub const MEDIA_FETCH: Duration = Duration::from_secs(60 * 60);
}
