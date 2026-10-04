//! Сообщения страниц (`chrome.webview.postMessage`): имя события и лимит частоты.
//!
//! Канал открыт любому документу и любому фрейму, а разбирается всё на главном
//! потоке — том же, что рисует окна и пропускает сетевые запросы вкладок.
//! Поэтому сообщение разбирается один раз, ровно настолько, чтобы узнать имя
//! события, и уходит сразу своему обработчику; а вкладка, которая шлёт
//! сообщения в цикле, упирается в лимит, и лишнее выбрасывается.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Instant;

use parking_lot::Mutex;

/// Имя события сообщения (`{"evt": "…"}`); `None` — сообщение не наше.
pub fn event_name(payload: &str) -> Option<Cow<'_, str>> {
    #[derive(serde::Deserialize)]
    struct Envelope<'a> {
        #[serde(borrow)]
        evt: Cow<'a, str>,
    }
    serde_json::from_str::<Envelope>(payload)
        .ok()
        .map(|envelope| envelope.evt)
}

/// Сколько сообщений вкладка может прислать разом и сколько в секунду потом.
/// Свои скрипты шлют единицы в секунду: косметика — пачкой раз в четверть
/// секунды, остальные — по событиям страницы.
const BURST: f64 = 40.0;
const PER_SECOND: f64 = 20.0;

struct Bucket {
    tokens: f64,
    at: Instant,
    /// Сообщения уже выбрасываются: в журнал об этом пишется один раз за поток.
    dropping: bool,
}

impl Bucket {
    fn new(now: Instant) -> Self {
        Self {
            tokens: BURST,
            at: now,
            dropping: false,
        }
    }

    fn take(&mut self, now: Instant) -> bool {
        let gained = now.duration_since(self.at).as_secs_f64() * PER_SECOND;
        self.tokens = (self.tokens + gained).min(BURST);
        self.at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

static BUCKETS: LazyLock<Mutex<HashMap<u32, Bucket>>> = LazyLock::new(Mutex::default);

/// Пропустить ли ещё одно сообщение вкладки.
pub fn allow(tab: u32) -> bool {
    let now = Instant::now();
    let mut buckets = BUCKETS.lock();
    let bucket = buckets.entry(tab).or_insert_with(|| Bucket::new(now));
    let allowed = bucket.take(now);
    if !allowed && !bucket.dropping {
        tracing::debug!(
            tab,
            "страница шлёт сообщения слишком часто — лишние выброшены"
        );
    }
    bucket.dropping = !allowed;
    allowed
}

/// Вкладку закрыли — её счёт больше не нужен.
pub fn forget(tab: u32) {
    BUCKETS.lock().remove(&tab);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn event_name_is_read_once() {
        assert_eq!(
            event_name(r#"{"evt":"password_form","step":"login"}"#).as_deref(),
            Some("password_form")
        );
        assert_eq!(
            event_name(r#"{"classes":["a"],"evt":"cosmetic_ids"}"#).as_deref(),
            Some("cosmetic_ids")
        );
        assert_eq!(event_name(r#""{\"evt\":\"navigate\"}""#), None);
        assert_eq!(event_name(r#"{"cmd":"x"}"#), None);
        assert_eq!(event_name("[1,2]"), None);
    }

    #[test]
    fn bursts_pass_and_floods_stop() {
        let start = Instant::now();
        let mut bucket = Bucket::new(start);
        let passed = (0..100).filter(|_| bucket.take(start)).count();
        assert_eq!(passed, BURST as usize);
        // Через секунду — ещё столько, сколько набирается за секунду.
        let later = start + Duration::from_secs(1);
        let passed = (0..100).filter(|_| bucket.take(later)).count();
        assert_eq!(passed, PER_SECOND as usize);
    }
}
