//! Клон, массовое создание и массовые операции над выбранными профилями (спецификация 2А, «Группы, теги, клоны, массовые операции»).

use super::forms::{NAME_MAX, clean_lines, launch_line_errors, tag_errors};
use super::profiles::not_found;
use super::{CommandError, Core};
use crate::engine::Engine;
use crate::fingerprint::{Arch, Catalog, FieldError, OsRandom, generate, regenerate};
use crate::store::profiles::{GroupFilter, ListQuery, ProfileEdit, ProfileInput, ProfileRecord};
use futures_util::future::join_all;
use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};

/// Массовый запуск: столько профилей запускается одновременно.
pub const BULK_START_PARALLEL: usize = 4;
pub const BULK_CREATE_MAX: u32 = 1000;
const COPY_SUFFIX: &str = " (копия)";

/// Профиль, который операция не обработала, и почему.
#[derive(Debug, Clone, Serialize)]
pub struct BulkFailure {
    pub id: String,
    pub error: CommandError,
}

/// Отчёт массовой операции: успешные и ошибочные профили (порядок — как в запросе).
#[derive(Debug, Clone, Default, Serialize)]
pub struct BulkReport {
    pub ok: Vec<String>,
    pub failed: Vec<BulkFailure>,
    /// Массовый запуск: профили, которые ждут поток лицензии (их запуск идёт дальше сам).
    pub waiting: Vec<String>,
}

impl BulkReport {
    pub(super) fn push(&mut self, id: String, result: Result<(), CommandError>) {
        match result {
            Ok(()) => self.ok.push(id),
            Err(error) => self.failed.push(BulkFailure { id, error }),
        }
    }
}

/// Шлюз новых профилей: случайный при первом запуске или один прокси для всех.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ProxyChoice {
    Random,
    Fixed { proxy_id: i64 },
}

#[derive(Debug, Clone, Deserialize)]
pub struct BulkCreateForm {
    pub count: u32,
    pub prefix: String,
    pub group_id: Option<i64>,
    pub tags: Vec<String>,
    pub note: String,
    pub start_urls: Vec<String>,
    pub restore_tabs: bool,
    pub extra_args: Vec<String>,
    pub proxy: ProxyChoice,
}

/// Шлюз в массовой правке: сбросить или назначить.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ProxyEdit {
    Reset,
    Set { proxy_id: i64 },
}

/// Массовая правка: незаданные поля не меняются.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BulkEditForm {
    pub add_tags: Vec<String>,
    pub remove_tags: Vec<String>,
    pub note: Option<String>,
    pub proxy: Option<ProxyEdit>,
    pub start_urls: Option<Vec<String>>,
    pub restore_tabs: Option<bool>,
    pub extra_args: Option<Vec<String>>,
}

/// Ширина номера: не меньше трёх знаков и не меньше, чем знаков в количестве.
fn number_width(count: u32) -> usize {
    count.to_string().len().max(3)
}

/// Имена «префикс 001» … «префикс N».
pub fn bulk_names(prefix: &str, count: u32) -> Vec<String> {
    let width = number_width(count);
    (1..=count).map(|n| format!("{prefix} {n:0width$}")).collect()
}

/// «<имя> (копия)»; имя обрезается так, чтобы итог уложился в 100 символов.
pub fn clone_name(name: &str) -> String {
    let room = NAME_MAX - COPY_SUFFIX.chars().count();
    let base: String = name.chars().take(room).collect();
    format!("{}{COPY_SUFFIX}", base.trim_end())
}

