use super::forms::{ProfileForm, SecretInput};
use super::settings::QuitPlan;
use super::profiles::ListQueryDto;
use crate::store::profiles::SortKey;
use super::*;
use crate::fingerprint::{Arch, Catalog, FieldError, OsRandom, generate};
use crate::store::profiles::ProfileRecord;
use crate::store::proxies::{ProxyKind, ProxyRecord};
use crate::store::tests::memory_store;
use crate::supervisor::test_engine::{FakeEngine, add_proxy, fast_timings, geo_stack, wait_until};
use crate::supervisor::{Event, StatusFilter};
use std::time::Duration;

pub(crate) struct Env {
    pub(crate) core: Core<FakeEngine>,
    pub(crate) proxy_port: u16,
    _dir: tempfile::TempDir,
}

/// Ядро с подставным движком, порт тестового HTTP-прокси `user:pass` и временная папка данных.
pub(crate) async fn test_core() -> (Core<FakeEngine>, u16, tempfile::TempDir) {
    test_core_with_sink(Arc::new(|_: Event| {})).await
}

/// То же, но события ядра уходят в `sink` — для проверки событий интерфейса.
pub(crate) async fn test_core_with_sink(sink: EventSink) -> (Core<FakeEngine>, u16, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let stack = geo_stack(200).await;
    let core = Core::new(
        Paths::new(dir.path()),
        Arc::new(memory_store()),
        Arc::new(FakeEngine::new()),
        Arc::new(stack.geo),
        sink,
        fast_timings(),
    );
    (core, stack.proxy.port(), dir)
}

pub(crate) async fn env() -> Env {
    let (core, proxy_port, dir) = test_core().await;
    Env { core, proxy_port, _dir: dir }
}

/// Форма профиля. `proxy_id` — id записи списка прокси: `Some` закрепляет её, `None` — поля нет, шлюз не меняется.
pub(crate) fn form(name: &str, proxy_id: Option<i64>) -> ProfileForm {
    ProfileForm {
        name: name.into(),
        note: String::new(),
        platform_domain: String::new(),
        start_urls: vec![],
        extra_args: vec![],
        username: String::new(),
        password: SecretInput::Keep,
        totp: SecretInput::Keep,
        fingerprint: generate(Catalog::builtin(), Arch::host(), &mut OsRandom).unwrap(),
        proxy_id: proxy_id.map(Some),
        group_id: None,
        restore_tabs: None,
        tags: None,
        bookmarks: None,
        ext_category_id: None,
        humanize: None,
        network: None,
        inline_proxy: None,
        inline_proxy_source: None,
    }
}

/// Запись общего списка: HTTP-прокси `127.0.0.1:<port>` с `user:pass` — их ждёт тестовый прокси окружения.
pub(crate) fn test_proxy(core: &Core<FakeEngine>, port: u16) -> ProxyRecord {
    core.store.get_proxy(add_proxy(&core.store, port, "pass")).unwrap().unwrap()
}

/// Профиль, закреплённый за новой записью `test_proxy(core, proxy_port)`.
pub(crate) fn create_with_proxy(core: &Core<FakeEngine>, name: &str, proxy_port: u16, now: i64) -> ProfileRecord {
    let proxy = test_proxy(core, proxy_port);
    core.create_profile(&form(name, Some(proxy.id)), now).unwrap()
}

#[tokio::test]
async fn profile_form_pins_keeps_and_unpins_proxy() {
    let e = env().await;
    let proxy = test_proxy(&e.core, e.proxy_port);
    let rec = e.core.create_profile(&form("Магазин", Some(proxy.id)), 10).unwrap();
    assert_eq!(rec.proxy_id, Some(proxy.id), "число закрепляет");
    assert_eq!(e.core.profile_details(&rec.id).unwrap().proxy.map(|p| p.id), Some(proxy.id));
    assert_eq!(e.core.create_profile(&form("Без шлюза", None), 11).unwrap().proxy_id, None, "без поля — без закрепления");

    e.core.store.record_gateway(&rec.id, proxy.id, Some(50)).unwrap();
    let kept = e.core.update_profile(&rec.id, &form("Магазин 2", None)).unwrap();
    assert_eq!(
        (kept.name.as_str(), kept.proxy_id, kept.gateway_replaced_at),
        ("Магазин 2", Some(proxy.id), Some(50)),
        "поля нет — шлюз и отметка о замене не меняются"
    );
    let same = e.core.update_profile(&rec.id, &form("Магазин 2", Some(proxy.id))).unwrap();
    assert_eq!(same.gateway_replaced_at, Some(50), "тот же прокси — отметка остаётся");
    let other = test_proxy(&e.core, e.proxy_port);
    let moved = e.core.update_profile(&rec.id, &form("Магазин 2", Some(other.id))).unwrap();
    assert_eq!((moved.proxy_id, moved.gateway_replaced_at), (Some(other.id), None), "другой прокси — как ручное назначение");

    e.core.store.record_gateway(&rec.id, other.id, Some(60)).unwrap();
    let mut unpin = form("Магазин 2", None);
    unpin.proxy_id = Some(None);
    let unpinned = e.core.update_profile(&rec.id, &unpin).unwrap();
    assert_eq!((unpinned.proxy_id, unpinned.gateway_replaced_at), (None, None), "null снимает закрепление и отметку");
    assert!(e.core.store.get_proxy(other.id).unwrap().is_some(), "запись общего списка остаётся");
    assert_eq!(e.core.store.list_proxies().unwrap().len(), 2, "сохранение профиля прокси не создаёт");
}

#[tokio::test]
async fn unknown_proxy_id_is_a_field_error_and_changes_nothing() {
    let e = env().await;
    let rec = create_with_proxy(&e.core, "a", e.proxy_port, 1);
    let missing = form("b", Some(999));
    let err = e.core.update_profile(&rec.id, &missing).unwrap_err();
    assert_eq!((err.code.as_str(), err.fields), ("Validation", Some(vec![FieldError { field: "proxy_id", code: "invalid" }])));
    let unchanged = e.core.store.get_profile(&rec.id).unwrap().unwrap();
    assert_eq!((unchanged.name.as_str(), unchanged.proxy_id), ("a", rec.proxy_id), "профиль не изменён");
    assert_eq!(e.core.create_profile(&missing, 2).unwrap_err().code, "Validation");
    assert_eq!(e.core.store.count_profiles().unwrap(), 1, "профиль не создан");
}

#[tokio::test]
async fn invalid_form_is_rejected_with_fields() {
    let e = env().await;
    let mut bad = form("  ", Some(999));
    bad.extra_args = vec!["--remote-debugging-port=9222".into()];
    let err = e.core.create_profile(&bad, 1).unwrap_err();
    assert_eq!(err.code, "Validation");
    let fields: Vec<&str> = err.fields.unwrap().iter().map(|f| f.field).collect();
    assert_eq!(fields, ["name", "extra_args", "proxy_id"], "неизвестный прокси — вместе с остальными ошибками");
    assert_eq!(e.core.store.count_profiles().unwrap(), 0);
    assert_eq!(e.core.update_profile("nope00", &form("a", None)).unwrap_err().code, "NotFound");
}

#[tokio::test]
async fn form_tags_replace_the_profile_set_only_when_given() {
    let e = env().await;
    let mut tagged = form("a", None);
    tagged.tags = Some(vec![" Shop ".into(), "vip".into(), String::new()]);
    let rec = e.core.create_profile(&tagged, 1).unwrap();
    assert_eq!(e.core.store.profile_tags(&rec.id).unwrap(), ["Shop", "vip"]);
    assert_eq!(e.core.profile_details(&rec.id).unwrap().tags, ["Shop", "vip"]);

    e.core.update_profile(&rec.id, &form("a", None)).unwrap();
    assert_eq!(e.core.store.profile_tags(&rec.id).unwrap(), ["Shop", "vip"], "без поля теги не меняются");

    let mut replaced = form("a", None);
    replaced.tags = Some(vec!["x".into()]);
    e.core.update_profile(&rec.id, &replaced).unwrap();
    assert_eq!(e.core.store.profile_tags(&rec.id).unwrap(), ["x"]);
    let all: Vec<String> = e.core.list_tags().unwrap().into_iter().map(|t| t.name).collect();
    assert_eq!(all, ["x"], "снятые теги без профилей удалены");

    replaced.tags = Some(vec![]);
    e.core.update_profile(&rec.id, &replaced).unwrap();
    assert!(e.core.store.profile_tags(&rec.id).unwrap().is_empty(), "пустой список снимает все");
}

