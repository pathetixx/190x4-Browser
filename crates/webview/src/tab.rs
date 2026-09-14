//! Одна вкладка = один `ICoreWebView2Controller` на общем HWND окна.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};

use browser190x4_adblock::Guard;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Controller, ICoreWebView2Environment, ICoreWebView2Find,
    ICoreWebView2_15,
};
use webview2_com::{
    take_pwstr, AddScriptToExecuteOnDocumentCreatedCompletedHandler,
    DocumentTitleChangedEventHandler, FaviconChangedEventHandler, HistoryChangedEventHandler,
    NavigationCompletedEventHandler, NavigationStartingEventHandler,
    NewWindowRequestedEventHandler, SourceChangedEventHandler, WebMessageReceivedEventHandler,
    ZoomFactorChangedEventHandler,
};
use windows::Win32::Foundation::RECT;
use windows_core::{Interface, HSTRING, PWSTR};

use crate::downloads::{self, SharedDownloads};
use crate::filter::{self, SourceUrl};
use crate::host::TabId;

/// Менеджер паролей на стороне страницы: сообщает о формах входа и
/// отправленных логинах. Решения принимает Rust — см. `src-tauri/src/passwords.rs`.
const PASSWORDS_SCRIPT: &str = include_str!("inject/passwords.js");

/// Скрипт паролей в том виде, в каком его получает движок: без строк-комментариев
/// — они нужны в исходнике, а не в каждом документе каждой вкладки.
///
/// Движок принимает скрипт строкой, завершённой нулём: символ U+0000 в тексте
/// молча отрезает всё после него, и скрипт не выполняется вовсе. Тест ниже не
/// пускает в скрипт управляющие символы.
static PASSWORDS_ENGINE: LazyLock<String> = LazyLock::new(|| engine_script(PASSWORDS_SCRIPT));

fn engine_script(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Ступени масштаба — как в Chrome: пользователь привык к этим числам.
const ZOOM_STEPS: &[f64] = &[
    0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0,
];

/// Что вкладка сообщает chrome-у. Уходит в UI одним `emit`, не через IPC
/// вкладки — у вкладки IPC нет.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TabEvent {
    /// Контроллер готов, вкладка живёт. Приходит раньше навигации: до этого
    /// момента у вкладки есть только номер.
    Opened {
        id: u32,
        url: String,
    },
    Started {
        id: u32,
        url: String,
    },
    Finished {
        id: u32,
        ok: bool,
        http_status: i32,
        /// Адрес документа после навигации. Навигация, ушедшая в загрузку,
        /// документ не меняет — и адрес остаётся прежним.
        url: String,
    },
    Title {
        id: u32,
        title: String,
    },
    Url {
        id: u32,
        url: String,
    },
    History {
        id: u32,
        can_back: bool,
        can_forward: bool,
    },
    /// Страница попросила открыть новое окно — мы вместо этого открываем вкладку.
    Popup {
        opener: u32,
        url: String,
    },
    /// `postMessage` со страницы: перевод, найденное видео и т.п.
    ///
    /// `source` — адрес документа по данным движка. Именно по нему решается,
    /// кому сообщение можно доверить: подделать его страница не может.
    Message {
        id: u32,
        source: String,
        payload: String,
    },
    /// Иконка сайта сменилась. `page` — адрес страницы, к которой она относится.
    Favicon {
        id: u32,
        page: String,
        url: String,
    },
    /// Масштаб страницы (1.0 — 100%).
    Zoom {
        id: u32,
        factor: f64,
    },
    /// Выбран наш пункт контекстного меню.
    ///
    /// `payload` — то, к чему пункт относится: выделенный текст или адрес
    /// медиа. Это данные со страницы, обращаться с ними как с недоверенными.
    MenuAction {
        id: u32,
        action: &'static str,
        payload: String,
    },
    /// Звук вкладки: играет ли что-то и заглушена ли она.
    Audio {
        id: u32,
        audible: bool,
        muted: bool,
    },
    /// Поиск по странице: сколько совпадений и какое активно.
    Find {
        id: u32,
        total: i32,
        current: i32,
    },
    /// Загрузка файла одним событием с полем `phase`: started, progress,
    /// paused, done, failed, cancelled. `key` — номер операции в реестре
    /// хоста, по нему загрузкой управляют.
    Download {
        id: u32,
        key: u64,
        phase: &'static str,
        url: String,
        path: String,
        bytes: i64,
        total: Option<i64>,
        error: &'static str,
    },
    /// Настройка «всегда спрашивать, куда сохранить»: загрузка ждёт ответа
    /// в [`crate::downloads::answer`]. `path` — что предложить в диалоге.
    DownloadAsk {
        id: u32,
        key: u64,
        path: String,
    },
    /// Горячая клавиша, нажатая пока фокус был на странице.
    ///
    /// Без этого Ctrl+K, Ctrl+T и Alt+← работают только когда фокус в
    /// chrome-е: нативная вкладка забирает клавиатуру себе, и интерфейс
    /// выглядит сломанным.
    Shortcut {
        id: u32,
        combo: String,
    },
}

