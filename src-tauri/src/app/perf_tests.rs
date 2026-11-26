//! Скорость ядра на объёмах приёмки этапа 2 (release, `#[ignore]`):
//! - список на 1000 профилях — 50 мс (критерий этапа 2 «список из 1000 не тормозит»);
//! - импорт 5000 строк прокси — 2 с, `cookie_stats` для 200 профилей по 50 cookies — 1 с (приёмка 2Г).
//!
//! Замер имеет смысл только в release: `cargo test --release --lib perf -- --ignored --nocapture --test-threads=1`.
//! Каждый тест печатает строку `ACCEPTANCE <замер> ms=<медиана> limit_ms=<порог>` — её берёт сводка прогона приёмки.
use super::Core;
use super::bulk::{BulkCreateForm, ProxyChoice};
use super::groups::GroupForm;
use super::profiles::ListQueryDto;
use crate::store::profiles::SortKey;
use super::tests::env;
use crate::cookies::tests::cookie;
use crate::paths::Paths;
use crate::relay::geo::GeoClient;
use crate::store::Store;
use crate::store::proxies::ProxyKind;
use crate::store::tests::test_box;
use crate::supervisor::test_engine::FakeEngine;
use crate::supervisor::{Event, StatusFilter, Timings};
use std::sync::Arc;
use std::time::{Duration, Instant};

const LIMIT: Duration = Duration::from_millis(50);
const RUNS: usize = 20;

/// Импорт строками: 5000 строк — не дольше 2 с.
const PROXY_LINES: usize = 5000;
const IMPORT_LIMIT: Duration = Duration::from_secs(2);
/// `cookie_stats` пачки: 200 профилей по 50 cookies — не дольше 1 с.
const COOKIE_PROFILES: u32 = 200;
const COOKIES_EACH: usize = 50;
const COOKIE_STATS_LIMIT: Duration = Duration::from_secs(1);
/// Прогонов тяжёлых замеров; итог — медиана.
const HEAVY_RUNS: usize = 5;

/// Строка для сводки прогона приёмки (`tools/acceptance/run.sh`).
fn report(name: &str, median: Duration, limit: Duration) {
    println!("ACCEPTANCE {name} ms={:.1} limit_ms={}", median.as_secs_f64() * 1000.0, limit.as_millis());
}

/// Медиана и худшее из замеров.
fn median_and_worst(mut times: Vec<Duration>) -> (Duration, Duration) {
    times.sort();
    (times[times.len() / 2], times[times.len() - 1])
}

/// Ядро над базой в файле, как в приложении (WAL на диске): в памяти не видна цена фиксации транзакций.
fn file_core() -> (Core<FakeEngine>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let store = Arc::new(Store::open(&paths.db(), test_box()).unwrap());
    let core = Core::new(
        paths,
        store,
        Arc::new(FakeEngine::new()),
        Arc::new(GeoClient::new()),
        Arc::new(|_: Event| {}),
        Timings::default(),
    );
    (core, dir)
}

#[tokio::test]
#[ignore = "замер скорости; cargo test --release --lib perf -- --ignored --nocapture --test-threads=1"]
async fn list_of_1000_profiles_answers_within_50_ms() {
    let e = env().await;
    let mut groups = Vec::new();
    for i in 0..5 {
        let form = GroupForm { name: format!("Группа {i}"), color: "#2447c9".into(), note: String::new(), quick_access: i == 0 };
        groups.push(e.core.create_group(&form, 1).unwrap().id);
    }
    // Пять партий по 200: четыре в группах, последняя «Без группы»; у каждой партии два своих тега (всего 10).
    for batch in 0..5_usize {
        let form = BulkCreateForm {
            count: 200,
            prefix: format!("Магазин {batch}"),
            group_id: groups[..4].get(batch).copied(),
            tags: vec![format!("тег {}", batch * 2), format!("тег {}", batch * 2 + 1)],
            note: format!("партия {batch}"),
            start_urls: vec![],
            restore_tabs: true,
            extra_args: vec![],
            proxy: ProxyChoice::Random,
        };
        e.core.create_profiles_bulk(&form, 1).unwrap();
    }
    assert_eq!(e.core.store.count_profiles().unwrap(), 1000);

    let queries = [
        ("без фильтра", ListQueryDto::default()),
        ("поиск «01»", ListQueryDto { search: "01".into(), ..Default::default() }),
        ("имя по убыванию", ListQueryDto { sort: SortKey::Name, descending: true, ..Default::default() }),
        ("группа", ListQueryDto { group_id: Some(groups[0]), ..Default::default() }),
        ("без группы", ListQueryDto { ungrouped: true, ..Default::default() }),
        ("тег", ListQueryDto { tag: Some("тег 3".into()), ..Default::default() }),
        ("статус «Остановлен»", ListQueryDto { status: Some(StatusFilter::Stopped), ..Default::default() }),
    ];
    let mut slow = Vec::new();
    let mut worst = Duration::ZERO;
    for (label, query) in &queries {
        let mut times: Vec<Duration> = (0..RUNS)
            .map(|_| {
                let started = Instant::now();
                let list = e.core.list_profiles(query).unwrap();
                let elapsed = started.elapsed();
                assert!(!list.rows.is_empty(), "{label}: пустой список");
                elapsed
            })
            .collect();
        times.sort();
        let median = times[RUNS / 2];
        println!("{label}: медиана {median:?}, худший {:?}", times[RUNS - 1]);
        worst = worst.max(median);
        if median > LIMIT {
            slow.push(format!("{label}: {median:?}"));
        }
    }
    // В сводку — худшая из медиан семи запросов.
    report("list_1000_profiles", worst, LIMIT);
    assert!(slow.is_empty(), "медленнее {LIMIT:?}: {slow:?}");
}