#[tokio::test]
async fn too_long_tag_is_a_field_error_and_writes_nothing() {
    let e = env().await;
    let mut bad = form("a", None);
    bad.tags = Some(vec!["я".repeat(31)]);
    let err = e.core.create_profile(&bad, 1).unwrap_err();
    assert_eq!(err.code, "Validation");
    assert_eq!(err.fields.unwrap()[0].field, "tags");
    assert_eq!(e.core.store.count_profiles().unwrap(), 0, "профиль не создан");

    let rec = e.core.create_profile(&form("a", None), 1).unwrap();
    let mut bad_edit = form("b", None);
    bad_edit.tags = Some(vec!["я".repeat(31)]);
    assert_eq!(e.core.update_profile(&rec.id, &bad_edit).unwrap_err().code, "Validation");
    assert_eq!(e.core.store.get_profile(&rec.id).unwrap().unwrap().name, "a", "профиль не изменён");
}

#[tokio::test]
async fn profile_and_tags_are_saved_in_one_transaction() {
    let e = env().await;
    let rec = e.core.create_profile(&form("a", None), 1).unwrap();
    // Сбой базы именно на тегах: проверки формы новый тег пропускают.
    e.core
        .store
        .raw()
        .execute_batch("CREATE TRIGGER fail_tags BEFORE INSERT ON tags BEGIN SELECT RAISE(ABORT, 'сбой записи тегов'); END;")
        .unwrap();
    let mut edit = form("b", None);
    edit.tags = Some(vec!["vip".into()]);
    assert_eq!(e.core.update_profile(&rec.id, &edit).unwrap_err().code, "Storage");
    assert_eq!(e.core.store.get_profile(&rec.id).unwrap().unwrap().name, "a", "профиль не изменён");
    assert!(e.core.store.profile_tags(&rec.id).unwrap().is_empty());
}

#[tokio::test]
async fn list_filters_by_status_and_shows_gateway() {
    let e = env().await;
    let a = create_with_proxy(&e.core, "Альфа", e.proxy_port, 1);
    let b = e.core.create_profile(&form("бета", None), 2).unwrap();
    e.core.supervisor.start(&a.id).await.unwrap();
    let all = e.core.list_profiles(&ListQueryDto::default()).unwrap();
    assert_eq!((all.total, all.running), (2, 1));
    let gateway = all.rows[0].gateway.as_ref().unwrap();
    assert_eq!(
        (gateway.kind, gateway.port, gateway.ip.as_deref(), gateway.country.as_deref()),
        (ProxyKind::Http, e.proxy_port, Some("203.0.113.24"), Some("DE"))
    );
    assert!(all.rows[1].gateway.is_none());
    assert!((10_000..=99_999).contains(&all.rows[0].seed));
    let running = e.core.list_profiles(&ListQueryDto { status: Some(StatusFilter::Running), ..Default::default() }).unwrap();
    assert_eq!(running.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), [a.id.as_str()]);
    let found =
        e.core.list_profiles(&ListQueryDto { search: "БЕТ".into(), sort: SortKey::Name, ..Default::default() }).unwrap();
    assert_eq!(found.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), [b.id.as_str()]);
    e.core.supervisor.stop(&a.id).await.unwrap();
    let after = e.core.list_profiles(&ListQueryDto::default()).unwrap();
    assert_eq!(after.rows[0].gateway.as_ref().unwrap().country.as_deref(), Some("DE"), "страна и IP помнятся после остановки");
}

#[tokio::test]
async fn running_profile_is_not_deleted_and_stopped_one_is_wiped() {
    let e = env().await;
    let rec = create_with_proxy(&e.core, "a", e.proxy_port, 1);
    assert_eq!(e.core.quit_plan(), QuitPlan::Exit);
    e.core.supervisor.start(&rec.id).await.unwrap();
    assert_eq!(e.core.quit_plan(), QuitPlan::Ask, "выход при работающем профиле требует вопроса");
    assert_eq!(e.core.delete_profile(&rec.id).await.unwrap_err().code, "ProfileRunning");
    e.core.supervisor.stop(&rec.id).await.unwrap();
    assert_eq!(e.core.quit_plan(), QuitPlan::Exit);

    let dir = e.core.paths.profile_dir(&rec.id);
    std::fs::create_dir_all(dir.join("Default")).unwrap();
    let chromium_log = e.core.paths.chromium_log(&rec.id);
    std::fs::create_dir_all(chromium_log.parent().unwrap()).unwrap();
    std::fs::write(&chromium_log, "log").unwrap();
    let relay_log = e.core.paths.relay_log(&rec.id);
    let rotated = relay_log.with_extension("jsonl.1");
    std::fs::write(&relay_log, "r").unwrap();
    std::fs::write(&rotated, "r").unwrap();
    let record = crate::supervisor::pidfile::PidRecord {
        pid: 1,
        binary: "/x".into(),
        profile_dir: dir.clone(),
        started_at: 0,
    };
    crate::supervisor::pidfile::write(&e.core.paths.run_dir(), &rec.id, &record).unwrap();
    let pid_path = crate::supervisor::pidfile::path(&e.core.paths.run_dir(), &rec.id);
    assert!(pid_path.exists());
    e.core.delete_profile(&rec.id).await.unwrap();
    assert!(dir.is_dir() && rotated.exists() && pid_path.exists(), "в корзине файлы остаются");
    assert_eq!(e.core.purge_profiles(std::slice::from_ref(&rec.id)).await.ok, std::slice::from_ref(&rec.id));
    assert!(!rotated.exists());
    assert!(!pid_path.exists());
    assert!(!dir.exists());
    assert!(!chromium_log.exists());
    assert!(!e.core.paths.relay_log(&rec.id).exists());
    assert!(e.core.store.get_proxy(rec.proxy_id.unwrap()).unwrap().is_some(), "прокси остаётся в общем списке");
    assert_eq!(e.core.delete_profile(&rec.id).await.unwrap_err().code, "NotFound");
}

#[tokio::test]
async fn batch_delete_with_a_starting_profile_deletes_nothing() {
    let e = env().await;
    let a = e.core.create_profile(&form("a", None), 1).unwrap().id;
    let b = create_with_proxy(&e.core, "b", e.proxy_port, 2).id;
    let ids = [a.clone(), b.clone()];
    let hold = Arc::new(tokio::sync::Notify::new());
    *e.core.engine.hold_next_launch.lock().unwrap() = Some(hold.clone());
    let (started, ()) = tokio::join!(e.core.supervisor.start(&b), async {
        while e.core.engine.launches() == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let err = e.core.delete_profiles_atomic(&ids).await.unwrap_err();
        assert_eq!(err.code, "ProfileRunning");
        assert!(ids.iter().all(|id| e.core.store.get_profile(id).unwrap().is_some()), "не удалён ни один");
        hold.notify_one();
    });
    started.unwrap();
    e.core.supervisor.stop(&b).await.unwrap();

    e.core.delete_profiles_atomic(&[b.clone(), a.clone(), b.clone()]).await.unwrap();
    assert_eq!(e.core.store.count_profiles().unwrap(), 0, "повтор id не мешает");
    assert!(e.core.paths.profile_dir(&b).exists(), "в корзине папка остаётся");
    e.core.purge_profiles(std::slice::from_ref(&b)).await;
    assert!(!e.core.paths.profile_dir(&b).exists(), "папка браузера удалена");
    assert_eq!(e.core.delete_profiles_atomic(std::slice::from_ref(&a)).await.unwrap_err().code, "NotFound");
}

/// Детерминированный путь гонки: пачка прошла первую проверку, ждёт очередь младшего профиля, а старший за это время
/// запустился. Повторная проверка после очередей отказывает: не удалён ни один, папки на месте, замки отпущены.
#[tokio::test]
async fn batch_delete_waits_for_queues_and_rechecks() {
    let e = env().await;
    let x = create_with_proxy(&e.core, "x", e.proxy_port, 1).id;
    let y = create_with_proxy(&e.core, "y", e.proxy_port, 2).id;
    let (low, high) = if x < y { (x, y) } else { (y, x) };
    for id in [&low, &high] {
        std::fs::create_dir_all(e.core.paths.profile_dir(id)).unwrap();
    }
    let held = e.core.supervisor.lock_idle(&low).await.unwrap();
    let ids = [low.clone(), high.clone()];
    let (result, ()) = tokio::join!(e.core.delete_profiles_atomic(&ids), async {
        // Пачка прошла первую проверку и ждёт очередь `low`; пока она там, старший запускается.
        // Ожидание очереди снаружи не наблюдаемо: даём пачке несколько опросов, затем запускаем.
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        e.core.supervisor.start(&high).await.unwrap();
        drop(held);
    });
    assert_eq!(result.unwrap_err().code, "ProfileRunning");
    for id in [&low, &high] {
        assert!(e.core.store.get_profile(id).unwrap().is_some(), "не удалён ни один");
        assert!(e.core.paths.profile_dir(id).exists(), "папка на месте");
    }
    assert!(e.core.supervisor.status(&high).is_active());
    e.core.supervisor.stop(&high).await.unwrap();
    e.core.delete_profiles_atomic(&[low, high]).await.unwrap();
}

