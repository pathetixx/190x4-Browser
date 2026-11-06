//! Контекст документа: фоновый ответ не переживает навигацию или перенос вкладки.
use std::collections::HashMap;

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
pub struct Documents(Mutex<Registry>);

type Slot = (u32, Option<u32>);

#[derive(Default)]
struct Registry {
    next: u64,
    /// Документ, который сейчас на экране вкладки или фрейма.
    entries: HashMap<Slot, Context>,
    /// Переход на другой origin начался, но документ ещё не сменился: переход
    /// могут отменить («Остаться»), он может уйти в загрузку. Прежний документ
    /// пока остаётся на месте, но секреты ни ему, ни новому не уходят.
    pending: HashMap<Slot, Context>,
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
                state.pending.insert(slot, context);
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
        let origin = origin_of(source)?;
        let mut state = self.0.lock();
        let slot = (tab, frame);
        let arrived = state
            .pending
            .get(&slot)
            .is_some_and(|next| next.origin == origin)
            && state
                .entries
                .get(&slot)
                .is_none_or(|current| current.origin != origin);
        if arrived {
            if let Some(context) = state.pending.remove(&slot) {
                state.replace(context);
            }
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
            Some(next) if next.origin == origin => next,
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

    pub fn forget(&self, tab: u32, frame: Option<u32>) {
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
        for ((id, _), context) in entries.iter_mut().chain(pending.iter_mut()) {
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
