//! Блокировщик рекламы во вкладке: сетевой фильтр и косметика.
//!
//! **Сеть.** `WebResourceRequested` вызывается на UI-потоке для каждого
//! запроса, попавшего под подписку. Всё, что здесь происходит, лежит на пути
//! кадра: тяжёлая работа тут превращается в лаг скролла на любом сайте. Поэтому
//! внутри — только `Guard::check` (микросекунды) и, если запрос не пропущен,
//! создание ответа. Ни логов, ни каналов, ни аллокаций строк сверх необходимого.
//!
//! Подписка — на запросы из всех источников: главного документа, фреймов
//! любых сайтов, service worker и shared worker. Прежняя подписка движка
//! (`AddWebResourceRequestedFilter`) видит только главный документ и фреймы
//! его же сайта — реклама во фреймах рекламных сетей (а это почти вся реклама
//! Яндекса, Mail.ru и ВК) и всё, что она догружала, шли мимо фильтра.
//!
//! **Косметика и скриптлеты** ([`CosmeticScripts`]) — скрипт на каждый хост,
//! которому они нужны: сайт вкладки и фреймы со своими правилами (плеер
//! чужого сайта, где реклама в ролике). Скрипт встраивается во все документы
//! вкладки и первой строкой сверяет хост, поэтому живёт, пока хост в ходу, а не
//! до следующего перехода: при возвращении на сайт он уже на месте.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use browser190x4_adblock::{document_host, document_script, Decision, Guard, ResourceKind};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Deferral, ICoreWebView2Environment,
    ICoreWebView2WebResourceRequest, ICoreWebView2WebResourceRequestedEventArgs, ICoreWebView2_22,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_WEBSOCKET,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST,
    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
};
use webview2_com::{
    take_pwstr, AddScriptToExecuteOnDocumentCreatedCompletedHandler, ContentLoadingEventHandler,
    ExecuteScriptCompletedHandler, NavigationStartingEventHandler,
    WebResourceRequestedEventHandler,
};
use windows_core::{h, Interface, HSTRING, PWSTR};

use crate::errors;
use crate::host::PAGES_HOST;
use crate::tab::{EventSink, TabEvent};

/// Документ — страница самого браузера, а не сайт.
fn is_browser_page(url: &str) -> bool {
    url.strip_prefix("http://")
        .and_then(|rest| rest.strip_prefix(PAGES_HOST))
        .is_some_and(|rest| rest.starts_with('/'))
}

/// Счётчик блокировок вкладки в интерфейс — не чаще этого: событие идёт в
/// chrome через IPC, а запросы летят сотнями за загрузку страницы.
const REPORT_EVERY: Duration = Duration::from_millis(300);

/// Источник, относительно которого считается third-party. Обновляется при
/// навигации вкладки: без него `Guard` не отличит рекламу на сайте от
/// ресурсов самого сайта.
///
/// `Rc<RefCell<_>>`, а не `Arc<Mutex<_>>`, намеренно: и запись (навигация), и
/// чтение (фильтр) происходят на одном UI-потоке, а лишний лок на горячем
/// пути — это те самые микросекунды, ради которых всё и затевалось.
pub type SourceUrl = Rc<RefCell<String>>;

