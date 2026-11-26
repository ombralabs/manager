//! Local API поверх ядра с подставным движком — для тестов модуля `api`.
use super::{ApiServer, ApiSettings, serve};
use crate::app::Core;
use crate::app::tests::test_core_with_sink;
use crate::supervisor::Event;
use crate::supervisor::test_engine::FakeEngine;
use serde_json::Value;
use std::sync::{Arc, Mutex};

pub(crate) struct ApiEnv {
    pub core: Arc<Core<FakeEngine>>,
    pub proxy_port: u16,
    pub server: ApiServer,
    pub base: String,
    pub client: reqwest::Client,
    /// Все события ядра по порядку.
    pub events: Arc<Mutex<Vec<Event>>>,
    _dir: tempfile::TempDir,
}

/// Сервер без ключа API.
pub(crate) async fn api_env() -> ApiEnv {
    start(None).await
}

pub(crate) async fn api_env_with_key(key: &str) -> ApiEnv {
    start(Some(key.to_string())).await
}

async fn start(key: Option<String>) -> ApiEnv {
    let events: Arc<Mutex<Vec<Event>>> = Arc::default();
    let recorder = events.clone();
    let (core, proxy_port, dir) = test_core_with_sink(Arc::new(move |event: Event| {
        recorder.lock().expect("мьютекс событий отравлен").push(event);
    }))
    .await;
    let core = Arc::new(core);
    let server = serve(core.clone(), ApiSettings { port: 0, key }).await.expect("Local API не поднялся");
    let base = format!("http://{}", server.addr());
    // Без системного прокси: запросы идут прямо на 127.0.0.1. User-Agent reqwest сам не шлёт — как скрипт без него.
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    ApiEnv { core, proxy_port, server, base, client, events, _dir: dir }
}

impl ApiEnv {
    /// Сколько раз ядро сообщило интерфейсу «данные изменились».
    pub fn changes(&self) -> usize {
        self.events.lock().expect("мьютекс событий отравлен").iter().filter(|e| matches!(e, Event::ProfilesChanged)).count()
    }

    /// GET; ответ обязан быть HTTP 200 с JSON.
    pub async fn get(&self, path_and_query: &str) -> Value {
        let response = self.client.get(format!("{}{path_and_query}", self.base)).send().await.unwrap();
        json_200(response).await
    }

    /// POST с JSON-телом; ответ обязан быть HTTP 200 с JSON.
    pub async fn post(&self, path: &str, body: Value) -> Value {
        let response = self.client.post(format!("{}{path}", self.base)).json(&body).send().await.unwrap();
        json_200(response).await
    }
}

async fn json_200(response: reqwest::Response) -> Value {
    assert_eq!(response.status().as_u16(), 200, "HTTP-статус ответа Local API");
    response.json().await.expect("тело ответа — JSON")
}

/// Имена полей объекта ответа — для сверки формы с примерами AdsPower из справочника.
pub(crate) fn keys(value: &serde_json::Value) -> std::collections::BTreeSet<String> {
    value.as_object().expect("в ответе объект").keys().cloned().collect()
}

pub(crate) fn names(list: &[&str]) -> std::collections::BTreeSet<String> {
    list.iter().map(|name| name.to_string()).collect()
}

/// Путь с query для GET: значения кодируются, как их кодирует HTTP-клиент скрипта (`launch_args=["--a"]`).
pub(crate) fn with_query(path: &str, pairs: &[(&str, &str)]) -> String {
    let mut url = reqwest::Url::parse("http://127.0.0.1").expect("базовый адрес").join(path).expect("путь");
    url.query_pairs_mut().extend_pairs(pairs);
    match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    }
}
