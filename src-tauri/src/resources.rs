//! Монитор ресурсов браузера — для новой вкладки.
//!
//! Браузер — не один процесс. Это наш exe (окно, вкладки, фильтр) и все его
//! дочерние процессы: процесс браузера WebView2, страницы, видеокарта, сеть,
//! звук, сборщик отчётов о сбоях. Роли процессов и страницы в них сообщает
//! движок (`GetProcessExtendedInfos`); всё, что он не называет, находится по
//! дереву процессов и считается службами.
//!
//! Цифры считаются так же, как в диспетчере задач Windows:
//! * память — частный рабочий набор (`PrivateWorkingSetSize`);
//! * процессор — время всех ядер за интервал между замерами;
//! * видеокарта — самый загруженный движок GPU процесса (счётчики PDH
//!   `GPU Engine`), видеопамять — `GPU Process Memory`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use browser190x4_webview::{EngineProcess, TabBrief};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::AppHandle;

use crate::state::with_host;

/// Замер свежее этого отдаётся из кэша: страницу могут открыть в двух вкладках.
const FRESH: Duration = Duration::from_millis(700);
/// Список процессов движка перечитывается не чаще.
const ENGINE_TTL: Duration = Duration::from_millis(2500);
/// Процессорное время за интервал длиннее этого — уже не «сейчас», а среднее
/// за минуты: такой замер процессор не показывает.
const STALE: Duration = Duration::from_secs(5);
const HISTORY: usize = 60;

#[derive(Default)]
pub struct Monitor {
    sampler: Mutex<Sampler>,
}

#[derive(Clone, Serialize)]
pub struct Report {
    pub cpus: usize,
    pub browser: Totals,
    pub system: System,
    pub groups: Vec<Group>,
    pub history: Vec<Point>,
}

#[derive(Clone, Serialize)]
pub struct Totals {
    /// Проценты от всех ядер; `None` — первый замер, сравнивать ещё не с чем.
    pub cpu: Option<f64>,
    pub memory: u64,
    pub gpu: Option<f64>,
    pub gpu_memory: Option<u64>,
    pub processes: usize,
}

#[derive(Clone, Serialize)]
pub struct System {
    pub cpu: Option<f64>,
    pub memory_total: u64,
    pub memory_used: u64,
}

/// Часть браузера: окно, вкладка, видеокарта, службы.
#[derive(Clone, Serialize)]
pub struct Group {
    /// `interface`, `tab`, `tabs`, `background`, `gpu` или `services`.
    pub kind: &'static str,
    pub title: String,
    /// Адрес вкладки — по нему страница подставляет значок сайта.
    pub url: Option<String>,
    pub tabs: Vec<u32>,
    pub cpu: Option<f64>,
    pub memory: u64,
    pub gpu: Option<f64>,
    pub processes: usize,
}

#[derive(Clone, Copy, Default, Serialize)]
pub struct Point {
    pub cpu: f64,
    pub memory: u64,
    pub gpu: f64,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Role {
    Interface,
    Tabs(Vec<u32>),
    Background,
    Gpu,
    Services,
}

#[derive(Default)]
struct Sampler {
    engine: Option<(Instant, Vec<EngineProcess>, Vec<TabBrief>)>,
    at: Option<Instant>,
    cpu_times: HashMap<u32, u64>,
    system_times: Option<(u64, u64)>,
    gpu: Option<Gpu>,
    gpu_failed: bool,
    history: VecDeque<Point>,
    last: Option<(Instant, Report)>,
}

impl Monitor {
    /// Замер для страницы. Блокирующий: звать не с главного потока.
    pub fn report(&self, app: &AppHandle) -> Report {
        let engine_stale = {
            let sampler = self.sampler.lock();
            if let Some((at, report)) = &sampler.last {
                if at.elapsed() < FRESH {
                    return report.clone();
                }
            }
            match &sampler.engine {
                Some((at, ..)) => at.elapsed() > ENGINE_TTL,
                None => true,
            }
        };

        // Список процессов — с главного потока, поэтому без лока замера.
        let engine = if engine_stale { load_engine(app) } else { None };

        let mut sampler = self.sampler.lock();
        if let Some((processes, tabs)) = engine {
            sampler.engine = Some((Instant::now(), processes, tabs));
        }
        let report = sampler.sample();
        sampler.last = Some((Instant::now(), report.clone()));
        report
    }
}

fn load_engine(app: &AppHandle) -> Option<(Vec<EngineProcess>, Vec<TabBrief>)> {
    let (tx, rx) = mpsc::channel();
    let tabs = with_host(app, move |host| {
        let reply = tx.clone();
        if let Err(err) = host.engine_processes(move |processes| {
            let _ = reply.send(processes);
        }) {
            tracing::debug!(%err, "процессы движка недоступны");
            let _ = tx.send(Vec::new());
        }
        host.tabs_brief()
    })
    .ok()?;
    let processes = rx.recv_timeout(Duration::from_secs(2)).ok()?;
    Some((processes, tabs))
}

impl Sampler {
    fn sample(&mut self) -> Report {
        let now = Instant::now();
        let interval = self.at.map(|at| now - at).filter(|gap| *gap <= STALE);
        self.at = Some(now);
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);

