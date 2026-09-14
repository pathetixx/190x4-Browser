//! Спайк: три замера, от которых зависят решения по архитектуре.
//!
//! 1. `memory`  — сколько ест 20 вкладок в одном Environment. Если цифра
//!    окажется рядом с Chrome/Edge на том же наборе сайтов, схема «один
//!    процесс + N контроллеров» подтверждена; если вдвое хуже — придётся
//!    усыплять фоновые вкладки с первого дня.
//! 2. `adblock` — сколько микросекунд стоит матчинг на тяжёлой странице.
//!    Порог решения: p99 ≤ 100 µs. Выше — синхронный матчинг в
//!    WebResourceRequested придётся резать (кэш по домену, bloom-префильтр).
//! 3. `drm`     — играет ли Widevine-контент (Кинопоиск) в WebView2 evergreen.
//!    Ответ «нет» ломает продуктовую часть, поэтому меряется до того, как
//!    написана хоть одна фича.
//!
//! Спайк намеренно НЕ использует Tauri: он меряет движок, а не обвязку.

#[cfg(not(windows))]
fn main() {
    eprintln!("browser190x4-spike меряет WebView2 — запускать только на Windows");
    std::process::exit(2);
}

#[cfg(windows)]
mod bench_adblock;
#[cfg(windows)]
mod cosmetics;
#[cfg(windows)]
mod drm;
#[cfg(windows)]
mod host;
#[cfg(windows)]
mod mem;
#[cfg(windows)]
mod report;
#[cfg(windows)]
mod script;

#[cfg(windows)]
use clap::{Parser, Subcommand};

#[cfg(windows)]
#[derive(Parser)]
#[command(name = "browser190x4-spike", about = "замеры для браузера 190x4")]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Куда писать JSON-отчёт.
    #[arg(long, default_value = "spike-out")]
    out: std::path::PathBuf,
}

#[cfg(windows)]
#[derive(Subcommand)]
enum Command {
    /// Память на N вкладках в одном Environment.
    Memory {
        #[arg(long, default_value_t = 20)]
        tabs: usize,
        /// Файл со списком URL (по одному на строку). По умолчанию — вшитый набор.
        #[arg(long)]
        urls: Option<std::path::PathBuf>,
        /// Пауза после загрузки перед замером, секунды: дать странице «устаканиться».
        #[arg(long, default_value_t = 20)]
        settle: u64,
    },
    /// Задержка матчинга adblock на живом трафике страницы.
    Adblock {
        #[arg(long, default_value = "https://www.kinopoisk.ru/")]
        url: String,
        /// Каталог с фильтр-списками (easylist.txt, easyprivacy.txt, ruadlist.txt).
        #[arg(long, default_value = "lists")]
        lists: std::path::PathBuf,
    },
    /// Проверка Widevine/EME и реального воспроизведения.
    Drm {
        #[arg(long, default_value = "https://www.kinopoisk.ru/")]
        url: String,
        /// Не закрывать окно: дать глазами посмотреть, играет ли видео.
        #[arg(long, default_value_t = true)]
        interactive: bool,
    },
    /// Какой текст WebView2 соглашается выполнить: встраивание и `ExecuteScript`.
    Script {
        #[arg(long, default_value = "https://the-internet.herokuapp.com/login")]
        url: String,
        /// JSON-массив пар `[метка, скрипт]`.
        #[arg(long)]
        variants: std::path::PathBuf,
        /// Скрипт, который встраивается при создании документа.
        #[arg(long)]
        inject: Option<std::path::PathBuf>,
    },
    /// Скрипт документа для адреса: скриптлеты и косметика из списков фильтров.
    Cosmetics {
        #[arg(long, default_value = "https://www.youtube.com/")]
        url: String,
        /// Встроенные списки (easylist.txt и другие).
        #[arg(long, default_value = "src-tauri/lists")]
        bundled: std::path::PathBuf,
        /// Скачанные фильтры: ubo-*.txt и resources.json.
        #[arg(long)]
        downloaded: std::path::PathBuf,
        #[arg(long, default_value = "spike-out/cosmetics.js")]
        out: std::path::PathBuf,
    },
    /// Все три подряд.
    All,
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    // WebView2 живёт в STA: без CoInitializeEx любой вызов Environment
    // падает с 0x800401F0. В приложении это делает wry за нас, здесь —
    // хост голый, поэтому инициализируем сами.
    unsafe {
        windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        )
        .ok()?;
    }

    let cli = Cli::parse();
    std::fs::create_dir_all(&cli.out)?;

    match cli.command {
        Command::Memory { tabs, urls, settle } => {
            let report = mem::run(tabs, urls.as_deref(), settle)?;
            report::write(&cli.out, "memory", &report)
        }
        Command::Adblock { url, lists } => {
            let report = bench_adblock::run(&url, &lists)?;
            report::write(&cli.out, "adblock", &report)
        }
        Command::Drm { url, interactive } => {
            let report = drm::run(&url, interactive)?;
            report::write(&cli.out, "drm", &report)
        }
        Command::Script {
            url,
            variants,
            inject,
        } => script::run(&url, &variants, inject.as_deref()),
        Command::Cosmetics {
            url,
            bundled,
            downloaded,
            out,
        } => cosmetics::run(&url, &bundled, &downloaded, &out),
        Command::All => {
            let memory = mem::run(20, None, 20)?;
            report::write(&cli.out, "memory", &memory)?;
            let adblock =
                bench_adblock::run("https://www.kinopoisk.ru/", std::path::Path::new("lists"))?;
            report::write(&cli.out, "adblock", &adblock)?;
            let drm = drm::run("https://www.kinopoisk.ru/", true)?;
            report::write(&cli.out, "drm", &drm)
        }
    }
}