impl<E: Engine> Core<E> {
    /// Клон: настройки и ручные поля отпечатка копируются; остальной отпечаток и seed — новые; настройки очеловечивания
    /// копируются, персона новая (ТЗ 7.2 п. 4); настройки модуля капч, закладки и набор расширений копируются; шлюз,
    /// логин, пароль, 2FA, cookies и папка браузера — нет.
    pub fn clone_profile(&self, id: &str, now: i64) -> Result<ProfileRecord, CommandError> {
        let source = self.store.get_profile(id)?.ok_or_else(not_found)?;
        let fingerprint = source.fingerprint()?;
        let fingerprint = regenerate(&fingerprint, Catalog::builtin(), Arch::host(), &mut OsRandom)?;
        let tags = self.store.profile_tags(id)?;
        let mut input = ProfileInput::new(clone_name(&source.name), fingerprint.to_json());
        input.note = source.note;
        input.platform_domain = source.platform_domain;
        input.start_urls = source.start_urls;
        input.extra_args = source.extra_args;
        input.group_id = source.group_id;
        input.ext_category_id = source.ext_category_id;
        input.restore_tabs = source.restore_tabs;
        input.humanize = Some(source.humanize);
        input.network = Some(source.network);
        input.bookmarks = Some(self.store.profile_bookmarks(id)?);
        let mut created = self.store.create_profiles(&[input], &tags, now)?;
        Ok(created.remove(0))
    }

    /// Клон каждого профиля списка («Клонировать» у выделения): `ok` — исходные профили, у которых появилась копия.
    pub fn clone_profiles(&self, ids: &[String], now: i64) -> BulkReport {
        let mut report = BulkReport::default();
        for id in ids {
            let result = self.clone_profile(id, now).map(|_| ());
            report.push(id.clone(), result);
        }
        report
    }

    /// Профили по шаблону одной транзакцией; у каждого свой отпечаток. Всё проверяется до записи.
    pub fn create_profiles_bulk(&self, form: &BulkCreateForm, now: i64) -> Result<Vec<ProfileRecord>, CommandError> {
        let prefix = form.prefix.trim();
        let start_urls = clean_lines(&form.start_urls);
        let extra_args = clean_lines(&form.extra_args);
        let tags = clean_lines(&form.tags);
        let mut errors = Vec::new();
        if !(1..=BULK_CREATE_MAX).contains(&form.count) {
            errors.push(FieldError { field: "count", code: "out_of_range" });
        }
        if prefix.is_empty() {
            errors.push(FieldError { field: "prefix", code: "required" });
        } else if prefix.chars().count() + 1 + number_width(form.count) > NAME_MAX {
            errors.push(FieldError { field: "prefix", code: "too_long" });
        }
        errors.extend(launch_line_errors(&start_urls, &extra_args));
        errors.extend(tag_errors(&tags));
        if !errors.is_empty() {
            return Err(CommandError::validation(errors));
        }
        let proxy_id = match form.proxy {
            ProxyChoice::Random => None,
            ProxyChoice::Fixed { proxy_id } => Some(proxy_id),
        };
        let mut inputs = Vec::with_capacity(form.count as usize);
        for name in bulk_names(prefix, form.count) {
            let fingerprint = generate(Catalog::builtin(), Arch::host(), &mut OsRandom)?;
            let mut input = ProfileInput::new(name, fingerprint.to_json());
            input.note = form.note.trim().to_string();
            input.start_urls = start_urls.clone();
            input.extra_args = extra_args.clone();
            input.group_id = form.group_id;
            input.restore_tabs = form.restore_tabs;
            input.proxy_id = proxy_id;
            inputs.push(input);
        }
        Ok(self.store.create_profiles(&inputs, &tags, now)?)
    }

    /// Не больше `BULK_START_PARALLEL` запусков одновременно; лимит запусков и прочие ошибки — в `failed`.
    /// Профиль, который ждёт поток лицензии, — исход (`waiting`): его запуск идёт дальше сам, а очередь — к следующему.
    pub async fn start_profiles(&self, ids: &[String]) -> BulkReport {
        let results: Vec<(String, Result<bool, CommandError>)> = stream::iter(ids.iter().cloned())
            .map(|id| async move {
                let result = tokio::select! {
                    biased;
                    started = self.supervisor.start(&id) => started.map(|_| false).map_err(CommandError::from),
                    () = self.supervisor.until_waiting(&id) => Ok(true),
                };
                (id, result)
            })
            .buffered(BULK_START_PARALLEL)
            .collect()
            .await;
        let mut report = BulkReport::default();
        for (id, result) in results {
            match result {
                Ok(true) => report.waiting.push(id),
                other => report.push(id, other.map(|_| ())),
            }
        }
        report
    }

