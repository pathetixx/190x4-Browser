//! Замер №1: память на N вкладках в одном Environment.
//!
//! Что именно считаем: сумму private bytes по всем процессам Environment
//! (браузерный, GPU, рендереры, утилиты) плюс наш процесс. Working set
//! показываем рядом, но решение принимаем по private: именно он не
//! схлопывается при нехватке RAM и именно его увидит пользователь в
//! диспетчере задач как «сколько жрёт браузер».

use serde::Serialize;
use windows::Win32::Foundation::{CloseHandle, RECT};
use windows::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
};

use crate::host::{pump, Host};

/// Набор по умолчанию: тяжёлые страницы, на которых браузеры и разъезжаются.
/// Специально смешаны SPA, ленты с бесконечным скроллом, видео и магазины.
const DEFAULT_URLS: &[&str] = &[
    "https://www.kinopoisk.ru/",
    "https://dzen.ru/",
    "https://vk.com/",
    "https://www.youtube.com/",
    "https://habr.com/ru/feed/",
    "https://lenta.ru/",
    "https://www.ozon.ru/",
    "https://www.wildberries.ru/",
    "https://www.avito.ru/",
    "https://rutube.ru/",
    "https://pikabu.ru/",
    "https://github.com/explore",
    "https://www.reddit.com/",
    "https://mail.ru/",
    "https://www.gismeteo.ru/",
    "https://tass.ru/",
    "https://sports.ru/",
    "https://www.twitch.tv/",
    "https://ya.ru/",
    "https://t.me/s/durov",
];

#[derive(Serialize)]
pub struct MemoryReport {
    pub tabs_requested: usize,
    pub settle_seconds: u64,
    pub steps: Vec<Step>,
    /// Прирост на вкладку между первым и последним шагом — то число, ради
    /// которого всё затевалось.
    pub marginal_mb_per_tab: f64,
}

#[derive(Serialize)]
pub struct Step {
    pub tabs: usize,
    pub processes: usize,
    pub private_mb: f64,
    pub working_set_mb: f64,
}

pub fn run(
    tabs: usize,
    urls: Option<&std::path::Path>,
    settle: u64,
) -> anyhow::Result<MemoryReport> {
    let list: Vec<String> = match urls {
        Some(path) => std::fs::read_to_string(path)?
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(str::to_string)
            .collect(),
        None => DEFAULT_URLS.iter().map(|s| s.to_string()).collect(),
    };

    let mut host = Host::create(r".\spike-userdata")?;
    host.show();

    // Точки замера: пусто → 1 → 5 → 10 → N. Промежуточные нужны, чтобы
    // отделить постоянную цену движка от цены вкладки.
    let checkpoints: Vec<usize> = [1usize, 5, 10, tabs]
        .into_iter()
        .filter(|c| *c <= tabs)
        .collect();

    let mut steps = vec![measure(&host, 0)?];
    let bounds = RECT {
        left: 0,
        top: 0,
        right: 1600,
        bottom: 950,
    };

    for opened in 1..=tabs {
        let url = list[(opened - 1) % list.len()].clone();
        host.open(&url, bounds)?;
        // Даём странице стартовать до открытия следующей: 20 одновременных
        // навигаций упрутся в сеть и замер поедет.
        pump(3_000);

        if checkpoints.contains(&opened) {
            println!("вкладок {opened}/{tabs}, отстаиваем {settle} с");
            pump(settle * 1_000);
            steps.push(measure(&host, opened)?);
        }
    }

    let marginal = match (steps.first(), steps.last()) {
        (Some(first), Some(last)) if last.tabs > 0 => {
            (last.private_mb - first.private_mb) / last.tabs as f64
        }
        _ => 0.0,
    };

    Ok(MemoryReport {
        tabs_requested: tabs,
        settle_seconds: settle,
        steps,
        marginal_mb_per_tab: (marginal * 10.0).round() / 10.0,
    })
}

fn measure(host: &Host, tabs: usize) -> anyhow::Result<Step> {
    let mut pids: Vec<u32> = host
        .process_ids()?
        .into_iter()
        .map(|(pid, _)| pid)
        .collect();
    pids.push(unsafe { GetCurrentProcessId() });

    let mut private = 0u64;
    let mut working = 0u64;
    for pid in &pids {
        if let Some((p, w)) = process_memory(*pid) {
            private += p;
            working += w;
        }
    }

    Ok(Step {
        tabs,
        processes: pids.len(),
        private_mb: bytes_to_mb(private),
        working_set_mb: bytes_to_mb(working),
    })
}

/// (private bytes, working set) одного процесса.
fn process_memory(pid: u32) -> Option<(u64, u64)> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid).ok()?;
        let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
        let ok = GetProcessMemoryInfo(
            handle,
            &mut counters as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        );
        let _ = CloseHandle(handle);
        ok.ok()?;
        Some((counters.PrivateUsage as u64, counters.WorkingSetSize as u64))
    }
}

fn bytes_to_mb(bytes: u64) -> f64 {
    ((bytes as f64 / 1024.0 / 1024.0) * 10.0).round() / 10.0
}