pub type EventSink = Rc<dyn Fn(TabEvent)>;

pub struct Tab {
    pub id: TabId,
    controller: ICoreWebView2Controller,
    core: ICoreWebView2,
    source: SourceUrl,
    visible: bool,
}

/// Перехват клавиш, принадлежащих браузеру, пока фокус на странице.
///
/// WebView2 отдаёт их до того, как страница их увидит. Всё, что относится к
/// chrome-у, помечаем `Handled` и пересылаем наверх; остальное — не наше дело
/// (Ctrl+C, Ctrl+A и прочее должно работать на странице как обычно).
fn wire_accelerators(
    id: TabId,
    controller: &ICoreWebView2Controller,
    sink: EventSink,
) -> windows_core::Result<()> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN, COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, GetKeyState, VIRTUAL_KEY, VK_CONTROL, VK_MENU, VK_SHIFT,
    };

    let mut token = 0i64;
    let id = id.0;

    unsafe {
        controller.add_AcceleratorKeyPressed(
            &webview2_com::AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };

                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN;
                args.KeyEventKind(&mut kind)?;
                if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                    && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
                {
                    return Ok(());
                }

                let mut key = 0u32;
                args.VirtualKey(&mut key)?;

                // Старший бит — «клавиша сейчас нажата».
                // Фокус живёт в окне процесса WebView2: очередь ввода нашего
                // потока может не знать, что Ctrl зажат. Смотрим и её, и
                // физическое состояние клавиатуры.
                let down = |vk: VIRTUAL_KEY| {
                    (GetKeyState(vk.0 as i32) as u16 & 0x8000) != 0
                        || (GetAsyncKeyState(vk.0 as i32) as u16 & 0x8000) != 0
                };
                let ctrl = down(VK_CONTROL);
                let shift = down(VK_SHIFT);
                let alt = down(VK_MENU);

                let Some(combo) = classify(key, ctrl, shift, alt) else {
                    return Ok(());
                };

                args.SetHandled(true)?;
                tracing::debug!(tab = id, %combo, "page shortcut");
                sink(TabEvent::Shortcut { id, combo });
                Ok(())
            })),
            &mut token,
        )?;
    }

    Ok(())
}

/// Настройки вкладки.
///
/// Главное здесь — выключить встроенные акселераторы движка. Иначе Ctrl+F
/// перехватывает сам WebView2 и показывает СВОЙ диалог поиска: до
/// `AcceleratorKeyPressed` клавиша уже не доходит, и строка поиска браузера
/// не открывается. То же касается Ctrl+P, F12 и прочих его умолчаний —
/// браузер здесь мы, а не движок.
pub(crate) fn configure(core: &ICoreWebView2) -> windows_core::Result<()> {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3;

    let settings = unsafe { core.Settings()? };
    if let Ok(settings3) = settings.cast::<ICoreWebView2Settings3>() {
        unsafe { settings3.SetAreBrowserAcceleratorKeysEnabled(false)? };
    }
    Ok(())
}

