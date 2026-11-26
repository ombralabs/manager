//! Синхронизатор окон в ядре: одна сессия на окно Ombra. Профили сессии заняты `flows.claim`, пока она идёт, —
//! процесс автоматизации ждёт её конца.
use super::{CommandError, Core};
use crate::engine::page::Browser;
use crate::engine::Engine;
use crate::fingerprint::FieldError;
use crate::flow::Busy;
use crate::supervisor::{Event, StartOptions, Supervisor, SupervisorError};
use crate::sync::session::{self, OnChange, Session, SyncState, WindowSpec};
use crate::sync::{self, SyncSettings};
use std::{collections::HashSet, sync::Arc, time::Duration};

/// Сессия и занятость её профилей. `Arc`: сторож конца сессии очищает слот без ссылки на ядро.
pub type SyncSlot = Arc<tokio::sync::Mutex<Option<(Session, Vec<Busy>)>>>;

fn cdp(e: impl ToString) -> CommandError {
    CommandError::new("Cdp", e.to_string())
}

/// Уборка относится только к своей сессии; её профили заняты до конца закрытия ведомых.
async fn finish<E: Engine>(slot: &SyncSlot, session: &Session, supervisor: &Supervisor<E>) {
    let mut slot = slot.lock().await;
    if !slot.as_ref().is_some_and(|(current, _)| current.same_session(session)) {
        return;
    }
    if session.master_closed() {
        let ids: Vec<_> = session.state().windows.into_iter().filter(|w| !w.main).map(|w| w.profile_id).collect();
        if let Err(e) = stop_profiles(supervisor, &ids).await {
            log::error!("закрытие ведомых синхронизатора: {}", e.message);
        }
    }
    slot.take();
    supervisor.emit(Event::SyncChanged(SyncState::idle()));
}

