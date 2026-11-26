//! Автоматизация: процессы по образцу AdsPower RPA Plus — граф операций, исполнитель в ядре
//! (спецификация `docs/superpowers/specs/2026-10-20-automation-design.md`).
pub mod model;
pub mod validate;
pub mod vars;
pub mod params;
pub mod ops;
pub mod bundle;
pub mod interp;
pub mod journal;
pub mod host;
pub mod run;
pub mod google;
pub mod services;
pub mod tasks;
mod task_queue;
#[cfg(test)]
pub(crate) mod live_tests;
#[cfg(test)]
mod examples_tests;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// Профили, на которых в этом процессе Ombra идёт запуск процесса автоматизации. Запуск задачи ждёт освобождения
/// профиля (`claim_wait`); запуск вне задачи и New Browser на занятом получают ошибку. Клоны делят один набор.
#[derive(Default, Clone)]
pub struct FlowState {
    busy: Arc<Mutex<HashSet<String>>>,
    freed: Arc<Notify>,
}

/// Профиль занят запуском, пока жив этот объект.
pub struct Busy {
    busy: Arc<Mutex<HashSet<String>>>,
    freed: Arc<Notify>,
    id: String,
}

impl FlowState {
    pub(crate) fn claim(&self, id: &str) -> Option<Busy> {
        let fresh = self.busy.lock().expect("мьютекс занятых профилей отравлен").insert(id.to_string());
        fresh.then(|| Busy { busy: self.busy.clone(), freed: self.freed.clone(), id: id.to_string() })
    }

    /// Ждёт освобождения профиля и занимает его; `None` — отмена раньше.
    pub(crate) async fn claim_wait(&self, id: &str, cancel: &CancellationToken) -> Option<Busy> {
        self.claim_with(cancel, || self.claim(id)).await
    }

    /// Проверяет весь доступный пул после каждого освобождения; подписка до проверки не теряет пробуждения.
    pub(crate) async fn claim_with<T>(&self, cancel: &CancellationToken, try_claim: impl Fn() -> Option<T>) -> Option<T> {
        loop {
            let freed = self.freed.notified();
            tokio::pin!(freed);
            // Подписка до проверки: освобождение между проверкой и ожиданием не теряется.
            freed.as_mut().enable();
            if let Some(claimed) = try_claim() {
                return Some(claimed);
            }
            tokio::select! {
                _ = &mut freed => {}
                _ = cancel.cancelled() => return None,
            }
        }
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.busy.lock().expect("мьютекс занятых профилей отравлен").remove(&self.id);
        self.freed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn waiting_claim_takes_the_profile_when_it_is_freed_or_gives_up_on_cancel() {
        let state = FlowState::default();
        let first = state.claim("p1").unwrap();
        let cancel = CancellationToken::new();
        let waiting = {
            let (state, cancel) = (state.clone(), cancel.clone());
            tokio::spawn(async move { state.claim_wait("p1", &cancel).await.is_some() })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!waiting.is_finished(), "профиль занят — ждёт");
        drop(first);
        assert!(tokio::time::timeout(Duration::from_secs(1), waiting).await.unwrap().unwrap(), "освободили — занял");
        let _held = state.claim("p2").unwrap();
        let waiting = {
            let (state, cancel) = (state.clone(), cancel.clone());
            tokio::spawn(async move { state.claim_wait("p2", &cancel).await.is_some() })
        };
        cancel.cancel();
        assert!(!tokio::time::timeout(Duration::from_secs(1), waiting).await.unwrap().unwrap(), "отмена — без профиля");
    }
}