/// Звук вкладки.
///
/// Нужен для значка динамика в строке вкладок: «эта вкладка сейчас орёт» —
/// первое, что пользователь хочет знать, когда звук пошёл непонятно откуда.
/// Интерфейса старше 1.0.774 может не быть — тогда просто живём без значка.
fn wire_audio(id: TabId, core: &ICoreWebView2, sink: EventSink) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_8;
    use webview2_com::{IsDocumentPlayingAudioChangedEventHandler, IsMutedChangedEventHandler};

    let Ok(core8) = core.cast::<ICoreWebView2_8>() else {
        return;
    };

    let tab_id = id.0;
    let mut token = 0i64;

    let report = move |core8: &ICoreWebView2_8, sink: &EventSink| -> windows_core::Result<()> {
        let mut audible = windows_core::BOOL::default();
        let mut muted = windows_core::BOOL::default();
        unsafe {
            core8.IsDocumentPlayingAudio(&mut audible)?;
            core8.IsMuted(&mut muted)?;
        }
        sink(TabEvent::Audio {
            id: tab_id,
            audible: audible.as_bool(),
            muted: muted.as_bool(),
        });
        Ok(())
    };

    unsafe {
        let playing_sink = sink.clone();
        let playing_core = core8.clone();
        let playing_report = report;
        let _ = core8.add_IsDocumentPlayingAudioChanged(
            &IsDocumentPlayingAudioChangedEventHandler::create(Box::new(move |_, _| {
                playing_report(&playing_core, &playing_sink)
            })),
            &mut token,
        );

        let muted_sink = sink;
        let muted_core = core8.clone();
        let _ = core8.add_IsMutedChanged(
            &IsMutedChangedEventHandler::create(Box::new(move |_, _| {
                report(&muted_core, &muted_sink)
            })),
            &mut token,
        );
    }
}

/// Свои пункты в контекстном меню страницы.
///
/// Меню остаётся нативным — его рисует движок поверх страницы. Своё, в HTML,
/// потребовало бы overlay-режима, то есть страница исчезала бы ровно в тот
/// момент, когда пользователь щёлкает по её элементу. Поэтому мы только
/// добавляем пункты в существующее меню.
fn wire_context_menu(
    id: TabId,
    core: &ICoreWebView2,
    env: &ICoreWebView2Environment,
    sink: EventSink,
) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Environment9, ICoreWebView2_11, COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND,
        COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_AUDIO,
        COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_SELECTED_TEXT,
        COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_VIDEO,
    };
    use webview2_com::{ContextMenuRequestedEventHandler, CustomItemSelectedEventHandler};

    let (Ok(core11), Ok(env9)) = (
        core.cast::<ICoreWebView2_11>(),
        env.cast::<ICoreWebView2Environment9>(),
    ) else {
        tracing::warn!("движок не умеет расширять контекстное меню");
        return;
    };

    let tab_id = id.0;
    let mut token = 0i64;

    let result = unsafe {
        core11.add_ContextMenuRequested(
            &ContextMenuRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };

                let target = args.ContextMenuTarget()?;
                let mut kind = COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_SELECTED_TEXT;
                target.Kind(&mut kind)?;

                let items = args.MenuItems()?;
                let mut count = 0u32;
                items.Count(&mut count)?;

                // Наши пункты идут первыми: то, ради чего браузер и делался,
                // не должно прятаться под «Сохранить как».
                let mut position = 0u32;

                if kind == COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_SELECTED_TEXT {
                    let mut raw = PWSTR::null();
                    target.SelectionText(&mut raw)?;
                    let text = take_pwstr(raw);

                    let item = env9.CreateContextMenuItem(
                        &HSTRING::from("Перевести выделенное"),
                        None,
                        COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND,
                    )?;
                    let selected_sink = sink.clone();
                    let mut item_token = 0i64;
                    item.add_CustomItemSelected(
                        &CustomItemSelectedEventHandler::create(Box::new(move |_, _| {
                            selected_sink(TabEvent::MenuAction {
                                id: tab_id,
                                action: "translate",
                                payload: text.clone(),
                            });
                            Ok(())
                        })),
                        &mut item_token,
                    )?;
                    items.InsertValueAtIndex(position, &item)?;
                    position += 1;
                }

                if kind == COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_VIDEO
                    || kind == COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_AUDIO
                {
                    let mut raw = PWSTR::null();
                    target.SourceUri(&mut raw)?;
                    let source = take_pwstr(raw);

                    let item = env9.CreateContextMenuItem(
                        &HSTRING::from("Скачать через 190x4"),
                        None,
                        COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND,
                    )?;
                    let media_sink = sink.clone();
                    let mut item_token = 0i64;
                    item.add_CustomItemSelected(
                        &CustomItemSelectedEventHandler::create(Box::new(move |_, _| {
                            media_sink(TabEvent::MenuAction {
                                id: tab_id,
                                action: "download_media",
                                payload: source.clone(),
                            });
                            Ok(())
                        })),
                        &mut item_token,
                    )?;
                    items.InsertValueAtIndex(position, &item)?;
                }

                Ok(())
            })),
            &mut token,
        )
    };

    if let Err(err) = result {
        tracing::warn!(%err, "контекстное меню не расширено");
    }
}

