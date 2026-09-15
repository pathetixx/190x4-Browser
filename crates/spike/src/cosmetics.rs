//! Скрипт документа, который вкладка встроит на адресе: скриптлеты и стиль
//! скрытия из тех же списков, что у браузера. Ради проверки рекламы в видео без
//! запуска браузера: скрипт можно подать в любой Chromium.

use std::path::Path;
use std::time::Instant;

use anyhow::Context;
use browser190x4_adblock::{
    document_host, document_script, FilterList, Guard, ListSource, Subscriptions,
};

/// Фильтр из тех же списков, что у браузера: встроенных и скачанных.
pub fn load_guard(bundled: &Path, downloaded: &Path) -> anyhow::Result<Guard> {
    let mut lists = Vec::new();
    for spec in Subscriptions::default().lists {
        let path = match &spec.source {
            ListSource::Bundled(name) => bundled.join(name),
            ListSource::Downloaded(name) => downloaded.join(name),
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => lists.push(FilterList {
                text,
                trusted: spec.trusted,
            }),
            Err(err) => eprintln!("нет списка {}: {err}", path.display()),
        }
    }
    let resources = match std::fs::read_to_string(downloaded.join("resources.json")) {
        Ok(json) => Guard::parse_resources(&json)?,
        Err(err) => {
            eprintln!("нет resources.json: {err}");
            Vec::new()
        }
    };

    let started = Instant::now();
    let count = lists.len();
    let scriptlets = resources.len();
    let guard = Guard::empty();
    guard.swap(Guard::build(lists, resources));
    println!(
        "lists {count}, scriptlets {scriptlets}, build {} ms",
        started.elapsed().as_millis()
    );
    Ok(guard)
}

pub fn run(url: &str, bundled: &Path, downloaded: &Path, out: &Path) -> anyhow::Result<()> {
    let guard = load_guard(bundled, downloaded)?;
    let cosmetics = guard.cosmetics(url);
    println!(
        "hide {} selectors, scriptlets {} bytes",
        cosmetics.hide.len(),
        cosmetics.script.len()
    );
    let host = document_host(url).context("у адреса нет хоста")?;
    let script = document_script(&host, &cosmetics).unwrap_or_default();
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(out, &script)?;
    println!("script {} bytes -> {}", script.len(), out.display());
    Ok(())
}
