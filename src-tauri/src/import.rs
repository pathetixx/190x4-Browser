//! Перенос закладок и истории из браузеров на Chromium, установленных рядом:
//! Chrome, Edge, Яндекс Браузер, Brave, Vivaldi, Opera.
//!
//! Профили браузеров лежат в известных папках пользователя. Разбор файлов —
//! в крейте базы (`browser190x4_store::chromium`), здесь — где их искать и
//! как прочитать базу истории, которую браузер держит открытой: её копия
//! читается, пока оригинал не тронут.

use std::path::{Path, PathBuf};

use browser190x4_store::chromium;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::App;

/// Где лежат данные браузера.
enum Root {
    Local,
    Roaming,
}

/// Браузер: ярлык, имя, папка данных и одна ли в ней папка профиля (Opera).
const KNOWN: [(&str, &str, Root, &str, bool); 7] = [
    ("chrome", "Google Chrome", Root::Local, r"Google\Chrome\User Data", false),
    ("edge", "Microsoft Edge", Root::Local, r"Microsoft\Edge\User Data", false),
    ("yandex", "Яндекс Браузер", Root::Local, r"Yandex\YandexBrowser\User Data", false),
    ("brave", "Brave", Root::Local, r"BraveSoftware\Brave-Browser\User Data", false),
    ("vivaldi", "Vivaldi", Root::Local, r"Vivaldi\User Data", false),
    ("opera", "Opera", Root::Roaming, r"Opera Software\Opera Stable", true),
    ("opera-gx", "Opera GX", Root::Roaming, r"Opera Software\Opera GX Stable", true),
];

/// Историю старше этого браузер не переносит — Chrome и сам столько хранит.
const HISTORY_DAYS: i64 = 90;
/// Посещений на странице истории — не больше стольких последних.
const VISITS_LIMIT: u32 = 20_000;

#[derive(Serialize)]
pub struct Source {
    id: &'static str,
    name: &'static str,
    profiles: Vec<Profile>,
}

#[derive(Clone, Serialize)]
pub struct Profile {
    /// Папка профиля внутри данных браузера — по ней профиль и выбирают.
    dir: String,
    /// Как профиль называется в самом браузере.
    name: String,
    #[serde(skip)]
    path: PathBuf,
}

#[derive(Default, Serialize)]
pub struct Imported {
    links: usize,
    folders: usize,
    skipped: usize,
    pages: usize,
    visits: usize,
}

fn root_dir(root: &Root) -> Option<PathBuf> {
    let var = match root {
        Root::Local => "LOCALAPPDATA",
        Root::Roaming => "APPDATA",
    };
    std::env::var_os(var).map(PathBuf::from)
}

/// Профили с закладками или историей. Имена — из `Local State` браузера.
fn profiles(data: &Path, single: bool) -> Vec<Profile> {
    let usable = |path: &Path| path.join("Bookmarks").exists() || path.join("History").exists();
    if single {
        // У новой Opera профиль в папке Default, у старой — прямо в папке данных.
        let path = if usable(&data.join("Default")) {
            data.join("Default")
        } else {
            data.to_path_buf()
        };
        if !usable(&path) {
            return Vec::new();
        }
        return vec![Profile {
            dir: String::new(),
            name: String::new(),
            path,
        }];
    }

    let names: Vec<(String, String)> = std::fs::read_to_string(data.join("Local State"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|state| state.pointer("/profile/info_cache").cloned())
        .and_then(|cache| cache.as_object().cloned())
        .map(|cache| {
            cache
                .iter()
                .map(|(dir, info)| {
                    let name = info.get("name").and_then(|name| name.as_str());
                    (dir.clone(), name.unwrap_or(dir).to_string())
                })
                .collect()
        })
        .unwrap_or_else(|| vec![("Default".to_string(), String::new())]);

    let mut found: Vec<Profile> = names
        .into_iter()
        // Имя папки — из чужого файла: только имя, без путей.
        .filter(|(dir, _)| !dir.is_empty() && !dir.contains(['/', '\\', '.']))
        .map(|(dir, name)| Profile {
            path: data.join(&dir),
            dir,
            name,
        })
        .filter(|profile| usable(&profile.path))
        .collect();
    // Основной профиль — первым.
    found.sort_by_key(|profile| (profile.dir != "Default", profile.dir.clone()));
    found
}

fn sources() -> Vec<Source> {
    KNOWN
        .iter()
        .filter_map(|(id, name, root, relative, single)| {
            let data = root_dir(root)?.join(relative);
            let profiles = profiles(&data, *single);
            (!profiles.is_empty()).then_some(Source {
                id: *id,
                name: *name,
                profiles,
            })
        })
        .collect()
}

/// Браузеры на Chromium, из которых есть что перенести.
#[tauri::command(async)]
pub fn browsers_found() -> Vec<Source> {
    sources()
}

/// Перенести закладки и (или) историю из профиля другого браузера.
#[tauri::command(async)]
pub fn browser_import(
    app: AppHandle,
    browser: String,
    profile: String,
    bookmarks: bool,
    history: bool,
) -> Result<Imported, String> {
    // Профиль ищется заново среди найденных: путь из интерфейса не принимаем.
    let source = sources()
        .into_iter()
        .find(|source| source.id == browser)
        .ok_or("этот браузер не найден")?;
    let target = source
        .profiles
        .iter()
        .find(|candidate| candidate.dir == profile)
        .ok_or("этот профиль не найден")?;
    let state = app.state::<App>();
    let store = &state.store;
    let mut imported = Imported::default();

    if bookmarks {
        if let Ok(json) = std::fs::read_to_string(target.path.join("Bookmarks")) {
            let nodes = chromium::parse_bookmarks(&json).map_err(|err| err.to_string())?;
            let report = store.import_bookmarks(&nodes).map_err(|err| err.to_string())?;
            imported.links = report.links;
            imported.folders = report.folders;
            imported.skipped = report.skipped;
            let _ = app.emit("bookmarks", ());
        }
    }

    if history && target.path.join("History").exists() {
        let (pages, visits) = read_history_copy(&target.path).map_err(|err| {
            tracing::warn!(%err, browser = source.name, "история не прочитана");
            format!(
                "История {} не прочиталась — закройте браузер и повторите",
                source.name
            )
        })?;
        let (pages, visits) = store
            .import_history(&pages, &visits)
            .map_err(|err| err.to_string())?;
        imported.pages = pages;
        imported.visits = visits;
    }
    Ok(imported)
}

type History = (Vec<chromium::ImportedPage>, Vec<chromium::ImportedVisit>);

/// Базу истории браузер держит открытой — читается её копия.
fn read_history_copy(profile: &Path) -> anyhow::Result<History> {
    let copy = crate::profile_dir().join(format!("import-history-{}", std::process::id()));
    let result = copy_and_read(&profile.join("History"), &copy);
    let _ = std::fs::remove_file(&copy);
    let _ = std::fs::remove_file(wal(&copy));
    result
}

fn copy_and_read(history: &Path, copy: &Path) -> anyhow::Result<History> {
    std::fs::copy(history, copy)?;
    // Свежие посещения могут ещё лежать в журнале базы.
    if wal(history).exists() {
        std::fs::copy(wal(history), wal(copy))?;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    chromium::read_history(copy, now - HISTORY_DAYS * 86_400, VISITS_LIMIT)
}

/// Журнал базы SQLite рядом с ней: `History-wal`.
fn wal(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push("-wal");
    PathBuf::from(name)
}