/// Отчёт о состоянии поиска: сколько нашли и на каком совпадении стоим.
fn report_find(id: u32, find: &ICoreWebView2Find, sink: &EventSink) -> windows_core::Result<()> {
    let mut total = 0i32;
    let mut current = 0i32;
    unsafe {
        find.MatchCount(&mut total)?;
        find.ActiveMatchIndex(&mut current)?;
    }
    sink(TabEvent::Find { id, total, current });
    Ok(())
}

/// Комбинации, которые принадлежат браузеру, а не странице.
///
/// Список закрытый: всё, чего здесь нет, достаётся странице как обычно
/// (Ctrl+C, Ctrl+A, Ctrl+Z и прочее).
fn classify(key: u32, ctrl: bool, shift: bool, alt: bool) -> Option<String> {
    const VK_TAB: u32 = 0x09;
    const VK_LEFT: u32 = 0x25;
    const VK_RIGHT: u32 = 0x27;
    const VK_DELETE: u32 = 0x2E;
    const VK_F5: u32 = 0x74;
    const VK_F12: u32 = 0x7B;
    const VK_ADD: u32 = 0x6B;
    const VK_SUBTRACT: u32 = 0x6D;
    const VK_NUMPAD0: u32 = 0x60;
    const VK_OEM_PLUS: u32 = 0xBB;
    const VK_OEM_MINUS: u32 = 0xBD;

    let combo = match (ctrl, shift, alt, key) {
        (true, false, false, 0x4B) => "ctrl+k", // командная палитра
        (true, false, false, 0x54) => "ctrl+t", // новая вкладка
        (true, false, false, 0x57) => "ctrl+w", // закрыть вкладку
        (true, false, false, 0x4C) => "ctrl+l", // адресная строка
        (true, false, false, 0x52) => "ctrl+r", // обновить
        (true, false, false, 0x44) => "ctrl+d", // в закладки
        (true, false, false, 0x46) => "ctrl+f", // поиск по странице
        (true, false, false, 0x4A) => "ctrl+j", // загрузки
        (true, false, false, 0x48) => "ctrl+h", // история
        (true, false, false, 0x50) => "ctrl+p", // печать
        (true, false, false, 0x30) | (true, false, false, VK_NUMPAD0) => "ctrl+0",
        (true, _, false, VK_OEM_PLUS) | (true, false, false, VK_ADD) => "ctrl+=",
        (true, false, false, VK_OEM_MINUS) | (true, false, false, VK_SUBTRACT) => "ctrl+-",
        (true, false, false, VK_TAB) => "ctrl+tab",
        (true, true, false, VK_TAB) => "ctrl+shift+tab",
        (true, true, false, 0x54) => "ctrl+shift+t", // вернуть закрытую вкладку
        (true, true, false, 0x52) => "ctrl+shift+r", // обновить без кэша
        (true, true, false, 0x44) => "ctrl+shift+d", // загрузчик видео
        (true, true, false, 0x42) => "ctrl+shift+b", // панель закладок
        (true, true, false, 0x4F) => "ctrl+shift+o", // диспетчер закладок
        (true, true, false, VK_DELETE) => "ctrl+shift+delete",
        (true, false, false, 0x31..=0x39) => return Some(format!("ctrl+{}", key - 0x30)),
        (false, false, true, VK_LEFT) => "alt+left",
        (false, false, true, VK_RIGHT) => "alt+right",
        (false, false, false, VK_F5) => "f5",
        (false, false, false, VK_F12) => "f12",
        _ => return None,
    };
    Some(combo.to_string())
}

/// Скрипты, которые движок вставляет в каждый документ до его собственных.
fn inject_scripts(core: &ICoreWebView2) -> windows_core::Result<()> {
    unsafe {
        core.AddScriptToExecuteOnDocumentCreated(
            &HSTRING::from(PASSWORDS_ENGINE.as_str()),
            &AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(|code, _id| {
                if let Err(err) = code {
                    tracing::warn!(%err, "скрипт паролей не встроен");
                }
                Ok(())
            })),
        )
    }
}

/// Масштаб страницы меняется и колесом с Ctrl — интерфейс должен об этом знать.
fn wire_zoom(
    id: TabId,
    controller: &ICoreWebView2Controller,
    sink: EventSink,
) -> windows_core::Result<()> {
    let id = id.0;
    let mut token = 0i64;
    unsafe {
        controller.add_ZoomFactorChanged(
            &ZoomFactorChangedEventHandler::create(Box::new(move |sender, _| {
                let Some(sender) = sender else { return Ok(()) };
                let mut factor = 1.0f64;
                sender.ZoomFactor(&mut factor)?;
                sink(TabEvent::Zoom { id, factor });
                Ok(())
            })),
            &mut token,
        )
    }
}

