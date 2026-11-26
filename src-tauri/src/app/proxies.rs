//! Список прокси на уровне приложения: правка, импорт строками, проверка (спецификация 2А, «Список прокси»).

use super::forms::{SecretInput, proxy_input, validate_proxy_form};
use super::proxy_import::parse_proxy_line;
use super::{CommandError, Core};
use crate::sys::unix_now;
use crate::engine::Engine;
use crate::relay::geo::{GeoError, GeoInfo};
use crate::store::StoreError;
use crate::store::proxies::{ProxyInput, ProxyKind, ProxyRecord};
use crate::store::secrets::SecretField;
use crate::supervisor::gateway;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;

/// Запись прокси в форме экрана «Прокси». `password` — как в редакторе: оставить, удалить или задать.
#[derive(Clone, Deserialize)]
pub struct ProxyEditForm {
    pub kind: ProxyKind,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: SecretInput,
    pub note: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ConnectionCheck {
    Direct,
    Saved { id: i64 },
    Manual { proxy: ProxyEditForm, source_id: Option<i64> },
}

impl ProxyEditForm {
    fn input(&self) -> ProxyInput {
        proxy_input(self)
    }
}

// Логин прокси в `Debug` не попадает.
impl fmt::Debug for ProxyEditForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyEditForm")
            .field("kind", &self.kind)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &if self.username.is_empty() { "" } else { "***" })
            .field("password", &self.password)
            .field("note", &self.note)
            .field("enabled", &self.enabled)
            .finish()
    }
}

/// Строка экрана «Прокси»: запись, сколько профилей за ней закреплено и кто её сейчас занимает.
#[derive(Debug, Clone, Serialize)]
pub struct ProxyRow {
    #[serde(flatten)]
    pub proxy: ProxyRecord,
    pub profiles: i64,
    /// Имя профиля, если прокси закреплён ровно за одним, — колонка «Закреплён за».
    pub pinned_to: Option<String>,
    /// Id запущенного профиля, который сейчас занимает прокси.
    pub used_by: Option<String>,
    /// Его имя — «работает: <имя>» на экране «Прокси», «занят: <имя>» в редакторе.
    pub used_by_name: Option<String>,
}

/// Строка импорта, которую не удалось разобрать: номер с 1 и код (`format`, `kind`, `port`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LineError {
    pub line: usize,
    pub code: &'static str,
}

/// Итог импорта. Пароли сюда не попадают.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ImportReport {
    pub added: usize,
    pub duplicates: usize,
    pub errors: Vec<LineError>,
    /// Id добавленных записей в порядке строк — для «Проверить после импорта».
    pub ids: Vec<i64>,
}

/// Ошибка проверки для интерфейса: прокси обозначается типом, хостом и портом, без логина и пароля.
pub(super) fn geo_error(error: GeoError, label: &str) -> CommandError {
    match error {
        GeoError::ProxyUnavailable => CommandError::new("ProxyUnavailable", format!("прокси {label} недоступен")),
        _ => CommandError::new("GeoUnavailable", "сервисы геоданных недоступны"),
    }
}

fn proxy_not_found() -> CommandError {
    CommandError::new("NotFound", "прокси не найден")
}

impl<E: Engine> Core<E> {
    /// Закрепления — одним запросом на весь список, занятость — из супервизора, имена занявших — одним запросом.
    pub fn list_proxies(&self) -> Result<Vec<ProxyRow>, CommandError> {
        let users = self.supervisor.proxy_users();
        let running: Vec<String> = users.values().cloned().collect();
        let names = self.store.profile_names(&running)?;
        let mut pins = self.store.proxy_pins()?;
        Ok(self
            .store
            .list_proxies()?
            .into_iter()
            .map(|proxy| {
                let pin = pins.remove(&proxy.id);
                let used_by = users.get(&proxy.id).cloned();
                let used_by_name = used_by.as_ref().and_then(|id| names.get(id).cloned());
                ProxyRow {
                    profiles: pin.as_ref().map_or(0, |p| p.profiles),
                    pinned_to: pin.and_then(|p| p.only),
                    used_by,
                    used_by_name,
                    proxy,
                }
            })
            .collect())
    }

