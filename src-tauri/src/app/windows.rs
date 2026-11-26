//! Расстановка окон работающих профилей по сетке (спецификация 3А, «Сетка окон»).
use super::{CommandError, Core};
use super::bulk::BulkReport;
use crate::engine::cdp::{Bounds, CdpClient};
use crate::engine::Engine;
use futures_util::future::join_all;

/// Рабочая область главного монитора в логических пикселях (без строки меню и дока).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenArea {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Сетка на `n` окон: `ceil(sqrt n)` колонок, ряды по `ceil(n / cols)`; остаток целочисленного деления
/// достаётся последней колонке и последнему ряду. Ячейки идут слева направо, сверху вниз.
pub fn grid(n: usize, area: &ScreenArea) -> Vec<Bounds> {
    if n == 0 {
        return vec![];
    }
    let cols = (1..=n).find(|c| c * c >= n).unwrap_or(1);
    let rows = n.div_ceil(cols);
    let (cell_w, cell_h) = (area.width / cols as u32, area.height / rows as u32);
    (0..n)
        .map(|i| {
            let (col, row) = ((i % cols) as u32, (i / cols) as u32);
            let last_col = col as usize == cols - 1;
            let last_row = row as usize == rows - 1;
            Bounds {
                left: area.x + (col * cell_w) as i32,
                top: area.y + (row * cell_h) as i32,
                width: if last_col { area.width - cell_w * (cols as u32 - 1) } else { cell_w },
                height: if last_row { area.height - cell_h * (rows as u32 - 1) } else { cell_h },
            }
        })
        .collect()
}

impl<E: Engine> Core<E> {
    /// Окна всех работающих профилей — по сетке, в порядке serial. Профиль, чей CDP не ответил, — в `failed` с `Cdp`.
    /// Профили обрабатываются одновременно: каждому не дольше нескольких вызовов по `CALL_TIMEOUT`.
    pub async fn arrange_windows(&self, area: &ScreenArea) -> BulkReport {
        let mut running: Vec<(i64, String, String)> = self
            .supervisor
            .running()
            .into_iter()
            .map(|(id, info)| {
                let serial = self.store.get_profile(&id).ok().flatten().map_or(i64::MAX, |p| p.serial);
                (serial, id, info.cdp_ws)
            })
            .collect();
        running.sort();
        let cells = grid(running.len(), area);
        let results = join_all(running.into_iter().zip(cells).map(|((_, id, ws), cell)| async move {
            let result = place(&ws, &cell).await.map_err(|e| CommandError::new("Cdp", e.to_string()));
            (id, result)
        }))
        .await;
        let mut report = BulkReport::default();
        for (id, result) in results {
            report.push(id, result);
        }
        report
    }
}

