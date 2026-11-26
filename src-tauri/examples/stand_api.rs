//! Local API Ombra на своей папке данных — для сквозного сценария стенда движка (`ombra-engine`, этап Э5:
//! `qa/tests/app/test_humanize_app.py`): стенд создаёт профиль через Local API, запускает его и подключается к выданному
//! WebSocket, как клиент AdsPower.
//!
//!     cargo run --example stand_api -- <папка данных>
//!
//! Движок — `<папка данных>/engines/Chromium.app` (ссылку на проверяемую сборку кладёт стенд); журналы очеловечивания —
//! в каталоге `OMBRA_HUMANIZE_DEBUG`. Геоданные не запрашиваются: у профилей стенда таймзона и язык заданы вручную.
//! В stdout — одна строка с адресом Local API; работает, пока открыт stdin, и останавливает профили на выходе.
use ombra_lib::api::{ApiSettings, serve};
use ombra_lib::app::open;
use ombra_lib::paths::Paths;
use ombra_lib::relay::geo::GeoClient;
use std::io::Read;
use std::sync::Arc;

fn main() {
    let root = std::env::args().nth(1).expect("папка данных первым аргументом");
    let core = Arc::new(open(Paths::new(root), Arc::new(|_| {}), GeoClient::with_endpoints(vec![])).expect("ядро не открылось"));
    let runtime = tokio::runtime::Runtime::new().expect("tokio");
    runtime.block_on(async {
        let server = serve(core.clone(), ApiSettings { port: 0, key: None }).await.expect("Local API не поднялся");
        println!("http://{}", server.addr());
        let _ = tokio::task::spawn_blocking(|| std::io::stdin().read_to_end(&mut Vec::new())).await;
        core.supervisor.stop_all().await;
        server.shutdown();
    });
}