    pub fn create_proxy(&self, form: &ProxyEditForm) -> Result<ProxyRecord, CommandError> {
        let errors = validate_proxy_form(form, false);
        if !errors.is_empty() {
            return Err(CommandError::validation(errors));
        }
        Ok(self.store.create_proxy(&form.input())?)
    }

    pub fn update_proxy(&self, id: i64, form: &ProxyEditForm) -> Result<ProxyRecord, CommandError> {
        let saved = self.store.get_proxy(id)?.ok_or_else(proxy_not_found)?;
        let errors = validate_proxy_form(form, saved.has_password);
        if !errors.is_empty() {
            return Err(CommandError::validation(errors));
        }
        Ok(self.store.update_proxy(id, &form.input())?)
    }

    pub fn set_proxy_enabled(&self, id: i64, enabled: bool) -> Result<(), CommandError> {
        Ok(self.store.set_proxy_enabled(id, enabled)?)
    }

    /// Занятый активным профилем → `ProxyInUse`; у остановленных профилей закрепление снимается (ON DELETE SET NULL).
    pub fn delete_proxy(&self, id: i64) -> Result<(), CommandError> {
        self.delete_proxies(std::slice::from_ref(&id))
    }

    /// Пачка (Local API): занятость всех и удаление — под мьютексом, под которым запуск выбирает шлюз, и одной
    /// транзакцией. Удаляется всё или ничего: занят хоть один — `ProxyInUse`, неизвестный — `NotFound`.
    pub fn delete_proxies(&self, ids: &[i64]) -> Result<(), CommandError> {
        let mut ids = ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        self.supervisor.while_proxies_idle(&ids, || self.store.delete_proxies(&ids))??;
        Ok(())
    }

    /// Строки по одной на прокси; дубликаты (тип, хост, порт, логин) — и в базе, и внутри текста — пропускаются.
    pub fn import_proxies(&self, text: &str, default_kind: ProxyKind) -> Result<ImportReport, CommandError> {
        let mut report = ImportReport::default();
        let mut seen = HashSet::new();
        for (index, line) in text.lines().enumerate() {
            let number = index + 1;
            let parsed = match parse_proxy_line(line, default_kind) {
                Ok(Some(parsed)) => parsed,
                Ok(None) => continue,
                Err(code) => {
                    report.errors.push(LineError { line: number, code });
                    continue;
                }
            };
            // `ProxyKind` не реализует `Hash`, поэтому в ключе — его строка.
            let key = (parsed.kind.as_str(), parsed.host.to_ascii_lowercase(), parsed.port, parsed.username.clone().unwrap_or_default());
            let known = self.store.find_proxy(parsed.kind, &parsed.host, parsed.port, parsed.username.as_deref())?;
            if !seen.insert(key) || known.is_some() {
                report.duplicates += 1;
                continue;
            }
            let input = ProxyInput {
                kind: parsed.kind,
                host: parsed.host,
                port: parsed.port,
                username: parsed.username,
                password: parsed.password.map(SecretField::Set).unwrap_or(SecretField::Clear),
                note: String::new(),
                enabled: true,
            };
            match self.store.create_proxy(&input) {
                Ok(record) => {
                    report.added += 1;
                    report.ids.push(record.id);
                }
                Err(StoreError::Invalid(_)) => report.errors.push(LineError { line: number, code: "format" }),
                Err(e) => return Err(e.into()),
            }
        }
        Ok(report)
    }

    /// «Проверить» на экране «Прокси»: временная прослойка без журнала; результат пишется в запись прокси.
    pub async fn check_proxy(&self, id: i64) -> Result<GeoInfo, CommandError> {
        self.check_proxy_with(id, crate::network::IpChecker::Auto).await
    }