        let (processes, tabs) = match &self.engine {
            Some((_, processes, tabs)) => (processes.clone(), tabs.clone()),
            None => (Vec::new(), Vec::new()),
        };

        // Кто входит в браузер: наш процесс, всё, что назвал движок, и все
        // потомки нашего процесса.
        let own = std::process::id();
        let mut members: HashMap<u32, Role> = HashMap::new();
        members.insert(own, Role::Interface);
        for process in &processes {
            members.insert(process.pid, role_of(process, &tabs));
        }
        for pid in descendants(&process_tree(), own) {
            members.entry(pid).or_insert(Role::Services);
        }

        let (gpu_usage, gpu_memory) = self.gpu_usage();
        let has_gpu = self.gpu.is_some();

        let mut cpu_times = HashMap::new();
        let mut groups: HashMap<Role, Group> = HashMap::new();
        let mut totals = Totals {
            cpu: interval.map(|_| 0.0),
            memory: 0,
            gpu: has_gpu.then_some(0.0),
            gpu_memory: has_gpu.then_some(0),
            processes: 0,
        };

        for (pid, role) in &members {
            let Some((cpu_time, memory)) = process_stats(*pid) else {
                continue;
            };
            cpu_times.insert(*pid, cpu_time);
            let cpu = interval.map(|gap| {
                let previous = self.cpu_times.get(pid).copied().unwrap_or(cpu_time);
                cpu_time.saturating_sub(previous) as f64 / 1e7 / gap.as_secs_f64() / cpus as f64
                    * 100.0
            });
            let gpu = has_gpu.then(|| gpu_usage.get(pid).copied().unwrap_or(0.0));

            let group = groups
                .entry(role.clone())
                .or_insert_with(|| describe(role, &tabs));
            group.processes += 1;
            group.memory += memory;
            if let (Some(sum), Some(value)) = (group.cpu.as_mut(), cpu) {
                *sum += value;
            } else if group.processes == 1 {
                group.cpu = cpu;
            }
            if let (Some(sum), Some(value)) = (group.gpu.as_mut(), gpu) {
                *sum += value;
            } else if group.processes == 1 {
                group.gpu = gpu;
            }

            totals.processes += 1;
            totals.memory += memory;
            if let (Some(sum), Some(value)) = (totals.cpu.as_mut(), cpu) {
                *sum += value;
            }
            if let (Some(sum), Some(value)) = (totals.gpu.as_mut(), gpu) {
                *sum += value;
            }
            if let Some(sum) = totals.gpu_memory.as_mut() {
                *sum += gpu_memory.get(pid).copied().unwrap_or(0);
            }
        }
        self.cpu_times = cpu_times;

        let system = self.system(interval.is_some());

        self.history.push_back(Point {
            cpu: totals.cpu.unwrap_or(0.0),
            memory: totals.memory,
            gpu: totals.gpu.unwrap_or(0.0),
        });
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }

