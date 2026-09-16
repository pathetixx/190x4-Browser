//! Одна вкладка = один `ICoreWebView2Controller` на общем HWND окна.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};

use browser190x4_adblock::Guard;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2ContextMenuItemCollection,
    ICoreWebView2ContextMenuRequestedEventArgs, ICoreWebView2ContextMenuTarget,
    ICoreWebView2Controller, ICoreWebView2Deferral, ICoreWebView2Environment, ICoreWebView2Find,
    ICoreWebView2Frame, ICoreWebView2Frame2, ICoreWebView2Frame5, ICoreWebView2Frame7,
    ICoreWebView2_15, ICoreWebView2_4,
};
use webview2_com::{
    take_pwstr, AddScriptToExecuteOnDocumentCreatedCompletedHandler,
    DocumentTitleChangedEventHandler, FaviconChangedEventHandler,
    FrameChildFrameCreatedEventHandler, FrameCreatedEventHandler, FrameDestroyedEventHandler,
    FrameWebMessageReceivedEventHandler, HistoryChangedEventHandler,
    NavigationCompletedEventHandler, NavigationStartingEventHandler,
    NewWindowRequestedEventHandler, SourceChangedEventHandler, WebMessageReceivedEventHandler,
    ZoomFactorChangedEventHandler,
};
use windows::Win32::Foundation::{POINT, RECT};
use windows_core::{Interface, BOOL, HSTRING, PWSTR};

use crate::dialogs::{self, DialogAnswer, DialogRequest, Dialogs};
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
    /// Вкладка так и не родилась: движок не отдал контроллер. Без этого
    /// события в строке вкладок осталась бы пустая вкладка-призрак.
    OpenFailed {
        id: u32,
    },
    /// Сколько запросов фильтр снял на этой вкладке с начала загрузки
    /// страницы. Счётчик у каждой вкладки свой — общий по браузеру показывать
    /// в адресной строке нельзя.
    Blocked {
        id: u32,
        count: u64,
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
        /// Фрейм, из которого пришло сообщение; `None` — документ вкладки.
        #[serde(skip_serializing_if = "Option::is_none")]
        frame: Option<u32>,
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
    /// Правый щелчок по странице. Меню рисует chrome; движок ждёт ответа
    /// в [`Tab::context_menu_done`] с тем же `menu`.
    ///
    /// Цель щелчка — данные со страницы: адреса и выделенный текст обращать
    /// только как с недоверенными.
    ContextMenu {
        id: u32,
        menu: u64,
        /// Точка щелчка в физических пикселях от левого верхнего угла вкладки.
        x: i32,
        y: i32,
        target: MenuTarget,
        items: Vec<MenuItem>,
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
    /// Страница просит окно: alert, confirm, prompt, «Покинуть сайт?»,
    /// разрешение, вход по паролю или запуск приложения. Окно рисует chrome,
    /// движок ждёт ответа в [`Tab::dialog_done`] с тем же `token`. Запуск
    /// приложения движок уже отменил и ответа не ждёт.
    Dialog {
        id: u32,
        token: u64,
        request: DialogRequest,
    },
    /// Окна, ответа на которые больше не ждут: страница ушла.
    DialogsClosed {
        id: u32,
        tokens: Vec<u64>,
    },
}

/// По чему щёлкнули правой кнопкой.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MenuTarget {
    /// `page`, `image`, `selection`, `audio` или `video`.
    pub kind: &'static str,
    pub page_url: String,
    pub frame_url: String,
    pub main_frame: bool,
    pub editable: bool,
    pub link_url: Option<String>,
    pub link_text: Option<String>,
    pub source_url: Option<String>,
    pub selection: Option<String>,
}

/// Пункт меню движка: команда, её подпись и состояние.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MenuItem {
    /// Неизменное имя пункта (`copy`, `saveImageAs`, `spellCheck`…) — по нему
    /// chrome решает, что показать и как подписать.
    pub name: String,
    pub label: String,
    pub command: i32,
    pub shortcut: String,
    /// `command`, `checkbox`, `radio`, `separator` или `submenu`.
    pub kind: &'static str,
    pub enabled: bool,
    pub checked: bool,
    pub children: Vec<MenuItem>,
}

