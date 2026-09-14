//! Движковая часть браузера: вкладки как `ICoreWebView2Controller` поверх
//! одного HWND, поверх одного Environment.
//!
//! # Модель
//!
//! Один процесс приложения. Внутри него:
//!
//! * webview #0 — chrome браузера (создаёт Tauri). Единственный, у кого есть
//!   Tauri-IPC. Из него мы забираем `ICoreWebView2Environment` — см.
//!   [`interop::environment_of`];
//! * webview #1..N — вкладки. Создаются **нашим** кодом из того же
//!   Environment, живут на том же HWND, позиционируются через `put_Bounds`.
//!   Tauri-IPC у них нет и быть не должно: всё общение — только
//!   `PostWebMessageAsJson` в обе стороны, по схеме из [`message`].
//!
//! # Потоки
//!
//! Весь COM здесь — STA главного потока. `Tab` и `TabHost` намеренно не
//! `Send`/`Sync`: любой вызов из Tauri-команды обязан быть завёрнут в
//! `AppHandle::run_on_main_thread`.

#![cfg(windows)]

pub mod container;
pub mod downloads;
pub mod filter;
pub mod host;
pub mod interop;
pub mod message;
pub mod tab;

pub use downloads::DownloadPolicy;
pub use host::{Layout, TabHost, TabId, PAGES_HOST};
pub use message::{ChromeEvent, TabCommand};
pub use tab::Tab;
pub use tab::TabEvent;
