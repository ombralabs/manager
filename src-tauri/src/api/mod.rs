//! Local API, совместимый с AdsPower (спецификация 2Б): переводит запросы AdsPower в вызовы ядра 2А
//! и ответы ядра — в формат AdsPower. Модуль не знает о Tauri: сервером управляет `control::ApiController`.
pub mod control;
pub mod envelope;
pub mod mapping;
pub mod params;
mod browser;
mod extensions;
mod groups;
mod profiles;
mod proxies;
mod server;
mod stubs;
#[cfg(test)]
pub(crate) mod test_support;

pub use server::{ApiError, ApiServer, ApiSettings, router, serve};
pub use control::{ApiConfig, ApiConfigForm, ApiController, ApiStatus, StatusSink};