/// Меню страницы, которое ждёт выбора пользователя.
struct PendingMenu {
    token: u64,
    args: ICoreWebView2ContextMenuRequestedEventArgs,
    deferral: ICoreWebView2Deferral,
}

type MenuSlot = Rc<RefCell<Option<PendingMenu>>>;

/// Фреймы вкладки по номеру движка: ответ на сообщение уходит в тот же фрейм.
type FrameMap = Rc<RefCell<HashMap<u32, ICoreWebView2Frame2>>>;

pub type EventSink = Rc<dyn Fn(TabEvent)>;

pub struct Tab {
    pub id: TabId,
    controller: ICoreWebView2Controller,
    core: ICoreWebView2,
    source: SourceUrl,
    visible: bool,
    menu: MenuSlot,
    frames: FrameMap,
    dialogs: Dialogs,
    /// Обработчики поиска вешаются на объект `Find` вкладки один раз: он у
    /// вкладки один, и повторная подписка на каждый набранный символ
    /// размножала бы события счётчика.
    find_wired: Cell<bool>,
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

/// Контекстное меню страницы рисует браузер.
///
/// Движок отдаёт свои пункты (команда, подпись, состояние) и цель щелчка, а
/// показывает меню всплывающее окно chrome-а — то же, что у остальных меню,
/// поэтому страница под ним не прячется. Пока пользователь выбирает, событие
/// держится отсрочкой; выбранную команду выполняет сам движок через
/// `SelectedCommandId` — «Вставить», «Сохранить картинку как» и подсказки
/// орфографии работают так же, как в его собственном меню.
fn wire_context_menu(id: TabId, core: &ICoreWebView2, sink: EventSink, slot: MenuSlot) {
    use webview2_com::ContextMenuRequestedEventHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_11;

    let Ok(core11) = core.cast::<ICoreWebView2_11>() else {
        tracing::warn!("движок не отдаёт контекстное меню");
        return;
    };

    let tab_id = id.0;
    let counter = Rc::new(Cell::new(0u64));
    let mut token = 0i64;

    let result = unsafe {
        core11.add_ContextMenuRequested(
            &ContextMenuRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };

                // Меню, на которое так и не ответили, закрываем без выбора.
                if let Some(stale) = slot.borrow_mut().take() {
                    let _ = stale.deferral.Complete();
                }

                tracing::debug!(tab = tab_id, "запрошено меню страницы");
                let (target, items, point, deferral) = match intercept_menu(&args) {
                    Ok(parts) => parts,
                    Err(err) => {
                        // Ошибка до SetHandled оставляет меню движка: причину — в лог.
                        tracing::warn!(tab = tab_id, %err, "меню страницы не перехвачено");
                        return Ok(());
                    }
                };
                let menu = counter.get().wrapping_add(1);
                counter.set(menu);
                tracing::debug!(
                    tab = tab_id,
                    menu,
                    kind = target.kind,
                    items = items.len(),
                    "меню страницы"
                );
                *slot.borrow_mut() = Some(PendingMenu {
                    token: menu,
                    args: args.clone(),
                    deferral,
                });

                sink(TabEvent::ContextMenu {
                    id: tab_id,
                    menu,
                    x: point.x,
                    y: point.y,
                    target,
                    items,
                });
                Ok(())
            })),
            &mut token,
        )
    };

    if let Err(err) = result {
        tracing::warn!(%err, "контекстное меню не перехвачено");
    }
}

