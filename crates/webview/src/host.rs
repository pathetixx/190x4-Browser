//! Хост вкладок одного окна браузера: раскладка, активная вкладка, режим
//! оверлея, разделённый экран.
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
//!
//! # Несколько окон
//!
//! У каждого окна браузера свой `TabHost` со своим контейнером, но общий
//! `ICoreWebView2Environment` — иначе окна разъехались бы по разным процессам
//! движка. Номера вкладок и загрузок глобальные ([`next_tab_id`]): так любая
//! команда находит вкладку, не зная, в каком она окне.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use browser190x4_adblock::Guard;
use webview2_com::CreateCoreWebView2ControllerCompletedHandler;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2Controller, ICoreWebView2Environment, ICoreWebView2Environment10,
    ICoreWebView2Profile4, ICoreWebView2_13, COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC,
};
use windows::Win32::Foundation::{E_POINTER, HWND, RECT};
use windows_core::{Interface, HSTRING};

use crate::container;
use crate::dialogs::{self, PermissionSetting};
use crate::downloads::{self, DownloadPolicy, SharedDownloads};
use crate::tab::{EventSink, PopupSlots, Tab, TabEvent};

/// Хост встроенных страниц. `.invalid` — зарезервированный TLD (RFC 2606):
/// такое имя гарантированно не уедет в реальный DNS, если маппинг не встал.
pub const PAGES_HOST: &str = "190x4-pages.invalid";

/// Профиль движка для приватных окон. Отдельное имя обязательно: InPrivate
/// работает поверх профиля, и смешивать его с обычным нельзя.
const PRIVATE_PROFILE: &str = "private";

/// Номера вкладок общие на все окна: команда находит вкладку по номеру, не
/// зная окна, а сессия и загрузки не путают вкладки разных окон.
static NEXT_TAB: AtomicU32 = AtomicU32::new(1);

fn next_tab_id() -> TabId {
    TabId(NEXT_TAB.fetch_add(1, Ordering::Relaxed))
}

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
    use windows_core::{BOOL, PWSTR};

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
    /// Закрытые вкладки, с которых ещё идёт загрузка: в строке их нет, на
    /// экране тоже, но вебвью живёт — иначе движок перестанет сообщать о ходе
    /// загрузки. Закрываются, когда загрузка кончится.
    parked: HashMap<TabId, Tab>,
    order: Vec<TabId>,
    active: Option<TabId>,
    /// Вторая вкладка разделённого экрана. Обычно она справа, а активная
    /// слева; `swapped` — активной стала правая половина (по ней щёлкнули),
    /// и вкладки при этом остаются на своих местах.
    split: Option<TabId>,
    swapped: bool,
    layout: Layout,
    overlay: bool,
    pages_dir: Option<PathBuf>,
    /// Загрузки живут дольше вкладок, из которых начались.
    downloads: SharedDownloads,
    /// Окна, которые открыли страницы и которые ждут своей вкладки.
    popups: PopupSlots,
    /// Вебвью интерфейса: ему возвращается клавиатура, когда горячая клавиша
    /// со страницы открывает поле ввода браузера.
    chrome: Option<ICoreWebView2Controller>,
    /// Приватное окно: вкладки живут в профиле InPrivate, история и пароли
    /// в базу не пишутся.
    private: bool,
}

impl HostState {
    /// Видимость: внутри контейнера видны активная вкладка и её пара по
    /// разделённому экрану, а сам контейнер скрывается целиком на время оверлея.
    fn apply_visibility(&mut self) -> anyhow::Result<()> {
        let active = self.active;
        let split = self.split;
        for (id, tab) in self.tabs.iter_mut() {
            tab.set_visible(Some(*id) == active || Some(*id) == split)?;
        }
        container::set_visible(self.container, !self.overlay);
        Ok(())
    }

    /// Куда встаёт вкладка внутри контейнера. Без разделённого экрана — целиком,
    /// с ним — половина: активная слева, вторая справа.
    fn bounds_for(&self, id: TabId) -> RECT {
        let full = RECT {
            left: 0,
            top: 0,
            right: self.layout.width,
            bottom: self.layout.height,
        };
        let Some(split) = self.split else { return full };
        if Some(id) != self.active && id != split {
            return full;
        }
        // Правая половина — вторая вкладка, пока активной не стала она сама.
        let right = if self.swapped {
            self.active
        } else {
            Some(split)
        };
        // Полоса-разделитель между половинами: её рисует контейнер (он тёмный),
        // потому что HTML под нативной поверхностью не виден.
        const GAP: i32 = 2;
        let half = (self.layout.width - GAP) / 2;
        if Some(id) == right {
            RECT {
                left: half + GAP,
                top: 0,
                right: self.layout.width,
                bottom: self.layout.height,
            }
        } else {
            RECT {
                left: 0,
                top: 0,
                right: half,
                bottom: self.layout.height,
            }
        }
    }