/// Удаление пачки (Local API) и запуск одного из её профилей в одно время. Проверяет два исхода: удаление целиком раньше
/// запуска (запуск не удался) и отказ первой проверки (профиль уже «запускается»). Путь «запуск между проверкой и очередями»
/// покрывает `batch_delete_waits_for_queues_and_rechecks`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn batch_delete_racing_a_start_is_all_or_nothing() {
    let e = env().await;
    for round in 0..20 {
        let a = create_with_proxy(&e.core, &format!("a{round}"), e.proxy_port, 1).id;
        let b = create_with_proxy(&e.core, &format!("b{round}"), e.proxy_port, 1).id;
        let ids = [a.clone(), b.clone()];
        let (deleted, started) = tokio::join!(
            async {
                // Разное число уступок — удаление застаёт запуск на разных шагах.
                for _ in 0..round {
                    tokio::task::yield_now().await;
                }
                e.core.delete_profiles_atomic(&ids).await
            },
            e.core.supervisor.start(&b)
        );
        let kept: Vec<bool> = ids.iter().map(|id| e.core.store.get_profile(id).unwrap().is_some()).collect();
        match deleted {
            Ok(()) => {
                assert_eq!(kept, [false, false], "раунд {round}: удалены оба");
                assert!(started.is_err(), "раунд {round}: запуск удалённого профиля не удался: {started:?}");
                assert!(!e.core.supervisor.status(&b).is_active(), "раунд {round}");
                assert!(!e.core.engine.is_running(&e.core.paths.profile_dir(&b)), "раунд {round}: браузера без профиля нет");
            }
            Err(err) => {
                assert_eq!(err.code, "ProfileRunning", "раунд {round}");
                assert_eq!(kept, [true, true], "раунд {round}: не удалён ни один");
                started.unwrap_or_else(|error| panic!("раунд {round}: начатый запуск доходит до конца: {error}"));
                e.core.supervisor.stop(&b).await.unwrap();
            }
        }
    }
}

#[tokio::test]
async fn fingerprint_options_cover_catalog() {
    let e = env().await;
    let options = e.core.fingerprint_options();
    assert!(!options.screens.is_empty() && !options.gpus.is_empty() && !options.hardware_concurrency.is_empty());
    assert_eq!(options.device_memory, [2, 4, 8, 16, 32]);
    assert!(options.screens.windows(2).all(|w| w[0].width * w[0].height <= w[1].width * w[1].height));
    assert_eq!(options.engine_version.as_deref(), Some("150.0.7871.114"));
    let fingerprint = e.core.new_fingerprint().unwrap();
    assert!(e.core.validate_fingerprint(&fingerprint).is_empty());
    let next = e.core.regenerate_fingerprint(&fingerprint).unwrap();
    assert!(e.core.validate_fingerprint(&next).is_empty());
}

#[tokio::test]
async fn language_and_engine_settings() {
    let e = env().await;
    assert_eq!(e.core.language(), "ru");
    e.core.set_language("en").unwrap();
    assert_eq!(e.core.app_info().language, "en");
    assert_eq!(e.core.set_language("de").unwrap_err().code, "Validation");
    assert_eq!(e.core.app_info().engine.unwrap().version, crate::supervisor::test_engine::VERSION);
}

#[tokio::test]
async fn import_engine_does_not_stop_running_profiles() {
    let e = env().await;
    let profile = create_with_proxy(&e.core, "a", e.proxy_port, 1);
    let running = e.core.supervisor.start(&profile.id).await.unwrap();
    e.core.import_engine(std::path::Path::new("/tmp/ombra-engine.tar.gz")).await.unwrap();
    assert_eq!(e.core.engine.imports(), 1);
    assert_eq!(e.core.supervisor.running_info(&profile.id), Some(running));
    e.core.supervisor.stop(&profile.id).await.unwrap();
}

#[tokio::test]
async fn import_engine_preserves_records_of_other_browsers() {
    let e = env().await;
    std::fs::create_dir_all(e.core.paths.run_dir()).unwrap();
    let pid = e.core.paths.run_dir().join("other01.pid");
    std::fs::write(&pid, "other browser").unwrap();
    e.core.import_engine(std::path::Path::new("/tmp/ombra-engine.tar.gz")).await.unwrap();
    assert_eq!(e.core.engine.imports(), 1);
    assert_eq!(std::fs::read_to_string(pid).unwrap(), "other browser");
}

#[tokio::test]
async fn failed_file_cleanup_keeps_the_profile_for_retry() {
    use std::os::unix::fs::PermissionsExt;
    let e = env().await;
    let rec = e.core.create_profile(&form("a", None), 1).unwrap();
    let dir = e.core.paths.profile_dir(&rec.id);
    let locked = dir.join("Default").join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join("f"), "x").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(&locked).is_ok() {
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        return; // root: права не мешают, сбой не воспроизвести
    }
    e.core.delete_profile(&rec.id).await.unwrap();
    assert!(dir.is_dir(), "в корзину папка не трогается");
    let failed = e.core.purge_profiles(std::slice::from_ref(&rec.id)).await;
    assert_eq!(failed.failed.len(), 1);
    assert_eq!(failed.failed[0].error.code, "Io");
    assert!(e.core.store.get_profile_any(&rec.id).unwrap().is_some(), "запись остаётся для повтора");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    e.core.purge_profiles(std::slice::from_ref(&rec.id)).await;
    assert!(!dir.exists());
    assert!(e.core.store.get_profile_any(&rec.id).unwrap().is_none());
}

#[tokio::test]
async fn profile_form_sets_group_and_restore_tabs() {
    let e = env().await;
    let group = e
        .core
        .create_group(
            &crate::app::groups::GroupForm { name: "Магазины".into(), color: "#0b6e6e".into(), note: String::new(), quick_access: false },
            1,
        )
        .unwrap();
    let mut f = form("a", None);
    f.group_id = Some(Some(group.id));
    f.restore_tabs = Some(false);
    let rec = e.core.create_profile(&f, 1).unwrap();
    assert_eq!((rec.group_id, rec.restore_tabs), (Some(group.id), false));
    f.group_id = Some(None);
    let updated = e.core.update_profile(&rec.id, &f).unwrap();
    assert_eq!((updated.group_id, updated.restore_tabs), (None, false), "явный null — «Без группы»");
    f.group_id = Some(Some(999));
    let err = e.core.update_profile(&rec.id, &f).unwrap_err();
    assert_eq!((err.code.as_str(), err.fields.unwrap()[0].field), ("Validation", "group_id"));
}

#[tokio::test]
async fn list_filters_by_group_and_tag_and_shows_them() {
    let e = env().await;
    let shops = e
        .core
        .create_group(
            &crate::app::groups::GroupForm { name: "Магазины".into(), color: "#0b6e6e".into(), note: String::new(), quick_access: true },
            1,
        )
        .unwrap();
    let mut in_group = form("Альфа", None);
    in_group.group_id = Some(Some(shops.id));
    let a = e.core.create_profile(&in_group, 1).unwrap();
    let b = e.core.create_profile(&form("Бета", None), 2).unwrap();
    in_group.name = "Гамма".into();
    let c = e.core.create_profile(&in_group, 3).unwrap();
    e.core.store.add_tags(&[b.id.clone(), c.id.clone()], &["Shop".into()]).unwrap();
    let ids = |query: ListQueryDto| -> Vec<String> {
        e.core.list_profiles(&query).unwrap().rows.into_iter().map(|r| r.id).collect()
    };
    assert_eq!(ids(ListQueryDto { group_id: Some(shops.id), ..Default::default() }), [a.id.clone(), c.id.clone()]);
    assert_eq!(
        ids(ListQueryDto { ungrouped: true, group_id: Some(shops.id), ..Default::default() }),
        vec![b.id.clone()],
        "«Без группы» важнее group_id"
    );
    assert_eq!(ids(ListQueryDto { tag: Some("shop".into()), ..Default::default() }), [b.id.clone(), c.id.clone()]);
    assert_eq!(ids(ListQueryDto { group_id: Some(shops.id), tag: Some("SHOP".into()), ..Default::default() }), vec![c.id.clone()]);
    assert_eq!(ids(ListQueryDto { tag: Some("  ".into()), ..Default::default() }).len(), 3, "пустой тег — без фильтра");

    let rows = e.core.list_profiles(&ListQueryDto::default()).unwrap().rows;
    let group = rows[0].group.clone().unwrap();
    assert_eq!((group.id, group.name.as_str(), group.color.as_str()), (shops.id, "Магазины", "#0b6e6e"));
    assert!(rows[1].group.is_none());
    assert_eq!((rows[0].tags.clone(), rows[2].tags.clone()), (Vec::<String>::new(), vec!["Shop".to_string()]));
    assert!(rows.iter().all(|r| r.restore_tabs && r.gateway_replaced_at.is_none()));
}

