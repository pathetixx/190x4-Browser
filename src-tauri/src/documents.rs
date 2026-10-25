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

#[derive(Default)]
struct Registry {
    next: u64,
    entries: HashMap<(u32, Option<u32>), Context>,
}

impl Documents {
    pub fn navigate(&self, tab: u32, frame: Option<u32>, url: &str, window: &str, private: bool) {
        let mut state = self.0.lock();
        state
            .entries
            .retain(|(id, slot), _| *id != tab || (frame.is_some() && *slot != frame));
        state.next += 1;
        let context = Context {
            tab,
            frame,
            generation: state.next,
            origin: origin_of(url).unwrap_or_default(),
            window: window.into(),
            private,
        };
        state.entries.insert((tab, frame), context);
    }

    pub fn capture(&self, tab: u32, frame: Option<u32>, source: &str) -> Option<Context> {
        let origin = origin_of(source)?;
        self.0
            .lock()
            .entries
            .get(&(tab, frame))
            .filter(|entry| entry.origin == origin)
            .cloned()
    }

    pub fn current(&self, context: &Context) -> bool {
        self.0.lock().entries.get(&(context.tab, context.frame)) == Some(context)
    }

    pub fn forget(&self, tab: u32, frame: Option<u32>) {
        self.0
            .lock()
            .entries
            .retain(|(id, slot), _| *id != tab || (frame.is_some() && *slot != frame));
    }

    pub fn moved(&self, tab: u32, window: &str) {
        for ((id, _), context) in &mut self.0.lock().entries {
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
        assert!(docs
            .capture(1, Some(7), "https://bank.test/login")
            .is_none());
        let new = docs.capture(1, Some(7), "https://other.test/").unwrap();
        docs.moved(1, "chrome-2");
        assert!(!docs.current(&new));
        docs.navigate(1, None, "https://a.test/next", "chrome-2", false);
        assert!(docs.capture(1, Some(7), "https://other.test/").is_none());
        let top = docs.capture(1, None, "https://a.test/next").unwrap();
        docs.forget(1, None);
        assert!(!docs.current(&top));
    }
}