        let mut groups: Vec<Group> = groups.into_values().collect();
        groups.sort_by_key(|group| std::cmp::Reverse(group.memory));

        Report {
            cpus,
            browser: totals,
            system,
            groups,
            history: self.history.iter().copied().collect(),
        }
    }

    fn gpu_usage(&mut self) -> (HashMap<u32, f64>, HashMap<u32, u64>) {
        if self.gpu.is_none() && !self.gpu_failed {
            self.gpu = Gpu::open();
            self.gpu_failed = self.gpu.is_none();
            if self.gpu_failed {
                tracing::debug!("счётчики видеокарты недоступны");
            }
        }
        match &self.gpu {
            Some(gpu) => gpu.read(),
            None => Default::default(),
        }
    }

    fn system(&mut self, with_cpu: bool) -> System {
        use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

        let mut status = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        let (memory_total, memory_used) = match unsafe { GlobalMemoryStatusEx(&mut status) } {
            Ok(()) => (
                status.ullTotalPhys,
                status.ullTotalPhys.saturating_sub(status.ullAvailPhys),
            ),
            Err(_) => (0, 0),
        };

        let times = system_times();
        let cpu = match (self.system_times, times) {
            (Some((idle_before, total_before)), Some((idle, total))) if with_cpu => {
                let busy_total = total.saturating_sub(total_before);
                (busy_total > 0).then(|| {
                    (busy_total.saturating_sub(idle.saturating_sub(idle_before))) as f64
                        / busy_total as f64
                        * 100.0
                })
            }
            _ => None,
        };
        self.system_times = times;

        System {
            cpu,
            memory_total,
            memory_used,
        }
    }
}

/// Роль процесса по данным движка.
fn role_of(process: &EngineProcess, tabs: &[TabBrief]) -> Role {
    match process.kind {
        "browser" => Role::Interface,
        "gpu" => Role::Gpu,
        "renderer" => {
            let mut ids: Vec<u32> = tabs
                .iter()
                .filter(|tab| process.pages.contains(&tab.url))
                .map(|tab| tab.id)
                .collect();
            ids.sort_unstable();
            ids.dedup();
            if !ids.is_empty() {
                Role::Tabs(ids)
            } else if process.pages.iter().any(|page| is_interface(page)) {
                Role::Interface
            } else {
                Role::Background
            }
        }
        _ => Role::Services,
    }
}

/// Окно браузера и его всплывающее окно — страницы Tauri.
fn is_interface(url: &str) -> bool {
    url.starts_with("http://tauri.localhost")
        || url.starts_with("https://tauri.localhost")
        || url.starts_with("tauri://")
}

fn describe(role: &Role, tabs: &[TabBrief]) -> Group {
    let title_of = |id: &u32| {
        tabs.iter()
            .find(|tab| tab.id == *id)
            .map(|tab| {
                if tab.title.trim().is_empty() {
                    tab.url.clone()
                } else {
                    tab.title.trim().to_string()
                }
            })
            .unwrap_or_default()
    };
    let (kind, title, url, ids) = match role {
        Role::Interface => ("interface", "Окно браузера".to_string(), None, Vec::new()),
        Role::Gpu => ("gpu", "Видеокарта".to_string(), None, Vec::new()),
        Role::Services => (
            "services",
            "Сеть, звук и другие службы".to_string(),
            None,
            Vec::new(),
        ),
        Role::Background => (
            "background",
            "Фоновые страницы".to_string(),
            None,
            Vec::new(),
        ),
        Role::Tabs(ids) => {
            let url = ids
                .first()
                .and_then(|id| tabs.iter().find(|tab| tab.id == *id))
                .map(|tab| tab.url.clone());
            if ids.len() == 1 {
                ("tab", title_of(&ids[0]), url, ids.clone())
            } else {
                let names: Vec<String> = ids.iter().map(title_of).collect();
                (
                    "tabs",
                    format!("Вкладки: {}", names.join(", ")),
                    url,
                    ids.clone(),
                )
            }
        }
    };
    Group {
        kind,
        title,
        url,
        tabs: ids,
        cpu: None,
        memory: 0,
        gpu: None,
        processes: 0,
    }
}