    /// Переставить видимые вкладки после смены раскладки или пары split.
    fn apply_bounds(&self) -> anyhow::Result<()> {
        for id in [self.active, self.split].into_iter().flatten() {
            let bounds = self.bounds_for(id);
            if let Some(tab) = self.tabs.get(&id) {
                tab.set_bounds(bounds)?;
            }
        }
        Ok(())
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
        private: bool,
    ) -> anyhow::Result<Self> {
        let container = container::create(hwnd)?;
        let host = Self {
            inner: Rc::new(RefCell::new(HostState {
                container,
                env,
                guard,
                sink,
                tabs: HashMap::new(),
                parked: HashMap::new(),
                order: Vec::new(),
                active: None,
                split: None,
                swapped: false,
                layout: Layout::default(),
                overlay: false,
                pages_dir: None,
                downloads: SharedDownloads::default(),
                popups: PopupSlots::default(),
                chrome: None,
                private,
            })),
        };
        // Загрузка закрытой вкладки кончилась — теперь закрывается и она.
        let weak = Rc::downgrade(&host.inner);
        host.inner
            .borrow()
            .downloads
            .set_on_idle(Rc::new(move |tab| {
                let Some(inner) = weak.upgrade() else { return };
                let parked = match inner.try_borrow_mut() {
                    Ok(mut state) => state.parked.remove(&TabId(tab)),
                    Err(_) => None,
                };
                if let Some(parked) = parked {
                    tracing::debug!(tab, "загрузка закрытой вкладки кончилась");
                    let _ = parked.close();
                }
            }));
        Ok(host)
    }

    /// Приватное окно: вкладки в профиле InPrivate, ничего не пишется на диск.
    pub fn is_private(&self) -> bool {
        self.inner.borrow().private
    }

    /// Где лежат newtab и страницы ошибок. Ставится один раз при старте.
    pub fn set_pages_dir(&self, dir: PathBuf) {
        self.inner.borrow_mut().pages_dir = Some(dir);
    }

    /// Есть ли такая вкладка в этом окне — по номеру команда находит окно.
    pub fn has_tab(&self, id: TabId) -> bool {
        let state = self.inner.borrow();
        state.order.contains(&id) || state.tabs.contains_key(&id)
    }

    /// Завести вкладку. Возвращает id сразу; сама вкладка появится, когда
    /// WebView2 отдаст контроллер, и сообщит о себе событием навигации.
    pub fn open(&self, url: &str) -> anyhow::Result<TabId> {
        self.open_with(url, None)
    }