async fn place(ws: &str, cell: &Bounds) -> Result<(), crate::engine::cdp::CdpError> {
    let mut cdp = CdpClient::connect(ws).await?;
    let window = cdp.window_for_first_page().await?;
    cdp.set_window_bounds(window, cell).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{create_with_proxy, env};

    fn cover(n: usize, area: &ScreenArea) -> Vec<Bounds> {
        let cells = grid(n, area);
        assert_eq!(cells.len(), n);
        let covered: u64 = cells.iter().map(|c| u64::from(c.width) * u64::from(c.height)).sum();
        if n > 0 {
            let full = [1usize, 2, 4, 6, 9, 12, 20].contains(&n);
            if full {
                assert_eq!(covered, u64::from(area.width) * u64::from(area.height), "n={n}: ячейки покрывают область");
            }
            for (i, a) in cells.iter().enumerate() {
                assert!(a.left >= area.x && a.top >= area.y);
                assert!(a.left + a.width as i32 <= area.x + area.width as i32);
                assert!(a.top + a.height as i32 <= area.y + area.height as i32);
                for b in &cells[i + 1..] {
                    let apart = a.left + a.width as i32 <= b.left
                        || b.left + b.width as i32 <= a.left
                        || a.top + a.height as i32 <= b.top
                        || b.top + b.height as i32 <= a.top;
                    assert!(apart, "n={n}: {a:?} и {b:?} пересекаются");
                }
            }
        }
        cells
    }

    #[test]
    fn grid_splits_the_work_area() {
        let area = ScreenArea { x: 0, y: 25, width: 1440, height: 875 };
        assert!(grid(0, &area).is_empty());
        assert_eq!(cover(1, &area), [Bounds { left: 0, top: 25, width: 1440, height: 875 }]);
        let two = cover(2, &area);
        assert_eq!((two[0].width, two[1].left), (720, 720), "2 колонки в один ряд");
        assert_eq!(two[0].height, 875);
        let five = cover(5, &area);
        assert_eq!(five[0].width, 480, "3 колонки");
        assert_eq!(five[3].top, 25 + 437, "второй ряд");
        assert_eq!(five[3].height, 438, "последний ряд берёт остаток");
        let odd = ScreenArea { x: 10, y: 0, width: 1001, height: 503 };
        let twenty = cover(20, &odd);
        assert_eq!(twenty[19].left, 10 + 4 * 200, "5 колонок");
        assert_eq!(twenty[4].width, 1001 - 4 * 200, "последняя колонка берёт остаток");
    }

    #[tokio::test]
    async fn arrange_windows_sets_cells_and_reports_unreachable() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "a", e.proxy_port, 1);
        let b = create_with_proxy(&e.core, "b", e.proxy_port, 2);
        e.core.supervisor.start(&a.id).await.unwrap();
        e.core.supervisor.start(&b.id).await.unwrap();
        for id in [&a.id, &b.id] {
            e.core.engine.cdp(&e.core.paths.profile_dir(id)).add_page("about:blank");
        }
        let area = ScreenArea { x: 0, y: 25, width: 1440, height: 875 };
        let cells = grid(2, &area);

        let report = e.core.arrange_windows(&area).await;
        assert_eq!(report.ok, [a.id.clone(), b.id.clone()], "порядок — по serial");
        assert!(report.failed.is_empty(), "{report:?}");
        assert_eq!(e.core.engine.cdp(&e.core.paths.profile_dir(&a.id)).bounds(), Some(cells[0]));
        assert_eq!(e.core.engine.cdp(&e.core.paths.profile_dir(&b.id)).bounds(), Some(cells[1]));

        let first = e.core.engine.cdp(&e.core.paths.profile_dir(&a.id));
        first.set_window_state("maximized");
        e.core.arrange_windows(&area).await;
        assert_eq!(first.window_state(), "normal", "развёрнутое окно сначала становится обычным");
        assert_eq!(first.bounds(), Some(cells[0]), "и получает ячейку");

        e.core.engine.cdp(&e.core.paths.profile_dir(&b.id)).go_down();
        let report = e.core.arrange_windows(&area).await;
        assert_eq!(report.ok, std::slice::from_ref(&a.id));
        assert_eq!((report.failed[0].id.as_str(), report.failed[0].error.code.as_str()), (b.id.as_str(), "Cdp"));
        assert_eq!(e.core.engine.cdp(&e.core.paths.profile_dir(&a.id)).bounds(), Some(cells[0]), "остальные расставлены");
        e.core.supervisor.stop_all().await;
    }

    /// Два настоящих окна встают в ячейки сетки: границы по CDP равны ячейкам. Окна видимые — проверяется геометрия.
    #[tokio::test]
    #[ignore = "запускает настоящий браузер; cargo test --lib live_arrange_windows -- --ignored --nocapture"]
    async fn live_arrange_windows() {
        use crate::engine::ombra::OmbraEngine;
        use crate::paths::Paths;
        use std::time::Duration;
        let slot = std::env::var_os("OMBRA_PROBE_ENGINES")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| Paths::new(Paths::default_root()).engines());
        let engine = OmbraEngine::new(slot);
        engine.installed().expect("движок не импортирован");
        let dir = tempfile::tempdir().unwrap();
        let area = ScreenArea { x: 0, y: 40, width: 1200, height: 700 };
        let cells = grid(2, &area);
        let mut browsers = Vec::new();
        for i in 0..2 {
            let mut req = crate::engine::ombra::flags::tests::request();
            req.profile_dir = dir.path().join(format!("p{i}"));
            req.log_path = dir.path().join(format!("chromium{i}.log"));
            req.headless = false;
            req.extra_args = vec![];
            browsers.push(engine.launch(engine.build_command(&req).unwrap(), crate::engine::LaunchHooks::none()).await.unwrap());
        }
        let mut seen = Vec::new();
        for (browser, cell) in browsers.iter().zip(&cells) {
            place(&browser.ws_endpoint, cell).await.unwrap();
            let mut cdp = CdpClient::connect(&browser.ws_endpoint).await.unwrap();
            let window = cdp.window_for_first_page().await.unwrap();
            let reply = cdp.send("Browser.getWindowBounds", serde_json::json!({ "windowId": window })).await.unwrap();
            seen.push(reply["bounds"].clone());
        }
        for mut browser in browsers {
            engine.stop(&mut browser, Duration::from_secs(10)).await.unwrap();
        }
        println!("ОКНА: {seen:?}; ЯЧЕЙКИ: {cells:?}");
        for (bounds, cell) in seen.iter().zip(&cells) {
            assert_eq!(
                (bounds["left"].as_i64(), bounds["top"].as_i64(), bounds["width"].as_u64(), bounds["height"].as_u64()),
                (Some(i64::from(cell.left)), Some(i64::from(cell.top)), Some(u64::from(cell.width)), Some(u64::from(cell.height))),
                "окно не в своей ячейке",
            );
        }
    }
}