    /// «Запустить все» у группы (`None` — «Без группы»): все её профили, кроме уже активных, как `start_profiles`.
    pub async fn start_group(&self, group_id: Option<i64>) -> Result<BulkReport, CommandError> {
        let ids: Vec<String> =
            self.group_profile_ids(group_id)?.into_iter().filter(|id| !self.supervisor.status(id).is_active()).collect();
        Ok(self.start_profiles(&ids).await)
    }

    /// Живые профили группы (`None` — «Без группы»); неизвестная группа — `NotFound`.
    pub(super) fn group_profile_ids(&self, group_id: Option<i64>) -> Result<Vec<String>, CommandError> {
        let group = match group_id {
            Some(id) => {
                self.store.get_group(id)?.ok_or_else(|| CommandError::new("NotFound", "группа не найдена"))?;
                GroupFilter::Id(id)
            }
            None => GroupFilter::Ungrouped,
        };
        Ok(self.store.list_profiles(&ListQuery { group: Some(group), ..Default::default() })?.into_iter().map(|p| p.id).collect())
    }

    /// Все сразу: остановка каждого профиля и так идёт в своей очереди.
    pub async fn stop_profiles(&self, ids: &[String]) -> BulkReport {
        let results = join_all(ids.iter().map(|id| async move {
            (id.clone(), self.supervisor.stop(id).await.map_err(CommandError::from))
        }))
        .await;
        let mut report = BulkReport::default();
        for (id, result) in results {
            report.push(id, result);
        }
        report
    }

    /// По одному; запущенные профили не удаляются и попадают в `failed` с `ProfileRunning`.
    pub async fn delete_profiles(&self, ids: &[String]) -> BulkReport {
        let mut report = BulkReport::default();
        for id in ids {
            let result = self.delete_profile(id).await;
            report.push(id.clone(), result);
        }
        report
    }

    /// Перенос в группу или в «Без группы»; несуществующая группа — ошибка формы `group_id`.
    pub fn move_profiles(&self, ids: &[String], group_id: Option<i64>) -> Result<BulkReport, CommandError> {
        let (existing, mut report) = self.split_existing(ids)?;
        self.store.move_profiles(&existing, group_id)?;
        report.ok = existing;
        Ok(report)
    }

    /// Всё проверяется до записи: при ошибке не меняется ничего. Затем поля, теги и шлюз — каждое своей транзакцией.
    pub fn edit_profiles(&self, ids: &[String], form: &BulkEditForm) -> Result<BulkReport, CommandError> {
        let start_urls = form.start_urls.as_deref().map(clean_lines);
        let extra_args = form.extra_args.as_deref().map(clean_lines);
        let add_tags = clean_lines(&form.add_tags);
        let remove_tags = clean_lines(&form.remove_tags);
        let mut errors =
            launch_line_errors(start_urls.as_deref().unwrap_or_default(), extra_args.as_deref().unwrap_or_default());
        errors.extend(tag_errors(&add_tags));
        errors.extend(tag_errors(&remove_tags));
        if let Some(ProxyEdit::Set { proxy_id }) = &form.proxy
            && self.store.get_proxy(*proxy_id)?.is_none()
        {
            errors.push(FieldError { field: "proxy_id", code: "invalid" });
        }
        if !errors.is_empty() {
            return Err(CommandError::validation(errors));
        }
        let (existing, mut report) = self.split_existing(ids)?;
        let edit = ProfileEdit {
            note: form.note.as_deref().map(|note| note.trim().to_string()),
            start_urls,
            restore_tabs: form.restore_tabs,
            extra_args,
        };
        if edit.note.is_some() || edit.start_urls.is_some() || edit.restore_tabs.is_some() || edit.extra_args.is_some() {
            self.store.edit_profiles(&existing, &edit)?;
        }
        if !add_tags.is_empty() {
            self.store.add_tags(&existing, &add_tags)?;
        }
        if !remove_tags.is_empty() {
            self.store.remove_tags(&existing, &remove_tags)?;
        }
        match &form.proxy {
            Some(ProxyEdit::Reset) => {
                self.store.set_profiles_proxy(&existing, None)?;
            }
            Some(ProxyEdit::Set { proxy_id }) => {
                self.store.set_profiles_proxy(&existing, Some(*proxy_id))?;
            }
            None => {}
        }
        report.ok = existing;
        Ok(report)
    }