    pub async fn check_proxy_with(&self, id: i64, checker: crate::network::IpChecker) -> Result<GeoInfo, CommandError> {
        let credentials = self.store.proxy_credentials(id)?;
        let label = format!("{} {}:{}", credentials.kind.as_str(), credentials.host, credentials.port);
        let result = gateway::check_with(&self.geo, gateway::upstream_config(&credentials), checker).await;
        match &result {
            Ok(geo) => self.store.record_proxy_check(id, unix_now(), Some((geo.ip.as_str(), geo.country.as_str())))?,
            Err(GeoError::ProxyUnavailable) => self.store.record_proxy_check(id, unix_now(), None)?,
            // Сервисы геоданных недоступны — о самом прокси это ничего не говорит.
            Err(_) => {}
        }
        result.map_err(|e| geo_error(e, &label))
    }

    pub async fn check_connection(&self, target: &ConnectionCheck, checker: crate::network::IpChecker) -> Result<GeoInfo, CommandError> {
        let upstream = match target {
            ConnectionCheck::Direct => crate::relay::UpstreamConfig::direct(),
            ConnectionCheck::Saved { id } => return self.check_proxy_with(*id, checker).await,
            ConnectionCheck::Manual { proxy, source_id } => {
                let old = source_id.map(|id| self.store.proxy_credentials(id)).transpose()?;
                let errors = validate_proxy_form(proxy, old.as_ref().is_some_and(|old| old.password.is_some()));
                if !errors.is_empty() { return Err(CommandError::validation(errors)); }
                let password = match &proxy.password { SecretInput::Set { value } => Some(crate::store::secrets::Secret::new(value)), SecretInput::Keep => old.and_then(|old| old.password), SecretInput::Clear => None };
                gateway::upstream_config(&crate::store::proxies::ProxyCredentials { id: 0, kind: proxy.kind, host: proxy.host.trim().into(), port: proxy.port, username: (!proxy.username.trim().is_empty()).then(|| proxy.username.trim().into()), password })
            }
        };
        gateway::check_with(&self.geo, upstream, checker).await.map_err(|error| geo_error(error, "соединение профиля"))
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{create_with_proxy, env, form};
    use crate::relay::test_support::closed_port;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn profile_network_survives_legacy_edits_and_direct_is_distinct_from_random() {
        let e = env().await;
        let mut direct = form("Direct", None);
        direct.network = Some(crate::network::ProfileNetwork { direct: true, ip_checker: crate::network::IpChecker::IpWhoIs });
        let record = e.core.create_profile(&direct, 1).unwrap();
        assert_eq!(record.proxy_id, None);
        assert!(record.network.direct);
        let changed = e.core.update_profile(&record.id, &form("Renamed", None)).unwrap();
        assert_eq!(changed.network, record.network, "старый клиент не стирает новые настройки");
        let clone = e.core.clone_profile(&record.id, 2).unwrap();
        assert_eq!(clone.network, record.network);
        let proxy = e.core.create_proxy(&edit_form(e.proxy_port, Some("pass"))).unwrap();
        let assigned = e.core.update_profile(&record.id, &form("Saved", Some(proxy.id))).unwrap();
        assert!(!assigned.network.direct);
        assert_eq!(assigned.network.ip_checker, crate::network::IpChecker::IpWhoIs);
    }

    #[tokio::test]
    async fn manual_proxy_is_atomic_with_profile_and_keeps_the_source_password_without_editing_it() {
        let e = env().await;
        let mut manual = form("Manual", None);
        manual.inline_proxy = Some(edit_form(e.proxy_port, Some("private-manual-password")));
        let record = e.core.create_profile(&manual, 1).unwrap();
        let source = record.proxy_id.unwrap();
        assert_eq!(e.core.store.proxy_credentials(source).unwrap().password.unwrap().expose(), "private-manual-password");
        manual.inline_proxy.as_mut().unwrap().password = SecretInput::Keep;
        manual.inline_proxy.as_mut().unwrap().host = "other.example".into();
        manual.inline_proxy_source = Some(source);
        let edited = e.core.update_profile(&record.id, &manual).unwrap();
        assert_ne!(edited.proxy_id, Some(source));
        assert_eq!(e.core.store.get_proxy(source).unwrap().unwrap().host, "127.0.0.1");
        assert_eq!(e.core.store.proxy_credentials(edited.proxy_id.unwrap()).unwrap().password.unwrap().expose(), "private-manual-password");
        let count = e.core.store.list_proxies().unwrap().len();
        let mut input = crate::store::profiles::ProfileInput::new("rollback", record.fingerprint_json);
        input.inline_proxy = Some(super::proxy_input(&edit_form(8080, None)));
        assert!(e.core.store.create_profiles(&[input], &["x".repeat(crate::store::tags::TAG_MAX + 1)], 3).is_err());
        assert_eq!(e.core.store.list_proxies().unwrap().len(), count, "ошибка тегов откатывает и вставку прокси");
        assert_eq!(e.core.store.count_profiles().unwrap(), 1);
    }

    /// Прокси на 127.0.0.1; с паролем — логин `user` (его ждёт подставной прокси окружения).
    fn edit_form(port: u16, password: Option<&str>) -> ProxyEditForm {
        ProxyEditForm {
            kind: ProxyKind::Http,
            host: "127.0.0.1".into(),
            port,
            username: if password.is_some() { "user".into() } else { String::new() },
            password: match password {
                Some(value) => SecretInput::Set { value: value.into() },
                None => SecretInput::Keep,
            },
            note: " основной ".into(),
            enabled: true,
        }
    }

    #[tokio::test]
    async fn proxies_are_created_edited_toggled_and_listed() {
        let e = env().await;
        let created = e.core.create_proxy(&edit_form(e.proxy_port, Some("pass"))).unwrap();
        assert!(created.has_password);
        assert_eq!(created.note, "основной");
        let mut change = edit_form(e.proxy_port, None);
        change.username = "user".into();
        change.note = "запасной".into();
        let updated = e.core.update_proxy(created.id, &change).unwrap();
        assert_eq!(updated.note, "запасной");
        assert_eq!(
            e.core.store.proxy_credentials(created.id).unwrap().password.unwrap().expose(),
            "pass",
            "Keep оставляет сохранённый пароль"
        );
        let mut login_only = edit_form(e.proxy_port, None);
        login_only.username = "user".into();
        let err = e.core.create_proxy(&login_only).unwrap_err();
        assert_eq!(err.code, "Validation");
        assert_eq!(err.fields.unwrap()[0].field, "proxy_password");
        e.core.set_proxy_enabled(created.id, false).unwrap();
        let rows = e.core.list_proxies().unwrap();
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].proxy.enabled);
        assert_eq!((rows[0].profiles, rows[0].used_by.as_deref()), (0, None));
        assert_eq!(e.core.update_proxy(999, &change).unwrap_err().code, "NotFound");
        assert_eq!(e.core.set_proxy_enabled(999, true).unwrap_err().code, "NotFound");
    }

    #[test]
    fn edit_form_debug_hides_login_and_password() {
        let mut form = edit_form(8080, Some("hunter2"));
        form.username = "alice-login".into();
        let text = format!("{form:?}");
        assert!(!text.contains("alice-login") && !text.contains("hunter2"), "{text}");
        assert!(text.contains("127.0.0.1"), "{text}");
    }

    #[tokio::test]
    async fn busy_proxy_is_not_deleted_and_idle_one_unpins_profiles() {
        let e = env().await;
        let rec = create_with_proxy(&e.core, "a", e.proxy_port, 1);
        let proxy_id = rec.proxy_id.unwrap();
        e.core.supervisor.start(&rec.id).await.unwrap();
        let rows = e.core.list_proxies().unwrap();
        assert_eq!((rows[0].profiles, rows[0].used_by.as_deref()), (1, Some(rec.id.as_str())));
        assert_eq!(e.core.delete_proxy(proxy_id).unwrap_err().code, "ProxyInUse");
        assert!(e.core.store.get_proxy(proxy_id).unwrap().is_some());
        e.core.supervisor.stop(&rec.id).await.unwrap();
        e.core.delete_proxy(proxy_id).unwrap();
        assert_eq!(e.core.store.get_profile(&rec.id).unwrap().unwrap().proxy_id, None, "закрепление снято");
        assert_eq!(e.core.delete_proxy(proxy_id).unwrap_err().code, "NotFound");
    }

    #[tokio::test]
    async fn batch_delete_refuses_a_proxy_taken_by_a_starting_profile() {
        let e = env().await;
        let rec = create_with_proxy(&e.core, "a", e.proxy_port, 1);
        let taken = rec.proxy_id.unwrap();
        let free = e.core.create_proxy(&edit_form(e.proxy_port, Some("pass"))).unwrap().id;
        let hold = Arc::new(tokio::sync::Notify::new());
        *e.core.engine.hold_next_launch.lock().unwrap() = Some(hold.clone());
        let (started, ()) = tokio::join!(e.core.supervisor.start(&rec.id), async {
            // Запуск уже выбрал шлюз и ждёт браузер.
            while e.core.engine.launches() == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(e.core.delete_proxies(&[free, taken]).unwrap_err().code, "ProxyInUse");
            assert!(e.core.store.get_proxy(free).unwrap().is_some(), "свободный тоже не удалён");
            hold.notify_one();
        });
        started.unwrap();
        e.core.supervisor.stop(&rec.id).await.unwrap();
        e.core.delete_proxies(&[taken, free, taken]).unwrap();
        assert!(e.core.store.list_proxies().unwrap().is_empty());
        assert_eq!(e.core.delete_proxies(&[free]).unwrap_err().code, "NotFound");
    }

    /// Удаление пачки прокси и запуск, выбирающий один из них: либо удалены все и запуск не нашёл прокси,
    /// либо не удалён ни один и профиль работает.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn batch_delete_racing_a_start_is_all_or_nothing() {
        let e = env().await;
        for round in 0..20 {
            let rec = create_with_proxy(&e.core, &format!("p{round}"), e.proxy_port, 1);
            let spare = e.core.create_proxy(&edit_form(e.proxy_port, Some("pass"))).unwrap().id;
            let ids = [rec.proxy_id.unwrap(), spare];
            let (deleted, started) = tokio::join!(
                async {
                    for _ in 0..round {
                        tokio::task::yield_now().await;
                    }
                    e.core.delete_proxies(&ids)
                },
                e.core.supervisor.start(&rec.id)
            );
            let left = ids.iter().filter(|id| e.core.store.get_proxy(**id).unwrap().is_some()).count();
            match deleted {
                Ok(()) => {
                    assert_eq!(left, 0, "раунд {round}: удалены все");
                    let err = started.expect_err("прокси удалены до выбора шлюза");
                    assert_eq!(err.code(), "NoFreeProxy", "раунд {round}");
                }
                Err(err) => {
                    assert_eq!((err.code.as_str(), left), ("ProxyInUse", 2), "раунд {round}: не удалён ни один");
                    started.unwrap_or_else(|error| panic!("раунд {round}: запуск, занявший прокси, доходит до конца: {error}"));
                    e.core.supervisor.stop(&rec.id).await.unwrap();
                    // Следующий раунд не должен найти чужие свободные прокси.
                    e.core.delete_proxies(&ids).unwrap();
                }
            }
        }
    }

    const LINES: &str = "http://user:p@ss:w0rd@gw1.example.net:8080
