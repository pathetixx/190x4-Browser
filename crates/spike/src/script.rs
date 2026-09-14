//! Какой текст WebView2 соглашается выполнить.
//!
//! Скрипт при создании документа (как в браузере) и `ExecuteScript` по списку
//! вариантов — у каждого свой результат. Так нашёлся нулевой символ в скрипте
//! паролей: движок принимает текст строкой, завершённой нулём, и молча
//! отрезал всё после него.
//!
//! Контроллер создаётся только в интерактивной сессии: из ssh (session 0)
//! WebView2 отвечает `0x80070578`.

use std::path::Path;
use std::sync::mpsc;

use webview2_com::{
    AddScriptToExecuteOnDocumentCreatedCompletedHandler, ExecuteScriptCompletedHandler,
    NavigationCompletedEventHandler,
};
use windows::Win32::Foundation::{E_UNEXPECTED, RECT};
use windows_core::HSTRING;

use crate::host::{pump, Host};

/// `variants` — JSON-массив пар `[метка, скрипт]`; `inject` — скрипт,
/// который встраивается в документ до навигации.
pub fn run(url: &str, variants: &Path, inject: Option<&Path>) -> anyhow::Result<()> {
    let variants: Vec<(String, String)> =
        serde_json::from_str(&std::fs::read_to_string(variants)?)?;

    let version = {
        let mut raw = windows_core::PWSTR::null();
        unsafe {
            webview2_com::Microsoft::Web::WebView2::Win32::GetAvailableCoreWebView2BrowserVersionString(
                windows_core::PCWSTR::null(),
                &mut raw,
            )?;
        }
        webview2_com::take_pwstr(raw)
    };
    println!("runtime\t{version}");

    let mut host = Host::create("spike-out/script-profile")?;
    let bounds = RECT {
        left: 0,
        top: 0,
        right: 1200,
        bottom: 800,
    };
    let index = host.open("about:blank", bounds)?;
    let core = host.views[index].core.clone();
    pump(1000);

    if let Some(path) = inject {
        let script = std::fs::read_to_string(path)?;
        let (tx, rx) = mpsc::channel();
        unsafe {
            core.AddScriptToExecuteOnDocumentCreated(
                &HSTRING::from(script.as_str()),
                &AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(
                    move |code, _| {
                        tx.send(code)
                            .map_err(|_| windows_core::Error::from(E_UNEXPECTED))
                    },
                )),
            )?;
        }
        webview2_com::wait_with_pump(rx)??;
        println!("inject\tregistered");
    }

    let (tx, rx) = mpsc::channel();
    let mut token = 0i64;
    unsafe {
        core.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |_, _| {
                let _ = tx.send(());
                Ok(())
            })),
            &mut token,
        )?;
        core.Navigate(&HSTRING::from(url))?;
    }
    webview2_com::wait_with_pump(rx)?;
    pump(1500);

    for (label, script) in variants {
        let (tx, rx) = mpsc::channel();
        unsafe {
            core.ExecuteScript(
                &HSTRING::from(script.as_str()),
                &ExecuteScriptCompletedHandler::create(Box::new(move |code, result| {
                    let _ = tx.send(code.map(|()| result));
                    Ok(())
                })),
            )?;
        }
        match webview2_com::wait_with_pump(rx)? {
            Ok(result) => println!("{label}\t{result}"),
            Err(err) => println!("{label}\terror {err}"),
        }
    }
    Ok(())
}