    /// Существующие профили и отчёт, где несуществующие уже записаны в `failed` с `NotFound`.
    pub(super) fn split_existing(&self, ids: &[String]) -> Result<(Vec<String>, BulkReport), CommandError> {
        let mut existing = Vec::new();
        let mut report = BulkReport::default();
        for id in ids {
            if self.store.get_profile(id)?.is_some() {
                existing.push(id.clone());
            } else {
                report.failed.push(BulkFailure { id: id.clone(), error: not_found() });
            }
        }
        Ok((existing, report))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::forms::SecretInput;
    use crate::app::groups::GroupForm;
    use crate::app::proxies::ProxyEditForm;
    use crate::app::tests::{create_with_proxy, env, form, test_proxy};
    use crate::fingerprint::Fingerprint;
    use crate::cookies::Cookie;
    use crate::engine::process::Probe;
    use crate::fingerprint::Field;
    use crate::supervisor::ProfileStatus;
    use crate::store::proxies::ProxyKind;
    use crate::store::secrets::{Secret, SecretField};
    use std::collections::HashSet;
    use std::time::Duration;

    fn create_form(count: u32) -> BulkCreateForm {
        BulkCreateForm {
            count,
            prefix: " Магазин ".into(),
            group_id: None,
            tags: vec!["shop".into()],
            note: "партия".into(),
            start_urls: vec!["https://example.com/".into()],
            restore_tabs: false,
            extra_args: vec!["--disable-sync".into()],
            proxy: ProxyChoice::Random,
        }
    }

    fn group_form(name: &str) -> GroupForm {
        GroupForm { name: name.into(), color: "#7a3fc4".into(), note: String::new(), quick_access: false }
    }

    fn codes(error: &CommandError) -> Vec<(&'static str, &'static str)> {
        error.fields.as_ref().unwrap().iter().map(|f| (f.field, f.code)).collect()
    }

    #[test]
    fn names_are_zero_padded() {
        assert_eq!(bulk_names("Магазин", 3), ["Магазин 001", "Магазин 002", "Магазин 003"]);
        let thousand = bulk_names("М", 1000);
        assert_eq!((thousand[0].as_str(), thousand[999].as_str()), ("М 0001", "М 1000"));
        assert!(bulk_names("М", 0).is_empty());
    }

    #[test]
    fn copy_name_fits_the_limit() {
        assert_eq!(clone_name("Магазин"), "Магазин (копия)");
        let long = clone_name(&"я".repeat(100));
        assert_eq!(long.chars().count(), 100);
        assert!(long.ends_with("я (копия)"));
    }

    #[tokio::test]
    async fn clone_copies_settings_but_not_identity() {
        let e = env().await;
        let group = e.core.create_group(&group_form("Магазины"), 1).unwrap();
        let proxy = test_proxy(&e.core, e.proxy_port);
        let mut fingerprint = e.core.new_fingerprint().unwrap();
        // Такого числа ядер нет в наборе комбинаций: случайно совпасть с новым отпечатком оно не может.
        fingerprint.hardware_concurrency = 6;
        fingerprint.manual = vec![Field::HardwareConcurrency];
        let mut input = crate::store::profiles::ProfileInput::new("Магазин", fingerprint.to_json());
        input.note = "заметка".into();
        input.platform_domain = "shop.example".into();
        input.start_urls = vec!["https://example.com/".into()];
        input.extra_args = vec!["--disable-sync".into()];
        input.username = Some("seller".into());
        input.password = SecretField::Set(Secret::new("hunter2"));
        input.proxy_id = Some(proxy.id);
        input.group_id = Some(group.id);
        input.restore_tabs = false;
        input.humanize = Some(crate::humanize::Humanize { preset: crate::humanize::Preset::Careful, ..Default::default() });
        let source = e.core.store.create_profile(&input, 1).unwrap();
        e.core.store.add_tags(std::slice::from_ref(&source.id), &["shop".into(), "vip".into()]).unwrap();
        let cookie = Cookie {
            domain: "shop.example".into(),
            name: "sid".into(),
            value: "1".into(),
            path: "/".into(),
            expires: None,
            http_only: true,
            secure: true,
            same_site: None,
            host_only: true,
        };
        e.core.store.merge_cookies(&source.id, &[cookie]).unwrap();

        let copy = e.core.clone_profile(&source.id, 5).unwrap();
        assert_ne!(copy.id, source.id);
        assert_eq!((copy.name.as_str(), copy.created_at), ("Магазин (копия)", 5));
        assert_eq!(
            (copy.note.as_str(), copy.platform_domain.as_str(), copy.group_id, copy.restore_tabs),
            ("заметка", "shop.example", Some(group.id), false)
        );
        assert_eq!((&copy.start_urls, &copy.extra_args), (&source.start_urls, &source.extra_args));
        assert_eq!((copy.username.clone(), copy.has_password, copy.has_totp, copy.proxy_id), (None, false, false, None));
        assert_eq!(e.core.store.profile_tags(&copy.id).unwrap(), ["shop", "vip"]);
        assert_eq!(e.core.store.count_cookies(&copy.id).unwrap(), 0);
        let copied = Fingerprint::from_json(&copy.fingerprint_json).unwrap();
        assert_ne!(copied.seed, fingerprint.seed, "seed новый");
        assert_eq!(copy.humanize, source.humanize, "настройки очеловечивания копируются");
        assert_ne!(copy.humanize_seed, source.humanize_seed, "персона новая (ТЗ 7.2 п. 4)");
        assert_eq!((copied.hardware_concurrency, copied.manual.clone()), (6, vec![Field::HardwareConcurrency]));
        assert_eq!(e.core.clone_profile("nope00", 5).unwrap_err().code, "NotFound");
    }

    #[tokio::test]
    async fn clone_of_selection_reports_each_source() {
        let e = env().await;
        let a = e.core.create_profile(&form("a", None), 1).unwrap();
        let b = e.core.create_profile(&form("b", None), 2).unwrap();
        let ids = [a.id.clone(), "nope00".to_string(), b.id.clone()];
        let report = e.core.clone_profiles(&ids, 5);
        assert_eq!(report.ok, [a.id.clone(), b.id.clone()]);
        assert_eq!(report.failed.len(), 1);
        assert_eq!((report.failed[0].id.as_str(), report.failed[0].error.code.as_str()), ("nope00", "NotFound"));
        let names: Vec<String> =
            e.core.store.list_profiles(&ListQuery::default()).unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["a", "b", "a (копия)", "b (копия)"]);
    }