/// 5000 разных строк в обоих форматах импорта, у каждой логин и пароль: пароли шифруются, как в жизни.
fn proxy_lines() -> String {
    (0..PROXY_LINES)
        .map(|i| {
            if i % 2 == 0 {
                format!("socks5://user{i}:pass{i}@gw{i}.example.net:1080\n")
            } else {
                format!("gw{i}.example.net:3128:user{i}:pass{i}\n")
            }
        })
        .collect()
}

#[test]
#[ignore = "замер скорости; cargo test --release --lib perf -- --ignored --nocapture --test-threads=1"]
fn import_5000_proxies_within_2_s() {
    let text = proxy_lines();
    // Каждый прогон — в новую базу: повторный импорт того же текста дал бы одни дубликаты.
    let times = (0..HEAVY_RUNS)
        .map(|_| {
            let (core, _dir) = file_core();
            let started = Instant::now();
            let imported = core.import_proxies(&text, ProxyKind::Http).unwrap();
            let elapsed = started.elapsed();
            assert_eq!((imported.added, imported.duplicates, imported.errors.len()), (PROXY_LINES, 0, 0));
            elapsed
        })
        .collect();
    let (median, worst) = median_and_worst(times);
    println!("импорт {PROXY_LINES} строк прокси: медиана {median:?}, худший {worst:?}");
    report("import_5000_proxies", median, IMPORT_LIMIT);
    assert!(median <= IMPORT_LIMIT, "медленнее {IMPORT_LIMIT:?}: {median:?}");
}

#[test]
#[ignore = "замер скорости; cargo test --release --lib perf -- --ignored --nocapture --test-threads=1"]
fn cookie_stats_for_200_profiles_within_1_s() {
    let (core, _dir) = file_core();
    let form = BulkCreateForm {
        count: COOKIE_PROFILES,
        prefix: "Cookies".into(),
        group_id: None,
        tags: vec![],
        note: String::new(),
        start_urls: vec![],
        restore_tabs: true,
        extra_args: vec![],
        proxy: ProxyChoice::Random,
    };
    let ids: Vec<String> = core.create_profiles_bulk(&form, 1).unwrap().into_iter().map(|p| p.id).collect();
    for (n, id) in ids.iter().enumerate() {
        let cookies: Vec<_> = (0..COOKIES_EACH)
            .map(|k| cookie(&format!(".shop{}.example", k % 5), &format!("c{k}"), &format!("значение {n}-{k}")))
            .collect();
        core.store.merge_cookies(id, &cookies).unwrap();
    }
    let times = (0..HEAVY_RUNS)
        .map(|_| {
            let started = Instant::now();
            // Как диалог экспорта cookies: по вызову на профиль.
            let total: i64 = ids.iter().map(|id| core.cookie_stats(id).unwrap().total).sum();
            let elapsed = started.elapsed();
            assert_eq!(total, (ids.len() * COOKIES_EACH) as i64);
            elapsed
        })
        .collect();
    let (median, worst) = median_and_worst(times);
    println!("cookie_stats, {COOKIE_PROFILES} профилей по {COOKIES_EACH}: медиана {median:?}, худший {worst:?}");
    report("cookie_stats_200_profiles", median, COOKIE_STATS_LIMIT);
    assert!(median <= COOKIE_STATS_LIMIT, "медленнее {COOKIE_STATS_LIMIT:?}: {median:?}");
}
