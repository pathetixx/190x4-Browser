//! Контекст документа: фоновый ответ не переживает навигацию или перенос вкладки.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use browser190x4_store::passwords_csv::origin_of;
use parking_lot::Mutex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    pub tab: u32,
    pub frame: Option<u32>,
    pub generation: u64,
    pub origin: String,
    pub window: String,
    pub private: bool,
}

#[derive(Default)]
pub struct Documents(Mutex<Registry>, Mutex<HashMap<u32, Typing>>);

/// Печатали ли в документе вкладки: такую вкладку браузер не выгружает, иначе
/// черновик пропал бы. Признак живёт здесь, а не в интерфейсе окна: он
/// переезжает с вкладкой в другое окно и переживает переход, ушедший в
/// загрузку или отменённый.
#[derive(Default, Clone, Copy)]
struct Typing {
    /// Начало последнего перехода документа вкладки.
    started: Option<Instant>,
    /// Когда в документе начали печатать.
    edited: Option<Instant>,
}

type Slot = (u32, Option<u32>);

/// Сколько ждать нового документа фрейма. Конец перехода движок сообщает
/// только для документа вкладки; если прежний документ фрейма и через это
/// время пишет браузеру, переход отменили или он ушёл в загрузку.
const FRAME_SETTLE: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Registry {
    next: u64,
    /// Документ, который сейчас на экране вкладки или фрейма.
    entries: HashMap<Slot, Context>,
    /// Переход на другой origin начался, но документ ещё не сменился: переход
    /// могут отменить («Остаться»), он может уйти в загрузку. Прежний документ
    /// пока остаётся на месте, но секреты ни ему, ни новому не уходят.
    pending: HashMap<Slot, (Context, Instant)>,
}

impl Registry {
    fn context(
        &mut self,
        tab: u32,
        frame: Option<u32>,
        origin: String,
        window: &str,
        private: bool,
    ) -> Context {
        self.next += 1;
        Context {
            tab,
            frame,
            generation: self.next,
            origin,
            window: window.into(),
            private,
        }
    }

    /// Новый документ занял место прежнего. Документ вкладки уносит с собой и
    /// фреймы прежней страницы.
    fn replace(&mut self, context: Context) {
        let (tab, frame) = (context.tab, context.frame);
        if frame.is_none() {
            self.entries.retain(|(id, _), _| *id != tab);
            self.pending.retain(|(id, _), _| *id != tab);
        } else {
            self.pending.remove(&(tab, frame));
        }
        self.entries.insert((tab, frame), context);
    }
}

impl Documents {
    /// Начался переход вкладки или фрейма. На тот же origin документ остаётся
    /// тем же адресатом; на другой — переход ждёт, пока документ сменится.
    pub fn navigate(&self, tab: u32, frame: Option<u32>, url: &str, window: &str, private: bool) {
        let mut state = self.0.lock();
        let slot = (tab, frame);
        let origin = origin_of(url).unwrap_or_default();
        match state.entries.get(&slot) {
            Some(current) if !origin.is_empty() && current.origin == origin => {
                state.pending.remove(&slot);
            }
            Some(_) => {
                let context = state.context(tab, frame, origin, window, private);
                state.pending.insert(slot, (context, Instant::now()));
            }
            None => {
                let context = state.context(tab, frame, origin, window, private);
                state.replace(context);
            }
        }
    }

    /// Контекст документа, приславшего сообщение. Сообщение нового документа
    /// (origin ожидающего перехода) значит, что переход состоялся.
    pub fn capture(&self, tab: u32, frame: Option<u32>, source: &str) -> Option<Context> {
        self.capture_at(tab, frame, source, Instant::now())
    }

