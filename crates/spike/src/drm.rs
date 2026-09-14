//! Замер №3: играет ли платное видео.
//!
//! WebView2 — это Edge-рантайм, и Widevine/PlayReady в нём штатно есть. Но
//! «есть CDM» и «Кинопоиск отдаёт поток» — разные утверждения: сайт смотрит
//! на User-Agent, на уровень robustness и на то, не запущены ли мы в
//! режиме, который он считает небезопасным. Поэтому меряем в два шага:
//! машинный (EME-проба) и глазной (человек смотрит, играет ли трейлер).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;

use serde::Serialize;
use webview2_com::{take_pwstr, ExecuteScriptCompletedHandler, WebMessageReceivedEventHandler};
use windows::Win32::Foundation::{E_UNEXPECTED, RECT};
use windows_core::{HSTRING, PWSTR};

use crate::host::{pump, Host};

#[derive(Serialize)]
pub struct DrmReport {
    pub url: String,
    /// Сырой JSON от EME-пробы: по ключевой системе и уровню robustness.
    pub eme: serde_json::Value,
    pub user_agent: String,
    pub note: &'static str,
}

/// Проба EME. Спрашиваем ровно те конфигурации, которые запрашивают
/// российские онлайн-кинотеатры: Widevine L3 (SW_SECURE_DECODE) и PlayReady
/// SL2000 — на Windows сайты часто идут именно через него.
///
/// Результат уходит через `postMessage`, а НЕ возвратом значения:
/// `ExecuteScript` промис не ждёт и сериализует его как `{}` — именно на это
/// первый прогон и напоролся.
const EME_PROBE: &str = r#"
(async () => {
  const systems = [
    ["widevine",   "com.widevine.alpha"],
    ["playready",  "com.microsoft.playready.recommendation"],
    ["clearkey",   "org.w3.clearkey"]
  ];
  const robustness = ["", "SW_SECURE_CRYPTO", "SW_SECURE_DECODE", "HW_SECURE_ALL"];
  const out = { supported: {}, videoElement: !!document.createElement("video").canPlayType };

  for (const [name, keySystem] of systems) {
    out.supported[name] = {};
    for (const level of robustness) {
      try {
        const access = await navigator.requestMediaKeySystemAccess(keySystem, [{
          initDataTypes: ["cenc"],
          videoCapabilities: [{ contentType: 'video/mp4;codecs="avc1.42E01E"', robustness: level }],
          audioCapabilities: [{ contentType: 'audio/mp4;codecs="mp4a.40.2"' }]
        }]);
        const keys = await access.createMediaKeys();
        out.supported[name][level || "default"] = keys ? "ok" : "access-only";
      } catch (e) {
        out.supported[name][level || "default"] = String(e && e.name || e);
      }
    }
  }
  window.chrome.webview.postMessage(JSON.stringify(out));
})()
"#;

pub fn run(url: &str, interactive: bool) -> anyhow::Result<DrmReport> {
    let mut host = Host::create(r".\spike-userdata-drm")?;
    host.show();

    let bounds = RECT {
        left: 0,
        top: 0,
        right: 1600,
        bottom: 950,
    };
    let index = host.open(url, bounds)?;
    let core = host.views[index].core.clone();

    // Ждём, пока страница поднимет свой плеер: проба до этого момента
    // отвечает за голый документ, а не за то, что увидит пользователь.
    pump(25_000);

    let received: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    {
        let received = received.clone();
        let mut token = 0i64;
        unsafe {
            core.add_WebMessageReceived(
                &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else { return Ok(()) };
                    let mut raw = PWSTR::null();
                    args.TryGetWebMessageAsString(&mut raw)?;
                    *received.borrow_mut() = Some(take_pwstr(raw));
                    Ok(())
                })),
                &mut token,
            )?;
        }
    }

    execute(&core, EME_PROBE)?;

    // Проба асинхронная: ждём сообщение, а не возврат скрипта. 30 секунд с
    // запасом — `createMediaKeys` на первом вызове поднимает CDM с диска.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while received.borrow().is_none() && std::time::Instant::now() < deadline {
        pump(200);
    }

    let eme_raw = received
        .borrow()
        .clone()
        .unwrap_or_else(|| "{\"error\":\"проба не ответила за 30 с\"}".to_string());
    let user_agent = execute(&core, "navigator.userAgent")?;

    let eme: serde_json::Value =
        serde_json::from_str(&eme_raw).unwrap_or(serde_json::Value::String(eme_raw));

    if interactive {
        println!("\n─── Окно оставлено открытым на 3 минуты ───");
        println!("Запусти трейлер руками и смотри: играет / чёрный экран / ошибка лицензии.");
        println!("EME-проба отвечает только на «CDM на месте», не на «сайт отдал поток».");
        pump(180_000);
    }

    Ok(DrmReport {
        url: url.to_string(),
        eme,
        user_agent: user_agent.trim_matches('"').to_string(),
        note:
            "«ok» на widevine/SW_SECURE_DECODE — необходимое условие. Достаточное — только глазами.",
    })
}

fn execute(
    core: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
    script: &str,
) -> anyhow::Result<String> {
    let (tx, rx) = mpsc::channel();
    let handler = ExecuteScriptCompletedHandler::create(Box::new(move |code, result| {
        // Макрос `#[completed_callback]` уже скопировал PCWSTR в String —
        // отдельная конвертация не нужна.
        let value = code.map(|_| result);
        tx.send(value)
            .map_err(|_| windows_core::Error::from(E_UNEXPECTED))
    }));

    unsafe { core.ExecuteScript(&HSTRING::from(script), &handler)? };
    Ok(webview2_com::wait_with_pump(rx)??)
}