    #[tokio::test]
    async fn bulk_create_makes_a_thousand_at_once() {
        let e = env().await;
        let created = e.core.create_profiles_bulk(&create_form(1000), 7).unwrap();
        assert_eq!(created.len(), 1000);
        assert_eq!((created[0].name.as_str(), created[999].name.as_str()), ("Магазин 0001", "Магазин 1000"));
        let seeds: HashSet<u32> =
            created.iter().map(|p| Fingerprint::from_json(&p.fingerprint_json).unwrap().seed).collect();
        assert!(seeds.len() > 900, "у каждого профиля свой отпечаток: разных seed {}", seeds.len());
        assert!(created.iter().all(|p| p.note == "партия"
            && !p.restore_tabs
            && p.extra_args == ["--disable-sync"]
            && p.start_urls == ["https://example.com/"]
            && p.proxy_id.is_none()
            && p.created_at == 7));
        assert_eq!(e.core.list_tags().unwrap()[0].profile_count, 1000);
    }

    #[tokio::test]
    async fn bulk_create_checks_everything_before_writing() {
        let e = env().await;
        let mut bad = create_form(1001);
        bad.prefix = " ".into();
        bad.extra_args = vec!["--user-data-dir=/tmp/x".into()];
        let err = e.core.create_profiles_bulk(&bad, 1).unwrap_err();
        assert_eq!(codes(&err), [("count", "out_of_range"), ("prefix", "required"), ("extra_args", "forbidden")]);
        let mut long = create_form(5);
        long.prefix = "я".repeat(97);
        assert_eq!(codes(&e.core.create_profiles_bulk(&long, 1).unwrap_err()), [("prefix", "too_long")]);
        let mut missing_group = create_form(5);
        missing_group.group_id = Some(999);
        assert_eq!(codes(&e.core.create_profiles_bulk(&missing_group, 1).unwrap_err()), [("group_id", "invalid")]);
        let mut missing_proxy = create_form(5);
        missing_proxy.proxy = ProxyChoice::Fixed { proxy_id: 999 };
        assert_eq!(codes(&e.core.create_profiles_bulk(&missing_proxy, 1).unwrap_err()), [("proxy_id", "invalid")]);
        assert_eq!(e.core.store.count_profiles().unwrap(), 0, "ничего не создано");

        let proxy = test_proxy(&e.core, e.proxy_port);
        let mut fixed = create_form(3);
        fixed.proxy = ProxyChoice::Fixed { proxy_id: proxy.id };
        assert!(e.core.create_profiles_bulk(&fixed, 1).unwrap().iter().all(|p| p.proxy_id == Some(proxy.id)));
    }