/// Сообщения из фреймов. Форма входа часто живёт во фрейме (вход в почту
/// Mail.ru — фрейм VK ID): у каждого фрейма свой канал сообщений, и ответ
/// уходит в тот же фрейм.
fn wire_frames(id: TabId, core: &ICoreWebView2, sink: EventSink, frames: FrameMap) {
    let Ok(core4) = core.cast::<ICoreWebView2_4>() else {
        return;
    };
    let tab = id.0;
    let mut token = 0i64;
    let result = unsafe {
        core4.add_FrameCreated(
            &FrameCreatedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                wire_frame(tab, args.Frame()?, sink.clone(), frames.clone());
                Ok(())
            })),
            &mut token,
        )
    };
    if let Err(err) = result {
        tracing::warn!(%err, "фреймы вкладки не подключены");
    }
}

fn wire_frame(tab: u32, frame: ICoreWebView2Frame, sink: EventSink, frames: FrameMap) {
    let (Ok(frame2), Ok(frame5)) = (
        frame.cast::<ICoreWebView2Frame2>(),
        frame.cast::<ICoreWebView2Frame5>(),
    ) else {
        return;
    };
    let mut frame_id = 0u32;
    if unsafe { frame5.FrameId(&mut frame_id) }.is_err() {
        return;
    }
    frames.borrow_mut().insert(frame_id, frame2.clone());

    let mut token = 0i64;
    unsafe {
        let s = sink.clone();
        let _ = frame2.add_WebMessageReceived(
            &FrameWebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut raw = PWSTR::null();
                args.WebMessageAsJson(&mut raw)?;
                let payload = take_pwstr(raw);
                let mut raw = PWSTR::null();
                args.Source(&mut raw)?;
                s(TabEvent::Message {
                    id: tab,
                    frame: Some(frame_id),
                    source: take_pwstr(raw),
                    payload,
                });
                Ok(())
            })),
            &mut token,
        );
        let gone = frames.clone();
        let _ = frame.add_Destroyed(
            &FrameDestroyedEventHandler::create(Box::new(move |_, _| {
                gone.borrow_mut().remove(&frame_id);
                Ok(())
            })),
            &mut token,
        );
        // Фреймы внутри фрейма.
        if let Ok(frame7) = frame.cast::<ICoreWebView2Frame7>() {
            let nested = frames.clone();
            let _ = frame7.add_FrameCreated(
                &FrameChildFrameCreatedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else { return Ok(()) };
                    wire_frame(tab, args.Frame()?, sink.clone(), nested.clone());
                    Ok(())
                })),
                &mut token,
            );
        }
    }
}

/// Цель и пункты меню, `SetHandled` и отсрочка ответа — всё, без чего своё меню
/// не показать. Порядок важен: пока `SetHandled` не вызван, движок покажет своё.
fn intercept_menu(
    args: &ICoreWebView2ContextMenuRequestedEventArgs,
) -> windows_core::Result<(MenuTarget, Vec<MenuItem>, POINT, ICoreWebView2Deferral)> {
    // Шаг в тексте ошибки: по одному коду не понять, какой вызов не прошёл.
    let step = |name: &'static str| {
        move |err: windows_core::Error| {
            windows_core::Error::new(err.code(), format!("{name}: {}", err.message()))
        }
    };
    unsafe {
        let target = args
            .ContextMenuTarget()
            .and_then(|target| read_menu_target(&target))
            .map_err(step("цель"))?;
        let items = args
            .MenuItems()
            .and_then(|items| read_menu_items(&items))
            .map_err(step("пункты"))?;
        let mut point = POINT::default();
        args.Location(&mut point).map_err(step("место"))?;
        args.SetHandled(true).map_err(step("SetHandled"))?;
        let deferral = args.GetDeferral().map_err(step("отсрочка"))?;
        Ok((target, items, point, deferral))
    }
}

fn read_string(
    get: impl FnOnce(*mut PWSTR) -> windows_core::Result<()>,
) -> windows_core::Result<String> {
    let mut raw = PWSTR::null();
    get(&mut raw)?;
    Ok(take_pwstr(raw))
}