impl Tab {
    /// Обернуть готовый контроллер.
    ///
    /// Ждать контроллер здесь нельзя: `CreateCoreWebView2Controller`
    /// асинхронный, а единственный способ дождаться его синхронно — крутить
    /// вложенный насос сообщений. Внутри обработчика события tao (а именно
    /// туда попадает `run_on_main_thread`) такой насос не получает
    /// завершение операции, и вкладка не создаётся никогда — проверено на
    /// первом живом запуске. Поэтому ожидание живёт в [`crate::host`], в
    /// completion-коллбеке, а сюда контроллер приходит готовым.
    #[allow(clippy::too_many_arguments)]
    pub fn from_controller(
        id: TabId,
        controller: ICoreWebView2Controller,
        env: &ICoreWebView2Environment,
        guard: Arc<Guard>,
        sink: EventSink,
        downloads: SharedDownloads,
        bounds: RECT,
        visible: bool,
    ) -> anyhow::Result<Self> {
        let core = unsafe { controller.CoreWebView2()? };
        let source: SourceUrl = Rc::new(RefCell::new(String::new()));

        unsafe {
            controller.SetBounds(bounds)?;
            controller.SetIsVisible(visible)?;
        }

        configure(&core)?;
        inject_scripts(&core)?;
        filter::install(&core, env, guard, source.clone())?;
        wire_accelerators(id, &controller, sink.clone())?;
        downloads::wire(id, &core, downloads, sink.clone())?;
        wire_zoom(id, &controller, sink.clone())?;
        wire_audio(id, &core, sink.clone());
        wire_context_menu(id, &core, env, sink.clone());

        let tab = Self {
            id,
            controller,
            core,
            source,
            visible,
        };
        tab.wire_events(sink)?;
        tracing::debug!(?id, "вкладка готова");
        Ok(tab)
    }

