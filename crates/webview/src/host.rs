//! Хост вкладок: раскладка, активная вкладка, режим оверлея.
//!
//! # Почему раскладкой занимается Rust, а не CSS
//!
//! Вкладки — нативные HWND-поверхности поверх chrome-вебвью. CSS о них не
//! знает: любой попап, выпадашка или модалка chrome-а, попавшие в область
//! контента, окажутся ПОД страницей. Поэтому геометрия области контента
//! живёт здесь, а chrome лишь сообщает её (см. `set_layout`), и всё, что
//! должно перекрыть страницу, проходит через [`TabHost::set_overlay`].
//!
//! # Почему открытие вкладки асинхронное
//!
//! `CreateCoreWebView2Controller` завершается коллбеком. Дождаться его
//! синхронно можно только вложенным насосом сообщений, а внутри обработчика
//! события tao (туда попадает `run_on_main_thread`) такой насос завершения не
//! получает — вкладка не создаётся вообще. Поэтому [`TabHost::open`] выдаёт
//! `TabId` сразу, а достраивается вкладка в коллбеке, когда цикл сообщений
//! доберётся до него. Состояние из-за этого лежит в `Rc<RefCell<…>>`: коллбек
//! переживает вызов `open`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use browser190x4_adblock::Guard;
use webview2_com::CreateCoreWebView2ControllerCompletedHandler;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2Controller, ICoreWebView2Environment, COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC,
};
use windows::Win32::Foundation::{E_POINTER, HWND, RECT};

use crate::container;
use crate::downloads::{self, DownloadPolicy, SharedDownloads};
use crate::tab::{EventSink, Tab, TabEvent};

/// Хост встроенных страниц. `.invalid` — зарезервированный TLD (RFC 2606):
/// такое имя гарантированно не уедет в реальный DNS, если маппинг не встал.
pub const PAGES_HOST: &str = "190x4-pages.invalid";

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct TabId(pub u32);

/// Область под страницу, в **физических** пикселях окна.
///
/// Chrome считает её в CSS-пикселях и умножает на `scale_factor` перед
/// отправкой: WebView2 `SetBounds` работает в клиентских координатах HWND,
/// и на 150% DPI несогласованность сразу видна как полоса под тулбаром.
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct Layout {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl From<Layout> for RECT {
    fn from(l: Layout) -> Self {
        RECT {
            left: l.x,
            top: l.y,
            right: l.x + l.width,
            bottom: l.y + l.height,
        }
    }
}

/// Процесс движка и документы, которые он показывает.
#[derive(Debug, Clone)]
pub struct EngineProcess {
    pub pid: u32,
    /// `browser`, `renderer`, `gpu`, `utility`, `sandbox` или `plugin`.
    pub kind: &'static str,
    /// Адреса документов верхнего уровня, чьи фреймы живут в процессе: iframe
    /// чужого сайта в отдельном процессе засчитывается странице, в которую встроен.
    pub pages: Vec<String>,
}

/// Вкладка для отчёта о ресурсах.
#[derive(Debug, Clone)]
pub struct TabBrief {
    pub id: u32,
    pub url: String,
    pub title: String,
}

fn read_processes(
    collection: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2ProcessExtendedInfoCollection,
) -> windows_core::Result<Vec<EngineProcess>> {
    use webview2_com::take_pwstr;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2FrameInfo, ICoreWebView2FrameInfo2, COREWEBVIEW2_PROCESS_KIND_BROWSER,
        COREWEBVIEW2_PROCESS_KIND_GPU, COREWEBVIEW2_PROCESS_KIND_PPAPI_BROKER,
        COREWEBVIEW2_PROCESS_KIND_PPAPI_PLUGIN, COREWEBVIEW2_PROCESS_KIND_RENDERER,
        COREWEBVIEW2_PROCESS_KIND_SANDBOX_HELPER, COREWEBVIEW2_PROCESS_KIND_UTILITY,
    };
    use windows_core::{Interface, BOOL, PWSTR};

    let mut count = 0u32;
    unsafe { collection.Count(&mut count)? };
    let mut out = Vec::with_capacity(count as usize);

    for index in 0..count {
        unsafe {
            let info = collection.GetValueAtIndex(index)?;
            let process = info.ProcessInfo()?;
            let mut pid = 0i32;
            process.ProcessId(&mut pid)?;
            let mut kind = COREWEBVIEW2_PROCESS_KIND_BROWSER;
            process.Kind(&mut kind)?;

            let mut pages = Vec::new();
            let frames = info.AssociatedFrameInfos()?;
            let iterator = frames.GetIterator()?;
            let mut has = BOOL::default();
            iterator.HasCurrent(&mut has)?;
            while has.as_bool() {
                let mut frame: ICoreWebView2FrameInfo = iterator.GetCurrent()?;
                while let Ok(parent) = frame
                    .cast::<ICoreWebView2FrameInfo2>()
                    .and_then(|frame| frame.ParentFrameInfo())
                {
                    frame = parent;
                }
                let mut raw = PWSTR::null();
                frame.Source(&mut raw)?;
                let url = take_pwstr(raw);
                if !url.is_empty() && !pages.contains(&url) {
                    pages.push(url);
                }
                iterator.MoveNext(&mut has)?;
            }

            out.push(EngineProcess {
                pid: pid.max(0) as u32,
                kind: match kind {
                    COREWEBVIEW2_PROCESS_KIND_BROWSER => "browser",
                    COREWEBVIEW2_PROCESS_KIND_RENDERER => "renderer",
                    COREWEBVIEW2_PROCESS_KIND_GPU => "gpu",
                    COREWEBVIEW2_PROCESS_KIND_UTILITY => "utility",
                    COREWEBVIEW2_PROCESS_KIND_SANDBOX_HELPER => "sandbox",
                    COREWEBVIEW2_PROCESS_KIND_PPAPI_PLUGIN
                    | COREWEBVIEW2_PROCESS_KIND_PPAPI_BROKER => "plugin",
                    _ => "utility",
                },
                pages,
            });
        }
    }
    Ok(out)
}