/// Необязательное поле цели меню. Без флага `Has…` движок на геттер отвечает
/// ошибкой 0x8000000E, а не пустой строкой, как обещает документация.
fn read_optional(
    has: bool,
    get: impl FnOnce(*mut PWSTR) -> windows_core::Result<()>,
) -> Option<String> {
    if !has {
        return None;
    }
    read_string(get).ok().filter(|value| !value.is_empty())
}

fn read_flag(
    get: impl FnOnce(*mut BOOL) -> windows_core::Result<()>,
) -> windows_core::Result<bool> {
    let mut value = BOOL::default();
    get(&mut value)?;
    Ok(value.as_bool())
}

/// Выделенный текст длиннее этого в меню не нужен: он уходит в поиск и перевод.
const MENU_SELECTION_LIMIT: usize = 10_000;

fn read_menu_target(target: &ICoreWebView2ContextMenuTarget) -> windows_core::Result<MenuTarget> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_AUDIO, COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_IMAGE,
        COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_PAGE,
        COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_SELECTED_TEXT,
        COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_VIDEO,
    };

    unsafe {
        let mut kind = COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_PAGE;
        target.Kind(&mut kind)?;

        Ok(MenuTarget {
            kind: match kind {
                COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_IMAGE => "image",
                COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_SELECTED_TEXT => "selection",
                COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_AUDIO => "audio",
                COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_VIDEO => "video",
                _ => "page",
            },
            page_url: read_string(|out| target.PageUri(out))?,
            frame_url: read_string(|out| target.FrameUri(out))?,
            main_frame: read_flag(|out| target.IsRequestedForMainFrame(out))?,
            editable: read_flag(|out| target.IsEditable(out))?,
            link_url: read_optional(read_flag(|out| target.HasLinkUri(out))?, |out| {
                target.LinkUri(out)
            }),
            link_text: read_optional(read_flag(|out| target.HasLinkText(out))?, |out| {
                target.LinkText(out)
            }),
            source_url: read_optional(read_flag(|out| target.HasSourceUri(out))?, |out| {
                target.SourceUri(out)
            }),
            selection: read_optional(read_flag(|out| target.HasSelection(out))?, |out| {
                target.SelectionText(out)
            })
            .map(|text| text.chars().take(MENU_SELECTION_LIMIT).collect()),
        })
    }
}

fn read_menu_items(
    items: &ICoreWebView2ContextMenuItemCollection,
) -> windows_core::Result<Vec<MenuItem>> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_CHECK_BOX, COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND,
        COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_RADIO, COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR,
        COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SUBMENU,
    };

    let mut count = 0u32;
    unsafe { items.Count(&mut count)? };
    let mut out = Vec::with_capacity(count as usize);

    for index in 0..count {
        unsafe {
            let item = items.GetValueAtIndex(index)?;
            let mut kind = COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND;
            item.Kind(&mut kind)?;
            let mut command = 0i32;
            item.CommandId(&mut command)?;
            let children = if kind == COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SUBMENU {
                read_menu_items(&item.Children()?)?
            } else {
                Vec::new()
            };

            out.push(MenuItem {
                name: read_string(|out| item.Name(out))?,
                label: read_string(|out| item.Label(out))?,
                command,
                shortcut: read_string(|out| item.ShortcutKeyDescription(out))?,
                kind: match kind {
                    COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_CHECK_BOX => "checkbox",
                    COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_RADIO => "radio",
                    COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR => "separator",
                    COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SUBMENU => "submenu",
                    _ => "command",
                },
                enabled: read_flag(|out| item.IsEnabled(out))?,
                checked: read_flag(|out| item.IsChecked(out))?,
                children,
            });
        }
    }
    Ok(out)
}

