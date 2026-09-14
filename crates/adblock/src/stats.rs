use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::engine::Decision;

/// Счётчики горячего пути.
///
/// Всё на relaxed-атомиках: цифры показываются в статус-баре и в отчёте
/// спайка, точная синхронизация между ними не нужна, а `fetch_add` с
/// `SeqCst` на каждом запросе — это уже заметная доля бюджета матчинга.
#[derive(Default)]
pub struct Stats {
    checked: AtomicU64,
    blocked: AtomicU64,
    rewritten: AtomicU64,
    nanos_total: AtomicU64,
    nanos_max: AtomicU64,
}

impl Stats {
    pub(crate) fn record(&self, elapsed: Duration, decision: &Decision) {
        let nanos = elapsed.as_nanos() as u64;
        self.checked.fetch_add(1, Ordering::Relaxed);
        self.nanos_total.fetch_add(nanos, Ordering::Relaxed);
        self.nanos_max.fetch_max(nanos, Ordering::Relaxed);
        match decision {
            Decision::Block => {
                self.blocked.fetch_add(1, Ordering::Relaxed);
            }
            Decision::Rewrite(_) => {
                self.rewritten.fetch_add(1, Ordering::Relaxed);
            }
            Decision::Allow => {}
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let checked = self.checked.load(Ordering::Relaxed);
        let nanos_total = self.nanos_total.load(Ordering::Relaxed);
        Snapshot {
            checked,
            blocked: self.blocked.load(Ordering::Relaxed),
            rewritten: self.rewritten.load(Ordering::Relaxed),
            avg_micros: if checked == 0 {
                0.0
            } else {
                nanos_total as f64 / checked as f64 / 1000.0
            },
            max_micros: self.nanos_max.load(Ordering::Relaxed) as f64 / 1000.0,
        }
    }

    pub fn reset(&self) {
        self.checked.store(0, Ordering::Relaxed);
        self.blocked.store(0, Ordering::Relaxed);
        self.rewritten.store(0, Ordering::Relaxed);
        self.nanos_total.store(0, Ordering::Relaxed);
        self.nanos_max.store(0, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Snapshot {
    pub checked: u64,
    pub blocked: u64,
    pub rewritten: u64,
    /// Среднее время одного `check`, микросекунды.
    pub avg_micros: f64,
    /// Худший `check` за сессию — именно он виден как фриз.
    pub max_micros: f64,
}