    #[tokio::test]
    async fn unrestricted_bulk_start_ignores_the_legacy_limit() {
        let e = env().await;
        e.core.store.set_setting("profiles.max_running", "3").unwrap();
        let ids: Vec<String> =
            (0..5).map(|i| create_with_proxy(&e.core, &format!("p{i}"), e.proxy_port, 1).id).collect();
        let report = e.core.start_profiles(&ids).await;
        assert_eq!(report.ok.len(), 5, "{report:?}");
        assert!(report.failed.is_empty());
        let stopped = e.core.stop_profiles(&ids).await;
        assert_eq!(stopped.ok.len(), 5);
        assert!(stopped.failed.iter().all(|f| f.error.code == "NotRunning"), "{stopped:?}");
        assert_eq!(e.core.supervisor.active_count(), 0);
    }

    #[tokio::test]
    async fn bulk_start_runs_at_most_four_at_once() {
        let e = env().await;
        *e.core.engine.launch_delay.lock().unwrap() = Duration::from_millis(200);
        let ids: Vec<String> =
            (0..6).map(|i| create_with_proxy(&e.core, &format!("p{i}"), e.proxy_port, 1).id).collect();
        let report = e.core.start_profiles(&ids).await;
        assert_eq!(report.ok, ids, "порядок отчёта — как в запросе");
        let parallel = e.core.engine.max_parallel_launches();
        assert!((2..=BULK_START_PARALLEL).contains(&parallel), "одновременно запускалось {parallel}");
        assert_eq!(e.core.stop_profiles(&ids).await.ok.len(), 6);
    }

    #[tokio::test]
    async fn bulk_start_returns_when_profiles_wait_for_a_thread() {
        let e = env().await;
        e.core.engine.answer(Probe::License { state: "no_slot".into(), owner_hint: None });
        let ids: Vec<String> =
            (0..6).map(|i| create_with_proxy(&e.core, &format!("p{i}"), e.proxy_port, 1).id).collect();
        let report = tokio::time::timeout(Duration::from_secs(5), e.core.start_profiles(&ids))
            .await
            .expect("массовый запуск не ждёт поток");
        assert_eq!(report.waiting, ids, "{report:?}");
        assert!(report.ok.is_empty() && report.failed.is_empty(), "{report:?}");
        assert!(ids.iter().all(|id| matches!(e.core.supervisor.status(id), ProfileStatus::Waiting { .. })));
        assert_eq!(e.core.stop_profiles(&ids).await.ok.len(), 6);
        assert_eq!(e.core.supervisor.active_count(), 0);
    }