/// Удалить данные сайтов и/или кэш профиля движка.
///
/// Профиль общий на все окна и вкладки, поэтому годится любое живое вебвью —
/// в том числе интерфейс, когда открыты только встроенные страницы.
pub(crate) fn clear_browsing_data(
    core: &ICoreWebView2,
    site_data: bool,
    cache: bool,
) -> anyhow::Result<()> {
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
        core.cast::<ICoreWebView2_13>()
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

/// Адрес документа по данным движка — для страницы ошибки.
fn document_url(core: Option<&ICoreWebView2>) -> String {
    let Some(core) = core else {
        return String::new();
    };
    let mut raw = PWSTR::null();
    match unsafe { core.Source(&mut raw) } {
        Ok(()) => take_pwstr(raw),
        Err(_) => String::new(),
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

    const VK_HOME: u32 = 0x24;

    let combo = match (ctrl, shift, alt, key) {
        (true, false, false, 0x4B) => "ctrl+k", // командная палитра
        (true, false, false, 0x54) => "ctrl+t", // новая вкладка
        (true, false, false, 0x57) => "ctrl+w", // закрыть вкладку
        (true, false, false, 0x4E) => "ctrl+n", // новое окно
        (true, true, false, 0x4E) => "ctrl+shift+n", // приватное окно
        (true, true, false, 0x57) => "ctrl+shift+w", // закрыть окно
        (true, false, false, 0x4C) => "ctrl+l", // адресная строка
        (true, false, false, 0x52) => "ctrl+r", // обновить
        (true, false, false, VK_F5) => "ctrl+f5", // обновить без кэша
        (true, true, false, 0x4A) => "ctrl+shift+j", // инструменты разработчика
        (false, false, true, VK_HOME) => "alt+home", // домашняя страница
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
        filter::install(
            &core,
            env,
            guard.clone(),
            source.clone(),
            id.0,
            sink.clone(),
        )?;
        filter::install_cosmetics(&core, guard)?;
        wire_accelerators(id, &controller, sink.clone())?;
        downloads::wire(id, &core, downloads, sink.clone())?;
        wire_zoom(id, &controller, sink.clone())?;
        wire_audio(id, &core, sink.clone());
        let menu = MenuSlot::default();
        wire_context_menu(id, &core, sink.clone(), menu.clone());
        let frames = FrameMap::default();
        wire_frames(id, &core, sink.clone(), frames.clone());
        let dialogs = Dialogs::default();
        dialogs::wire(id.0, &core, sink.clone(), dialogs.clone());

        let tab = Self {
            id,
            controller,
            core,
            source,
            visible,
            menu,
            frames,
            dialogs,
            find_wired: Cell::new(false),
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
            let page_dialogs = self.dialogs.clone();
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
                    let closed = dialogs::navigation_started(&page_dialogs);
                    if !closed.is_empty() {
                        s(TabEvent::DialogsClosed { id, tokens: closed });
                    }
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
                    // Страница ошибки движка — чужая деталь: она на языке
                    // системы, с оформлением Edge и советами про Edge. Рисуем
                    // свою прямо в документе, не трогая ни адрес, ни историю.
                    if !ok.as_bool() {
                        let mut status =
                            webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_WEB_ERROR_STATUS_UNKNOWN;
                        args.WebErrorStatus(&mut status)?;
                        if let (Some(core), Some(script)) = (
                            sender.as_ref(),
                            crate::errors::error_script(status, &document_url(sender.as_ref())),
                        ) {
                            let _ = core.ExecuteScript(
                                &HSTRING::from(script),
                                &webview2_com::ExecuteScriptCompletedHandler::create(Box::new(
                                    |code, _| {
                                        if let Err(err) = code {
                                            tracing::debug!(%err, "страница ошибки не нарисована");
                                        }
                                        Ok(())
                                    },
                                )),
                            );
                        }
                    }
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
                        frame: None,
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

    /// Ctrl+F5: перезагрузка мимо кэша. Своего вызова у движка нет, зато есть
    /// та же команда протокола отладки, которой это делает сам Chromium.
    pub fn reload_ignoring_cache(&self) -> windows_core::Result<()> {
        use webview2_com::CallDevToolsProtocolMethodCompletedHandler;

        unsafe {
            self.core.CallDevToolsProtocolMethod(
                &HSTRING::from("Page.reload"),
                &HSTRING::from(r#"{"ignoreCache":true}"#),
                &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|code, _| {
                    if let Err(err) = code {
                        tracing::debug!(%err, "перезагрузка без кэша не удалась");
                    }
                    Ok(())
                })),
            )
        }
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

    /// Сообщение в документ вкладки или в её фрейм (`frame` из
    /// `TabEvent::Message`). Закрытому фрейму отправлять нечего.
    pub fn post_to(&self, frame: Option<u32>, json: &str) -> windows_core::Result<()> {
        let Some(frame) = frame else {
            return self.post(json);
        };
        match self.frames.borrow().get(&frame) {
            Some(frame) => unsafe { frame.PostWebMessageAsJson(&HSTRING::from(json)) },
            None => Ok(()),
        }
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

            // Объект Find у вкладки один: подписка на каждый набранный символ
            // множила бы события счётчика и держала бы мёртвые обработчики.
            if !self.find_wired.replace(true) {
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
            }

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

    /// Заголовок текущего документа.
    pub fn title(&self) -> String {
        let mut raw = PWSTR::null();
        match unsafe { self.core.DocumentTitle(&mut raw) } {
            Ok(()) => take_pwstr(raw),
            Err(_) => String::new(),
        }
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

    /// Ответ на меню страницы: команда движка или `None` — меню закрыли без
    /// выбора. Ответ на устаревшее меню (`menu` не совпал) ничего не делает.
    pub fn context_menu_done(&self, menu: u64, command: Option<i32>) -> windows_core::Result<()> {
        let pending = {
            let mut slot = self.menu.borrow_mut();
            match slot.as_ref() {
                Some(pending) if pending.token == menu => slot.take(),
                _ => None,
            }
        };
        let Some(pending) = pending else {
            return Ok(());
        };
        unsafe {
            if let Some(command) = command {
                pending.args.SetSelectedCommandId(command)?;
            }
            pending.deferral.Complete()?;
            if command.is_some() {
                // Меню забирало фокус себе: без возврата «Вставить» и набор
                // текста после него уходили бы не в страницу.
                let _ = self
                    .controller
                    .MoveFocus(webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
            }
        }
        Ok(())
    }

    /// Ответ на окно страницы (`TabEvent::Dialog`). Ответ на окно, которое уже
    /// закрыто, ничего не делает.
    pub fn dialog_done(&self, token: u64, answer: &DialogAnswer) -> windows_core::Result<()> {
        if dialogs::answer(&self.dialogs, token, answer)? == Some(true) && self.visible {
            // Окно забирало клавиатуру: без возврата текст после «ОК» уходил
            // бы не в страницу.
            let _ = unsafe {
                self.controller.MoveFocus(
                    webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC,
                )
            };
        }
        Ok(())
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
        clear_browsing_data(&self.core, site_data, cache)
    }

    /// Масштаб страницы напрямую: его помнит сайт, а не вкладка.
    pub fn set_zoom(&self, factor: f64) -> windows_core::Result<()> {
        unsafe { self.controller.SetZoomFactor(factor.clamp(0.25, 5.0)) }
    }

    pub fn zoom_factor(&self) -> f64 {
        let mut factor = 1.0f64;
        let _ = unsafe { self.controller.ZoomFactor(&mut factor) };
        factor
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
        assert!(engine_script(PASSWORDS_SCRIPT).contains("password_submit"));
    }

    /// Движок обрезает скрипт на нулевом символе, прочие управляющие символы в
    /// тексте тоже не нужны — в строках для них есть `\u` и `\n`. Проверяется
    /// то, что уходит в движок: перевод строки Windows (`\r\n` после checkout с
    /// autocrlf) `engine_script` уже убирает.
    #[test]
    fn password_script_has_no_control_characters() {
        let bad = engine_script(PASSWORDS_SCRIPT)
            .char_indices()
            .find(|(_, ch)| ch.is_control() && *ch != '\n');
        assert_eq!(bad, None);
    }
}
