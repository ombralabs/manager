//! Ядро приложения без Tauri: то, что вызывают команды интерфейса (на этапе 2 — и Local API).
pub mod error;
pub mod archive;
pub mod announcements;
pub mod automation;
pub mod bulk;
pub mod cookies;
pub mod extensions;
pub mod csv;
pub mod forms;
pub mod groups;
pub mod import_support;
pub mod patch;
pub mod profiles;
pub mod profile_labels;
pub mod proxies;
pub mod proxy_import;
pub mod settings;
pub mod sync;
pub mod totp;
pub mod traffic;
pub mod trash;
pub mod windows;
#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)]
mod perf_tests;

pub use error::CommandError;

use crate::engine::ombra::OmbraEngine;
use crate::engine::{Engine, EngineBuild};
use crate::paths::Paths;
use crate::relay::geo::GeoClient;
use crate::store::Store;
use crate::store::secrets::SecretBox;
use crate::supervisor::{EventSink, Supervisor, Timings};
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;

pub const LANGUAGE_KEY: &str = "ui.language";

pub struct Core<E: Engine = OmbraEngine> {
    pub paths: Paths,
    pub store: Arc<Store>,
    pub engine: Arc<E>,
    pub geo: Arc<GeoClient>,
    pub supervisor: Supervisor<E>,
    /// Библиотека расширений: замок операций и адрес Web Store.
    pub library: extensions::ExtensionLibrary,
    /// Профили, занятые запуском процесса автоматизации.
    pub flows: crate::flow::FlowState,
    /// Пакеты задач, идущие в этом окне.
    pub automation: automation::Automation,
    /// Сессия синхронизатора окон.
    pub sync: sync::SyncSlot,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppInfo {
    pub version: String,
    pub data_dir: String,
    pub language: String,
    pub engine: Option<EngineBuild>,
    /// Почему ядро не открылось (связка ключей, база); интерфейс показывает это вместо экранов.
    pub init_error: Option<CommandError>,
}

impl<E: Engine> Core<E> {
    pub fn new(
        paths: Paths,
        store: Arc<Store>,
        engine: Arc<E>,
        geo: Arc<GeoClient>,
        sink: EventSink,
        timings: Timings,
    ) -> Self {
        let supervisor = Supervisor::new(paths.clone(), store.clone(), engine.clone(), geo.clone(), sink, timings);
        Self { paths, store, engine, geo, supervisor, library: extensions::ExtensionLibrary::default(), flows: Default::default(), automation: Default::default(), sync: Default::default() }
    }

    pub fn app_info(&self) -> AppInfo {
        AppInfo {
            version: env!("CARGO_PKG_VERSION").into(),
            data_dir: self.paths.root().display().to_string(),
            language: self.language(),
            engine: self.engine.installed(),
            init_error: None,
        }
    }

    pub fn language(&self) -> String {
        self.store
            .get_setting(LANGUAGE_KEY)
            .ok()
            .flatten()
            .filter(|l| l == "ru" || l == "en")
            .unwrap_or_else(|| "ru".into())
    }

    pub fn set_language(&self, language: &str) -> Result<(), CommandError> {
        if language != "ru" && language != "en" {
            return Err(CommandError::new("Validation", format!("неизвестный язык интерфейса: {language}")));
        }
        self.store.set_setting(LANGUAGE_KEY, language)?;
        crate::security::remember_language(&self.paths, language);
        Ok(())
    }

    /// Установка в новую неизменяемую папку: работающие браузеры продолжают использовать прежнюю сборку.
    pub async fn import_engine(&self, archive: &Path) -> Result<EngineBuild, CommandError> {
        Ok(self.engine.import_archive(archive).await?)
    }
}

/// Хранилище, ключ секретов и движок в папке данных — то, что `lib.rs` открывает при старте приложения.
pub fn open_default(paths: Paths, sink: EventSink) -> Result<Core, CommandError> {
    open(paths, sink, GeoClient::new())
}

/// Как `open_default`, со своими сервисами геоданных: стенд движка (`examples/stand_api.rs`) обходится без них.
pub fn open(paths: Paths, sink: EventSink, geo: GeoClient) -> Result<Core, CommandError> {
    std::fs::create_dir_all(paths.root())?;
    let key = crate::security::profile_key(&paths)?;
    let store = Arc::new(Store::open(&paths.db(), SecretBox::from_key(&key))?);
    let engine = Arc::new(OmbraEngine::new(paths.engines()));
    let core = Core::new(paths, store, engine, Arc::new(geo), sink, Timings::default());
    if core.paths.root().join("ui.json").exists() { core.set_language(&crate::security::boot_language(&core.paths))?; }
    else { crate::security::remember_language(&core.paths, &core.language()); }
    // Пакеты задач процессов Ombra, которых уже нет (падение, kill -9): их запуски — «Отменён» (спецификация А2).
    match core.store.close_dead_batches(&crate::sys::pid_alive, crate::sys::unix_now()) {
        Ok(0) => {}
        Ok(count) => log::warn!("закрыты пакеты задач завершившихся процессов Ombra: {count}"),
        Err(e) => log::warn!("пакеты задач завершившихся процессов не закрыты: {e}"),
    }
    Ok(core)
}