/// Перевод контекста WebView2 в тип ресурса фильтр-списков.
///
/// У WebView2 нет отдельного контекста для фрейма: и главная навигация, и
/// документ iframe приходят как `DOCUMENT`. Различаем их по адресу: у главной
/// навигации он совпадает с тем, что вкладка получила в `NavigationStarting`.
/// Без этого правила без модификатора типа не действовали бы на фреймы вовсе
/// (в них не входит `document`), и вся рекламная врезка во фрейме проходила бы
/// мимо фильтра.
pub fn map_context(context: COREWEBVIEW2_WEB_RESOURCE_CONTEXT, main_frame: bool) -> ResourceKind {
    match context {
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT => {
            if main_frame {
                ResourceKind::Document
            } else {
                ResourceKind::Subdocument
            }
        }
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

/// Подписать вкладку на запросы для фильтра или снять подписку.
///
/// Каждый запрос, попавший под подписку, движок отдаёт главному потоку и ждёт
/// ответа — пока поток занят, сеть всех вкладок стоит. Поэтому подписка есть,
/// только пока она нужна: страницам браузера, сайтам без блокировки и при
/// выключенной блокировке её нет вовсе. Плейлисты Twitch подписаны отдельно
/// и всегда (`install`).
///
/// Подписка одна — на все типы ресурсов (`CONTEXT_ALL`), медиа тоже: рекламный
/// ролик плеер берёт обычным `<video>`, и правила `$media` с заглушками
/// написаны про это. Подписки на отдельные типы (`DOCUMENT`, `SCRIPT`…) движок
/// принимает без ошибки, но событий по ним не присылает вовсе — так в 0.9.0
/// фильтр не видел ни одного запроса (проверено пробой на рантайме 154).
///
/// И на все источники запросов (`ICoreWebView2_22`): главный документ, фреймы
/// любых сайтов, service worker и shared worker. Движок старше 1.0.2365 этого
/// не умеет: тогда — прежняя подписка, только главный документ и фреймы его
/// сайта. Снимается подписка тем же способом, каким ставилась.
pub(crate) fn set_filtering(core: &ICoreWebView2, on: bool) -> windows_core::Result<()> {
    let context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
    let sources = COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL;
    unsafe {
        match (core.cast::<ICoreWebView2_22>().ok(), on) {
            (Some(core), true) => {
                core.AddWebResourceRequestedFilterWithRequestSourceKinds(h!("*"), context, sources)
            }
            (Some(core), false) => core.RemoveWebResourceRequestedFilterWithRequestSourceKinds(
                h!("*"),
                context,
                sources,
            ),
            (None, true) => core.AddWebResourceRequestedFilter(h!("*"), context),
            (None, false) => core.RemoveWebResourceRequestedFilter(h!("*"), context),
        }
    }
}

/// Счётчик заблокированного на вкладке: его показывает щит в адресной строке.
struct Counter {
    blocked: Cell<u64>,
    reported: Cell<u64>,
    at: Cell<Instant>,
    /// Отложенный отчёт уже заведён.
    flush: Cell<bool>,
}

impl Counter {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            blocked: Cell::new(0),
            reported: Cell::new(0),
            at: Cell::new(Instant::now()),
            flush: Cell::new(false),
        })
    }

    /// Новая страница — счёт заново.
    fn reset(&self, id: u32, sink: &EventSink) {
        self.blocked.set(0);
        self.reported.set(0);
        self.at.set(Instant::now());
        sink(TabEvent::Blocked { id, count: 0 });
    }

    /// Блокировки идут пачками: реклама страницы запрашивается разом. Внутри
    /// интервала отчёта счёт копится, а конец пачки приходит отложенным
    /// отчётом — иначе щит застывал бы на первой блокировке пачки.
    fn hit(self: &Rc<Self>, id: u32, sink: &EventSink) {
        self.blocked.set(self.blocked.get() + 1);
        let since = self.at.get().elapsed();
        if since >= REPORT_EVERY {
            self.report(id, sink);
            return;
        }
        if self.flush.replace(true) {
            return;
        }
        let (counter, sink) = (self.clone(), sink.clone());
        let wait = (REPORT_EVERY - since).as_millis() as u32 + 1;
        crate::later::after(wait, move || {
            counter.flush.set(false);
            counter.report(id, &sink);
        });
    }

    fn report(&self, id: u32, sink: &EventSink) {
        let count = self.blocked.get();
        if self.reported.get() == count {
            return;
        }
        self.at.set(Instant::now());
        self.reported.set(count);
        sink(TabEvent::Blocked { id, count });
    }
}

