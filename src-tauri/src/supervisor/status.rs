use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::flow::journal::Entry;
use crate::flow::run::RunStatus;

/// Данные запущенного профиля (спецификация этапа 1, «Супервизор»; `proxy_id` — этап 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunningInfo {
    pub pid: u32,
    pub cdp_ws: String,
    pub debug_port: u16,
    pub relay_port: u16,
    /// Прокси, через который идёт профиль.
    /// 0 — явно выбранное прямое соединение; положительное значение — запись прокси.
    pub proxy_id: i64,
    pub exit_ip: Option<String>,
    pub country: Option<String>,
    pub timezone: String,
    pub started_at: i64,
}

/// Статус живёт только в памяти: после перезапуска приложения все профили «остановлены».
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ProfileStatus {
    Stopped,
    Starting,
    Running(RunningInfo),
    /// Шлюз не отвечает: браузер открыт, соединения получают 502, прокси не меняется (решение заказчика 01.10.2026).
    Paused(RunningInfo),
    /// Браузер ждёт поток лицензии (`503` на `/json/version`, состояние не `ok`): `license` — состояние от браузера.
    /// `info` нет — ждёт при запуске; есть — работающий браузер потерял поток и заблокирован до его возврата.
    Waiting {
        license: String,
        info: Option<RunningInfo>,
        #[serde(rename = "ownerHint", skip_serializing_if = "Option::is_none")]
        owner_hint: Option<String>,
    },
    Crashed { code: Option<i32>, signal: Option<i32>, at: i64 },
}

impl ProfileStatus {
    /// «Запускается», «работает», «пауза» и «ждёт поток»: повторный запуск и удаление запрещены, прокси занят,
    /// лимит считает.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Starting | Self::Running(_) | Self::Paused(_) | Self::Waiting { .. })
    }
}

/// Фильтр списка по статусу (его применяет ядро приложения, а не база).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusFilter {
    Stopped,
    Starting,
    Running,
    Paused,
    Waiting,
    Crashed,
}

impl StatusFilter {
    pub fn matches(self, status: &ProfileStatus) -> bool {
        matches!(
            (self, status),
            (Self::Stopped, ProfileStatus::Stopped)
                | (Self::Starting, ProfileStatus::Starting)
                | (Self::Running, ProfileStatus::Running(_))
                | (Self::Paused, ProfileStatus::Paused(_))
                | (Self::Waiting, ProfileStatus::Waiting { .. })
                | (Self::Crashed, ProfileStatus::Crashed { .. })
        )
    }
}

/// События для интерфейса. Модуль не знает о Tauri: доставку делает получатель.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Status { profile_id: String, status: ProfileStatus },
    GatewayUnreachable { profile_id: String },
    /// Закреплённый прокси не подошёл (выключен, занят, недоступен): профиль запущен с другим, и тот закреплён.
    GatewayReplaced { profile_id: String, name: String },
    /// Local API изменил профили, группы или прокси: интерфейс перечитывает списки.
    ProfilesChanged,
    /// Пакет задачи окна начался, продвинулся или кончился; `error` — пакет не удалось начать.
    TasksChanged { task_id: i64, error: Option<String> },
    /// Ход отладки процесса из редактора (канал `flow-debug`).
    FlowDebug(DebugEvent),
    /// Сессия синхронизатора изменилась (канал `sync`).
    SyncChanged(crate::sync::session::SyncState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DebugKind {
    /// Узел начал выполняться (`node`).
    Node,
    /// Строка журнала (`entry`).
    Entry,
    /// Запуск закончился (`status`, `error`).
    End,
}

/// В JSON поле вида называется `phase`: `kind` занят меткой самого события.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DebugEvent {
    pub run_id: String,
    #[serde(rename = "phase")]
    pub kind: DebugKind,
    pub node: Option<String>,
    pub entry: Option<Entry>,
    pub status: Option<RunStatus>,
    pub error: Option<String>,
}