    #[tokio::test]
    async fn bulk_start_does_not_outlive_stop_all() {
        let e = env().await;
        *e.core.engine.launch_delay.lock().unwrap() = Duration::from_millis(200);
        let ids: Vec<String> =
            (0..8).map(|i| create_with_proxy(&e.core, &format!("p{i}"), e.proxy_port, 1).id).collect();
        let (report, ()) = tokio::join!(e.core.start_profiles(&ids), async {
            // Первые запуски уже идут, остальные ждут своей очереди в массовом запуске.
            while e.core.engine.launches() == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            e.core.supervisor.stop_all().await;
        });
        assert_eq!(e.core.supervisor.active_count(), 0, "{report:?}");
        let launched = e.core.engine.launches() as usize;
        assert!(launched <= BULK_START_PARALLEL, "после начала выхода браузеры не запускаются: {launched}");
        let cancelled = report.failed.iter().filter(|f| f.error.code == "NotRunning").count();
        assert!(cancelled >= ids.len() - BULK_START_PARALLEL, "{report:?}");
    }

    #[tokio::test]
    async fn group_start_launches_only_its_idle_profiles() {
        let e = env().await;
        let group = e
            .core
            .create_group(
                &GroupForm { name: "Магазины".into(), color: "#2447c9".into(), note: String::new(), quick_access: false },
                1,
            )
            .unwrap();
        let mut in_group = Vec::new();
        for i in 0..2 {
            let mut f = form(&format!("g{i}"), Some(test_proxy(&e.core, e.proxy_port).id));
            f.group_id = Some(Some(group.id));
            in_group.push(e.core.create_profile(&f, 1).unwrap().id);
        }
        let outside = create_with_proxy(&e.core, "other", e.proxy_port, 1).id;
        e.core.supervisor.start(&in_group[0]).await.unwrap();

        let report = e.core.start_group(Some(group.id)).await.unwrap();
        assert_eq!(report.ok, std::slice::from_ref(&in_group[1]), "уже работающий не запускается повторно: {report:?}");
        assert!(report.failed.is_empty(), "{report:?}");
        assert!(!e.core.supervisor.status(&outside).is_active(), "профиль вне группы не тронут");
        assert_eq!(e.core.start_group(None).await.unwrap().ok, std::slice::from_ref(&outside), "«Без группы»");
        assert_eq!(e.core.start_group(Some(9999)).await.unwrap_err().code, "NotFound");
        e.core.stop_profiles(&[in_group[0].clone(), in_group[1].clone(), outside]).await;
    }

    #[tokio::test]
    async fn bulk_delete_skips_active_profiles() {
        let e = env().await;
        let running = create_with_proxy(&e.core, "a", e.proxy_port, 1);
        let idle = e.core.create_profile(&form("b", None), 2).unwrap();
        e.core.supervisor.start(&running.id).await.unwrap();
        let report = e.core.delete_profiles(&[running.id.clone(), idle.id.clone(), "nope00".into()]).await;
        assert_eq!(report.ok, std::slice::from_ref(&idle.id));
        assert_eq!(
            report.failed.iter().map(|f| (f.id.as_str(), f.error.code.as_str())).collect::<Vec<_>>(),
            [(running.id.as_str(), "ProfileRunning"), ("nope00", "NotFound")]
        );
        assert!(e.core.store.get_profile(&running.id).unwrap().is_some());
        assert!(e.core.store.get_profile(&idle.id).unwrap().is_none());
        e.core.supervisor.stop(&running.id).await.unwrap();
    }