    /// Вкладка для окна, которое открыла страница (`TabEvent::Popup`): движок
    /// сам поведёт её на адрес окна, а странице достанется ссылка на неё.
    /// Если окна уже никто не ждёт, вкладка просто откроет `url`.
    pub fn open_with(&self, url: &str, popup: Option<u64>) -> anyhow::Result<TabId> {
        let (id, container, env, private) = {
            let mut state = self.inner.borrow_mut();
            let id = next_tab_id();
            state.order.push(id);
            // Намерение показать именно её: к моменту готовности контроллера
            // пользователь может успеть переключиться, и тогда мы не будем
            // дёргать экран.
            if state.active.is_none() {
                state.active = Some(id);
            }
            (id, state.container, state.env.clone(), state.private)
        };

        let inner = self.inner.clone();
        let url = url.to_string();

        let failed = self.inner.clone();
        let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
            move |code, controller| {
                let controller = match (|| {
                    code?;
                    controller.ok_or_else(|| windows_core::Error::from(E_POINTER))
                })() {
                    Ok(controller) => controller,
                    Err(err) => {
                        tracing::error!(?id, %err, "контроллер вкладки не создан");
                        let waiting = popup
                            .and_then(|token| failed.borrow().popups.borrow_mut().remove(&token));
                        if let Some(popup) = waiting {
                            popup.deny();
                        }
                        drop_failed(&failed, id);
                        return Ok(());
                    }
                };

                let mut state = inner.borrow_mut();
                let popup = popup.and_then(|token| state.popups.borrow_mut().remove(&token));
                // Вкладку закрыли раньше, чем движок её достроил: закрываем и
                // контроллер, иначе живая страница осталась бы без места в строке.
                if !state.order.contains(&id) {
                    tracing::debug!(?id, "вкладку закрыли до готовности");
                    let _ = unsafe { controller.Close() };
                    if let Some(popup) = popup {
                        popup.deny();
                    }
                    return Ok(());
                }
                let bounds = state.bounds_for(id);
                let visible = state.active == Some(id) || state.split == Some(id);

                let tab = match Tab::from_controller(
                    id,
                    controller,
                    &state.env,
                    state.guard.clone(),
                    state.sink.clone(),
                    state.downloads.clone(),
                    state.popups.clone(),
                    bounds,
                    visible,
                ) {
                    Ok(tab) => tab,
                    Err(err) => {
                        tracing::error!(?id, %err, "вкладка не настроена");
                        if let Some(popup) = popup {
                            popup.deny();
                        }
                        drop(state);
                        drop_failed(&inner, id);
                        return Ok(());
                    }
                };

                if let Some(dir) = state.pages_dir.clone() {
                    if let Err(err) = tab.map_pages(PAGES_HOST, &dir) {
                        tracing::warn!(?id, %err, "встроенные страницы не смонтированы");
                    }
                }

                match popup {
                    Some(popup) => tab.attach_popup(popup),
                    None => {
                        if let Err(err) = tab.navigate(&url) {
                            tracing::error!(?id, %err, "навигация не началась");
                        }
                    }
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
        unsafe {
            match private_options(&env, private)? {
                Some(options) => {
                    let env10: ICoreWebView2Environment10 = env.cast()?;
                    env10.CreateCoreWebView2ControllerWithOptions(container, &options, &handler)?
                }
                None => env.CreateCoreWebView2Controller(container, &handler)?,
            }
        }
        Ok(id)
    }

    pub fn close(&self, id: TabId) -> anyhow::Result<Option<TabId>> {
        let mut state = self.inner.borrow_mut();

        let position = state.order.iter().position(|t| *t == id);
        if let Some(pos) = position {
            state.order.remove(pos);
        }

        if let Some(mut tab) = state.tabs.remove(&id) {
            if state.downloads.busy(id.0) {
                tracing::debug!(?id, "вкладка закрыта, её загрузка ещё идёт");
                tab.set_visible(false)?;
                state.parked.insert(id, tab);
            } else {
                tab.close()?;
            }
        }
        // Окна, которые открывала эта страница, больше никто не ждёт.
        let orphans: Vec<u64> = state
            .popups
            .borrow()
            .iter()
            .filter(|(_, popup)| popup.opener == id.0)
            .map(|(token, _)| *token)
            .collect();
        for token in orphans {
            if let Some(popup) = state.popups.borrow_mut().remove(&token) {
                popup.deny();
            }
        }

        if state.split == Some(id) {
            state.split = None;
            state.swapped = false;
        }
        if state.active == Some(id) {
            // Закрыли половину разделённого экрана — вторая занимает окно
            // целиком (так же решает интерфейс). Иначе фокус уходит на соседа
            // справа, как в любом браузере; если соседа нет — на последнюю.
            state.active = match state.split.take() {
                Some(partner) => Some(partner),
                None => position
                    .and_then(|pos| state.order.get(pos).or_else(|| state.order.last()))
                    .copied(),
            };
            state.swapped = false;
        }
        state.apply_visibility()?;
        state.apply_bounds()?;

        Ok(state.active)
    }

    /// Сделать вкладку активной.
    ///
    /// Вкладка может быть ещё в процессе создания — это не ошибка: намерение
    /// запоминается, а видимость применится, когда контроллер будет готов.
    ///
    /// В разделённом экране вторая половина становится активной на своём
    /// месте, а вкладка не из пары встаёт на место активной половины.
    pub fn activate(&self, id: TabId) -> anyhow::Result<()> {
        let mut state = self.inner.borrow_mut();
        if !state.order.contains(&id) {
            anyhow::bail!("нет вкладки {id:?}");
        }
        if state.split == Some(id) {
            state.split = state.active;
            state.swapped = !state.swapped;
            if state.split.is_none() {
                state.swapped = false;
            }
        }
        state.active = Some(id);
        state.apply_bounds()?;
        state.apply_visibility()
    }

    /// Вторая вкладка разделённого экрана (`None` — выйти из режима).
    pub fn set_split(&self, id: Option<TabId>) -> anyhow::Result<()> {
        let mut state = self.inner.borrow_mut();
        match id {
            Some(id) => {
                if !state.order.contains(&id) {
                    anyhow::bail!("нет вкладки {id:?}");
                }
                if state.active == Some(id) {
                    anyhow::bail!("эта вкладка уже открыта слева");
                }
                state.split = Some(id);
                state.swapped = false;
            }
            None => {
                state.split = None;
                state.swapped = false;
            }
        }
        state.apply_bounds()?;
        state.apply_visibility()
    }

    pub fn split_id(&self) -> Option<TabId> {
        self.inner.borrow().split
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

        state.apply_bounds()
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

    /// Окно, которое открыла страница, не открывается: `window.open` получит
    /// `null`. Браузер так блокирует окна, которые сайт открывает сам по себе.
    pub fn popup_deny(&self, token: u64) {
        let popup = self.inner.borrow().popups.borrow_mut().remove(&token);
        if let Some(popup) = popup {
            popup.deny();
        }
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

    /// Знает ли это окно такую загрузку: команда приходит без номера окна.
    pub fn has_download(&self, key: u64) -> bool {
        self.inner.borrow().downloads.has(key)
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

    /// Удалить куки, хранилища сайтов и кэш. Профиль общий на все окна и
    /// вкладки, так что годится любое живое вебвью — в том числе интерфейс,
    /// когда открыты только встроенные страницы.
    pub fn clear_browsing_data(&self, site_data: bool, cache: bool) -> anyhow::Result<()> {
        let core = self.any_core()?;
        crate::tab::clear_browsing_data(&core, site_data, cache)
    }

    /// Разрешения, которые пользователь дал или запретил сайтам. Список приходит
    /// в `done`, когда движок его соберёт.
    pub fn permission_settings(
        &self,
        done: impl FnOnce(Vec<PermissionSetting>) + 'static,
    ) -> anyhow::Result<()> {
        dialogs::permission_settings(&self.profile()?, done)?;
        Ok(())
    }

    /// Забыть решение о разрешении сайта: при следующем запросе сайт спросит
    /// снова. `done(true)` — движок записал.
    pub fn permission_reset(
        &self,
        permission: &str,
        origin: &str,
        done: impl FnOnce(bool) + 'static,
    ) -> anyhow::Result<()> {
        dialogs::permission_reset(&self.profile()?, permission, origin, done)
    }

    /// Любое живое вебвью окна: вкладка, а без вкладок — интерфейс.
    fn any_core(
        &self,
    ) -> anyhow::Result<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2> {
        let state = self.inner.borrow();
        match state.tabs.values().next() {
            Some(tab) => Ok(tab.core().clone()),
            None => Ok(unsafe {
                state
                    .chrome
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("движок ещё не запущен"))?
                    .CoreWebView2()?
            }),
        }
    }

    /// Профиль движка. Он общий на все вкладки и окно интерфейса: годится любая
    /// вкладка, а без вкладок — вебвью интерфейса.
    fn profile(&self) -> anyhow::Result<ICoreWebView2Profile4> {
        let core = self.any_core()?;
        let unsupported = || anyhow::anyhow!("движок не хранит разрешения сайтов");
        let profile = unsafe {
            core.cast::<ICoreWebView2_13>()
                .map_err(|_| unsupported())?
                .Profile()?
        };
        profile.cast().map_err(|_| unsupported())
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

/// Настройки контроллера приватной вкладки. `None` — обычное окно или движок
/// старее 1.0.1518: приватного режима у него нет, окно будет обычным.
fn private_options(
    env: &ICoreWebView2Environment,
    private: bool,
) -> windows_core::Result<
    Option<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2ControllerOptions>,
> {
    if !private {
        return Ok(None);
    }
    let Ok(env10) = env.cast::<ICoreWebView2Environment10>() else {
        tracing::warn!("движок не умеет приватный режим — окно будет обычным");
        return Ok(None);
    };
    unsafe {
        let options = env10.CreateCoreWebView2ControllerOptions()?;
        options.SetProfileName(&HSTRING::from(PRIVATE_PROFILE))?;
        options.SetIsInPrivateModeEnabled(true)?;
        Ok(Some(options))
    }
}

/// Вкладка, которая так и не родилась: убрать её из порядка и сказать окну,
/// иначе она навсегда остаётся пустым местом, которое нечем закрыть.
fn drop_failed(inner: &Rc<RefCell<HostState>>, id: TabId) {
    let sink = {
        let mut state = inner.borrow_mut();
        state.order.retain(|other| *other != id);
        if state.active == Some(id) {
            state.active = state.order.last().copied();
        }
        if state.split == Some(id) || state.split == state.active {
            state.split = None;
            state.swapped = false;
        }
        let _ = state.apply_visibility();
        state.sink.clone()
    };
    sink(TabEvent::OpenFailed { id: id.0 });
}