    fn wire_events(&self, sink: EventSink) -> anyhow::Result<()> {
        let id = self.id.0;
        let core = &self.core;
        let mut token = 0i64;

        unsafe {
            let s = sink.clone();
            let source = self.source.clone();
            core.add_NavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else { return Ok(()) };
                    let mut raw = PWSTR::null();
                    args.Uri(&mut raw)?;
                    let url = take_pwstr(raw);
                    // Источник для third-party обновляем ровно здесь: до
                    // первого запроса ресурсов страницы.
                    *source.borrow_mut() = url.clone();
                    s(TabEvent::Started { id, url });
                    Ok(())
                })),
                &mut token,
            )?;

            let s = sink.clone();
            let source = self.source.clone();
            core.add_NavigationCompleted(
                &NavigationCompletedEventHandler::create(Box::new(move |sender, args| {
                    let Some(args) = args else { return Ok(()) };
                    let mut ok = windows_core::BOOL::default();
                    args.IsSuccess(&mut ok)?;
                    let mut status = 0i32;
                    // HTTP-статус живёт в ICoreWebView2NavigationCompletedEventArgs2;
                    // на старом evergreen интерфейса может не быть — это не повод падать.
                    if let Ok(args2) = args.cast::<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2NavigationCompletedEventArgs2>() {
                        let _ = args2.HttpStatusCode(&mut status);
                    }
                    let url = match &sender {
                        Some(core) => {
                            let mut raw = PWSTR::null();
                            core.Source(&mut raw)?;
                            take_pwstr(raw)
                        }
                        None => String::new(),
                    };
                    // Навигация, ушедшая в загрузку, документ не сменила: источник
                    // для third-party и автозаполнения — снова адрес документа.
                    if !url.is_empty() {
                        *source.borrow_mut() = url.clone();
                    }
                    s(TabEvent::Finished { id, ok: ok.as_bool(), http_status: status, url });
                    Ok(())
                })),
                &mut token,
            )?;

            let s = sink.clone();
            core.add_DocumentTitleChanged(
                &DocumentTitleChangedEventHandler::create(Box::new(move |sender, _| {
                    let Some(sender) = sender else { return Ok(()) };
                    let mut raw = PWSTR::null();
                    sender.DocumentTitle(&mut raw)?;
                    s(TabEvent::Title {
                        id,
                        title: take_pwstr(raw),
                    });
                    Ok(())
                })),
                &mut token,
            )?;

            let s = sink.clone();
            core.add_SourceChanged(
                &SourceChangedEventHandler::create(Box::new(move |sender, _| {
                    let Some(sender) = sender else { return Ok(()) };
                    let mut raw = PWSTR::null();
                    sender.Source(&mut raw)?;
                    s(TabEvent::Url {
                        id,
                        url: take_pwstr(raw),
                    });
                    Ok(())
                })),
                &mut token,
            )?;

            let s = sink.clone();
            core.add_HistoryChanged(
                &HistoryChangedEventHandler::create(Box::new(move |sender, _| {
                    let Some(sender) = sender else { return Ok(()) };
                    let mut back = windows_core::BOOL::default();
                    let mut fwd = windows_core::BOOL::default();
                    sender.CanGoBack(&mut back)?;
                    sender.CanGoForward(&mut fwd)?;
                    s(TabEvent::History {
                        id,
                        can_back: back.as_bool(),
                        can_forward: fwd.as_bool(),
                    });
                    Ok(())
                })),
                &mut token,
            )?;

            // window.open / target=_blank: собственное окно WebView2 нам не
            // нужно — гасим Handled и открываем вкладку у себя.
            let s = sink.clone();
            core.add_NewWindowRequested(
                &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else { return Ok(()) };
                    let mut raw = PWSTR::null();
                    args.Uri(&mut raw)?;
                    args.SetHandled(true)?;
                    s(TabEvent::Popup {
                        opener: id,
                        url: take_pwstr(raw),
                    });
                    Ok(())
                })),
                &mut token,
            )?;

            // Единственный канал «страница → приложение». Payload — JSON от
            // недоверенного кода: парсить в строгий enum, никогда не
            // подставлять в пути и аргументы процессов как есть.
            let s = sink.clone();
            core.add_WebMessageReceived(
                &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else { return Ok(()) };
                    let mut raw = PWSTR::null();
                    args.WebMessageAsJson(&mut raw)?;
                    let payload = take_pwstr(raw);
                    let mut raw = PWSTR::null();
                    args.Source(&mut raw)?;
                    s(TabEvent::Message {
                        id,
                        source: take_pwstr(raw),
                        payload,
                    });
                    Ok(())
                })),
                &mut token,
            )?;

            // Иконка сайта. Интерфейса нет на совсем старом рантайме — тогда
            // вкладки живут с глобусом.
            if let Ok(core15) = core.cast::<ICoreWebView2_15>() {
                let s = sink.clone();
                core15.add_FaviconChanged(
                    &FaviconChangedEventHandler::create(Box::new(move |sender, _| {
                        let Some(sender) = sender else { return Ok(()) };
                        let core15: ICoreWebView2_15 = sender.cast()?;
                        let mut raw = PWSTR::null();
                        core15.FaviconUri(&mut raw)?;
                        let url = take_pwstr(raw);
                        let mut raw = PWSTR::null();
                        sender.Source(&mut raw)?;
                        s(TabEvent::Favicon {
                            id,
                            page: take_pwstr(raw),
                            url,
                        });
                        Ok(())
                    })),
                    &mut token,
                )?;
            }
        }

        Ok(())
    }

    pub fn navigate(&self, url: &str) -> windows_core::Result<()> {
        unsafe { self.core.Navigate(&HSTRING::from(url)) }
    }

    pub fn reload(&self) -> windows_core::Result<()> {
        unsafe { self.core.Reload() }
    }

    pub fn go_back(&self) -> windows_core::Result<()> {
        unsafe { self.core.GoBack() }
    }

    pub fn go_forward(&self) -> windows_core::Result<()> {
        unsafe { self.core.GoForward() }
    }

    /// Chrome → страница. Обратное направление — `TabEvent::Message`.
    pub fn post(&self, json: &str) -> windows_core::Result<()> {
        unsafe { self.core.PostWebMessageAsJson(&HSTRING::from(json)) }
    }

    pub fn set_bounds(&self, bounds: RECT) -> windows_core::Result<()> {
        unsafe { self.controller.SetBounds(bounds) }
    }

    /// Скрытая вкладка не рисуется и не ест кадры, но остаётся живой —
    /// именно так мы уводим её из-под оверлеев chrome-а.
    pub fn set_visible(&mut self, visible: bool) -> windows_core::Result<()> {
        if self.visible != visible {
            unsafe { self.controller.SetIsVisible(visible)? };
            self.visible = visible;
        }
        Ok(())
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    /// Иконка сайта — через ICoreWebView2_15. На старом evergreen интерфейса
    /// нет: вкладка тогда живёт с дефолтной иконкой, а не падает.
    pub fn favicon_url(&self) -> Option<String> {
        let core15: ICoreWebView2_15 = self.core.cast().ok()?;
        let mut raw = PWSTR::null();
        unsafe { core15.FaviconUri(&mut raw).ok()? };
        Some(take_pwstr(raw))
    }

    pub fn close(self) -> windows_core::Result<()> {
        unsafe { self.controller.Close() }
    }

    pub fn core(&self) -> &ICoreWebView2 {
        &self.core
    }

    /// Заглушить или вернуть звук.
    pub fn set_muted(&self, muted: bool) -> anyhow::Result<()> {
        use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_8;

        let core8: ICoreWebView2_8 = self
            .core
            .cast()
            .map_err(|_| anyhow::anyhow!("движок не умеет управлять звуком вкладки"))?;
        unsafe { core8.SetIsMuted(muted)? };
        Ok(())
    }

    /// Поиск по странице.
    ///
    /// Свой диалог движка подавляем: строка поиска нарисована в chrome и
    /// живёт по его правилам, а два поля ввода на экране — это брак.
    pub fn find(
        &self,
        env: &ICoreWebView2Environment,
        query: &str,
        sink: EventSink,
    ) -> anyhow::Result<()> {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            ICoreWebView2Environment15, ICoreWebView2_28,
        };
        use webview2_com::{
            FindActiveMatchIndexChangedEventHandler, FindMatchCountChangedEventHandler,
            FindStartCompletedHandler,
        };

        let env15: ICoreWebView2Environment15 = env
            .cast()
            .map_err(|_| anyhow::anyhow!("движок не умеет искать по странице"))?;
        let core28 = self
            .core
            .cast::<ICoreWebView2_28>()
            .map_err(|_| anyhow::anyhow!("движок не умеет искать по странице"))?;
        let find: ICoreWebView2Find = unsafe { core28.Find()? };

        let id = self.id.0;
        let mut token = 0i64;

        unsafe {
            let options = env15.CreateFindOptions()?;
            options.SetFindTerm(&HSTRING::from(query))?;
            options.SetIsCaseSensitive(false)?;
            options.SetShouldHighlightAllMatches(true)?;
            options.SetSuppressDefaultFindDialog(true)?;

            let count_find = find.clone();
            let count_sink = sink.clone();
            find.add_MatchCountChanged(
                &FindMatchCountChangedEventHandler::create(Box::new(move |_, _| {
                    report_find(id, &count_find, &count_sink)
                })),
                &mut token,
            )?;

            let index_find = find.clone();
            let index_sink = sink.clone();
            find.add_ActiveMatchIndexChanged(
                &FindActiveMatchIndexChangedEventHandler::create(Box::new(move |_, _| {
                    report_find(id, &index_find, &index_sink)
                })),
                &mut token,
            )?;

            find.Start(
                &options,
                &FindStartCompletedHandler::create(Box::new(move |code| {
                    code?;
                    Ok(())
                })),
            )?;
        }

        Ok(())
    }

    pub fn find_step(&self, forward: bool) -> anyhow::Result<()> {
        let find = self.find_handle()?;
        unsafe {
            if forward {
                find.FindNext()?;
            } else {
                find.FindPrevious()?;
            }
        }
        Ok(())
    }

    pub fn find_stop(&self) -> anyhow::Result<()> {
        let find = self.find_handle()?;
        unsafe { find.Stop()? };
        Ok(())
    }

    fn find_handle(&self) -> anyhow::Result<ICoreWebView2Find> {
        use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_28;

        let core28 = self
            .core
            .cast::<ICoreWebView2_28>()
            .map_err(|_| anyhow::anyhow!("движок не умеет искать по странице"))?;
        Ok(unsafe { core28.Find()? })
    }

    /// Адрес текущего документа — по данным движка, а не chrome-а.
    pub fn source_url(&self) -> String {
        let mut raw = PWSTR::null();
        match unsafe { self.core.Source(&mut raw) } {
            Ok(()) => take_pwstr(raw),
            Err(_) => String::new(),
        }
    }

    /// Диалог печати браузерного вида (с предпросмотром), как в Edge.
    pub fn print(&self) -> anyhow::Result<()> {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            ICoreWebView2_16, COREWEBVIEW2_PRINT_DIALOG_KIND_BROWSER,
        };
        let core16: ICoreWebView2_16 = self
            .core
            .cast()
            .map_err(|_| anyhow::anyhow!("движок не умеет печатать"))?;
        unsafe { core16.ShowPrintUI(COREWEBVIEW2_PRINT_DIALOG_KIND_BROWSER)? };
        Ok(())
    }

    pub fn open_devtools(&self) -> windows_core::Result<()> {
        unsafe { self.core.OpenDevToolsWindow() }
    }

    /// Шаг масштаба: `1` — крупнее, `-1` — мельче, `0` — сбросить. Возвращает
    /// новый масштаб.
    pub fn zoom(&self, direction: i32) -> anyhow::Result<f64> {
        let mut current = 1.0f64;
        unsafe { self.controller.ZoomFactor(&mut current)? };
        let next = match direction.signum() {
            0 => 1.0,
            1 => ZOOM_STEPS
                .iter()
                .copied()
                .find(|step| *step > current + 0.001)
                .unwrap_or(current),
            _ => ZOOM_STEPS
                .iter()
                .rev()
                .copied()
                .find(|step| *step < current - 0.001)
                .unwrap_or(current),
        };
        unsafe { self.controller.SetZoomFactor(next)? };
        Ok(next)
    }

    /// Удалить данные сайтов и/или кэш профиля. Профиль общий на все вкладки,
    /// поэтому вызывать можно на любой из них.
    pub fn clear_browsing_data(&self, site_data: bool, cache: bool) -> anyhow::Result<()> {
        use webview2_com::ClearBrowsingDataCompletedHandler;
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            ICoreWebView2Profile2, ICoreWebView2_13, COREWEBVIEW2_BROWSING_DATA_KINDS,
            COREWEBVIEW2_BROWSING_DATA_KINDS_ALL_SITE, COREWEBVIEW2_BROWSING_DATA_KINDS_DISK_CACHE,
        };

        let mut kinds = 0;
        if site_data {
            kinds |= COREWEBVIEW2_BROWSING_DATA_KINDS_ALL_SITE.0;
        }
        if cache {
            kinds |= COREWEBVIEW2_BROWSING_DATA_KINDS_DISK_CACHE.0;
        }
        if kinds == 0 {
            return Ok(());
        }

        let profile: ICoreWebView2Profile2 = unsafe {
            self.core
                .cast::<ICoreWebView2_13>()
                .map_err(|_| anyhow::anyhow!("движок не умеет удалять данные"))?
                .Profile()?
                .cast()
                .map_err(|_| anyhow::anyhow!("движок не умеет удалять данные"))?
        };
        unsafe {
            profile.ClearBrowsingData(
                COREWEBVIEW2_BROWSING_DATA_KINDS(kinds),
                &ClearBrowsingDataCompletedHandler::create(Box::new(|code| {
                    if let Err(err) = code {
                        tracing::warn!(%err, "данные браузера не удалены");
                    }
                    Ok(())
                })),
            )?;
        }
        Ok(())
    }

    /// Отдать папку встроенных страниц (новая вкладка, ошибки) по
    /// `http://<host>/…`.
    ///
    /// Альтернатива — `file://`, но тогда страницы получают origin файла со
    /// всеми его ограничениями (нет localStorage, нет fetch, странный CSP).
    /// `DENY_CORS` запрещает чужим сайтам тянуть оттуда что-либо.
    pub fn map_pages(&self, host: &str, folder: &std::path::Path) -> windows_core::Result<()> {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            ICoreWebView2_3, COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY_CORS,
        };

        let core3: ICoreWebView2_3 = self.core.cast()?;
        unsafe {
            core3.SetVirtualHostNameToFolderMapping(
                &HSTRING::from(host),
                &HSTRING::from(folder.as_os_str()),
                COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY_CORS,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_script_drops_comment_lines() {
        assert_eq!(
            engine_script("// комментарий\nconst a = 1;\n  // ещё\n  a;"),
            "const a = 1;\n  a;"
        );
        assert!(engine_script(PASSWORDS_SCRIPT).contains("__190x4Passwords"));
    }

    /// Движок обрезает скрипт на нулевом символе; прочие управляющие символы в
    /// исходнике тоже не нужны — в строках для них есть `\u` и `\n`.
    #[test]
    fn password_script_has_no_control_characters() {
        let bad = PASSWORDS_SCRIPT
            .char_indices()
            .find(|(_, ch)| ch.is_control() && *ch != '\n');
        assert_eq!(bad, None);
    }
}