fn filetime(time: windows::Win32::Foundation::FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

/// Процессорное время (100 нс) и частный рабочий набор процесса.
fn process_stats(pid: u32) -> Option<(u64, u64)> {
    use windows::Win32::Foundation::{CloseHandle, FILETIME};
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX2,
    };
    use windows::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let times = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user);
        let mut counters = PROCESS_MEMORY_COUNTERS_EX2 {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32,
            ..Default::default()
        };
        let memory = GetProcessMemoryInfo(
            handle,
            &mut counters as *mut PROCESS_MEMORY_COUNTERS_EX2 as *mut PROCESS_MEMORY_COUNTERS,
            counters.cb,
        );
        let _ = CloseHandle(handle);
        times.ok()?;
        memory.ok()?;
        Some((
            filetime(kernel) + filetime(user),
            counters.PrivateWorkingSetSize as u64,
        ))
    }
}

/// Простой и полное время всех ядер (100 нс).
fn system_times() -> Option<(u64, u64)> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetSystemTimes;

    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).ok()? };
    // Время ядра включает простой.
    Some((filetime(idle), filetime(kernel) + filetime(user)))
}

/// Родитель каждого процесса системы.
fn process_tree() -> HashMap<u32, u32> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut parents = HashMap::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return parents;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    parents
}

fn descendants(parents: &HashMap<u32, u32>, root: u32) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, parent) in parents {
        if pid != parent {
            children.entry(*parent).or_default().push(*pid);
        }
    }
    let mut out = Vec::new();
    let mut seen = HashSet::from([root]);
    let mut queue = vec![root];
    while let Some(pid) = queue.pop() {
        for child in children.get(&pid).into_iter().flatten() {
            if seen.insert(*child) {
                out.push(*child);
                queue.push(*child);
            }
        }
    }
    out
}

/// Счётчики видеокарты: загрузка движков и видеопамять по процессам.
struct Gpu {
    query: windows::Win32::System::Performance::PDH_HQUERY,
    engine: windows::Win32::System::Performance::PDH_HCOUNTER,
    memory: windows::Win32::System::Performance::PDH_HCOUNTER,
}

// SAFETY: дескрипторы PDH не привязаны к потоку, а обращения к ним идут только
// под мьютексом монитора.
unsafe impl Send for Gpu {}

impl Gpu {
    fn open() -> Option<Self> {
        use windows::core::{w, PCWSTR};
        use windows::Win32::System::Performance::{
            PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhOpenQueryW, PDH_HCOUNTER,
            PDH_HQUERY,
        };

        unsafe {
            let mut query = PDH_HQUERY(std::ptr::null_mut());
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            let mut engine = PDH_HCOUNTER(std::ptr::null_mut());
            let mut memory = PDH_HCOUNTER(std::ptr::null_mut());
            if PdhAddEnglishCounterW(
                query,
                w!("\\GPU Engine(*)\\Utilization Percentage"),
                0,
                &mut engine,
            ) != 0
                || PdhAddEnglishCounterW(
                    query,
                    w!("\\GPU Process Memory(*)\\Dedicated Usage"),
                    0,
                    &mut memory,
                ) != 0
            {
                PdhCloseQuery(query);
                return None;
            }
            // Загрузка — счётчик-скорость: первое чтение только запоминает точку.
            PdhCollectQueryData(query);
            Some(Self {
                query,
                engine,
                memory,
            })
        }
    }

