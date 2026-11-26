//! Трафик профиля: сводка байтов и хвост журнала прослойки (спецификация 3А, «Трафик»). Журнал только читается.
use super::{CommandError, Core};
use crate::engine::Engine;
use crate::relay::log::{self, ConnRecord};
use serde::Serialize;

const MAX_LIMIT: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TrafficSummary {
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub connections: u64,
    /// `ts_ms` самой ранней учтённой записи; записей нет, но профиль работает — запуск прослойки; иначе `None`.
    pub since: Option<u64>,
    /// Профиль работает: в суммах есть живая статистика прослойки.
    pub live: bool,
}

fn io_error(e: std::io::Error) -> CommandError {
    CommandError::new("Io", e.to_string())
}

impl<E: Engine> Core<E> {
    fn live_profile(&self, id: &str) -> Result<(), CommandError> {
        // Профиль в корзине для трафика не существует.
        self.store.get_profile(id)?.map(|_| ()).ok_or_else(|| CommandError::new("NotFound", "профиль не найден"))
    }

    /// Сумма журнала (текущий файл и ротация `.jsonl.1`) плюс живая статистика прослойки у работающего профиля.
    /// Счётчики прослойки растут с её запуска и уже включают завершённые соединения этого запуска, а журнал пишется
    /// при завершении соединения. Поэтому из журнала берутся только записи, завершённые ДО запуска прослойки
    /// (`ts_ms < started_ms`); открытое сейчас соединение учтено живыми счётчиками и попадёт в журнал позже.
    pub fn traffic_summary(&self, id: &str) -> Result<TrafficSummary, CommandError> {
        self.live_profile(id)?;
        let live = self.supervisor.relay_stats(id);
        let before = live.map(|(_, started_ms)| started_ms);
        let path = self.paths.relay_log(id);
        let (mut up, mut down, mut n, mut since) = (0u64, 0u64, 0u64, None::<u128>);
        for file in [path.with_extension("jsonl.1"), path] {
            let (u, d, c, oldest) = log::totals(&file, before).map_err(io_error)?;
            (up, down, n) = (up + u, down + d, n + c);
            since = [since, oldest].into_iter().flatten().min();
        }
        let since = since.or(before).map(|ms| ms as u64);
        if let Some((s, _)) = live {
            (up, down, n) = (up + s.bytes_up, down + s.bytes_down, n + s.connections);
        }
        Ok(TrafficSummary { bytes_up: up, bytes_down: down, connections: n, since, live: live.is_some() })
    }

    /// Последние `limit` (1..=500) записей текущего файла журнала в порядке файла; ротация не читается.
    pub fn connection_log(&self, id: &str, limit: usize) -> Result<Vec<ConnRecord>, CommandError> {
        self.live_profile(id)?;
        log::tail(&self.paths.relay_log(id), limit.clamp(1, MAX_LIMIT)).map_err(io_error)
    }
}

#[cfg(test)]
mod tests {
    use crate::app::tests::{create_with_proxy, env, form};
    use crate::relay::log::{ConnLog, Record};

    fn write(core: &crate::app::tests::Env, id: &str, ts_ms: u128, up: u64) {
        let log = ConnLog::open(core.core.paths.relay_log(id)).unwrap();
        log.write(&Record { ts_ms, proxy_id: 1, target: "example.com:443", bytes_up: up, bytes_down: up * 2, ms: 1, result: "ok" });
    }

    #[tokio::test]
    async fn no_log_gives_zeros_and_log_gives_sums() {
        let e = env().await;
        let p = e.core.create_profile(&form("a", None), 1).unwrap();
        let zero = e.core.traffic_summary(&p.id).unwrap();
        assert_eq!((zero.bytes_up, zero.bytes_down, zero.connections, zero.live), (0, 0, 0, false));
        assert!(e.core.connection_log(&p.id, 10).unwrap().is_empty());
        write(&e, &p.id, 1, 10);
        write(&e, &p.id, 2, 5);
        let s = e.core.traffic_summary(&p.id).unwrap();
        assert_eq!((s.bytes_up, s.bytes_down, s.connections, s.live), (15, 30, 2, false));
        assert_eq!(e.core.connection_log(&p.id, 0).unwrap().len(), 1, "limit 0 поднимается до 1");
        assert_eq!(e.core.connection_log(&p.id, 99).unwrap().len(), 2);
    }

    #[tokio::test]
    async fn rotated_file_is_summed_and_since_is_oldest_record() {
        let e = env().await;
        let p = e.core.create_profile(&form("a", None), 1).unwrap();
        assert_eq!(e.core.traffic_summary(&p.id).unwrap().since, None, "нет записей и профиль не работает");
        write(&e, &p.id, 5, 10);
        let current = e.core.paths.relay_log(&p.id);
        std::fs::rename(&current, current.with_extension("jsonl.1")).unwrap();
        write(&e, &p.id, 9, 1);
        let s = e.core.traffic_summary(&p.id).unwrap();
        assert_eq!((s.bytes_up, s.bytes_down, s.connections, s.since), (11, 22, 2, Some(5)), "`.jsonl.1` суммируется");
    }

    #[tokio::test]
    async fn since_of_running_profile_follows_started_ms_when_log_is_empty() {
        let e = env().await;
        let p = create_with_proxy(&e.core, "a", e.proxy_port, 1);
        write(&e, &p.id, 3, 10);
        e.core.supervisor.start(&p.id).await.unwrap();
        let (_, started) = e.core.supervisor.relay_stats(&p.id).unwrap();
        assert_eq!(e.core.traffic_summary(&p.id).unwrap().since, Some(3), "самая старая запись до запуска");
        std::fs::remove_file(e.core.paths.relay_log(&p.id)).unwrap();
        assert_eq!(e.core.traffic_summary(&p.id).unwrap().since, Some(started as u64), "записей нет — старт прослойки");
    }

    #[tokio::test]
    async fn running_profile_is_live_and_log_before_start_counts() {
        let e = env().await;
        let p = create_with_proxy(&e.core, "a", e.proxy_port, 1);
        write(&e, &p.id, 1, 10); // до запуска
        e.core.supervisor.start(&p.id).await.unwrap();
        write(&e, &p.id, u128::MAX, 7); // «завершилось после запуска»: уже в живых счётчиках, не считается дважды
        let s = e.core.traffic_summary(&p.id).unwrap();
        // Проба геолокации идёт через прослойку, так что живая статистика не нулевая.
        let (live, _) = e.core.supervisor.relay_stats(&p.id).unwrap();
        assert!(s.live && live.connections >= 1);
        assert_eq!(s.bytes_up, 10 + live.bytes_up);
        assert_eq!(s.bytes_down, 20 + live.bytes_down);
        assert_eq!(s.connections, 1 + live.connections);
    }

    #[tokio::test]
    async fn trashed_or_missing_profile_is_not_found() {
        let e = env().await;
        assert_eq!(e.core.traffic_summary("nope00").unwrap_err().code, "NotFound");
        assert_eq!(e.core.connection_log("nope00", 5).unwrap_err().code, "NotFound");
    }
}