async fn stop_profiles<E: Engine>(supervisor: &Supervisor<E>, ids: &[String]) -> Result<(), CommandError> {
    let results = futures_util::future::join_all(ids.iter().map(|id| async move {
        match supervisor.stop(id).await {
            Ok(()) | Err(SupervisorError::NotRunning) => {}
            Err(e) => return Err(CommandError::from(e)),
        }
        // Ранний запуск ещё ждёт готовности супервизора: Stop отменил его, но уборка не закончилась.
        tokio::time::timeout(Duration::from_secs(30), async {
            while supervisor.status(id).is_active() {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }).await.map_err(|_| CommandError::new("StopFailed", "Профиль не завершил остановку"))
    })).await;
    results.into_iter().collect()
}

impl<E: Engine> Core<E> {
    fn profile_name(&self, id: &str) -> String {
        self.store.get_profile_any(id).ok().flatten().map(|p| p.name).unwrap_or_else(|| id.to_string())
    }

    /// Адрес CDP браузера работающего профиля.
    fn running_ws(&self, id: &str) -> Result<String, CommandError> {
        self.supervisor
            .running_info(id)
            .map(|info| info.cdp_ws)
            .ok_or_else(|| CommandError::new("NotRunning", format!("профиль «{}» не запущен", self.profile_name(id))))
    }

    /// Подтверждённый в попапе чистый запуск: проверки и занятость — до перезапуска браузеров.
    pub async fn sync_start(&self, main: &str, followers: &[String]) -> Result<SyncState, CommandError> {
        let ids: Vec<String> = std::iter::once(main.to_owned()).chain(followers.iter().cloned()).collect();
        if followers.is_empty() || ids.iter().collect::<HashSet<_>>().len() != ids.len() {
            return Err(CommandError::validation(vec![FieldError { field: "followers", code: "invalid" }]));
        }
        let mut slot = self.sync.clone().lock_owned().await;
        if slot.is_some() {
            return Err(CommandError::new("SyncRunning", "синхронизация уже идёт"));
        }
        let mut options = Vec::new();
        for id in &ids {
            let profile = self.store.get_profile(id)?.ok_or_else(|| CommandError::new("NotFound", format!("Профиль «{id}» не найден")))?;
            let mut args = profile.extra_args;
            args.retain(|arg| arg.split('=').next() != Some("--restore-last-session"));
            args.push("--no-startup-window".into());
            options.push(StartOptions {
                extra_args: Some(args), restore_tabs: Some(false), start_urls: Some(vec!["about:blank".into()]), ..Default::default()
            });
        }
        let mut busy = Vec::new();
        for id in &ids {
            busy.push(self.flows.claim(id).ok_or_else(|| CommandError::new(
                "ProfileBusy", format!("Профиль «{}» используется выполняющимся сценарием", self.profile_name(id)),
            ))?);
        }
        let supervisor = self.supervisor.clone();
        let slot_ref = self.sync.clone();
        let settings = sync::load(&self.store);
        // Брошенный вызов интерфейса не бросает перезапуск и его откат на полпути.
        tokio::spawn(async move {
            let start = async {
                stop_profiles(&supervisor, &ids).await?;
                let mut specs = futures_util::future::try_join_all(ids.iter().zip(&options).map(|(id, options)| async {
                    let info = supervisor.start_with(id, options.clone()).await?;
                    Ok::<_, CommandError>(WindowSpec { profile_id: id.clone(), ws: info.cdp_ws })
                })).await?;
                let main = specs.remove(0);
                session::align_tabs(&main, &specs).await.map_err(cdp)?;
                let sink = supervisor.sink();
                let on_change: OnChange = Arc::new(move |state| sink(Event::SyncChanged(state)));
                Session::start(main, specs, settings, on_change).await.map_err(cdp)
            };
            let result = tokio::time::timeout(Duration::from_secs(120), start).await
                .unwrap_or_else(|_| Err(CommandError::new("LaunchFailed", "Не удалось запустить все профили за две минуты")));
            let session = match result {
                Ok(session) => session,
                Err(mut error) => {
                    if let Err(cleanup) = stop_profiles(&supervisor, &ids).await {
                        error.message.push_str(&format!(". Не удалось закрыть все окна: {}", cleanup.message));
                    }
                    return Err(error);
                }
            };
            let state = session.state();
            *slot = Some((session.clone(), busy));
            drop(slot);
            supervisor.emit(Event::SyncChanged(state.clone()));
            tokio::spawn(async move {
                session.ended().await;
                finish(&slot_ref, &session, &supervisor).await;
            });
            Ok(state)
        }).await.map_err(cdp)?
    }

    pub async fn sync_stop(&self) {
        let slot = self.sync.clone();
        let supervisor = self.supervisor.clone();
        // Как и запуск, очистка переживает отмену вызова интерфейсом.
        let task = tokio::spawn(async move {
            let session = slot.lock().await.as_ref().map(|(s, _)| s.clone());
            if let Some(session) = session {
                session.stop().await;
                finish(&slot, &session, &supervisor).await;
            }
        });
        if let Err(error) = task.await {
            log::error!("остановка синхронизатора прервана: {error}");
        }
    }

    /// Состояние сессии; пока она стартует (слот занят стартом) — «сессии нет».
    pub fn sync_state(&self) -> SyncState {
        match self.sync.try_lock() {
            Ok(slot) => slot.as_ref().map(|(s, _)| s.state()).unwrap_or_else(SyncState::idle),
            Err(_) => SyncState::idle(),
        }
    }

    async fn session(&self) -> Result<Session, CommandError> {
        self.sync.lock().await.as_ref().map(|(s, _)| s.clone()).ok_or_else(|| CommandError::new("NotRunning", "синхронизация не идёт"))
    }

    pub async fn sync_release(&self, id: &str) -> Result<(), CommandError> {
        self.session().await?.release(id).await.map_err(|e| CommandError::new("BadParams", e))
    }

    pub async fn sync_resume(&self, id: &str) -> Result<(), CommandError> {
        self.session().await?.resume(id).await.map_err(|e| CommandError::new("BadParams", e))
    }

    /// Ведомое `id` открывает в своей вкладке адрес видимой вкладки главного (`Session::open_like_main`).
    pub async fn sync_open_like_main(&self, id: &str) -> Result<(), CommandError> {
        self.session().await?.open_like_main(id).await.map_err(cdp)
    }

    /// Окно выходит на передний план со своей видимой вкладкой. Не видна ни одна (окно свёрнуто или закрыто другими) —
    /// у окна сессии вкладка с номером видимой у главного, иначе первая.
    pub async fn show_window(&self, id: &str) -> Result<(), CommandError> {
        let browser = Browser::connect(&self.running_ws(id)?).await.map_err(cdp)?;
        let target = match browser.visible_tab().await.map_err(cdp)? {
            Some(target) => target,
            None => {
                let session = self.sync.lock().await.as_ref().map(|(s, _)| s.clone());
                let tab = match session {
                    Some(session) if session.state().windows.iter().any(|w| w.profile_id == id) => session.main_tab().await,
                    _ => 0,
                };
                let tabs = browser.tabs().await.map_err(cdp)?;
                tabs.get(tab).or(tabs.first()).ok_or_else(|| cdp("у окна нет вкладок"))?.target_id.clone()
            }
        };
        browser.activate(&target).await.map_err(cdp)
    }

    pub fn sync_settings(&self) -> SyncSettings {
        sync::load(&self.store)
    }

    pub fn set_sync_settings(&self, s: SyncSettings) -> Result<(), CommandError> {
        sync::save(&self.store, &s)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{create_with_proxy, env};
    use crate::sync::{Mode, SyncSettings};

    #[tokio::test]
    async fn sync_start_validates() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "Альфа", e.proxy_port, 1).id;
        let b = create_with_proxy(&e.core, "Бета", e.proxy_port, 2).id;
        let code = |r: Result<_, crate::app::CommandError>| r.map(|_: crate::sync::session::SyncState| ()).unwrap_err();

        assert_eq!(code(e.core.sync_start(&a, &[]).await).code, "Validation");
        assert_eq!(code(e.core.sync_start(&a, std::slice::from_ref(&a)).await).code, "Validation");
        assert_eq!(code(e.core.sync_start(&a, &[b.clone(), b.clone()]).await).code, "Validation");
        assert_eq!(code(e.core.sync_start(&a, &["missing".into()]).await).code, "NotFound");
        assert!(!e.core.supervisor.status(&a).is_active());

        e.core.supervisor.start(&a).await.unwrap();
        e.core.supervisor.start(&b).await.unwrap();
        let before = e.core.supervisor.running_info(&a).unwrap().pid;
        let _busy = e.core.flows.claim(&b).unwrap();
        let err = code(e.core.sync_start(&a, std::slice::from_ref(&b)).await);
        assert_eq!(err.code, "ProfileBusy");
        assert!(err.message.contains("Бета"), "{}", err.message);
        assert!(e.core.flows.claim(&a).is_some(), "отказ не оставляет занятости");
        assert!(!e.core.sync_state().active);
        assert_eq!(e.core.supervisor.running_info(&a).unwrap().pid, before, "занятость проверяется до перезапуска");
    }

    #[tokio::test]
    async fn failed_sync_start_rolls_back_clean_launches() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "Альфа", e.proxy_port, 1).id;
        let b = create_with_proxy(&e.core, "Бета", e.proxy_port, 2).id;
        for id in [&a, &b] {
            e.core.store.raw().execute("UPDATE profiles SET extra_args = '[\"--restore-last-session\"]' WHERE id = ?1", [id]).unwrap();
        }
        let err = e.core.sync_start(&a, std::slice::from_ref(&b)).await.unwrap_err();
        let args = e.core.engine.last_request().extra_args;
        assert!(args.iter().any(|a| a == "--no-startup-window"), "начальную вкладку создаёт синхронизатор после cookies");
        assert!(!args.iter().any(|a| a.starts_with("--restore-last-session")), "дополнительный флаг профиля не отменяет чистый старт");
        assert_eq!(err.code, "Cdp", "тестовый движок не поддерживает захват; запуск должен дойти до подключения");
        for id in [&a, &b] {
            assert!(!e.core.supervisor.status(id).is_active(), "частично начатый сеанс закрыт");
            assert!(e.core.flows.claim(id).is_some());
        }
        assert!(!e.core.sync_state().active);
    }

    #[tokio::test]
    async fn sync_settings_roundtrip() {
        let e = env().await;
        assert_eq!(e.core.sync_settings(), SyncSettings::default());
        let s = SyncSettings { mode: Mode::Varied, clicks: false, ..Default::default() };
        e.core.set_sync_settings(s.clone()).unwrap();
        assert_eq!(e.core.sync_settings(), s);
        let bad = SyncSettings { click_delay: [9, 1], ..Default::default() };
        assert_eq!(e.core.set_sync_settings(bad).unwrap_err().code, "Validation");
        assert_eq!(e.core.sync_settings(), s);
    }
}