struct HostState {
    /// Наше дочернее окно, в котором живут все вкладки. См. [`crate::container`].
    container: HWND,
    env: ICoreWebView2Environment,
    guard: Arc<Guard>,
    sink: EventSink,
    tabs: HashMap<TabId, Tab>,
    order: Vec<TabId>,
    active: Option<TabId>,
    layout: Layout,
    overlay: bool,
    next_id: u32,
    pages_dir: Option<PathBuf>,
    /// Загрузки живут дольше вкладок, из которых начались.
    downloads: SharedDownloads,
    /// Вебвью интерфейса: ему возвращается клавиатура, когда горячая клавиша
    /// со страницы открывает поле ввода браузера.
    chrome: Option<ICoreWebView2Controller>,
}

impl HostState {
    /// Видимость: внутри контейнера видна только активная вкладка, а сам
    /// контейнер скрывается целиком на время оверлея.
    fn apply_visibility(&mut self) -> anyhow::Result<()> {
        let active = self.active;
        for (id, tab) in self.tabs.iter_mut() {
            tab.set_visible(Some(*id) == active)?;
        }
        container::set_visible(self.container, !self.overlay);
        Ok(())
    }

    /// Вкладки всегда занимают контейнер целиком — двигаем только его.
    fn tab_bounds(&self) -> RECT {
        RECT {
            left: 0,
            top: 0,
            right: self.layout.width,
            bottom: self.layout.height,
        }
    }
}

/// `Rc` внутри уже делает тип не `Send`, так что случайно уехать в
/// `tauri::async_runtime::spawn` он не может — COM-объекты остаются на своём
/// STA-потоке.
pub struct TabHost {
    inner: Rc<RefCell<HostState>>,
}

impl TabHost {
    pub fn new(
        hwnd: HWND,
        env: ICoreWebView2Environment,
        guard: Arc<Guard>,
        sink: EventSink,
    ) -> anyhow::Result<Self> {
        let container = container::create(hwnd)?;
        Ok(Self {
            inner: Rc::new(RefCell::new(HostState {
                container,
                env,
                guard,
                sink,
                tabs: HashMap::new(),
                order: Vec::new(),
                active: None,
                layout: Layout::default(),
                overlay: false,
                next_id: 1,
                pages_dir: None,
                downloads: SharedDownloads::default(),
                chrome: None,
            })),
        })
    }

    /// Где лежат newtab и страницы ошибок. Ставится один раз при старте.
    pub fn set_pages_dir(&self, dir: PathBuf) {
        self.inner.borrow_mut().pages_dir = Some(dir);
    }