#[tokio::test]
async fn row_gateway_comes_from_last_proxy_check() {
    let e = env().await;
    let rec = create_with_proxy(&e.core, "a", e.proxy_port, 1);
    let gateway = |e: &Env| e.core.list_profiles(&ListQueryDto::default()).unwrap().rows[0].gateway.clone().unwrap();
    let before = gateway(&e);
    assert_eq!((before.ip, before.country, before.ok), (None, None, None), "прокси ещё не проверяли");
    e.core.supervisor.start(&rec.id).await.unwrap();
    e.core.supervisor.stop(&rec.id).await.unwrap();
    let after = gateway(&e);
    assert_eq!(
        (after.ip.as_deref(), after.country.as_deref(), after.ok),
        (Some("203.0.113.24"), Some("DE"), Some(true)),
        "итог проверки при запуске помнится после остановки"
    );
}

#[tokio::test]
async fn paused_profile_is_found_by_status_filter() {
    use tokio::io::AsyncWriteExt;
    let e = env().await;
    let (proxy, down) = crate::relay::test_support::switchable_proxy().await;
    let rec = create_with_proxy(&e.core, "Пауза", proxy.port(), 1);
    let idle = e.core.create_profile(&form("Стоит", None), 2).unwrap();
    let info = e.core.supervisor.start(&rec.id).await.unwrap();
    down.store(true, std::sync::atomic::Ordering::SeqCst);
    for _ in 0..crate::relay::UNREACHABLE_AFTER {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", info.relay_port)).await.unwrap();
        s.write_all(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n").await.unwrap();
        let head = String::from_utf8(crate::relay::test_support::read_head(&mut s).await).unwrap();
        assert!(head.starts_with("HTTP/1.1 502"), "{head}");
    }
    wait_until("профиль на паузе", || {
        matches!(e.core.supervisor.status(&rec.id), crate::supervisor::ProfileStatus::Paused(_))
    })
    .await;
    let paused = e.core.list_profiles(&ListQueryDto { status: Some(StatusFilter::Paused), ..Default::default() }).unwrap();
    assert_eq!(paused.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), [rec.id.as_str()]);
    assert_eq!(paused.running, 1, "пауза — активный статус");
    let stopped = e.core.list_profiles(&ListQueryDto { status: Some(StatusFilter::Stopped), ..Default::default() }).unwrap();
    assert_eq!(stopped.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), [idle.id.as_str()]);
    e.core.supervisor.stop(&rec.id).await.unwrap();
}

#[tokio::test]
async fn clear_cache_removes_only_cache_folders() {
    let e = env().await;
    let rec = create_with_proxy(&e.core, "a", e.proxy_port, 1);
    let dir = e.core.paths.profile_dir(&rec.id);
    for cache in crate::paths::CACHE_DIRS {
        std::fs::create_dir_all(dir.join(cache)).unwrap();
        std::fs::write(dir.join(cache).join("data"), "x").unwrap();
    }
    let keep = [dir.join("Default/Cookies"), dir.join("Default/Local Storage/leveldb/000003.log"), dir.join("Default/History")];
    for file in &keep {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, "x").unwrap();
    }
    e.core.supervisor.start(&rec.id).await.unwrap();
    assert_eq!(e.core.clear_cache(&rec.id).await.unwrap_err().code, "ProfileRunning");
    assert!(dir.join("Default/Cache/data").exists(), "у работающего профиля ничего не удалено");
    e.core.supervisor.stop(&rec.id).await.unwrap();
    e.core.clear_cache(&rec.id).await.unwrap();
    for cache in crate::paths::CACHE_DIRS {
        assert!(!dir.join(cache).exists(), "{cache}");
    }
    for file in &keep {
        assert!(file.exists(), "{}", file.display());
    }
    e.core.clear_cache(&rec.id).await.unwrap();
    assert_eq!(e.core.clear_cache("nope00").await.unwrap_err().code, "NotFound");
}

#[tokio::test]
async fn form_without_group_fields_keeps_existing_values_on_update() {
    let e = env().await;
    let group = e
        .core
        .create_group(
            &crate::app::groups::GroupForm { name: "Магазины".into(), color: "#0b6e6e".into(), note: String::new(), quick_access: false },
            1,
        )
        .unwrap();
    let other = e
        .core
        .create_group(
            &crate::app::groups::GroupForm { name: "Реклама".into(), color: "#0b6e6e".into(), note: String::new(), quick_access: false },
            2,
        )
        .unwrap();
    let mut f = form("a", None);
    f.group_id = Some(Some(group.id));
    f.restore_tabs = Some(false);
    let rec = e.core.create_profile(&f, 1).unwrap();
    // форма этапа 1: полей нет
    f.group_id = None;
    f.restore_tabs = None;
    let kept = e.core.update_profile(&rec.id, &f).unwrap();
    assert_eq!((kept.group_id, kept.restore_tabs), (Some(group.id), false), "отсутствие поля — оставить как есть");
    f.group_id = Some(Some(other.id));
    assert_eq!(e.core.update_profile(&rec.id, &f).unwrap().group_id, Some(other.id), "явный id переносит");
    f.group_id = Some(None);
    assert_eq!(e.core.update_profile(&rec.id, &f).unwrap().group_id, None, "явный null снимает группу");
    let created = e.core.create_profile(&form("b", None), 2).unwrap();
    assert_eq!((created.group_id, created.restore_tabs), (None, true), "создание без полей");
}

#[tokio::test]
async fn delete_moves_to_trash_and_purge_removes_files() {
    let e = env().await;
    let rec = create_with_proxy(&e.core, "a", e.proxy_port, 1);
    std::fs::create_dir_all(e.core.paths.profile_dir(&rec.id)).unwrap();
    e.core.delete_profile(&rec.id).await.unwrap();
    assert_eq!(e.core.profile_details(&rec.id).unwrap_err().code, "NotFound");
    assert!(e.core.paths.profile_dir(&rec.id).is_dir(), "папка остаётся до окончательного удаления");
    let trash = e.core.list_trash().unwrap();
    assert_eq!(trash.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), [rec.id.as_str()]);
    assert!(e.core.supervisor.start(&rec.id).await.is_err(), "из корзины не запускается");
    let restored = e.core.restore_profiles(std::slice::from_ref(&rec.id)).await;
    assert_eq!(restored.ok, std::slice::from_ref(&rec.id));
    assert!(e.core.profile_details(&rec.id).is_ok());
    e.core.delete_profile(&rec.id).await.unwrap();
    let purged = e.core.purge_profiles(std::slice::from_ref(&rec.id)).await;
    assert_eq!(purged.ok, std::slice::from_ref(&rec.id));
    assert!(!e.core.paths.profile_dir(&rec.id).exists());
    assert!(e.core.store.get_profile_any(&rec.id).unwrap().is_none());
}

#[tokio::test]
async fn restore_into_deleted_group_lands_ungrouped_and_running_cannot_be_trashed() {
    let e = env().await;
    let group = e
        .core
        .store
        .create_group(&crate::store::groups::GroupInput { name: "G".into(), color: "#2447c9".into(), note: String::new(), quick_access: false }, 1)
        .unwrap();
    let rec = create_with_proxy(&e.core, "a", e.proxy_port, 1);
    e.core.store.move_profiles(std::slice::from_ref(&rec.id), Some(group.id)).unwrap();
    e.core.delete_profile(&rec.id).await.unwrap();
    e.core.store.delete_group(group.id).unwrap();
    e.core.restore_profiles(std::slice::from_ref(&rec.id)).await;
    assert_eq!(e.core.store.get_profile(&rec.id).unwrap().unwrap().group_id, None);
    e.core.supervisor.start(&rec.id).await.unwrap();
    assert_eq!(e.core.delete_profile(&rec.id).await.unwrap_err().code, "ProfileRunning");
    let empty = e.core.empty_trash().await.unwrap();
    assert!(empty.ok.is_empty() && empty.failed.is_empty());
}

/// Записи архива `tar.gz` и текст его `profile.json`.
fn archive_entries(path: &std::path::Path) -> (Vec<String>, String) {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(std::fs::File::open(path).unwrap()));
    let (mut names, mut manifest) = (vec![], String::new());
    for entry in tar.entries().unwrap() {
        let mut entry = entry.unwrap();
        let name = entry.path().unwrap().display().to_string();
        if name == "profile.json" {
            std::io::Read::read_to_string(&mut entry, &mut manifest).unwrap();
        }
        names.push(name);
    }
    (names, manifest)
}