/// Отвечать ли браузеру на запросы плейлистов Twitch (расширение Twitch,
/// «лучшее качество»). Общее на все вкладки, меняется из настроек.
static TWITCH_PLAYLISTS: AtomicBool = AtomicBool::new(false);

/// Включить или выключить перехват плейлистов Twitch — сразу для всех вкладок.
pub fn set_twitch_playlists(on: bool) {
    TWITCH_PLAYLISTS.store(on, Ordering::Relaxed);
}

/// Главный плейлист трансляции Twitch: по нему плеер выбирает качество.
/// Записи и клипы — другие адреса, их браузер не трогает.
fn is_twitch_playlist(url: &str) -> bool {
    url.strip_prefix("https://usher.ttvnw.net/api/")
        .map(|rest| rest.strip_prefix("v2/").unwrap_or(rest))
        .is_some_and(|rest| rest.starts_with("channel/hls/"))
}

/// Ответ браузера на перехваченный запрос.
#[derive(Debug, Clone)]
pub struct InterceptReply {
    pub status: i32,
    pub reason: String,
    /// Заголовки ответа строками `Имя: значение`, через `\r\n`.
    pub headers: String,
    pub body: Vec<u8>,
}

/// Запросы вкладки, которые ждут ответа браузера: движок держит их под
/// отсрочкой, пока не придёт [`Intercepts::answer`]. Ответ приходит всегда —
/// хотя бы «пусть идёт как шёл», иначе запрос висел бы до закрытия вкладки.
pub(crate) struct Intercepts {
    env: ICoreWebView2Environment,
    next: Cell<u64>,
    pending: RefCell<
        HashMap<
            u64,
            (
                ICoreWebView2WebResourceRequestedEventArgs,
                ICoreWebView2Deferral,
            ),
        >,
    >,
}

impl Intercepts {
    pub(crate) fn new(env: &ICoreWebView2Environment) -> Rc<Self> {
        Rc::new(Self {
            env: env.clone(),
            next: Cell::new(0),
            pending: RefCell::default(),
        })
    }

    fn hold(
        &self,
        args: ICoreWebView2WebResourceRequestedEventArgs,
        deferral: ICoreWebView2Deferral,
    ) -> u64 {
        let token = self.next.get().wrapping_add(1);
        self.next.set(token);
        self.pending.borrow_mut().insert(token, (args, deferral));
        token
    }

    /// Ответить на запрос: `None` — запрос уходит в сеть как был.
    pub(crate) fn answer(&self, token: u64, reply: Option<InterceptReply>) {
        let Some((args, deferral)) = self.pending.borrow_mut().remove(&token) else {
            return;
        };
        if let Some(reply) = reply {
            let applied = unsafe {
                self.env
                    .CreateWebResourceResponse(
                        crate::stream::from_bytes(&reply.body).as_ref(),
                        reply.status,
                        &HSTRING::from(reply.reason.as_str()),
                        &HSTRING::from(reply.headers.as_str()),
                    )
                    .and_then(|response| args.SetResponse(&response))
            };
            if let Err(err) = applied {
                tracing::debug!(%err, "ответ браузера на запрос не записан");
            }
        }
        // Страница могла уйти, пока ждали: движок тогда отсрочку уже не ждёт.
        let _ = unsafe { deferral.Complete() };
    }
}

/// Заголовки CORS для ответа браузера вместо сайта. Запрос с учётными данными
/// (`fetch(…, {credentials: "include"})`) не примет `*` — ему нужен его же
/// origin и разрешение учётных данных, иначе заглушка для страницы — та же
/// сетевая ошибка, что и блокировка.
fn cors_headers(request: &ICoreWebView2WebResourceRequest) -> String {
    let origin = unsafe { request.Headers() }.ok().and_then(|headers| {
        let mut raw = PWSTR::null();
        unsafe { headers.GetHeader(h!("Origin"), &mut raw) }.ok()?;
        Some(take_pwstr(raw))
    });
    cors_for(origin.as_deref())
}

