//! Замер №2: чего стоит синхронный матчинг в `WebResourceRequested`.
//!
//! Меряем ровно то, что будет в проде: тот же `Guard`, тот же вызов из того
//! же обработчика, на живой странице со всей её рекламой. Синтетика по
//! списку URL врала бы в обе стороны — у неё нет ни разогрева кэшей движка,
//! ни реального распределения типов ресурсов.
//!
//! Порог решения: p99 ≤ 100 µs. Обработчик висит на UI-потоке; 1000 запросов
//! по 100 µs — это 100 мс, размазанных по всей загрузке, и они незаметны.
//! Если p99 уйдёт за 500 µs — синхронную схему надо резать.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use browser190x4_adblock::{Decision, Guard, ResourceKind};
use serde::Serialize;
use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
use webview2_com::{take_pwstr, WebResourceRequestedEventHandler};
use windows::Win32::Foundation::RECT;
use windows_core::{h, PWSTR};

use crate::host::{pump, Host};
use crate::report::{percentiles, Percentiles};

#[derive(Serialize)]
pub struct AdblockReport {
    pub url: String,
    pub rules_lists: usize,
    pub build_ms: u128,
    pub blocked: usize,
    /// Первая загрузка: сюда попадает прогрев кэша скомпилированных регулярок.
    pub cold: Percentiles,
    /// Повторная загрузка той же страницы: установившийся режим, именно с ним
    /// пользователь живёт всё время, кроме первых секунд после старта.
    pub warm: Percentiles,
    /// Вердикт по порогам, чтобы не перечитывать цифры глазами.
    pub verdict: &'static str,
}

pub fn run(url: &str, lists_dir: &std::path::Path) -> anyhow::Result<AdblockReport> {
    let mut raw = Vec::new();
    for name in ["easylist.txt", "easyprivacy.txt", "ruadlist.txt"] {
        match std::fs::read_to_string(lists_dir.join(name)) {
            Ok(text) => raw.push(text),
            Err(err) => eprintln!("нет списка {name}: {err}"),
        }
    }
    anyhow::ensure!(
        !raw.is_empty(),
        "не найден ни один список в {} — замер без правил бессмыслен",
        lists_dir.display()
    );

    let started = std::time::Instant::now();
    let guard = Arc::new(Guard::empty());
    let lists_count = raw.len();
    guard.swap(Guard::build(raw));
    let build_ms = started.elapsed().as_millis();

    let mut host = Host::create(r".\spike-userdata-adblock")?;
    host.show();

    let samples: Rc<RefCell<Vec<u64>>> = Rc::new(RefCell::new(Vec::with_capacity(4096)));
    let blocked = Rc::new(RefCell::new(0usize));
    let source = Rc::new(RefCell::new(url.to_string()));

    let bounds = RECT {
        left: 0,
        top: 0,
        right: 1600,
        bottom: 950,
    };
    let index = host.open("about:blank", bounds)?;
    let core = host.views[index].core.clone();

    {
        let guard = guard.clone();
        let samples = samples.clone();
        let blocked = blocked.clone();
        let source = source.clone();
        let mut token = 0i64;

        unsafe {
            core.AddWebResourceRequestedFilter(h!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;
            core.add_WebResourceRequested(
                &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else { return Ok(()) };
                    let request = args.Request()?;
                    let mut raw_uri = PWSTR::null();
                    request.Uri(&mut raw_uri)?;
                    let uri = take_pwstr(raw_uri);

                    let mut context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
                    args.ResourceContext(&mut context)?;

                    let method = {
                        let mut raw_method = PWSTR::null();
                        request.Method(&mut raw_method)?;
                        take_pwstr(raw_method)
                    };

                    // Замер снаружи `Guard`: внутри он считает среднее, а нам
                    // нужен хвост распределения — фризы живут именно там.
                    let t0 = std::time::Instant::now();
                    let decision =
                        guard.check(&uri, &source.borrow(), map_context(context), &method);
                    samples.borrow_mut().push(t0.elapsed().as_nanos() as u64);

                    if matches!(decision, Decision::Block) {
                        *blocked.borrow_mut() += 1;
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }
    }

    unsafe { core.Navigate(&windows_core::HSTRING::from(url))? };
    // 45 секунд: первая загрузка + доскроллить рекламу ленивых блоков.
    pump(45_000);
    // Значения вынимаем до сборки отчёта: `Ref` из RefCell, созданный прямо
    // в выражении-хвосте функции, живёт дольше локальных переменных.
    let cold = percentiles(samples.borrow().clone());

    // Второй проход по той же странице. Регулярки в adblock компилируются
    // лениво, при первой встрече, — без этого замера холодный хвост не
    // отличить от честной стоимости матчинга.
    samples.borrow_mut().clear();
    unsafe { core.Reload()? };
    pump(45_000);
    let warm = percentiles(samples.borrow().clone());

    let blocked_count = *blocked.borrow();

    // Порог по одному запросу — не единственное, что важно: обработчик висит
    // на UI-потоке, поэтому смотрим и суммарную цену за загрузку (сколько
    // кадров он украл), и худший единичный вызов (заметный фриз).
    let verdict = if warm.p99_micros <= 100.0 && warm.max_micros <= 1_000.0 {
        "синхронный матчинг проходит по бюджету"
    } else if warm.total_ms <= 50.0 && warm.max_micros <= 5_000.0 {
        "синхронный матчинг приемлем: суммарная цена мала, хвост — прогрев regex-кэша"
    } else if warm.total_ms <= 150.0 {
        "на грани: нужен кэш решений по (домен, тип)"
    } else {
        "синхронную схему резать: выносить матчинг за UI-поток"
    };

    Ok(AdblockReport {
        url: url.to_string(),
        rules_lists: lists_count,
        build_ms,
        blocked: blocked_count,
        cold,
        warm,
        verdict,
    })
}

/// Тот же маппинг, что в проде. Дублируется здесь, потому что спайк не тянет
/// `browser190x4-webview` (иначе мерил бы обвязку, а не движок).
pub fn map_context(
    context: webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_WEB_RESOURCE_CONTEXT,
) -> ResourceKind {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    match context {
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT => ResourceKind::Document,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET => ResourceKind::Stylesheet,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE => ResourceKind::Image,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA => ResourceKind::Media,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT => ResourceKind::Font,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT => ResourceKind::Script,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST => ResourceKind::Xhr,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH => ResourceKind::Fetch,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_WEBSOCKET => ResourceKind::Websocket,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING => ResourceKind::Ping,
        _ => ResourceKind::Other,
    }
}