pub type EventSink = Arc<dyn Fn(Event) + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn info() -> RunningInfo {
        RunningInfo {
            pid: 1,
            cdp_ws: "ws://127.0.0.1:2/devtools/browser/x".into(),
            debug_port: 2,
            relay_port: 3,
            proxy_id: 7,
            exit_ip: None,
            country: None,
            timezone: "UTC".into(),
            started_at: 4,
        }
    }

    #[test]
    fn statuses_and_events_serialize_for_the_interface() {
        assert_eq!(serde_json::to_value(ProfileStatus::Stopped).unwrap(), json!({"state": "stopped"}));
        let crashed = ProfileStatus::Crashed { code: Some(3), signal: None, at: 5 };
        assert_eq!(
            serde_json::to_value(&crashed).unwrap(),
            json!({"state": "crashed", "code": 3, "signal": null, "at": 5})
        );
        let running = ProfileStatus::Running(info());
        let value = serde_json::to_value(&running).unwrap();
        assert_eq!((value["state"].as_str(), value["relay_port"].as_u64()), (Some("running"), Some(3)));
        assert_eq!(value["proxy_id"].as_i64(), Some(7));
        assert!(value.get("gateway_unreachable").is_none());
        let paused = ProfileStatus::Paused(info());
        let value = serde_json::to_value(&paused).unwrap();
        assert_eq!((value["state"].as_str(), value["pid"].as_u64()), (Some("paused"), Some(1)));
        let event = Event::GatewayUnreachable { profile_id: "k7f2q9".into() };
        assert_eq!(serde_json::to_value(&event).unwrap(), json!({"kind": "gateway_unreachable", "profile_id": "k7f2q9"}));
        let replaced = Event::GatewayReplaced { profile_id: "k7f2q9".into(), name: "Магазин".into() };
        assert_eq!(
            serde_json::to_value(&replaced).unwrap(),
            json!({"kind": "gateway_replaced", "profile_id": "k7f2q9", "name": "Магазин"})
        );
        assert_eq!(serde_json::to_value(Event::ProfilesChanged).unwrap(), json!({"kind": "profiles_changed"}));
        assert!(StatusFilter::Running.matches(&running) && !StatusFilter::Stopped.matches(&running));
        let filter: StatusFilter = serde_json::from_str("\"crashed\"").unwrap();
        assert!(filter.matches(&crashed) && !ProfileStatus::Stopped.is_active() && running.is_active());
    }

    #[test]
    fn paused_is_active_and_has_its_own_filter() {
        let paused = ProfileStatus::Paused(info());
        assert!(paused.is_active(), "пауза — активный статус: браузер открыт");
        let filter: StatusFilter = serde_json::from_str("\"paused\"").unwrap();
        assert!(filter.matches(&paused));
        assert!(!StatusFilter::Running.matches(&paused) && !StatusFilter::Paused.matches(&ProfileStatus::Running(info())));
    }

    #[test]
    fn waiting_serializes_with_license_and_info() {
        let launching = ProfileStatus::Waiting { license: "no_slot".into(), info: None, owner_hint: None };
        assert_eq!(serde_json::to_value(&launching).unwrap(), json!({"state": "waiting", "license": "no_slot", "info": null}));
        let blocked = ProfileStatus::Waiting { license: "offline".into(), info: Some(info()), owner_hint: None };
        let value = serde_json::to_value(&blocked).unwrap();
        assert_eq!((value["license"].as_str(), value["info"]["pid"].as_u64()), (Some("offline"), Some(1)));
        assert!(launching.is_active() && blocked.is_active(), "ждущий профиль занимает лимит и не запускается повторно");
        let filter: StatusFilter = serde_json::from_str("\"waiting\"").unwrap();
        assert!(filter.matches(&launching) && filter.matches(&blocked));
        assert!(!StatusFilter::Starting.matches(&launching) && !StatusFilter::Running.matches(&blocked));
    }
}
