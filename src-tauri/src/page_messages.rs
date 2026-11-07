//! Сообщения страниц (`chrome.webview.postMessage`): имя события и лимит частоты.
//!
//! Канал открыт любому документу и любому фрейму, а разбирается всё на главном
//! потоке — том же, что рисует окна и пропускает сетевые запросы вкладок.
//! Поэтому сообщение разбирается один раз, ровно настолько, чтобы узнать имя
//! события, и уходит сразу своему обработчику; а документ, который шлёт
//! сообщения в цикле, упирается в лимит, и лишнее выбрасывается. Лимит свой у
//! вкладки и у каждого её фрейма: рекламный фрейм, засыпающий канал
//! сообщениями, не отнимает его у страницы.

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

/// Сколько сообщений документ может прислать разом и сколько в секунду потом.
/// Свои скрипты шлют единицы в секунду: косметика — пачкой раз в четверть
/// секунды из страницы и из каждого фрейма со своими правилами, остальные — по
/// событиям страницы.
const BURST: f64 = 60.0;
const PER_SECOND: f64 = 30.0;

/// Общий лимит всех фреймов вкладки. Без него страница обходила бы лимит
/// фрейма, создавая пустые фреймы десятками. Документ вкладки в него не входит:
/// рекламные фреймы не отнимают канал у страницы.
const FRAMES_BURST: f64 = 240.0;
const FRAMES_PER_SECOND: f64 = 120.0;

struct Bucket {
    burst: f64,
    per_second: f64,
    tokens: f64,
    at: Instant,
    /// Сообщения уже выбрасываются: в журнал об этом пишется один раз за поток.
    dropping: bool,
}

impl Bucket {
    fn new(now: Instant) -> Self {
        Self::with(now, BURST, PER_SECOND)
    }

    fn with(now: Instant, burst: f64, per_second: f64) -> Self {
        Self {
            burst,
            per_second,
            tokens: burst,
            at: now,
            dropping: false,
        }
    }

    fn take(&mut self, now: Instant) -> bool {
        let gained = now.duration_since(self.at).as_secs_f64() * self.per_second;
        self.tokens = (self.tokens + gained).min(self.burst);
        self.at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Вкладка и её фрейм (`None` — документ вкладки).
type Source = (u32, Option<u32>);

static BUCKETS: LazyLock<Mutex<HashMap<Source, Bucket>>> = LazyLock::new(Mutex::default);

/// Общий счёт фреймов каждой вкладки.
static FRAMES: LazyLock<Mutex<HashMap<u32, Bucket>>> = LazyLock::new(Mutex::default);

/// Пропустить ли ещё одно сообщение документа вкладки или её фрейма.
pub fn allow(tab: u32, frame: Option<u32>) -> bool {
    let now = Instant::now();
    let mut buckets = BUCKETS.lock();
    let bucket = buckets
        .entry((tab, frame))
        .or_insert_with(|| Bucket::new(now));
    let mut allowed = bucket.take(now);
    if allowed && frame.is_some() {
        allowed = FRAMES
            .lock()
            .entry(tab)
            .or_insert_with(|| Bucket::with(now, FRAMES_BURST, FRAMES_PER_SECOND))
            .take(now);
    }
    if !allowed && !bucket.dropping {
        tracing::debug!(
            tab,
            ?frame,
            "страница шлёт сообщения слишком часто — лишние выброшены"
        );
    }
    bucket.dropping = !allowed;
    allowed
}

/// Вкладку закрыли — счёт её и её фреймов больше не нужен.
pub fn forget(tab: u32) {
    BUCKETS.lock().retain(|(id, _), _| *id != tab);
    FRAMES.lock().remove(&tab);
}

/// Фрейм исчез — его счёт больше не нужен.
pub fn forget_frame(tab: u32, frame: u32) {
    BUCKETS.lock().remove(&(tab, Some(frame)));
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

    #[test]
    fn many_frames_share_one_limit() {
        let tab = 900_001;
        let passed = (0..1_000).filter(|frame| allow(tab, Some(*frame))).count();
        // Пока идёт цикл, счёт чуть пополняется — точное число не сверяем.
        let burst = FRAMES_BURST as usize;
        assert!((burst..burst + 20).contains(&passed), "{passed}");
        // Документ вкладки фреймы не задевают.
        assert!(allow(tab, None));
        forget(tab);
    }
}
