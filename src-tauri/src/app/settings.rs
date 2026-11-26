//! Карточка «Профили и шлюзы» экрана «Настройки» и выход из приложения по настройке «При закрытии приложения».
use super::{CommandError, Core};
use crate::engine::Engine;
use crate::store::QuitAction;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ProfileSettings {
    pub on_quit: QuitAction,
}

/// Что делать по Cmd+Q и закрытию окна.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitPlan {
    /// Активных профилей нет — выйти сразу.
    Exit,
    /// Спросить «Остановить все профили и выйти?».
    Ask,
    /// Без вопроса остановить все профили и выйти.
    StopAll,
}

impl<E: Engine> Core<E> {
    pub fn profile_settings(&self) -> Result<ProfileSettings, CommandError> {
        Ok(ProfileSettings { on_quit: self.store.on_quit()? })
    }

    pub fn set_on_quit(&self, action: QuitAction) -> Result<(), CommandError> {
        self.store.set_on_quit(action)?;
        Ok(())
    }

    /// Ключ Google из JSON-файла (читается в ядре, не в окне); возвращает адрес аккаунта. Негодный файл — `BadParams`.
    /// Ключ сервисного аккаунта — пара килобайт: файл больше мегабайта — не он и целиком в память не читается.
    pub fn set_google_key(&self, path: &std::path::Path) -> Result<String, CommandError> {
        use std::io::Read;
        const LIMIT: u64 = 1 << 20;
        let unread = || CommandError::new("BadParams", "файл ключа Google не прочитан");
        let mut json = String::new();
        std::fs::File::open(path).map_err(|_| unread())?.take(LIMIT + 1).read_to_string(&mut json).map_err(|_| unread())?;
        if json.len() as u64 > LIMIT {
            return Err(CommandError::new("BadParams", "Файл ключа Google слишком большой. Выберите JSON-файл сервисного аккаунта размером до 1 МБ."));
        }
        crate::flow::google::set_key(&self.store, &json).map_err(|e| CommandError::new("BadParams", e))
    }

    pub fn clear_google_key(&self) -> Result<(), CommandError> {
        crate::flow::google::clear_key(&self.store).map_err(|e| CommandError::new("Storage", e))
    }

    /// Адрес сервисного аккаунта; `None` — ключ не загружен.
    pub fn google_key_email(&self) -> Result<Option<String>, CommandError> {
        crate::flow::google::key_email(&self.store).map_err(|e| CommandError::new("Storage", e))
    }

    /// Прокси для сервисов автоматизации (почта, OpenAI и др.); `None` — сеть компьютера.
    pub fn services_proxy(&self) -> Result<Option<i64>, CommandError> {
        crate::flow::services::proxy_id(&self.store).map_err(|e| CommandError::new("Storage", e))
    }

    /// Несуществующий прокси — `NotFound`.
    pub fn set_services_proxy(&self, id: Option<i64>) -> Result<(), CommandError> {
        Ok(crate::flow::services::set_proxy_id(&self.store, id)?)
    }

    /// Активных профилей нет — `Exit`; иначе по настройке. Настройка не прочиталась — спросить.
    pub fn quit_plan(&self) -> QuitPlan {
        if self.supervisor.active_count() == 0 {
            return QuitPlan::Exit;
        }
        match self.store.on_quit() {
            Ok(QuitAction::StopAll) => QuitPlan::StopAll,
            Ok(QuitAction::Ask) => QuitPlan::Ask,
            Err(e) => {
                log::warn!("настройка «При закрытии приложения» не прочитана: {e}");
                QuitPlan::Ask
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::env;
    use crate::supervisor::test_engine::add_profile;

    #[tokio::test]
    async fn services_proxy_roundtrip_and_not_found() {
        let e = env().await;
        assert_eq!(e.core.services_proxy().unwrap(), None);
        assert_eq!(e.core.set_services_proxy(Some(77)).unwrap_err().code, "NotFound");
        let id = e
            .core
            .store
            .create_proxy(&crate::store::proxies::ProxyInput {
                kind: crate::store::proxies::ProxyKind::Http,
                host: "127.0.0.1".into(),
                port: 3128,
                username: None,
                password: crate::store::secrets::SecretField::Keep,
                note: String::new(),
                enabled: true,
            })
            .unwrap()
            .id;
        e.core.set_services_proxy(Some(id)).unwrap();
        assert_eq!(e.core.services_proxy().unwrap(), Some(id));
        e.core.set_services_proxy(None).unwrap();
        assert_eq!(e.core.services_proxy().unwrap(), None);
    }

    #[tokio::test]
    async fn a_google_key_file_over_one_mebibyte_is_refused() {
        let e = env().await;
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.json");
        std::fs::write(&big, vec![b' '; (1 << 20) + 1]).unwrap();
        let err = e.core.set_google_key(&big).unwrap_err();
        assert_eq!((err.code.as_str(), err.message.as_str()), ("BadParams", "Файл ключа Google слишком большой. Выберите JSON-файл сервисного аккаунта размером до 1 МБ."));
    }

    #[tokio::test]
    async fn profile_settings_preserve_quit_behavior_without_exposing_a_run_limit() {
        let e = env().await;
        e.core.store.set_setting("profiles.max_running", "1").unwrap();
        assert_eq!(serde_json::to_value(e.core.profile_settings().unwrap()).unwrap(), serde_json::json!({"on_quit":"ask"}));
        e.core.set_on_quit(QuitAction::StopAll).unwrap();
        assert_eq!(e.core.profile_settings().unwrap().on_quit, QuitAction::StopAll);
        assert_eq!(e.core.store.get_setting("profiles.max_running").unwrap().as_deref(), Some("1"));
    }

    #[tokio::test]
    async fn quit_plan_follows_active_profiles_and_setting() {
        let e = env().await;
        assert_eq!(e.core.quit_plan(), QuitPlan::Exit);
        e.core.set_on_quit(QuitAction::StopAll).unwrap();
        assert_eq!(e.core.quit_plan(), QuitPlan::Exit, "без активных профилей выход сразу при любой настройке");

        let id = add_profile(&e.core.store, Some(e.proxy_port), "pass");
        e.core.supervisor.start(&id).await.unwrap();
        assert_eq!(e.core.quit_plan(), QuitPlan::StopAll);
        e.core.set_on_quit(QuitAction::Ask).unwrap();
        assert_eq!(e.core.quit_plan(), QuitPlan::Ask);
        e.core.supervisor.stop(&id).await.unwrap();
        assert_eq!(e.core.quit_plan(), QuitPlan::Exit);

        e.core.set_on_quit(QuitAction::StopAll).unwrap();
        e.core.supervisor.start(&id).await.unwrap();
        e.core.store.raw().execute_batch("DROP TABLE settings").unwrap();
        assert_eq!(e.core.quit_plan(), QuitPlan::Ask, "настройка не прочиталась — спросить");
    }
}
