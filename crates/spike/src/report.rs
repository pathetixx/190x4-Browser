//! Запись отчётов. Один JSON на замер + человекочитаемая сводка в stdout.

use std::path::Path;

use serde::Serialize;

pub fn write<T: Serialize>(dir: &Path, name: &str, report: &T) -> anyhow::Result<()> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let path = dir.join(format!("{name}-{stamp}.json"));
    let json = serde_json::to_string_pretty(report)?;
    std::fs::write(&path, &json)?;
    println!("{json}");
    println!("\n→ {}", path.display());
    Ok(())
}

/// Перцентили по массиву наносекунд. Считаем по nearest-rank: выборка
/// маленькая (тысячи запросов), интерполяция только запутала бы.
pub fn percentiles(mut samples: Vec<u64>) -> Percentiles {
    if samples.is_empty() {
        return Percentiles::default();
    }
    samples.sort_unstable();
    let at = |p: f64| -> f64 {
        let rank = ((p / 100.0) * samples.len() as f64).ceil() as usize;
        let index = rank.saturating_sub(1).min(samples.len() - 1);
        samples[index] as f64 / 1000.0
    };
    Percentiles {
        count: samples.len(),
        p50_micros: at(50.0),
        p95_micros: at(95.0),
        p99_micros: at(99.0),
        max_micros: *samples.last().unwrap() as f64 / 1000.0,
        total_ms: samples.iter().sum::<u64>() as f64 / 1_000_000.0,
    }
}

#[derive(Debug, Default, Serialize)]
pub struct Percentiles {
    pub count: usize,
    pub p50_micros: f64,
    pub p95_micros: f64,
    pub p99_micros: f64,
    pub max_micros: f64,
    /// Сколько суммарно съел матчинг за всю загрузку страницы.
    pub total_ms: f64,
}