socks5://gw2.example.net:1080
gw3.example.net:3128:bob:se:cret
gw4.example.net:3128

# комментарий
ftp://gw5.example.net:21
gw6.example.net:0
gw7.example.net
GW1.example.net:8080:user:other
https://[2001:db8::1]:8443
";

    #[tokio::test]
    async fn import_reports_added_duplicates_and_errors() {
        let e = env().await;
        let report = e.core.import_proxies(LINES, ProxyKind::Http).unwrap();
        assert_eq!(
            (report.added, report.duplicates, report.errors.clone()),
            (
                5,
                1,
                vec![
                    LineError { line: 7, code: "kind" },
                    LineError { line: 8, code: "port" },
                    LineError { line: 9, code: "format" },
                ]
            )
        );
        let again = e.core.import_proxies(LINES, ProxyKind::Http).unwrap();
        assert_eq!((again.added, again.duplicates, again.errors.len()), (0, 6, 3), "повторный импорт ничего не добавляет");
        assert!(again.ids.is_empty());

        let proxies = e.core.store.list_proxies().unwrap();
        assert_eq!(proxies.len(), 5);
        assert_eq!(report.ids, proxies.iter().map(|p| p.id).collect::<Vec<_>>(), "id добавленных — в порядке строк");
        let find = |host: &str| proxies.iter().find(|p| p.host == host).unwrap().clone();
        assert_eq!((report.ids[0], report.ids[4]), (find("gw1.example.net").id, find("2001:db8::1").id));
        let password = |id: i64| e.core.store.proxy_credentials(id).unwrap().password.map(|p| p.expose().to_string());
        assert_eq!(password(find("gw1.example.net").id).as_deref(), Some("p@ss:w0rd"));
        let gw3 = find("gw3.example.net");
        assert_eq!((gw3.kind, gw3.username.as_deref()), (ProxyKind::Http, Some("bob")));
        assert_eq!(password(gw3.id).as_deref(), Some("se:cret"));
        let gw2 = find("gw2.example.net");
        assert_eq!((gw2.kind, gw2.port, gw2.has_password), (ProxyKind::Socks5, 1080, false));
        let v6 = find("2001:db8::1");
        assert_eq!((v6.kind, v6.port, v6.enabled), (ProxyKind::Https, 8443, true));

        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("p@ss") && !json.contains("se:cret") && !json.contains("bob"), "{json}");
    }

    #[tokio::test]
    async fn check_records_result_for_working_and_dead_proxy() {
        let e = env().await;
        let alive = e.core.create_proxy(&edit_form(e.proxy_port, Some("pass"))).unwrap();
        let dead = e.core.create_proxy(&edit_form(closed_port().await, None)).unwrap();
        assert_eq!(e.core.check_proxy(alive.id).await.unwrap().country, "DE");
        let check = e.core.store.get_proxy(alive.id).unwrap().unwrap().check.unwrap();
        assert!(check.ok);
        assert_eq!((check.exit_ip.as_deref(), check.country.as_deref()), (Some("203.0.113.24"), Some("DE")));
        let err = e.core.check_proxy(dead.id).await.unwrap_err();
        assert_eq!(err.code, "ProxyUnavailable");
        assert!(err.message.contains("127.0.0.1"), "{}", err.message);
        assert!(!e.core.store.get_proxy(dead.id).unwrap().unwrap().check.unwrap().ok);
        assert_eq!(e.core.check_proxy(999).await.unwrap_err().code, "NotFound");
    }

    #[tokio::test]
    async fn rows_name_the_only_pinned_profile_and_the_running_one() {
        let e = env().await;
        let free = e.core.create_proxy(&edit_form(e.proxy_port, Some("pass"))).unwrap();
        let single = e.core.create_proxy(&edit_form(e.proxy_port, Some("pass"))).unwrap();
        let shared = e.core.create_proxy(&edit_form(e.proxy_port, Some("pass"))).unwrap();
        let alpha = e.core.create_profile(&form("Альфа", Some(single.id)), 1).unwrap();
        e.core.create_profile(&form("Бета", Some(shared.id)), 2).unwrap();
        e.core.create_profile(&form("Гамма", Some(shared.id)), 3).unwrap();
        e.core.supervisor.start(&alpha.id).await.unwrap();

        let rows = e.core.list_proxies().unwrap();
        let row = |id: i64| rows.iter().find(|r| r.proxy.id == id).unwrap();
        assert_eq!((row(free.id).profiles, row(free.id).pinned_to.as_deref()), (0, None), "свободен");
        assert_eq!((row(single.id).profiles, row(single.id).pinned_to.as_deref()), (1, Some("Альфа")));
        assert_eq!((row(shared.id).profiles, row(shared.id).pinned_to.as_deref()), (2, None), "несколько — без имени");
        assert_eq!(
            (row(single.id).used_by.as_deref(), row(single.id).used_by_name.as_deref()),
            (Some(alpha.id.as_str()), Some("Альфа"))
        );
        assert_eq!((row(shared.id).used_by.as_deref(), row(shared.id).used_by_name.as_deref()), (None, None));
        let json = serde_json::to_value(row(single.id)).unwrap();
        assert_eq!((json["pinned_to"].as_str(), json["used_by_name"].as_str()), (Some("Альфа"), Some("Альфа")));
        e.core.supervisor.stop(&alpha.id).await.unwrap();
    }
}