    fn read(&self) -> (HashMap<u32, f64>, HashMap<u32, u64>) {
        use windows::Win32::System::Performance::PdhCollectQueryData;

        if unsafe { PdhCollectQueryData(self.query) } != 0 {
            return Default::default();
        }

        // Как в диспетчере задач: у процесса берётся самый загруженный тип
        // движка (3D, видео, копирование), значения одного типа складываются.
        let mut engines: HashMap<(u32, String), f64> = HashMap::new();
        for (name, value) in counter_values(self.engine) {
            let Some(pid) = instance_pid(&name) else {
                continue;
            };
            let kind = name
                .rsplit_once("engtype_")
                .map(|(_, kind)| kind.to_string())
                .unwrap_or_default();
            *engines.entry((pid, kind)).or_default() += value;
        }
        let mut usage: HashMap<u32, f64> = HashMap::new();
        for ((pid, _), value) in engines {
            let entry = usage.entry(pid).or_default();
            *entry = entry.max(value.min(100.0));
        }

        let mut memory: HashMap<u32, u64> = HashMap::new();
        for (name, value) in counter_values(self.memory) {
            if let Some(pid) = instance_pid(&name) {
                *memory.entry(pid).or_default() += value.max(0.0) as u64;
            }
        }
        (usage, memory)
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::System::Performance::PdhCloseQuery(self.query);
        }
    }
}

/// Экземпляры счётчика с именами: `pid_1234_luid_…_engtype_3D`.
fn counter_values(
    counter: windows::Win32::System::Performance::PDH_HCOUNTER,
) -> Vec<(String, f64)> {
    use windows::Win32::System::Performance::{
        PdhGetFormattedCounterArrayW, PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA,
        PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_MORE_DATA,
    };

    // Между запросом размера и чтением могут появиться новые экземпляры —
    // тогда размер спрашивается заново.
    for _ in 0..3 {
        unsafe {
            let mut size = 0u32;
            let mut count = 0u32;
            let status =
                PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None);
            if status != PDH_MORE_DATA || size == 0 {
                return Vec::new();
            }
            let item = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            let mut buffer: Vec<PDH_FMT_COUNTERVALUE_ITEM_W> =
                Vec::with_capacity((size as usize).div_ceil(item) + 1);
            let status = PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                Some(buffer.as_mut_ptr()),
            );
            if status == PDH_MORE_DATA {
                continue;
            }
            if status != 0 {
                return Vec::new();
            }
            let items = std::slice::from_raw_parts(buffer.as_ptr(), count as usize);
            return items
                .iter()
                .filter(|item| {
                    item.FmtValue.CStatus == PDH_CSTATUS_VALID_DATA
                        || item.FmtValue.CStatus == PDH_CSTATUS_NEW_DATA
                })
                .map(|item| {
                    (
                        item.szName.to_string().unwrap_or_default(),
                        item.FmtValue.Anonymous.doubleValue,
                    )
                })
                .collect();
        }
    }
    Vec::new()
}

fn instance_pid(name: &str) -> Option<u32> {
    name.strip_prefix("pid_")?.split('_').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: u32, url: &str) -> TabBrief {
        TabBrief {
            id,
            url: url.to_string(),
            title: format!("Вкладка {id}"),
        }
    }

    #[test]
    fn renderer_goes_to_its_tabs_or_interface() {
        let tabs = [
            tab(1, "https://youtube.com/"),
            tab(2, "https://github.com/"),
        ];
        let process = |pages: &[&str]| EngineProcess {
            pid: 10,
            kind: "renderer",
            pages: pages.iter().map(|page| page.to_string()).collect(),
        };
        assert!(role_of(&process(&["https://github.com/"]), &tabs) == Role::Tabs(vec![2]));
        assert!(
            role_of(
                &process(&["https://github.com/", "https://youtube.com/"]),
                &tabs
            ) == Role::Tabs(vec![1, 2])
        );
        assert!(
            role_of(&process(&["http://tauri.localhost/index.html"]), &tabs) == Role::Interface
        );
        assert!(role_of(&process(&[]), &tabs) == Role::Background);
    }

    #[test]
    fn descendants_follow_the_whole_tree() {
        let parents = HashMap::from([(2, 1), (3, 2), (4, 3), (5, 9), (1, 0)]);
        let mut found = descendants(&parents, 1);
        found.sort_unstable();
        assert_eq!(found, vec![2, 3, 4]);
    }

    #[test]
    fn gpu_instance_names_carry_the_pid() {
        assert_eq!(
            instance_pid("pid_1234_luid_0x00000000_0x0000EB2D_phys_0_eng_0_engtype_3D"),
            Some(1234)
        );
        assert_eq!(instance_pid("_Total"), None);
    }
}