/// Очеловечивание ввода в редакторе (этап Э5 движка): настройки сохраняются формой, почерк — из seed профиля,
/// «Сбросить персону» меняет его; форма без настроек их не трогает; неверная горячая клавиша — ошибка поля.
#[tokio::test]
async fn editor_keeps_humanize_and_resets_the_persona() {
    use crate::humanize::{Humanize, Preset};
    let e = env().await;
    let mut f = form("shop", None);
    f.humanize = Some(Humanize { enabled: true, preset: Preset::Careful, block_input: false, paste_key: "Ctrl+Alt+V".into() });
    let rec = e.core.create_profile(&f, 1).unwrap();
    let details = e.core.profile_details(&rec.id).unwrap();
    assert_eq!(details.profile.humanize, f.humanize.clone().unwrap());
    assert_eq!(details.persona, crate::humanize::persona(rec.humanize_seed, details.fingerprint.os));
    let mut keep = form("shop", None);
    keep.humanize = None;
    assert_eq!(e.core.update_profile(&rec.id, &keep).unwrap().humanize, f.humanize.clone().unwrap(), "без поля — как было");
    let reset = e.core.reset_persona(&rec.id).unwrap();
    assert_ne!(reset.profile.humanize_seed, rec.humanize_seed, "новый seed");
    assert_ne!(reset.persona, details.persona, "почерк сменился");
    assert_eq!(reset.profile.humanize, f.humanize.clone().unwrap(), "настройки остались");
    assert_eq!(e.core.reset_persona("nope00").unwrap_err().code, "NotFound");
    let mut bad = form("shop", None);
    bad.humanize = Some(Humanize { paste_key: "V".into(), ..Humanize::default() });
    let err = e.core.update_profile(&rec.id, &bad).unwrap_err();
    let fields: Vec<&str> = err.fields.as_deref().unwrap_or_default().iter().map(|f| f.field).collect();
    assert_eq!((err.code.as_str(), fields), ("Validation", vec!["humanize.paste_key"]), "{err:?}");
}

/// P-01: экспорт и импорт переносят настройки и почерк профиля (seed); архив без них — по умолчанию и новый seed.
#[tokio::test]
async fn archive_carries_humanize_and_the_persona_seed() {
    use crate::humanize::{Humanize, Preset};
    let e = env().await;
    let mut f = form("shop", None);
    f.humanize = Some(Humanize { enabled: false, preset: Preset::Fast, block_input: true, paste_key: String::new() });
    let rec = e.core.create_profile(&f, 1).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("p.tar.gz");
    e.core.export_profile(&rec.id, &path, None).await.unwrap();
    let imported = e.core.import_profile(&path, None).await.unwrap();
    assert_eq!((imported.humanize, imported.humanize_seed), (rec.humanize.clone(), rec.humanize_seed));
    assert_ne!(imported.id, rec.id);
    // Архив до этапа Э5: настройки по умолчанию и новый seed, как у нового профиля; испорченный seed — не архив Ombra.
    let manifest = archive_entries(&path).1;
    let old = tmp.path().join("old.tar.gz");
    forged_archive(&old, &manifest, |m| {
        let m = m.as_object_mut().unwrap();
        m.remove("humanize");
        m.remove("humanize_seed");
    });
    let fresh = e.core.import_profile(&old, None).await.unwrap();
    assert_eq!(fresh.humanize, Humanize::default());
    assert_ne!(fresh.humanize_seed, rec.humanize_seed);
    let broken = tmp.path().join("broken.tar.gz");
    forged_archive(&broken, &manifest, |m| m["humanize_seed"] = serde_json::json!("-1"));
    assert_eq!(e.core.import_profile(&broken, None).await.unwrap_err().code, "ArchiveFormat");
    // Архив более новой сборки с незнакомым пресетом — не «не архив Ombra»: настройки по умолчанию, почерк тот же.
    let newer = tmp.path().join("newer.tar.gz");
    forged_archive(&newer, &manifest, |m| m["humanize"]["preset"] = serde_json::json!("slow"));
    let lenient = e.core.import_profile(&newer, None).await.unwrap();
    assert_eq!((lenient.humanize, lenient.humanize_seed), (Humanize::default(), rec.humanize_seed));
    // Горячая клавиша архива — то же правило, что у формы и Local API: ⌃ и/или ⌥, без ⌘.
    let keyed = tmp.path().join("keyed.tar.gz");
    for key in crate::humanize::refused_keys() {
        forged_archive(&keyed, &manifest, |m| m["humanize"]["paste_key"] = serde_json::json!(key));
        let err = e.core.import_profile(&keyed, None).await.unwrap_err();
        assert!(err.code == "ArchiveFormat" && err.message.contains("humanize.paste_key"), "{key}: {err:?}");
    }
    for key in crate::humanize::ACCEPTED_KEYS {
        forged_archive(&keyed, &manifest, |m| m["humanize"]["paste_key"] = serde_json::json!(key));
        assert_eq!(e.core.import_profile(&keyed, None).await.unwrap().humanize.paste_key, key);
    }
}

#[tokio::test]
async fn archive_carries_the_set_by_name() {
    let e = env().await;
    let shop = e.core.create_ext_category("Магазин", "", 1).unwrap();
    let mut f = form("shop", None);
    f.ext_category_id = Some(shop.id);
    let rec = e.core.create_profile(&f, 1).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("p.tar.gz");
    e.core.export_profile(&rec.id, &path, None).await.unwrap();
    assert_eq!(e.core.import_profile(&path, None).await.unwrap().ext_category_id, shop.id);
    e.core.delete_ext_category(shop.id).unwrap();
    assert_eq!(e.core.import_profile(&path, None).await.unwrap().ext_category_id, crate::store::extensions::DEFAULT_CATEGORY);
}