    fn capture_at(
        &self,
        tab: u32,
        frame: Option<u32>,
        source: &str,
        now: Instant,
    ) -> Option<Context> {
        let origin = origin_of(source)?;
        let mut state = self.0.lock();
        let slot = (tab, frame);
        let current = state
            .entries
            .get(&slot)
            .is_some_and(|current| current.origin == origin);
        let arrived = !current
            && state
                .pending
                .get(&slot)
                .is_some_and(|(next, _)| next.origin == origin);
        if arrived {
            if let Some((context, _)) = state.pending.remove(&slot) {
                state.replace(context);
            }
        }
        // Прежний документ фрейма всё ещё на месте — переход не состоялся.
        let stale = frame.is_some()
            && current
            && state
                .pending
                .get(&slot)
                .is_some_and(|(_, at)| now.duration_since(*at) >= FRAME_SETTLE);
        if stale {
            state.pending.remove(&slot);
        }
        state
            .entries
            .get(&slot)
            .filter(|entry| entry.origin == origin)
            .cloned()
    }

    /// Совпадает ли origin с документом на экране — без признания перехода.
    pub fn is_origin(&self, tab: u32, frame: Option<u32>, source: &str) -> bool {
        let Some(origin) = origin_of(source) else {
            return false;
        };
        self.0
            .lock()
            .entries
            .get(&(tab, frame))
            .is_some_and(|entry| entry.origin == origin)
    }

    /// Переход вкладки закончился, на экране — `url`. Возвращает `true`, если
    /// документ сменился; отменённый переход и загрузка документа не меняют.
    pub fn settle(&self, tab: u32, url: &str, window: &str, private: bool) -> bool {
        let mut state = self.0.lock();
        let slot = (tab, None);
        let pending = state.pending.remove(&slot);
        let origin = origin_of(url).unwrap_or_default();
        if state
            .entries
            .get(&slot)
            .is_some_and(|current| current.origin == origin)
        {
            return false;
        }
        let context = match pending {
            Some((next, _)) if next.origin == origin => next,
            _ => state.context(tab, None, origin, window, private),
        };
        state.replace(context);
        true
    }

    /// Годится ли контекст для ответа: документ тот же и никуда не уходит.
    pub fn current(&self, context: &Context) -> bool {
        let state = self.0.lock();
        let slot = (context.tab, context.frame);
        state.entries.get(&slot) == Some(context) && !state.pending.contains_key(&slot)
    }

    /// В документе вкладки (или в её фрейме) начали печатать.
    pub fn mark_edited(&self, tab: u32) {
        self.1.lock().entry(tab).or_default().edited = Some(Instant::now());
    }

    /// Можно ли выгрузить вкладку, не потеряв введённого.
    pub fn edited(&self, tab: u32) -> bool {
        self.1
            .lock()
            .get(&tab)
            .is_some_and(|typing| typing.edited.is_some())
    }

    /// Документ вкладки начал переход.
    pub fn typing_navigation(&self, tab: u32) {
        self.1.lock().entry(tab).or_default().started = Some(Instant::now());
    }

    /// Переход документа вкладки закончился. Новый документ (`ok`) — ввод
    /// прежнего к нему не относится; ввод, начатый уже после начала перехода,
    /// — его собственный. Переход, ушедший в загрузку или отменённый, документ
    /// не меняет, и признак остаётся.
    pub fn typing_settled(&self, tab: u32, ok: bool) {
        if !ok {
            return;
        }
        let mut typing = self.1.lock();
        if let Some(entry) = typing.get_mut(&tab) {
            let before = match (entry.edited, entry.started) {
                (Some(edited), Some(started)) => edited < started,
                (Some(_), None) => false,
                (None, _) => true,
            };
            if before {
                entry.edited = None;
            }
        }
    }

    pub fn forget(&self, tab: u32, frame: Option<u32>) {
        if frame.is_none() {
            self.1.lock().remove(&tab);
        }
        let mut state = self.0.lock();
        let gone = |(id, slot): &Slot| *id == tab && (frame.is_none() || *slot == frame);
        state.entries.retain(|key, _| !gone(key));
        state.pending.retain(|key, _| !gone(key));
    }