    #[tokio::test]
    async fn bulk_move_checks_group() {
        let e = env().await;
        let group = e.core.create_group(&group_form("Реклама"), 1).unwrap();
        let a = e.core.create_profile(&form("a", None), 1).unwrap();
        let b = e.core.create_profile(&form("b", None), 2).unwrap();
        let report = e.core.move_profiles(&[a.id.clone(), b.id.clone(), "nope00".into()], Some(group.id)).unwrap();
        assert_eq!(report.ok, [a.id.clone(), b.id.clone()]);
        assert_eq!(report.failed.iter().map(|f| f.error.code.as_str()).collect::<Vec<_>>(), ["NotFound"]);
        assert_eq!(e.core.list_groups().unwrap().groups[0].profile_count, 2);
        e.core.move_profiles(std::slice::from_ref(&a.id), None).unwrap();
        assert_eq!(e.core.store.get_profile(&a.id).unwrap().unwrap().group_id, None);
        assert_eq!(codes(&e.core.move_profiles(std::slice::from_ref(&a.id), Some(999)).unwrap_err()), [("group_id", "invalid")]);
    }

    #[tokio::test]
    async fn bulk_edit_changes_only_given_fields() {
        let e = env().await;
        let proxy = e
            .core
            .create_proxy(&ProxyEditForm {
                kind: ProxyKind::Http,
                host: "127.0.0.1".into(),
                port: e.proxy_port,
                username: "user".into(),
                password: SecretInput::Set { value: "pass".into() },
                note: String::new(),
                enabled: true,
            })
            .unwrap();
        let mut with_args = form("a", None);
        with_args.extra_args = vec!["--disable-sync".into()];
        let a = e.core.create_profile(&with_args, 1).unwrap();
        let b = e.core.create_profile(&form("b", None), 2).unwrap();
        e.core.store.add_tags(std::slice::from_ref(&a.id), &["old".into()]).unwrap();
        let edit = BulkEditForm {
            add_tags: vec![" new ".into()],
            remove_tags: vec!["old".into()],
            note: Some(" общая ".into()),
            proxy: Some(ProxyEdit::Set { proxy_id: proxy.id }),
            start_urls: Some(vec!["https://example.com/".into(), "  ".into()]),
            restore_tabs: Some(false),
            extra_args: None,
        };
        let report = e.core.edit_profiles(&[a.id.clone(), b.id.clone(), "nope00".into()], &edit).unwrap();
        assert_eq!(report.ok, [a.id.clone(), b.id.clone()]);
        assert_eq!(report.failed.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["nope00"]);
        for id in [&a.id, &b.id] {
            let p = e.core.store.get_profile(id).unwrap().unwrap();
            assert_eq!((p.note.as_str(), p.proxy_id, p.restore_tabs), ("общая", Some(proxy.id), false));
            assert_eq!(p.start_urls, ["https://example.com/"]);
            assert_eq!(e.core.store.profile_tags(id).unwrap(), ["new"]);
        }
        assert_eq!(e.core.store.get_profile(&a.id).unwrap().unwrap().extra_args, ["--disable-sync"], "не задано — не меняется");
        assert!(e.core.list_tags().unwrap().iter().all(|t| t.name != "old"), "тег без профилей удалён");

        let reset = BulkEditForm { proxy: Some(ProxyEdit::Reset), ..Default::default() };
        e.core.edit_profiles(std::slice::from_ref(&a.id), &reset).unwrap();
        assert_eq!(e.core.store.get_profile(&a.id).unwrap().unwrap().proxy_id, None);
        assert_eq!(e.core.store.get_profile(&b.id).unwrap().unwrap().proxy_id, Some(proxy.id));

        let bad = BulkEditForm {
            note: Some("другая".into()),
            extra_args: Some(vec!["--proxy-server=direct://".into()]),
            proxy: Some(ProxyEdit::Set { proxy_id: 999 }),
            ..Default::default()
        };
        let err = e.core.edit_profiles(std::slice::from_ref(&a.id), &bad).unwrap_err();
        assert_eq!(codes(&err), [("extra_args", "forbidden"), ("proxy_id", "invalid")]);
        assert_eq!(e.core.store.get_profile(&a.id).unwrap().unwrap().note, "общая", "при ошибке ничего не изменено");
    }
}
