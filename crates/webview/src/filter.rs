//! Установка сетевого фильтра на вкладку.
//!
//! `WebResourceRequested` вызывается на UI-потоке для **каждого** сетевого
//! запроса страницы. Всё, что здесь происходит, лежит на пути кадра: тяжёлая
//! работа тут превращается в лаг скролла на любом сайте. Поэтому внутри —
//! только `Guard::check` (микросекунды) и, в случае блока, создание ответа.
//! Ни логов, ни каналов, ни аллокаций строк сверх необходимого.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use browser190x4_adblock::{document_host, document_script, Decision, Guard, ResourceKind};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Deferral, ICoreWebView2Environment,
    ICoreWebView2WebResourceRequestedEventArgs, COREWEBVIEW2_WEB_RESOURCE_CONTEXT,
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

/// Счётчик заблокированного на вкладке: его показывает щит в адресной строке.
struct Counter {
    blocked: Cell<u64>,
    reported: Cell<u64>,
    at: Cell<Instant>,
}

impl Counter {
    fn new() -> Self {
        Self {
            blocked: Cell::new(0),
            reported: Cell::new(0),
            at: Cell::new(Instant::now()),
        }
    }

    /// Новая страница — счёт заново.
    fn reset(&self, id: u32, sink: &EventSink) {
        self.blocked.set(0);
        self.reported.set(0);
        self.at.set(Instant::now());
        sink(TabEvent::Blocked { id, count: 0 });
    }

    fn hit(&self, id: u32, sink: &EventSink) {
        let count = self.blocked.get() + 1;
        self.blocked.set(count);
        let now = Instant::now();
        if now.duration_since(self.at.get()) < REPORT_EVERY || self.reported.get() == count {
            return;
        }
        self.at.set(now);
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

/// Навесить фильтр на вкладку. Возвращает токен для `remove_WebResourceRequested`.
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

    unsafe {
        // Фильтр на всё: adblock-rust сам решает быстрее, чем WebView2
        // отфильтрует по маске, а сузив маску мы теряем счётчик «сколько всего».
        core.AddWebResourceRequestedFilter(h!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;

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
                if host == PAGES_HOST {
                    return Ok(());
                }
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

#[cfg(test)]
mod tests {
    use super::is_twitch_playlist;

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
}