    /// Завести вкладку. Возвращает id сразу; сама вкладка появится, когда
    /// WebView2 отдаст контроллер, и сообщит о себе событием навигации.
    pub fn open(&self, url: &str) -> anyhow::Result<TabId> {
        let (id, container, env) = {
            let mut state = self.inner.borrow_mut();
            let id = TabId(state.next_id);
            state.next_id += 1;
            state.order.push(id);
            // Намерение показать именно её: к моменту готовности контроллера
            // пользователь может успеть переключиться, и тогда мы не будем
            // дёргать экран.
            if state.active.is_none() {
                state.active = Some(id);
            }
            (id, state.container, state.env.clone())
        };

        let inner = self.inner.clone();
        let url = url.to_string();

        let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
            move |code, controller| {
                let controller = match (|| {
                    code?;
                    controller.ok_or_else(|| windows_core::Error::from(E_POINTER))
                })() {
                    Ok(controller) => controller,
                    Err(err) => {
                        tracing::error!(?id, %err, "контроллер вкладки не создан");
                        return Ok(());
                    }
                };

                let mut state = inner.borrow_mut();
                let bounds = state.tab_bounds();
                let visible = state.active == Some(id);

                let tab = match Tab::from_controller(
                    id,
                    controller,
                    &state.env,
                    state.guard.clone(),
                    state.sink.clone(),
                    state.downloads.clone(),
                    bounds,
                    visible,
                ) {
                    Ok(tab) => tab,
                    Err(err) => {
                        tracing::error!(?id, %err, "вкладка не настроена");
                        return Ok(());
                    }
                };

                if let Some(dir) = state.pages_dir.clone() {
                    if let Err(err) = tab.map_pages(PAGES_HOST, &dir) {
                        tracing::warn!(?id, %err, "встроенные страницы не смонтированы");
                    }
                }

                if let Err(err) = tab.navigate(&url) {
                    tracing::error!(?id, %err, "навигация не началась");
                }

                state.tabs.insert(id, tab);
                (state.sink)(TabEvent::Opened {
                    id: id.0,
                    url: url.clone(),
                });
                Ok(())
            },
        ));

        // Родитель — контейнер, а не окно приложения: иначе поверхность
        // окажется под chrome-вебвью.
        unsafe { env.CreateCoreWebView2Controller(container, &handler)? };
        Ok(id)
    }

    pub fn close(&self, id: TabId) -> anyhow::Result<Option<TabId>> {
        let mut state = self.inner.borrow_mut();

        let position = state.order.iter().position(|t| *t == id);
        if let Some(pos) = position {
            state.order.remove(pos);
        }

        if let Some(tab) = state.tabs.remove(&id) {
            tab.close()?;
        }

        if state.active == Some(id) {
            // Фокус уходит на соседа справа, как в любом браузере; если
            // соседа нет — на последнюю оставшуюся.
            state.active = position
                .and_then(|pos| state.order.get(pos).or_else(|| state.order.last()))
                .copied();
            state.apply_visibility()?;
        }

        Ok(state.active)
    }

    /// Сделать вкладку активной.
    ///
    /// Вкладка может быть ещё в процессе создания — это не ошибка: намерение
    /// запоминается, а видимость применится, когда контроллер будет готов.
    pub fn activate(&self, id: TabId) -> anyhow::Result<()> {
        let mut state = self.inner.borrow_mut();
        if !state.order.contains(&id) {
            anyhow::bail!("нет вкладки {id:?}");
        }
        state.active = Some(id);
        let bounds = state.tab_bounds();
        if let Some(tab) = state.tabs.get(&id) {
            tab.set_bounds(bounds)?;
        }
        state.apply_visibility()
    }

    /// Chrome сдвинул границы контента (открылась боковая панель, свернулась
    /// строка вкладок, изменился размер окна).
    pub fn set_layout(&self, layout: Layout) -> anyhow::Result<()> {
        let mut state = self.inner.borrow_mut();
        state.layout = layout;

        container::place(
            state.container,
            layout.x,
            layout.y,
            layout.width,
            layout.height,
        )?;

        let bounds = state.tab_bounds();
        if let Some(id) = state.active {
            if let Some(tab) = state.tabs.get(&id) {
                tab.set_bounds(bounds)?;
            }
        }
        Ok(())
    }

    /// Оверлей chrome-а (командная палитра, меню, модалка) требует, чтобы
    /// нативная поверхность ушла с дороги: иначе она перекроет всё, что мы
    /// нарисовали в HTML. Снимок страницы для фона под блюром chrome берёт
    /// отдельно — `CapturePreview` до скрытия.
    pub fn set_overlay(&self, on: bool) -> anyhow::Result<()> {
        let mut state = self.inner.borrow_mut();
        if state.overlay == on {
            return Ok(());
        }
        state.overlay = on;
        state.apply_visibility()
    }

    /// Поиск по странице активной вкладки: движку нужен ещё и Environment,
    /// поэтому команда идёт через хост, а не прямо в `Tab`.
    pub fn find(&self, id: TabId, query: &str) -> anyhow::Result<()> {
        let state = self.inner.borrow();
        let tab = state
            .tabs
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("вкладка ещё не готова"))?;
        tab.find(&state.env, query, state.sink.clone())
    }

    /// Действие над вкладкой, если она уже создана.
    pub fn with_tab<R>(&self, id: TabId, f: impl FnOnce(&Tab) -> R) -> Option<R> {
        let state = self.inner.borrow();
        state.tabs.get(&id).map(f)
    }

    pub fn set_chrome_controller(&self, controller: ICoreWebView2Controller) {
        // Браузерные сочетания движка интерфейсу не нужны: F5 перезагрузил бы
        // сам интерфейс, Ctrl+J открыл бы встроенную полку загрузок WebView2.
        if let Ok(core) = unsafe { controller.CoreWebView2() } {
            if let Err(err) = crate::tab::configure(&core) {
                tracing::warn!(%err, "настройки окна интерфейса не применены");
            }
        }
        self.inner.borrow_mut().chrome = Some(controller);
    }

    /// Отдать клавиатуру интерфейсу браузера.
    ///
    /// Ctrl+L, нажатый на странице, приходит в интерфейс событием, но фокус
    /// Windows остаётся в окне вкладки: `focus()` на поле адреса ставит DOM-фокус,
    /// а набранный текст всё равно уходит на страницу.
    pub fn focus_chrome(&self) -> anyhow::Result<()> {
        let state = self.inner.borrow();
        if let Some(chrome) = &state.chrome {
            unsafe { chrome.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)? };
        }
        Ok(())
    }

    /// Папка загрузок и «спрашивать, куда сохранить» — из настроек.
    pub fn set_download_policy(&self, policy: DownloadPolicy) {
        self.inner.borrow().downloads.set_policy(policy);
    }

    /// Пауза, продолжение или отмена загрузки по её номеру в реестре.
    pub fn download_control(&self, key: u64, action: &str) -> anyhow::Result<()> {
        let downloads = self.inner.borrow().downloads.clone();
        downloads.control(key, action)
    }

    /// Ответ на «куда сохранить»: путь или `None` — не скачивать.
    pub fn download_answer(&self, key: u64, path: Option<PathBuf>) -> anyhow::Result<()> {
        let downloads = self.inner.borrow().downloads.clone();
        downloads::answer(&downloads, key, path)
    }

    /// Удалить куки, хранилища сайтов и кэш. Профиль общий, так что хватает
    /// любой живой вкладки.
    pub fn clear_browsing_data(&self, site_data: bool, cache: bool) -> anyhow::Result<()> {
        let state = self.inner.borrow();
        let tab = state
            .tabs
            .values()
            .next()
            .ok_or_else(|| anyhow::anyhow!("нет ни одной открытой вкладки"))?;
        tab.clear_browsing_data(site_data, cache)
    }

    /// Версия рантайма WebView2 — для страницы «О браузере».
    pub fn browser_version(&self) -> String {
        use webview2_com::take_pwstr;
        use windows_core::PWSTR;

        let state = self.inner.borrow();
        let mut raw = PWSTR::null();
        match unsafe { state.env.BrowserVersionString(&mut raw) } {
            Ok(()) => take_pwstr(raw),
            Err(_) => String::new(),
        }
    }

    pub fn active_id(&self) -> Option<TabId> {
        self.inner.borrow().active
    }

    /// Вкладки по порядку: адрес и заголовок документа — для монитора ресурсов.
    pub fn tabs_brief(&self) -> Vec<TabBrief> {
        let state = self.inner.borrow();
        state
            .order
            .iter()
            .filter_map(|id| {
                state.tabs.get(id).map(|tab| TabBrief {
                    id: id.0,
                    url: tab.source_url(),
                    title: tab.title(),
                })
            })
            .collect()
    }

    /// Процессы движка с их ролями и страницами. Список приходит асинхронно:
    /// `done` зовётся на UI-потоке, при ошибке — с пустым списком.
    pub fn engine_processes(
        &self,
        done: impl FnOnce(Vec<EngineProcess>) + 'static,
    ) -> anyhow::Result<()> {
        use webview2_com::GetProcessExtendedInfosCompletedHandler;
        use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment13;
        use windows_core::Interface;

        let env: ICoreWebView2Environment13 = self
            .inner
            .borrow()
            .env
            .cast()
            .map_err(|_| anyhow::anyhow!("движок не сообщает о своих процессах"))?;
        let handler =
            GetProcessExtendedInfosCompletedHandler::create(Box::new(move |code, collection| {
                let processes = match (code, collection) {
                    (Ok(()), Some(collection)) => {
                        read_processes(&collection).unwrap_or_else(|err| {
                            tracing::debug!(%err, "процессы движка не прочитаны");
                            Vec::new()
                        })
                    }
                    _ => Vec::new(),
                };
                done(processes);
                Ok(())
            }));
        unsafe { env.GetProcessExtendedInfos(&handler)? };
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.inner.borrow().order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.borrow().order.is_empty()
    }
}
