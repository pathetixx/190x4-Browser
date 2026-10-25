//! Выход и установка обновления ждут согласия всех окон и записи их сессий.
use std::collections::HashSet;
use std::time::Duration;

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::oneshot;

#[derive(Default)]
pub struct Shutdown(Mutex<State>);

#[derive(Default)]
struct State {
    next: u64,
    active: Option<Attempt>,
}

struct Attempt {
    id: u64,
    waiting: HashSet<String>,
    reply: Option<oneshot::Sender<bool>>,
}

/// Пока разрешение живо, новая попытка выхода и новые окна не создаются.
pub struct Permit {
    app: AppHandle,
    id: u64,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let state = self.app.state::<Shutdown>();
        let mut inner = state.0.lock();
        if inner
            .active
            .as_ref()
            .is_some_and(|active| active.id == self.id)
        {
            inner.active = None;
        }
        drop(inner);
        let _ = self.app.emit("shutdown-finished", self.id);
    }
}

pub fn is_preparing(app: &AppHandle) -> bool {
    app.state::<Shutdown>().0.lock().active.is_some()
}

impl Attempt {
    fn answer(&mut self, label: &str, allowed: bool) {
        if !self.waiting.remove(label) {
            return;
        }
        if !allowed || self.waiting.is_empty() {
            if let Some(reply) = self.reply.take() {
                let _ = reply.send(allowed);
            }
        }
    }
}

#[tauri::command]
pub fn shutdown_answer(app: AppHandle, window: tauri::Window, attempt: u64, allowed: bool) {
    let state = app.state::<Shutdown>();
    let mut inner = state.0.lock();
    if let Some(active) = inner.active.as_mut().filter(|active| active.id == attempt) {
        active.answer(window.label(), allowed);
    }
}

pub fn window_gone(app: &AppHandle, label: &str) {
    if let Some(active) = app.state::<Shutdown>().0.lock().active.as_mut() {
        active.answer(label, false);
    }
}

pub async fn prepare(app: &AppHandle) -> Result<Option<Permit>, String> {
    let labels = app.state::<crate::state::App>().windows.labels();
    let (tx, rx) = oneshot::channel();
    let id = {
        let state = app.state::<Shutdown>();
        let mut inner = state.0.lock();
        if inner.active.is_some() {
            return Err("Браузер уже готовится к закрытию".into());
        }
        inner.next += 1;
        let id = inner.next;
        inner.active = Some(Attempt {
            id,
            waiting: labels.iter().cloned().collect(),
            reply: Some(tx),
        });
        id
    };
    let permit = Permit {
        app: app.clone(),
        id,
    };
    for label in &labels {
        app.emit_to(label.as_str(), "shutdown-prepare", id)
            .map_err(|err| err.to_string())?;
    }
    if labels.is_empty() {
        return Ok(Some(permit));
    }
    let allowed = tokio::time::timeout(Duration::from_secs(120), rx)
        .await
        .map_err(|_| "Окно не ответило. Закрытие отменено, чтобы сохранить данные".to_string())?
        .map_err(|_| "Проверка страниц прервалась".to_string())?;
    Ok(allowed.then_some(permit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_windows_must_agree_and_one_refusal_cancels() {
        let (tx, mut rx) = oneshot::channel();
        let mut attempt = Attempt {
            id: 1,
            waiting: ["a".into(), "b".into()].into(),
            reply: Some(tx),
        };
        attempt.answer("unknown", true);
        attempt.answer("a", true);
        assert!(rx.try_recv().is_err());
        attempt.answer("b", false);
        assert_eq!(rx.try_recv(), Ok(false));
        let (tx, mut rx) = oneshot::channel();
        let mut attempt = Attempt {
            id: 2,
            waiting: ["a".into(), "b".into()].into(),
            reply: Some(tx),
        };
        attempt.answer("a", true);
        attempt.answer("b", true);
        assert_eq!(rx.try_recv(), Ok(true));
    }
}