fn cors_for(origin: Option<&str>) -> String {
    match origin {
        Some(origin)
            if !origin.is_empty()
                && origin != "null"
                && origin.len() <= 512
                && !origin.chars().any(char::is_control) =>
        {
            format!(
                "Access-Control-Allow-Origin: {origin}\r\nAccess-Control-Allow-Credentials: true\r\nVary: Origin"
            )
        }
        _ => "Access-Control-Allow-Origin: *".to_string(),
    }
}

/// Навесить фильтр на вкладку. Возвращает токен для `remove_WebResourceRequested`.
/// Подписка на запросы сразу включена (`set_filtering`).
pub(crate) fn install(
    core: &ICoreWebView2,
    env: &ICoreWebView2Environment,
    guard: Arc<Guard>,
    source: SourceUrl,
    id: u32,
    sink: EventSink,
    intercepts: Rc<Intercepts>,
) -> windows_core::Result<i64> {
    let env = env.clone();
    let mut token = 0i64;
    let counter = Counter::new();

    // Подписка фильтра снимается и ставится на переходах вкладки
    // (`Tab::sync_filter`); плейлисты Twitch нужны и на сайте без блокировки.
    set_filtering(core, true)?;
    unsafe {
        core.AddWebResourceRequestedFilter(
            h!("https://usher.ttvnw.net/*"),
            COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        )?;

        core.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_sender, args| {
                let Some(args) = args else { return Ok(()) };

                // Страницы браузера (новая вкладка) — не сайт: списки
                // блокировки к ним не относятся, а их общие правила вроде
                // «прятать ссылки на dzen.ru» ломали плитки. Такой запрос не
                // разбираем вовсе — ни адреса, ни метода.
                if is_browser_page(&source.borrow()) {
                    return Ok(());
                }

                let request = args.Request()?;
                let url = {
                    let mut raw = PWSTR::null();
                    request.Uri(&mut raw)?;
                    take_pwstr(raw)
                };

                let mut context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
                args.ResourceContext(&mut context)?;

                let main_frame = context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT
                    && url.as_str() == source.borrow().as_str();
                if main_frame {
                    counter.reset(id, &sink);
                }
                // Плейлист Twitch берёт браузер (src-tauri/src/twitch.rs): запрос
                // ждёт под отсрочкой, пока не придёт ответ или «пусть идёт».
                if TWITCH_PLAYLISTS.load(Ordering::Relaxed) && is_twitch_playlist(&url) {
                    let deferral = args.GetDeferral()?;
                    let token = intercepts.hold(args.clone(), deferral);
                    sink(TabEvent::Intercept { id, token, url });
                    return Ok(());
                }
                // Блокировка выключена или сайт в исключениях — дальше смотреть
                // нечего, а счётчик новой страницы уже обнулён.
                if !guard.filters(&source.borrow()) {
                    return Ok(());
                }

                // Метод нужен правилам с `$method=`; для GET это лишний
                // вызов, но различать до чтения всё равно нечем.
                let method = {
                    let mut raw = PWSTR::null();
                    request.Method(&mut raw)?;
                    take_pwstr(raw)
                };

                let kind = map_context(context, main_frame);
                match guard.check(&url, &source.borrow(), kind, &method) {
                    Decision::Allow => {}
                    Decision::Block => {
                        counter.hit(id, &sink);
                        // Главную навигацию закрываем своей страницей, всё
                        // остальное — пустым 403: страницы, ждущие ответа от
                        // счётчика, не виснут на таймауте, а сразу идут по
                        // ветке ошибки.
                        let response = if kind == ResourceKind::Document {
                            let body = errors::blocked_html(&url);
                            env.CreateWebResourceResponse(
                                crate::stream::from_bytes(body.as_bytes()).as_ref(),
                                403,
                                h!("Blocked by 190x4"),
                                h!("Content-Type: text/html; charset=utf-8"),
                            )?
                        } else {
                            env.CreateWebResourceResponse(
                                None,
                                403,
                                h!("Blocked by 190x4"),
                                h!("Content-Type: text/plain\r\nAccess-Control-Allow-Origin: *"),
                            )?
                        };
                        args.SetResponse(&response)?;
                    }
                    Decision::Redirect { mime, body } => {
                        counter.hit(id, &sink);
                        // Заглушка правила — обычный успешный ответ: скрипт
                        // рекламы «загрузился» и ничего не делает, пустой VAST
                        // говорит плееру, что рекламы нет.
                        let headers = format!("Content-Type: {mime}\r\n{}", cors_headers(&request));
                        let response = env.CreateWebResourceResponse(
                            crate::stream::from_bytes(&body).as_ref(),
                            200,
                            h!("OK"),
                            &HSTRING::from(headers),
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

/// Больше скриптов косметики вкладка не держит: каждый из них движок
/// просматривает в каждом её документе. Давно не встречавшийся хост уступает
/// место; вернётся — скрипт посчитается заново.
const HOSTS_LIMIT: usize = 16;

/// Скрипт косметики одного хоста во вкладке.
struct HostScript {
    host: String,
    /// Номер у движка; `None` — встраивается или делать на хосте нечего.
    id: Option<String>,
    /// Делать нечего: правил для хоста нет. Помнится, чтобы не считать заново
    /// на каждом переходе.
    empty: bool,
    /// Посчитан как сайт вкладки, со всей косметикой.
    page: bool,
}

struct CosmeticState {
    /// Номер правки фильтра, по которому посчитаны скрипты ([`Guard::revision`]).
    revision: u64,
    /// Растёт при сбросе всех скриптов: ответ движка о встраивании, начатом
    /// до сброса, скрипт тут же снимает.
    epoch: u64,
    /// Хосты, давние — первыми.
    hosts: Vec<HostScript>,
    /// Адрес документа вкладки — по нему решается, работает ли блокировка во
    /// всех её фреймах.
    top: String,
    /// Хост документа вкладки, который уже начал загружаться
    /// (`ContentLoading`): встроенный позже скрипт ему уже не достанется.
    loading: Option<String>,
}

/// Скрипты косметики и скриптлетов вкладки — по хосту.
///
/// Скрипт хоста встраивается в начале перехода к нему: вкладки — в
/// `NavigationStarting`, фрейма — в `FrameNavigationStarting`, до создания
/// документа. Встраивание асинхронное; если документ вкладки начал загружаться
/// раньше, чем движок его подтвердил, скрипт выполняется в нём сразу
/// (`ExecuteScript`): стили и процедурные правила работают и так, скриптлеты —
/// с опозданием, но работают. Повторно на одном документе скрипт не
/// выполнится — его исполнитель ставит отметку.
pub(crate) struct CosmeticScripts {
    /// Ответ движка о встраивании приходит позже, и вкладка к тому времени
    /// может быть закрыта — он держит только слабую ссылку.
    me: std::rc::Weak<Self>,
    core: ICoreWebView2,
    guard: Arc<Guard>,
    state: RefCell<CosmeticState>,
}

impl CosmeticScripts {
    pub(crate) fn install(
        core: &ICoreWebView2,
        guard: Arc<Guard>,
    ) -> windows_core::Result<Rc<Self>> {
        let scripts = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            core: core.clone(),
            state: RefCell::new(CosmeticState {
                revision: guard.revision(),
                epoch: 0,
                hosts: Vec::new(),
                top: String::new(),
                loading: None,
            }),
            guard,
        });
        let mut token = 0i64;
        unsafe {
            let this = Rc::downgrade(&scripts);
            core.add_NavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                    let (Some(this), Some(args)) = (this.upgrade(), args) else {
                        return Ok(());
                    };
                    let mut raw = PWSTR::null();
                    args.Uri(&mut raw)?;
                    this.page_navigation(&take_pwstr(raw));
                    Ok(())
                })),
                &mut token,
            )?;
            // Фреймы любой вложенности; вложенные приходят ещё и от самих
            // фреймов (`tab.rs`, `wire_frame`) — второй раз хост уже на месте.
            let this = Rc::downgrade(&scripts);
            core.add_FrameNavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                    let (Some(this), Some(args)) = (this.upgrade(), args) else {
                        return Ok(());
                    };
                    let mut raw = PWSTR::null();
                    args.Uri(&mut raw)?;
                    this.frame_navigation(&take_pwstr(raw));
                    Ok(())
                })),
                &mut token,
            )?;
            let this = Rc::downgrade(&scripts);
            core.add_ContentLoading(
                &ContentLoadingEventHandler::create(Box::new(move |sender, _| {
                    let (Some(this), Some(core)) = (this.upgrade(), sender) else {
                        return Ok(());
                    };
                    let mut raw = PWSTR::null();
                    core.Source(&mut raw)?;
                    this.state.borrow_mut().loading = document_host(&take_pwstr(raw));
                    Ok(())
                })),
                &mut token,
            )?;
        }
        Ok(scripts)
    }

    /// Вкладка переходит на новый адрес.
    fn page_navigation(&self, url: &str) {
        {
            let mut state = self.state.borrow_mut();
            state.top = url.to_string();
            state.loading = None;
        }
        // Блокировка на сайте выключена — скрипты прочих хостов тоже не нужны:
        // они встроены во все документы вкладки, и фреймы этого сайта
        // получили бы косметику, которую человек выключил.
        if is_browser_page(url) || !self.guard.filters(url) {
            self.clear();
            return;
        }
        self.sync_revision();
        self.prepare(url, true, || self.guard.cosmetics(url));
    }

    /// Фрейм вкладки (любой вложенности) переходит на новый адрес.
    pub(crate) fn frame_navigation(&self, url: &str) {
        let top = self.state.borrow().top.clone();
        if is_browser_page(&top) || !self.guard.filters(&top) {
            return;
        }
        // Фрейм, который сеть всё равно закроет (рекламная врезка), косметика
        // не спасёт — считать её незачем.
        if self.guard.blocks(url, &top, ResourceKind::Subdocument) {
            return;
        }
        self.sync_revision();
        let guard = self.guard.clone();
        self.prepare(url, false, || {
            guard.frame_cosmetics(url, &top).unwrap_or_default()
        });
    }

    /// Фильтр сменился (списки, исключения, выключатель) — скрипты устарели.
    fn sync_revision(&self) {
        let revision = self.guard.revision();
        if self.state.borrow().revision != revision {
            self.clear();
            self.state.borrow_mut().revision = revision;
        }
    }

    /// Снять все скрипты вкладки.
    fn clear(&self) {
        let hosts = {
            let mut state = self.state.borrow_mut();
            state.epoch += 1;
            state.revision = self.guard.revision();
            std::mem::take(&mut state.hosts)
        };
        for script in hosts {
            if let Some(id) = script.id {
                let _ = unsafe {
                    self.core
                        .RemoveScriptToExecuteOnDocumentCreated(&HSTRING::from(id))
                };
            }
        }
    }

    /// Встроить скрипт хоста адреса, если его ещё нет. `page` — адрес вкладки:
    /// ей нужна вся косметика, в том числе общие правила, а хост, который до
    /// этого встречался только фреймом без своих правил, скрипта не получил.
    fn prepare(
        &self,
        url: &str,
        page: bool,
        cosmetics: impl FnOnce() -> browser190x4_adblock::Cosmetics,
    ) {
        let Some(host) = document_host(url) else {
            return;
        };
        if host == PAGES_HOST {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if let Some(at) = state.hosts.iter().position(|script| script.host == host) {
                let script = state.hosts.remove(at);
                let incomplete = page && script.empty && !script.page;
                if !incomplete {
                    // Свежий — в конец: уступать место он будет последним.
                    state.hosts.push(script);
                    return;
                }
            }
        }
        let script = document_script(&host, &cosmetics());
        let evicted = {
            let mut state = self.state.borrow_mut();
            state.hosts.push(HostScript {
                host: host.clone(),
                id: None,
                empty: script.is_none(),
                page,
            });
            let mut evicted = Vec::new();
            while state.hosts.len() > HOSTS_LIMIT {
                evicted.push(state.hosts.remove(0));
            }
            evicted
        };
        for old in evicted {
            if let Some(id) = old.id {
                let _ = unsafe {
                    self.core
                        .RemoveScriptToExecuteOnDocumentCreated(&HSTRING::from(id))
                };
            }
        }
        let Some(script) = script else {
            return;
        };
        self.register(host, script);
    }

    fn register(&self, host: String, script: String) {
        let epoch = self.state.borrow().epoch;
        let core = self.core.clone();
        let this = self.me.clone();
        let source = HSTRING::from(script.as_str());
        let added = unsafe {
            self.core.AddScriptToExecuteOnDocumentCreated(
                &source,
                &AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(
                    move |code, id| {
                        if let Err(err) = code {
                            tracing::debug!(%err, "скрипт косметики не встроен");
                            return Ok(());
                        }
                        let remove = || {
                            let _ = core.RemoveScriptToExecuteOnDocumentCreated(&HSTRING::from(
                                id.as_str(),
                            ));
                        };
                        let Some(this) = this.upgrade() else {
                            remove();
                            return Ok(());
                        };
                        // Скрипт ещё нужен: вкладку не сбросили, хост не вытеснен
                        // и не посчитан заново.
                        let late = {
                            let mut state = this.state.borrow_mut();
                            let current = state.epoch == epoch;
                            let slot = state.hosts.iter_mut().find(|script| script.host == host);
                            let taken = match slot {
                                Some(slot) if current && slot.id.is_none() && !slot.empty => {
                                    slot.id = Some(id.clone());
                                    true
                                }
                                _ => false,
                            };
                            taken.then(|| state.loading.as_deref() == Some(host.as_str()))
                        };
                        let Some(late) = late else {
                            remove();
                            return Ok(());
                        };
                        // Документ вкладки начал загружаться раньше, чем скрипт
                        // встал: выполнить в нём сейчас.
                        if late {
                            let _ = core.ExecuteScript(
                                &HSTRING::from(script.as_str()),
                                &ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(()))),
                            );
                        }
                        Ok(())
                    },
                )),
            )
        };
        if let Err(err) = added {
            tracing::debug!(%err, "скрипт косметики не встроен");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{cors_for, is_twitch_playlist};

    #[test]
    fn only_live_twitch_playlists_are_taken() {
        assert!(is_twitch_playlist(
            "https://usher.ttvnw.net/api/v2/channel/hls/ohnepixel.m3u8?acmb=1"
        ));
        assert!(is_twitch_playlist(
            "https://usher.ttvnw.net/api/channel/hls/x.m3u8"
        ));
        assert!(!is_twitch_playlist("https://usher.ttvnw.net/vod/123.m3u8"));
        assert!(!is_twitch_playlist(
            "https://usher.ttvnw.net.evil.example/api/channel/hls/x.m3u8"
        ));
        assert!(!is_twitch_playlist(
            "http://usher.ttvnw.net/api/channel/hls/x.m3u8"
        ));
    }

    #[test]
    fn stubs_answer_cors_for_their_origin() {
        assert_eq!(cors_for(None), "Access-Control-Allow-Origin: *");
        assert_eq!(cors_for(Some("null")), "Access-Control-Allow-Origin: *");
        assert!(cors_for(Some("https://vk.com")).starts_with(
            "Access-Control-Allow-Origin: https://vk.com\r\nAccess-Control-Allow-Credentials: true"
        ));
        assert_eq!(
            cors_for(Some("https://x.example\r\nSet-Cookie: a=b")),
            "Access-Control-Allow-Origin: *"
        );
    }
}