#[tokio::test]
async fn archive_round_trip_keeps_secrets_only_with_password() {
    use crate::store::secrets::{Secret, SecretField};
    let e = env().await;
    let rec = create_with_proxy(&e.core, "shop", e.proxy_port, 1);
    let mut f = form("shop", Some(rec.proxy_id.unwrap()));
    f.username = "ann".into();
    f.password = SecretInput::Set { value: "p@ssw0rd-x".into() };
    f.tags = Some(vec!["vip".into()]);
    let marks = vec![
        crate::store::bookmarks::Bookmark { title: "Shop".into(), url: "https://shop.test/".into() },
        crate::store::bookmarks::Bookmark { title: "Docs".into(), url: "http://docs.test/".into() },
    ];
    f.bookmarks = Some(marks.clone());
    e.core.update_profile(&rec.id, &f).unwrap();
    let mut input = crate::app::forms::profile_input(&f, Some(&rec));
    input.totp_secret = SecretField::Set(Secret::new("JBSWY3DPEHPK3PXP"));
    e.core.store.update_profile(&rec.id, &input).unwrap();
    let dir = e.core.paths.profile_dir(&rec.id);
    std::fs::create_dir_all(dir.join("Default/Cache")).unwrap();
    std::fs::write(dir.join("Default/Cookies"), b"db").unwrap();
    std::fs::write(dir.join("Default/Cache/junk"), b"x").unwrap();
    std::fs::write(dir.join("SingletonLock"), b"x").unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let with = tmp.path().join("with.tar.gz");
    let without = tmp.path().join("without.tar.gz");
    e.core.export_profile(&rec.id, &with, Some("archive-pw")).await.unwrap();
    e.core.export_profile(&rec.id, &without, None).await.unwrap();

    let leaks = ["p@ssw0rd-x", "JBSWY3DPEHPK3PXP", "\"pass\"", "\"ann\""];
    let (names, plain) = archive_entries(&without);
    assert!(names.contains(&"profile.json".to_string()) && names.contains(&"browser/Default/Cookies".to_string()), "{names:?}");
    assert!(!names.iter().any(|n| n.contains("Cache") || n.contains("Singleton")), "{names:?}");
    assert!(plain.contains(r#""secrets": null"#), "{plain}");
    for leak in leaks {
        assert!(!plain.contains(leak), "{leak} в архиве без пароля");
    }
    let (_, sealed) = archive_entries(&with);
    assert!(sealed.contains(r#""secrets": {"#), "{sealed}");
    for leak in leaks {
        assert!(!sealed.contains(leak), "{leak} открытым текстом в архиве с паролем");
    }

    let imported = e.core.import_profile(&with, Some("archive-pw")).await.unwrap();
    assert_ne!(imported.id, rec.id);
    assert_eq!(imported.name, "shop");
    assert_eq!(imported.username.as_deref(), Some("ann"));
    let (password, totp) = e.core.store.profile_secrets(&imported.id).unwrap();
    assert_eq!(password.unwrap().expose(), "p@ssw0rd-x");
    assert_eq!(totp.unwrap().expose(), "JBSWY3DPEHPK3PXP");
    assert_eq!(e.core.store.profile_tags(&imported.id).unwrap(), ["vip"]);
    assert_eq!(e.core.store.profile_bookmarks(&imported.id).unwrap(), marks, "закладки в архиве");
    assert_eq!(imported.proxy_id, rec.proxy_id, "прокси подобран по строке");
    let new_dir = e.core.paths.profile_dir(&imported.id);
    assert!(new_dir.join("Default/Cookies").is_file());
    assert!(!new_dir.join("Default/Cache").exists(), "кэш не в архиве");

    assert_eq!(e.core.import_profile(&with, Some("wrong")).await.unwrap_err().code, "ArchivePassword");
    assert_eq!(e.core.import_profile(&with, None).await.unwrap_err().code, "ArchivePassword", "архив с секретами без пароля");
    let plain = e.core.import_profile(&without, None).await.unwrap();
    assert!(e.core.store.profile_secrets(&plain.id).unwrap().0.is_none());
    assert_eq!(plain.proxy_id, rec.proxy_id, "без пароля прокси подобран по логину");
    assert_eq!(e.core.import_profile(&tmp.path().join("missing.tar.gz"), None).await.unwrap_err().code, "Io");
    let junk = tmp.path().join("junk.tar.gz");
    std::fs::write(&junk, b"not an archive").unwrap();
    assert_eq!(e.core.import_profile(&junk, None).await.unwrap_err().code, "ArchiveFormat");
    // Архив с настоящим profile.json, но со ссылкой не импортируется и ничего не оставляет.
    let manifest = archive_entries(&without).1;
    let before = (e.core.store.count_profiles().unwrap(), std::fs::read_dir(e.core.paths.profiles()).unwrap().count());
    for (kind, name, target) in [
        (tar::EntryType::Symlink, "browser", ".."),
        (tar::EntryType::Symlink, "browser/Default/x", "/"),
        (tar::EntryType::Link, "browser/Default/y", "profile.json"),
    ] {
        let path = tmp.path().join("linked.tar.gz");
        let gz = flate2::write::GzEncoder::new(std::fs::File::create(&path).unwrap(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        let mut h = tar::Header::new_gnu();
        h.set_size(manifest.len() as u64);
        tar.append_data(&mut h, "profile.json", manifest.as_bytes()).unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(kind);
        h.set_size(0);
        tar.append_link(&mut h, name, target).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        assert_eq!(e.core.import_profile(&path, None).await.unwrap_err().code, "ArchiveFormat", "{name}");
        let after = (e.core.store.count_profiles().unwrap(), std::fs::read_dir(e.core.paths.profiles()).unwrap().count());
        assert_eq!(after, before, "{name}: ни записи, ни папки, ни .tmp");
    }
    let count = e.core.store.count_profiles().unwrap();
    assert_eq!(count, 3, "неудачные импорты не оставили профилей");
    assert_eq!(std::fs::read_dir(e.core.paths.profiles()).unwrap().count(), 3, "и папок");
}

/// C1: в архив не попадает ни строка Ombra из `Login Data` (имя и пароль под публичным ключом), ни журналы SQLite.
#[tokio::test]
async fn archive_scrubs_ombra_login_row() {
    use crate::engine::login_data;
    use std::io::Read;
    let e = env().await;
    let rec = create_with_proxy(&e.core, "shop", e.proxy_port, 1);
    let dir = e.core.paths.profile_dir(&rec.id);
    std::fs::create_dir_all(dir.join("Default")).unwrap();
    let path = dir.join("Default/Login Data");
    login_data::tests::fake_login_data(&path);
    let secret = crate::store::secrets::Secret::new("p@ssw0rd-x");
    assert!(login_data::sync(&dir, "shop.test", Some("ann-ombra-user"), Some(&secret), 1).unwrap());
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "INSERT INTO logins (origin_url, username_element, username_value, signon_realm, date_created, blacklisted_by_user, scheme) \
             VALUES ('https://mine.test/', '', 'own-user', 'https://mine.test/', 0, 0, 0)",
            [],
        )
        .unwrap();
    std::fs::write(dir.join("Default/Login Data-journal"), [0u8; 512]).unwrap(); // обнулённый заголовок: не «горячий» журнал
    let tmp = tempfile::tempdir().unwrap();
    for (name, pw) in [("with.tar.gz", Some("archive-pw")), ("without.tar.gz", None)] {
        let out = tmp.path().join(name);
        e.core.export_profile(&rec.id, &out, pw).await.unwrap();
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(std::fs::File::open(&out).unwrap()));
        let (mut names, mut bytes) = (vec![], vec![]);
        for entry in tar.entries().unwrap() {
            let mut entry = entry.unwrap();
            let n = entry.path().unwrap().display().to_string();
            if n == "browser/Default/Login Data" {
                entry.read_to_end(&mut bytes).unwrap();
            }
            names.push(n);
        }
        assert!(!names.iter().any(|n| n.starts_with("browser/Default/Login Data-")), "{name}: {names:?}");
        assert!(!bytes.is_empty(), "{name}: Login Data в архиве");
        let copy = tmp.path().join(format!("{name}.db"));
        std::fs::write(&copy, &bytes).unwrap();
        let conn = rusqlite::Connection::open(&copy).unwrap();
        let ombra: i64 = conn.query_row("SELECT COUNT(*) FROM logins WHERE username_element = 'ombra'", [], |r| r.get(0)).unwrap();
        let own: i64 = conn.query_row("SELECT COUNT(*) FROM logins WHERE username_value = 'own-user'", [], |r| r.get(0)).unwrap();
        assert_eq!((ombra, own), (0, 1), "{name}");
        let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
        assert!(!has(&login_data::encrypt_password("p@ssw0rd-x")), "{name}: пароль в файле");
        assert!(!has(b"ann-ombra-user"), "{name}: имя в файле");
    }
    // Папка самого профиля не тронута.
    let conn = rusqlite::Connection::open(&path).unwrap();
    let left: i64 = conn.query_row("SELECT COUNT(*) FROM logins WHERE username_element = 'ombra'", [], |r| r.get(0)).unwrap();
    assert_eq!(left, 1);
}

#[tokio::test]
async fn archive_import_creates_missing_proxy_enabled_only_with_password() {
    let e = env().await;
    let rec = create_with_proxy(&e.core, "shop", e.proxy_port, 1);
    let tmp = tempfile::tempdir().unwrap();
    let with = tmp.path().join("with.tar.gz");
    let without = tmp.path().join("without.tar.gz");
    e.core.export_profile(&rec.id, &with, Some("pw")).await.unwrap();
    e.core.export_profile(&rec.id, &without, None).await.unwrap();

    let other = env().await;
    let sealed = other.core.import_profile(&with, Some("pw")).await.unwrap();
    let proxy = other.core.store.get_proxy(sealed.proxy_id.unwrap()).unwrap().unwrap();
    assert!(proxy.enabled && proxy.has_password && proxy.username.as_deref() == Some("user"));

    let other = env().await;
    let open = other.core.import_profile(&without, None).await.unwrap();
    let proxy = other.core.store.get_proxy(open.proxy_id.unwrap()).unwrap().unwrap();
    assert!(!proxy.enabled && !proxy.has_password && proxy.username.as_deref() == Some("user"));
}

#[tokio::test]
async fn csv_round_trip_and_bad_lines_report() {
    let e = env().await;
    let rec = create_with_proxy(&e.core, "shop", e.proxy_port, 1);
    let text = e.core.export_csv(&ListQueryDto::default(), true).unwrap();
    let header = text.lines().next().unwrap();
    assert_eq!(header, "name,group,tags,note,platform_domain,start_urls,username,password,totp,proxy,extra_args");
    assert!(text.lines().nth(1).unwrap().starts_with("shop,"));
    assert!(text.contains(&format!("http://user:pass@127.0.0.1:{}", e.proxy_port)), "{text}");
    let hidden = e.core.export_csv(&ListQueryDto::default(), false).unwrap();
    assert!(!hidden.lines().skip(1).any(|l| l.contains("pass")), "пароль прокси скрыт без with_secrets");
    assert!(hidden.contains(&format!("http://user@127.0.0.1:{}", e.proxy_port)));

    let import = format!(
        "name,group,tags,note,platform_domain,start_urls,username,password,totp,proxy,extra_args\n\
         A,New Group,vip|eu,note,shop.test,https://a.test/|https://b.test/,ann,p@ss,JBSWY3DPEHPK3PXP,http://user:pass@127.0.0.1:{port},--disable-sync\n\
         ,x,,,,,,,,,\n\
         B,,,,,,,,,socks5://bad host,\n",
        port = e.proxy_port
    );
    let report = e.core.import_csv(&import).unwrap();
    assert_eq!(report.created.len(), 1);
    assert_eq!(report.errors.iter().map(|e| e.line).collect::<Vec<_>>(), [3, 4]);
    let a = &report.created[0];
    assert_eq!(a.name, "A");
    assert_eq!(a.proxy_id, rec.proxy_id, "прокси подобран по строке");
    assert_eq!(e.core.store.profile_tags(&a.id).unwrap(), ["eu", "vip"]);
    assert_eq!(a.start_urls.len(), 2);
    assert_eq!(e.core.store.profile_secrets(&a.id).unwrap().0.unwrap().expose(), "p@ss");
    assert!(e.core.store.list_groups().unwrap().iter().any(|g| g.name == "New Group"));

    // Свой же экспорт читается обратно; без шапки, лимита и с одним `name`.
    let again = e.core.import_csv(&text).unwrap();
    assert_eq!((again.created.len(), again.errors.len()), (1, 0));
    assert!(e.core.import_csv("").unwrap().created.is_empty());
    assert!(e.core.import_csv("name\n").unwrap().created.is_empty());
    assert_eq!(e.core.import_csv("note\nx\n").unwrap_err().code, "Validation");
    let many = format!("name\n{}", "x\n".repeat(1001));
    assert_eq!(e.core.import_csv(&many).unwrap_err().code, "ImportFailed");
    let only = e.core.import_csv("name,unknown\nsolo,zzz\n").unwrap();
    assert_eq!(only.created[0].name, "solo");
    // Прокси без пароля — запись выключена.
    let r = e.core.import_csv("name,proxy\np,http://bob@10.9.9.9:8080\n").unwrap();
    let pid = r.created[0].proxy_id.unwrap();
    assert!(!e.core.store.get_proxy(pid).unwrap().unwrap().enabled);
}

#[tokio::test]
async fn csv_secrets_crlf_and_atomic_rejection() {
    let e = env().await;
    let head = "name,group,tags,note,platform_domain,start_urls,username,password,totp,proxy,extra_args";
    let row = "A,Ёлка,t1|t2,n,d.test,https://a.test/|https://b.test/,ann,pw,JBSWY3DPEHPK3PXP,http://a%3Ab%40c%25d:p%3Aq%40r@10.1.1.1:80,--disable-sync";
    let first = e.core.import_csv(&format!("{head}\r\n{row}\r\n,x\r\n")).unwrap();
    assert_eq!(first.errors.iter().map(|x| x.line).collect::<Vec<_>>(), [3], "CRLF: номер строки файла");
    let a = &first.created[0];
    let pid = a.proxy_id.unwrap();
    let creds = e.core.store.proxy_credentials(pid).unwrap();
    assert_eq!((creds.username.as_deref(), creds.password.as_ref().map(|s| s.expose())), (Some("a:b@c%d"), Some("p:q@r")));

    let with = e.core.export_csv(&ListQueryDto::default(), true).unwrap();
    let without = e.core.export_csv(&ListQueryDto::default(), false).unwrap();
    for secret in ["ann", "pw,", "JBSWY3DPEHPK3PXP", "p%3Aq%40r"] {
        assert!(with.contains(secret), "{secret}");
        assert!(!without.contains(secret), "{secret}");
    }
    assert!(without.contains("http://a%3Ab%40c%25d@10.1.1.1:80"));

    // Свой экспорт с секретами читается обратно в те же поля.
    e.core.store.delete_profile(&a.id).unwrap();
    for g in e.core.store.list_groups().unwrap() {
        e.core.store.delete_group(g.id).unwrap();
    }
    let back = e.core.import_csv(&with).unwrap();
    let b = &back.created[0];
    assert_eq!((b.name.as_str(), b.note.as_str(), b.platform_domain.as_str(), b.username.as_deref()), ("A", "n", "d.test", Some("ann")));
    assert_eq!((b.start_urls.len(), b.extra_args.clone(), b.proxy_id), (2, vec!["--disable-sync".to_string()], Some(pid)));
    assert_eq!(e.core.store.profile_tags(&b.id).unwrap(), ["t1", "t2"]);
    assert_eq!(e.core.store.profile_secrets(&b.id).unwrap().1.unwrap().expose(), "JBSWY3DPEHPK3PXP");
    assert_eq!(e.core.store.list_groups().unwrap()[0].name, "Ёлка");

    // Отказ любой строки не оставляет прокси и групп от остальных; лимит срабатывает до записи.
    let (proxies, groups) = (e.core.store.list_proxies().unwrap().len(), e.core.store.list_groups().unwrap().len());
    let bad = format!("name,group,proxy\nok,Новая,http://new:pw@10.2.2.2:1\nbad,{},\nbad2,,http://:pw@h:1\n", "g".repeat(51));
    let r = e.core.import_csv(&bad).unwrap();
    assert_eq!(
        (r.created.len(), r.errors.iter().map(|x| (x.line, x.code)).collect::<Vec<_>>()),
        (1, vec![(3, "group_invalid"), (4, "proxy_invalid")])
    );
    let (proxies, groups) = (proxies + 1, groups + 1);
    let too_many = format!("name,group,proxy\n{}", "z,Лишняя,http://u:p@10.3.3.3:1\n".repeat(1001));
    assert_eq!(e.core.import_csv(&too_many).unwrap_err().code, "ImportFailed");
    assert_eq!((e.core.store.list_proxies().unwrap().len(), e.core.store.list_groups().unwrap().len()), (proxies, groups));
    // `Елка` и `ёлка` — одна группа.
    e.core.import_csv("name,group\nx,Елка\ny,ёлка\n").unwrap();
    assert_eq!(e.core.store.list_groups().unwrap().iter().filter(|g| g.name.to_lowercase().contains("лка")).count(), 1);
}

#[tokio::test]
async fn ipv6_proxy_round_trips_through_csv_and_archive() {
    use crate::store::proxies::{ProxyInput, ProxyKind};
    use crate::store::secrets::SecretField;
    let e = env().await;
    let proxy = e
        .core
        .store
        .create_proxy(&ProxyInput {
            kind: ProxyKind::Socks5,
            host: "2001:db8::1".into(),
            port: 1080,
            username: Some("u".into()),
            password: SecretField::Clear,
            note: String::new(),
            enabled: true,
        })
        .unwrap();
    let rec = e.core.create_profile(&form("v6", Some(proxy.id)), 1).unwrap();
    let csv = e.core.export_csv(&ListQueryDto::default(), false).unwrap();
    assert!(csv.contains("socks5://u@[2001:db8::1]:1080"), "{csv}");
    let again = e.core.import_csv(&csv).unwrap();
    assert_eq!((again.errors.len(), again.created[0].proxy_id), (0, Some(proxy.id)));

    let path = e.core.paths.root().join("v6.tar.gz");
    e.core.export_profile(&rec.id, &path, None).await.unwrap();
    let imported = e.core.import_profile(&path, None).await.unwrap();
    assert_eq!(imported.proxy_id, Some(proxy.id));
}

#[tokio::test]
async fn totp_code_and_key_validation() {
    use crate::store::secrets::{Secret, SecretField};
    let e = env().await;
    let rec = e.core.create_profile(&form("t", None), 1).unwrap();
    let fields = |r: Result<_, CommandError>| -> Vec<(&'static str, &'static str)> {
        r.map(|_: super::totp::TotpCode| ()).unwrap_err().fields.unwrap().iter().map(|f| (f.field, f.code)).collect()
    };
    assert_eq!(fields(e.core.totp_code(&rec.id)), [("totp", "required")]);

    let mut f = form("t", None);
    f.totp = SecretInput::Set { value: "not*base32".into() };
    assert_eq!(e.core.update_profile(&rec.id, &f).unwrap_err().fields.unwrap()[0].code, "invalid");
    assert_eq!(e.core.create_profile(&f, 1).unwrap_err().fields.unwrap()[0].field, "totp");
    f.totp = SecretInput::Set { value: "jbsw y3dp ehpk 3pxp".into() };
    e.core.update_profile(&rec.id, &f).unwrap();
    let t = e.core.totp_code(&rec.id).unwrap();
    assert_eq!(t.code.len(), 6);
    assert!((1..=30).contains(&t.seconds_left));

    // Испорченный ключ в базе — `invalid`, а не паника.
    let mut input = crate::app::forms::profile_input(&form("t", None), Some(&rec));
    input.totp_secret = SecretField::Set(Secret::new("1!"));
    e.core.store.update_profile(&rec.id, &input).unwrap();
    assert_eq!(fields(e.core.totp_code(&rec.id)), [("totp", "invalid")]);

    // CSV и архив с плохим ключом.
    let csv = "name,totp\nA,JBSWY3DPEHPK3PXP\nB,1!\n";
    let report = e.core.import_csv(csv).unwrap();
    assert_eq!(report.created.len(), 1);
    assert_eq!((report.errors[0].line, report.errors[0].code), (3, "totp_invalid"));
    let path = e.core.paths.root().join("bad-totp.tar.gz");
    e.core.export_profile(&rec.id, &path, Some("pw")).await.unwrap();
    let bad = e.core.import_profile(&path, Some("pw")).await.unwrap_err();
    assert_eq!(bad.code, "ArchiveFormat");
    assert!(bad.message.contains("totp"), "{}", bad.message);

    // Корзина: код 2FA недоступен.
    let mut input = crate::app::forms::profile_input(&form("t", None), Some(&rec));
    input.totp_secret = SecretField::Set(Secret::new("JBSWY3DPEHPK3PXP"));
    e.core.store.update_profile(&rec.id, &input).unwrap();
    assert!(e.core.totp_code(&rec.id).is_ok());
    e.core.delete_profile(&rec.id).await.unwrap();
    assert_eq!(e.core.totp_code(&rec.id).unwrap_err().code, "NotFound");
}

#[tokio::test]
async fn clone_copies_bookmarks() {
    let e = env().await;
    let mut f = form("src", None);
    let marks = vec![crate::store::bookmarks::Bookmark { title: "Shop".into(), url: "https://shop.test/".into() }];
    f.bookmarks = Some(marks.clone());
    let rec = e.core.create_profile(&f, 1).unwrap();
    let copy = e.core.clone_profile(&rec.id, 2).unwrap();
    assert_eq!(e.core.store.profile_bookmarks(&copy.id).unwrap(), marks);
}

/// Архив с настоящим `profile.json`, правленным `edit`, и без папки браузера.
fn forged_archive(path: &std::path::Path, manifest: &str, edit: impl FnOnce(&mut serde_json::Value)) {
    let mut json: serde_json::Value = serde_json::from_str(manifest).unwrap();
    edit(&mut json);
    let bytes = serde_json::to_vec(&json).unwrap();
    let gz = flate2::write::GzEncoder::new(std::fs::File::create(path).unwrap(), flate2::Compression::fast());
    let mut tar = tar::Builder::new(gz);
    let mut h = tar::Header::new_gnu();
    h.set_size(bytes.len() as u64);
    tar.append_data(&mut h, "profile.json", bytes.as_slice()).unwrap();
    tar.into_inner().unwrap().finish().unwrap();
}

#[tokio::test]
async fn archive_import_reports_arch_and_group_errors_before_any_write() {
    let e = env().await;
    let rec = e.core.create_profile(&form("a", None), 1).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("a.tar.gz");
    e.core.export_profile(&rec.id, &out, None).await.unwrap();
    let manifest = archive_entries(&out).1;
    let before = (e.core.store.count_profiles().unwrap(), e.core.store.list_groups().unwrap().len());

    // Отпечаток другой архитектуры: ошибка проверки с полем, а не «не архив Ombra».
    let foreign = tmp.path().join("foreign.tar.gz");
    forged_archive(&foreign, &manifest, |m| {
        m["fingerprint"]["gpu"] = if cfg!(target_arch = "aarch64") {
            serde_json::json!({"vendor": "Google Inc. (Intel)", "renderer": "ANGLE (Intel, Intel(R) Iris(TM) Plus Graphics 655)"})
        } else {
            serde_json::json!({"vendor": "Google Inc. (Apple)", "renderer": "ANGLE (Apple, ANGLE Metal Renderer: Apple M3, Unspecified Version)"})
        };
    });
    let err = e.core.import_profile(&foreign, None).await.unwrap_err();
    assert_eq!(err.code, "Validation", "{err:?}");
    assert_eq!(err.fields.unwrap()[0].field, "gpu");

    // Слишком длинное имя группы — до записи группы и профиля.
    let long = tmp.path().join("long.tar.gz");
    forged_archive(&long, &manifest, |m| m["group"] = serde_json::Value::String("g".repeat(51)));
    let err = e.core.import_profile(&long, None).await.unwrap_err();
    assert_eq!(err.code, "ArchiveFormat");
    assert!(err.message.contains("group"), "{}", err.message);
    assert_eq!((e.core.store.count_profiles().unwrap(), e.core.store.list_groups().unwrap().len()), before);
}


/// Профиль, чью папку держит браузер другого процесса Ombra (`ombra flow` рядом с окном), окно не удаляет и не чистит.
#[tokio::test]
async fn profile_held_by_another_process_is_not_deleted_or_cleared() {
    let e = env().await;
    let rec = create_with_proxy(&e.core, "a", e.proxy_port, 1);
    let dir = e.core.paths.profile_dir(&rec.id);
    let ids = [rec.id.clone()];
    let foreign = crate::lock::DirLock::acquire(&dir).unwrap();
    assert_eq!(e.core.delete_profile(&rec.id).await.unwrap_err().code, "ProfileLocked");
    assert_eq!(e.core.delete_profiles_atomic(&ids).await.unwrap_err().code, "ProfileLocked");
    assert_eq!(e.core.clear_cache(&rec.id).await.unwrap_err().code, "ProfileLocked");
    assert_eq!(e.core.clear_cookies(&rec.id).await.unwrap_err().code, "ProfileLocked");
    drop(foreign);
    e.core.delete_profile(&rec.id).await.unwrap();
    let foreign = crate::lock::DirLock::acquire(&dir).unwrap();
    let report = e.core.purge_profiles(&ids).await;
    assert_eq!(report.failed.iter().map(|f| f.error.code.as_str()).collect::<Vec<_>>(), ["ProfileLocked"]);
    assert!(dir.join(crate::lock::LOCK_FILE).is_file(), "папка под чужим браузером на месте");
    drop(foreign);
    assert_eq!(e.core.purge_profiles(&ids).await.ok, ids);
    assert!(!dir.exists());
}

#[tokio::test]
async fn trash_rows_carry_deleted_at_and_restore_waits_for_idle_lock() {
    let e = env().await;
    let rec = e.core.create_profile(&form("a", None), 1).unwrap();
    e.core.delete_profile(&rec.id).await.unwrap();
    assert!(e.core.list_trash().unwrap()[0].deleted_at.is_some());
    assert!(e.core.list_profiles(&ListQueryDto::default()).unwrap().rows.is_empty());
    let guard = e.core.supervisor.lock_idle(&rec.id).await.unwrap();
    let ids = [rec.id.clone()];
    let waiting = tokio::time::timeout(Duration::from_millis(150), e.core.restore_profiles(&ids)).await;
    assert!(waiting.is_err(), "восстановление ждёт замок простоя");
    drop(guard);
    assert_eq!(e.core.restore_profiles(&ids).await.ok, ids);
}

#[tokio::test]
async fn csv_files_round_trip_and_reject_big_or_broken_files() {
    let e = env().await;
    e.core.create_profile(&form("shop", None), 1).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.csv");
    assert_eq!(e.core.export_csv_file(&ListQueryDto::default(), false, &path).unwrap(), 1);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("name,"), "{text}");
    // Таблица из Excel: BOM и CRLF.
    let excel = dir.path().join("excel.csv");
    std::fs::write(&excel, format!("\u{feff}{}", text.replace('\n', "\r\n"))).unwrap();
    assert_eq!(e.core.import_csv_file(&excel).unwrap().created.len(), 1);
    let big = dir.path().join("big.csv");
    std::fs::write(&big, vec![b'a'; 10 * 1024 * 1024 + 1]).unwrap();
    assert_eq!(e.core.import_csv_file(&big).unwrap_err().code, "ImportFailed");
    let latin = dir.path().join("latin.csv");
    std::fs::write(&latin, b"name\n\xff\xfe\n").unwrap();
    assert_eq!(e.core.import_csv_file(&latin).unwrap_err().code, "ImportFailed");
    assert_eq!(e.core.import_csv_file(&dir.path().join("missing.csv")).unwrap_err().code, "Io");
    // Папки нет — ошибка, временных файлов не остаётся.
    assert!(e.core.export_csv_file(&ListQueryDto::default(), false, &dir.path().join("nope/out.csv")).is_err());
    let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert!(left.iter().all(|n| !n.to_string_lossy().starts_with(".ombra-export-")), "{left:?}");
}
