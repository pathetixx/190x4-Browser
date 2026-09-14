//! Установка сетевого фильтра на вкладку.
//!
//! `WebResourceRequested` вызывается на UI-потоке для **каждого** сетевого
//! запроса страницы. Всё, что здесь происходит, лежит на пути кадра: тяжёлая
//! работа тут превращается в лаг скролла на любом сайте. Поэтому внутри —
//! только `Guard::check` (микросекунды) и, в случае блока, создание пустого
//! ответа. Ни логов, ни каналов, ни аллокаций строк сверх необходимого.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use browser190x4_adblock::{document_host, document_script, Decision, Guard, ResourceKind};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Environment, COREWEBVIEW2_WEB_RESOURCE_CONTEXT,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_WEBSOCKET,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST,
};
use webview2_com::{
    take_pwstr, AddScriptToExecuteOnDocumentCreatedCompletedHandler,
    NavigationStartingEventHandler, WebResourceRequestedEventHandler,
};
use windows_core::{h, HSTRING, PWSTR};

/// Источник, относительно которого считается third-party. Обновляется при
/// навигации вкладки: без него `Guard` не отличит рекламу на сайте от
/// ресурсов самого сайта.
///
/// `Rc<RefCell<_>>`, а не `Arc<Mutex<_>>`, намеренно: и запись (навигация), и
/// чтение (фильтр) происходят на одном UI-потоке, а лишний лок на горячем
/// пути — это те самые микросекунды, ради которых всё и затевалось.
pub type SourceUrl = Rc<RefCell<String>>;

/// Перевод контекста WebView2 в тип ресурса фильтр-списков.
pub fn map_context(context: COREWEBVIEW2_WEB_RESOURCE_CONTEXT) -> ResourceKind {
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

/// Навесить фильтр на вкладку. Возвращает токен для `remove_WebResourceRequested`.
pub fn install(
    core: &ICoreWebView2,
    env: &ICoreWebView2Environment,
    guard: Arc<Guard>,
    source: SourceUrl,
) -> windows_core::Result<i64> {
    let env = env.clone();
    let mut token = 0i64;

    unsafe {
        // Фильтр на всё: adblock-rust сам решает быстрее, чем WebView2
        // отфильтрует по маске, а сузив маску мы теряем счётчик «сколько всего».
        core.AddWebResourceRequestedFilter(h!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;

        core.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_sender, args| {
                let Some(args) = args else { return Ok(()) };

                let request = args.Request()?;
                let url = {
                    let mut raw = PWSTR::null();
                    request.Uri(&mut raw)?;
                    take_pwstr(raw)
                };

                let mut context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
                args.ResourceContext(&mut context)?;

                // Метод нужен правилам с `$method=`; для GET это лишний
                // вызов, но различать до чтения всё равно нечем.
                let method = {
                    let mut raw = PWSTR::null();
                    request.Method(&mut raw)?;
                    take_pwstr(raw)
                };

                match guard.check(&url, &source.borrow(), map_context(context), &method) {
                    Decision::Allow => {}
                    Decision::Block => {
                        // Пустой 403 вместо обрыва соединения: страницы,
                        // которые ждут ответа от счётчика, не виснут на
                        // таймауте, а сразу идут по ветке ошибки.
                        let response = env.CreateWebResourceResponse(
                            None,
                            403,
                            h!("Blocked by 190x4"),
                            h!("Content-Type: text/plain\r\nAccess-Control-Allow-Origin: *"),
                        )?;
                        args.SetResponse(&response)?;
                    }
                    Decision::Rewrite(clean) => {
                        request.SetUri(&HSTRING::from(clean))?;
                    }
                }

                Ok(())
            })),
            &mut token,
        )?;
    }

    Ok(token)
}

/// Косметика и скриптлеты: у каждого адреса свой скрипт документа.
///
/// Скрипт регистрируется в `NavigationStarting` — до создания документа, — а
/// прежний снимается. Регистрация асинхронная: если к её завершению началась
/// следующая навигация, запоздавший скрипт тут же снимается.
pub fn install_cosmetics(core: &ICoreWebView2, guard: Arc<Guard>) -> windows_core::Result<()> {
    let registered: Rc<RefCell<Option<String>>> = Rc::default();
    let generation = Rc::new(Cell::new(0u64));
    let mut token = 0i64;

    unsafe {
        core.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |sender, args| {
                let (Some(core), Some(args)) = (sender, args) else {
                    return Ok(());
                };
                let url = {
                    let mut raw = PWSTR::null();
                    args.Uri(&mut raw)?;
                    take_pwstr(raw)
                };

                let current = generation.get().wrapping_add(1);
                generation.set(current);
                if let Some(id) = registered.borrow_mut().take() {
                    let _ = core.RemoveScriptToExecuteOnDocumentCreated(&HSTRING::from(id));
                }

                let Some(host) = document_host(&url) else {
                    return Ok(());
                };
                let Some(script) = document_script(&host, &guard.cosmetics(&url)) else {
                    return Ok(());
                };

                let registered = registered.clone();
                let generation = generation.clone();
                let owner = core.clone();
                core.AddScriptToExecuteOnDocumentCreated(
                    &HSTRING::from(script.as_str()),
                    &AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(
                        move |code, id| {
                            if code.is_err() {
                                return Ok(());
                            }
                            if generation.get() == current {
                                *registered.borrow_mut() = Some(id);
                            } else {
                                let _ = owner
                                    .RemoveScriptToExecuteOnDocumentCreated(&HSTRING::from(id));
                            }
                            Ok(())
                        },
                    )),
                )
            })),
            &mut token,
        )?;
    }

    Ok(())
}