    pub fn moved(&self, tab: u32, window: &str) {
        let mut state = self.0.lock();
        let Registry {
            entries, pending, ..
        } = &mut *state;
        let pending = pending
            .iter_mut()
            .map(|(slot, (context, _))| (slot, context));
        for ((id, _), context) in entries.iter_mut().chain(pending) {
            if *id == tab {
                context.window = window.into();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_tickets_expire_on_navigation_close_and_move() {
        let docs = Documents::default();
        docs.navigate(1, None, "https://a.test/", "chrome", false);
        docs.navigate(1, Some(7), "https://bank.test/login", "chrome", false);
        let old = docs.capture(1, Some(7), "https://bank.test/login").unwrap();
        docs.navigate(1, Some(7), "https://other.test/", "chrome", false);
        assert!(!docs.current(&old));
        let new = docs.capture(1, Some(7), "https://other.test/").unwrap();
        assert!(docs
            .capture(1, Some(7), "https://bank.test/login")
            .is_none());
        docs.moved(1, "chrome-2");
        assert!(!docs.current(&new));
        docs.navigate(1, None, "https://a.test/next", "chrome-2", false);
        let top = docs.capture(1, None, "https://a.test/next").unwrap();
        docs.navigate(1, None, "https://b.test/", "chrome-2", false);
        assert!(docs.settle(1, "https://b.test/", "chrome-2", false));
        assert!(docs.capture(1, Some(7), "https://other.test/").is_none());
        assert!(!docs.current(&top));
        let top = docs.capture(1, None, "https://b.test/").unwrap();
        docs.forget(1, None);
        assert!(!docs.current(&top));
    }

    #[test]
    fn cancelled_navigation_keeps_the_document() {
        let docs = Documents::default();
        docs.navigate(1, None, "https://a.test/", "chrome", false);
        let page = docs.capture(1, None, "https://a.test/").unwrap();
        // Ссылка ушла в загрузку с другого сайта: пока переход не решён,
        // секретов нет, а потом страница — та же.
        docs.navigate(1, None, "https://files.test/x.zip", "chrome", false);
        assert!(!docs.current(&page));
        assert_eq!(docs.capture(1, None, "https://a.test/"), Some(page.clone()));
        assert!(!docs.settle(1, "https://a.test/", "chrome", false));
        assert!(docs.current(&page));
        // Тот же сайт — тот же адресат.
        docs.navigate(1, None, "https://a.test/next", "chrome", false);
        assert!(docs.current(&page));
    }

    #[test]
    fn typing_survives_downloads_and_ends_with_the_document() {
        let docs = Documents::default();
        docs.mark_edited(1);
        assert!(docs.edited(1));
        docs.typing_navigation(1);
        // Ссылка ушла в загрузку: документ тот же.
        docs.typing_settled(1, false);
        assert!(docs.edited(1));
        docs.typing_navigation(1);
        docs.typing_settled(1, true);
        assert!(!docs.edited(1));
        // Начали печатать в новом документе раньше, чем он догрузился.
        docs.typing_navigation(1);
        std::thread::sleep(Duration::from_millis(5));
        docs.mark_edited(1);
        docs.typing_settled(1, true);
        assert!(docs.edited(1));
        docs.forget(1, None);
        assert!(!docs.edited(1));
    }

    #[test]
    fn cancelled_frame_navigation_expires() {
        let docs = Documents::default();
        docs.navigate(1, None, "https://a.test/", "chrome", false);
        docs.navigate(1, Some(7), "https://bank.test/login", "chrome", false);
        let frame = docs.capture(1, Some(7), "https://bank.test/login").unwrap();
        docs.navigate(1, Some(7), "https://files.test/x.zip", "chrome", false);
        let soon = Instant::now();
        assert!(docs
            .capture_at(1, Some(7), "https://bank.test/login", soon)
            .is_some());
        assert!(!docs.current(&frame));
        let later = soon + FRAME_SETTLE;
        assert_eq!(
            docs.capture_at(1, Some(7), "https://bank.test/login", later),
            Some(frame.clone())
        );
        assert!(docs.current(&frame));
    }

    #[test]
    fn foreign_frame_does_not_complete_top_navigation() {
        let docs = Documents::default();
        docs.navigate(1, None, "https://a.test/", "chrome", false);
        docs.navigate(1, None, "https://b.test/", "chrome", false);
        assert!(!docs.is_origin(1, None, "https://b.test/"));
        assert!(docs.is_origin(1, None, "https://a.test/"));
        let next = docs.capture(1, None, "https://b.test/").unwrap();
        assert!(docs.current(&next));
    }
}
