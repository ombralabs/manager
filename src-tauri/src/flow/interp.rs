//! Интерпретатор процесса: обход графа, условия, циклы, группы, подпроцессы, Manage Variables, политика ошибок
//! (спецификация автоматизации, «Обход графа» и «Ошибки»).
use crate::flow::host::{Host, ProfileDraft};
use crate::flow::journal::{Journal, Status};
use crate::flow::model::{ErrorPolicy, Flow, Graph, Node, NodePolicy, VarType};
use crate::flow::ops::web::Tabs;
use crate::flow::ops::{self, Cfg, OpError, OpSpec};
use crate::flow::vars::{self, Scope};
use futures_util::future::BoxFuture;
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

/// Процесс по id — для `control.call`: `subflows` файла, затем база.
pub type Lookup = Box<dyn Fn(i64) -> Option<Flow> + Send + Sync>;

/// Сигнал начала узла (id узла).
pub type OnNode = Arc<dyn Fn(&str) + Send + Sync>;

/// Куда идёт управление после узла.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Port(&'static str),
    Break,
    Quit,
    Export(Map<String, Value>),
}

/// Чем кончилась цепочка узлов.
#[derive(Debug, Clone, PartialEq)]
pub enum Exit {
    Done,
    Break,
    Quit,
    Export(Map<String, Value>),
}

/// Запуск прерван.
#[derive(Debug, Clone, PartialEq)]
pub enum Fail {
    /// Ошибка узла с политикой `interrupt`.
    Node { node: String, message: String },
    /// Ошибку перехватывает ближайшая группа или вызов с политикой `branch`.
    Catch { node: String, message: String },
    Cancelled,
    Timeout,
    BrowserClosed,
}

/// Итерации цикла по одной: (элемент, индекс).
type LoopItems = Box<dyn Iterator<Item = (Value, Value)> + Send>;

/// Политика, унаследованная от группы или вызова.
#[derive(Debug, Clone)]
enum Inherited {
    Interrupt,
    Skip,
    Catch,
}

/// Ошибка обработчика узла: своя (её решает политика) или прерывание изнутри (идёт наверх как есть).
enum NodeError {
    Op(OpError),
    Run(Fail),
    /// Ошибку внутри группы или вызова с `branch` перехватил сам этот узел.
    Caught(String),
}

impl From<OpError> for NodeError {
    fn from(error: OpError) -> Self {
        NodeError::Op(error)
    }
}

impl From<String> for NodeError {
    fn from(message: String) -> Self {
        NodeError::Op(OpError::Failed(message))
    }
}

/// Состояние одного запуска.
pub struct Ctx {
    pub(crate) scope: Scope,
    /// Браузер запуска; `None` — без браузера (тесты интерпретатора).
    pub(crate) tabs: Option<Tabs>,
    pub(crate) journal: Journal,
    pub(crate) cancel: CancellationToken,
    lookup: Lookup,
    base: ErrorPolicy,
    policies: Vec<Inherited>,
    /// Процессы, вызванные сейчас друг из друга (`control.call`): защита от рекурсии.
    calls: Vec<i64>,
    /// Номер профиля в задаче (с 0): Input «In order» берёт строку с этим номером.
    pub(crate) position: usize,
    /// Очеловечивание ввода профиля включено: текст уходит движку одним `insertText`, темп задаёт он.
    pub(crate) humanize: bool,
    /// Папка файлов запуска: снимки по умолчанию.
    pub(crate) files: PathBuf,
    /// Приложение вокруг запуска; `None` — тесты без него.
    pub(crate) host: Option<Arc<dyn Host>>,
    /// Хранилище приложения: «Прокси для сервисов» для Email; `None` — тесты без него.
    pub(crate) store: Option<Arc<crate::store::Store>>,
    /// Изменения заметки и тегов основного профиля: запишутся в конце запуска.
    pub(crate) profile: ProfileDraft,
    /// Слушатели запросов запуска (Listener Request / Response).
    pub(crate) listeners: crate::flow::ops::net::Listeners,
    /// Сеть, включённая перед прошлым узлом для Request to finish; узел ожидания забирает её.
    pub(crate) armed: Option<crate::flow::ops::net::Armed>,
    /// Вызывается с id узла перед его выполнением (в том числе выключенного) — подсветка хода отладки.
    pub on_node: Option<OnNode>,
}

impl Ctx {
    /// Глубина вызовов подпроцессов (`control.call`).
    pub(crate) fn depth(&self) -> usize {
        self.calls.len()
    }

    pub fn new(scope: Scope, base: ErrorPolicy, lookup: Lookup, journal: Journal, cancel: CancellationToken) -> Self {
        Self {
            scope,
            tabs: None,
            journal,
            cancel,
            lookup,
            base,
            policies: Vec::new(),
            calls: Vec::new(),
            position: 0,
            humanize: false,
            files: PathBuf::new(),
            host: None,
            store: None,
            profile: ProfileDraft::default(),
            listeners: Default::default(),
            armed: None,
            on_node: None,
        }
    }

    /// Запуск без браузера и подпроцессов — для тестов операций.
    #[cfg(test)]
    pub(crate) fn bare() -> Self {
        Self::new(
            Scope::new(Default::default()),
            ErrorPolicy::Interrupt,
            Box::new(|_| None),
            Journal::memory(),
            CancellationToken::new(),
        )
    }

    /// Весь процесс от узла `start`.
    pub async fn run(&mut self, flow: &Flow) -> Result<Exit, Fail> {
        let graph = flow.graph();
        let Some(start) = graph.nodes.iter().find(|node| node.op == "start") else { return Ok(Exit::Done) };
        self.journal.node(start, Status::Ok, 0, "");
        let first = graph.next(&start.id, "next").map(|edge| edge.to.as_str());
        self.chain(graph, first).await
    }

    /// Обход с узла: `path` — id групп по вложенности, последним id самого узла. Внутренний уровень идёт с политиками
    /// объемлющих групп, затем каждая группа закрывается как при обычном выходе, и обход продолжается за ней.
    pub async fn run_from(&mut self, flow: &Flow, path: &[String]) -> Result<Exit, Fail> {
        let Some((last, groups)) = path.split_last() else { return self.run(flow).await };
        let missing = |id: &str| Fail::Node { node: id.to_string(), message: format!("нет узла {id}") };
        // Тело New Browser идёт в браузере другого профиля: с узла в нём обход пошёл бы в браузере этого.
        let in_body = |graph: Graph<'_>, id: &str| {
            graph.nodes.iter().filter(|n| n.op == "control.new_browser").any(|b| crate::flow::validate::body_of(graph, &b.id).contains(id))
        };
        let body = || Fail::Node { node: last.clone(), message: "Начать проверку с блока внутри «Нового браузера» нельзя. Запустите сценарий с начала.".into() };
        let mut graph = flow.graph();
        let mut open: Vec<(Graph<'_>, &Node, bool)> = Vec::new();
        // Путь проверяется целиком до политик: ранний выход не оставляет их в стеке.
        for id in groups {
            let group = graph.node(id).ok_or_else(|| missing(id))?;
            if in_body(graph, id) {
                return Err(body());
            }
            open.push((graph, group, false));
            graph = group.inner();
        }
        graph.node(last).ok_or_else(|| missing(last))?;
        if in_body(graph, last) {
            return Err(body());
        }
        for entry in &mut open {
            entry.2 = self.push_policy(entry.1);
        }
        let mut exit = self.chain(graph, Some(last.as_str())).await;
        while let Some((outer, group, pushed)) = open.pop() {
            if pushed {
                self.policies.pop();
            }
            // Как `group()` при обычном обходе: ошибку без своей политики ловит объемлющая группа.
            let port = match own_catch(group, exit) {
                Ok(Exit::Done) => {
                    self.journal.node(group, Status::Ok, 0, "");
                    "next"
                }
                Err(NodeError::Caught(message)) => {
                    self.scope.set_last_error(&message);
                    self.journal.node(group, Status::Branch, 0, &message);
                    "error"
                }
                // Break, Quit и Export выходят из группы, как при обычном обходе: со строкой группы.
                Ok(other) => {
                    self.journal.node(group, Status::Ok, 0, "");
                    exit = Ok(other);
                    continue;
                }
                Err(NodeError::Run(fail)) => {
                    exit = Err(fail);
                    continue;
                }
                Err(NodeError::Op(_)) => unreachable!("own_catch не отдаёт ошибку операции"),
            };
            let next = outer.next(&group.id, port).filter(|edge| edge.to_port != "loop").map(|edge| edge.to.as_str());
            exit = self.chain(outer, next).await;
        }
        exit
    }

    /// Цепочка от `current` до тупика или связи во вход `loop` (конец итерации). Возвращает упакованный future:
    /// обход рекурсивен (циклы, группы, подпроцессы), и тип future должен быть конечным.
    fn chain<'a, 'g: 'a>(&'a mut self, graph: Graph<'g>, mut current: Option<&'g str>) -> BoxFuture<'a, Result<Exit, Fail>> {
        Box::pin(async move {
            while let Some(id) = current {
                if self.cancel.is_cancelled() {
                    return Err(Fail::Cancelled);
                }
                let Some(node) = graph.node(id) else { break };
                // Найденное слушателями сети — в переменные до следующего узла, не посреди него.
                for (name, value) in self.listeners.take(self.depth()) {
                    if self.scope.set(&name, value).is_err() {
                        self.journal.event(Status::Error, &format!("Не удалось сохранить результат отслеживания сети в переменную {name}"));
                    }
                }
                // Сеть, включённая перед прошлым узлом, нужна только Request to finish; следующему за этим узлом
                // Request to finish она включается до него — запрос, который узел вызовет сам, не пройдёт мимо.
                let early = self.armed.take().filter(|_| node.op == "wait.request");
                let ahead = if arms_wait(graph, node) {
                    let cancel = self.cancel.clone();
                    tokio::select! {
                        armed = self.arm() => armed,
                        _ = cancel.cancelled() => None,
                    }
                } else {
                    None
                };
                self.armed = early;
                let step = self.node(graph, node).await;
                // Не взятое узлом гаснет здесь; включённое для следующего — тоже, если цепочка дальше не идёт.
                self.armed = if matches!(step, Ok(Step::Port(_))) { ahead } else { None };
                let port = match step? {
                    Step::Port(port) => port,
                    Step::Break => return Ok(Exit::Break),
                    Step::Quit => return Ok(Exit::Quit),
                    Step::Export(values) => return Ok(Exit::Export(values)),
                };
                current = graph.next(id, port).filter(|edge| edge.to_port != "loop").map(|edge| edge.to.as_str());
            }
            // Тупик (в том числе ветка без связи): включённое для ожидания, до которого не дошли, гаснет.
            self.armed = None;
            Ok(Exit::Done)
        })
    }

    /// Сеть текущей вкладки для Request to finish за узлом; нет вкладки или сеть не включилась — без неё.
    async fn arm(&mut self) -> Option<crate::flow::ops::net::Armed> {
        let net = self.tabs.as_mut()?.page().await.ok()?.network();
        crate::flow::ops::net::arm(net).await.ok()
    }

    async fn node<'g>(&mut self, graph: Graph<'g>, node: &'g Node) -> Result<Step, Fail> {
        // Только узлы самого процесса: id узлов подпроцесса совпадают с id вызывающего, подсвечен остаётся вызов.
        if self.calls.is_empty()
            && let Some(on_node) = &self.on_node
        {
            on_node(&node.id);
        }
        let Some(spec) = ops::spec(&node.op) else {
            return Err(Fail::Node { node: node.id.clone(), message: format!("неизвестная операция {}", node.op) });
        };
        if node.disabled {
            self.journal.node(node, Status::Disabled, 0, "");
            return Ok(Step::Port(spec.shape.main_exit()));
        }
        let started = Instant::now();
        let result = self.dispatch(graph, spec, node).await;
        let ms = started.elapsed().as_millis() as u64;
        match result {
            Ok((step, note)) => {
                self.journal.node(node, Status::Ok, ms, &join(&ops::describe(spec, &node.config), &note));
                Ok(step)
            }
            Err(NodeError::Caught(message)) => {
                self.scope.set_last_error(&message);
                self.journal.node(node, Status::Branch, ms, &message);
                Ok(Step::Port("error"))
            }
            Err(NodeError::Run(fail)) => Err(fail),
            Err(NodeError::Op(OpError::BrowserClosed)) => Err(Fail::BrowserClosed),
            Err(NodeError::Op(OpError::Cancelled)) => Err(Fail::Cancelled),
            Err(NodeError::Op(OpError::Failed(message))) => self.failed(spec, node, ms, format!("{}: {message}", spec.adspower)),
        }
    }

    async fn dispatch<'g>(&mut self, graph: Graph<'g>, spec: &'static OpSpec, node: &'g Node) -> Result<(Step, String), NodeError> {
        let cfg = Cfg::new(spec, &node.config);
        match spec.id {
            "start" => Ok((Step::Port("next"), String::new())),
            "control.if" => {
                let yes = self.decide(&cfg)?;
                Ok((Step::Port(if yes { "true" } else { "false" }), if yes { "истина" } else { "ложь" }.to_string()))
            }
            "control.for_times" | "control.for_data" | "control.while" | "control.for_elements" => self.repeat(graph, node, &cfg).await,
            "control.break" => Ok((Step::Break, String::new())),
            "control.quit_browser" => Ok((Step::Quit, String::new())),
            "control.group" => self.group(node).await,
            "control.call" => self.call(node, &cfg).await,
            "control.new_browser" => self.new_browser(graph, node, &cfg).await,
            "proc.vars" => self.manage(&cfg),
            _ => {
                let cancel = self.cancel.clone();
                tokio::select! {
                    result = ops::exec(self, spec, node) => Ok(result?),
                    _ = cancel.cancelled() => Err(NodeError::Op(OpError::Cancelled)),
                }
            }
        }
    }

    fn decide(&self, cfg: &Cfg<'_>) -> Result<bool, String> {
        let name = cfg.string("variable");
        let right = self.scope.resolve(&cfg.raw("value"))?;
        vars::check(&name, &cfg.string("condition"), self.scope.get(&name), &right)
    }

    /// For Loop Times, For Loop Data и While Loop: тело — цепочка из выхода `body`.
    async fn repeat<'g>(&mut self, graph: Graph<'g>, node: &'g Node, cfg: &Cfg<'_>) -> Result<(Step, String), NodeError> {
        let body = graph.next(&node.id, "body").map(|edge| edge.to.as_str());
        let mut items = match node.op.as_str() {
            "control.for_times" => {
                let times = cfg.int(&self.scope, "times")?;
                if times < 0 {
                    return Err("число повторов отрицательное".to_string().into());
                }
                Some(Box::new((0..times).map(|i| (Value::Null, Value::from(i)))) as LoopItems)
            }
            "control.for_data" => Some(Box::new(
                match self.scope.resolve(&cfg.raw("data"))? {
                    Value::Array(list) => list.into_iter().enumerate().map(|(i, item)| (item, Value::from(i))).collect::<Vec<_>>(),
                    Value::Object(map) => map.into_iter().map(|(key, item)| (item, Value::String(key))).collect(),
                    _ => return Err("данные не массив и не объект".to_string().into()),
                }
                .into_iter(),
            ) as LoopItems),
            "control.for_elements" => {
                // Снятие списка — вызовы браузера на каждый элемент: «Стоп» прерывает его, как обычную операцию.
                let cancel = self.cancel.clone();
                let items = tokio::select! {
                    items = crate::flow::ops::data::loop_elements(self, cfg) => items?,
                    _ = cancel.cancelled() => return Err(NodeError::Op(OpError::Cancelled)),
                };
                Some(Box::new(items.into_iter().enumerate().map(|(i, item)| (item, Value::from(i)))) as LoopItems)
            }
            // While Loop: условие проверяется перед каждой итерацией.
            _ => None,
        };
        let mut done = 0usize;
        loop {
            // Пустое тело не ждёт ничего: без уступки «Стоп» не дошёл бы до цикла.
            tokio::task::yield_now().await;
            if self.cancel.is_cancelled() {
                return Err(NodeError::Run(Fail::Cancelled));
            }
            match &mut items {
                Some(iter) => {
                    let Some((item, index)) = iter.next() else { break };
                    if let Some(name) = cfg.var_out("item_to") {
                        self.scope.set(&name, item)?;
                    }
                    if let Some(name) = cfg.var_out("index_to") {
                        self.scope.set(&name, index)?;
                    }
                }
                None => {
                    if !self.decide(cfg)? {
                        break;
                    }
                }
            }
            done += 1;
            match self.chain(graph, body).await.map_err(NodeError::Run)? {
                Exit::Done => {}
                Exit::Break => break,
                Exit::Quit => return Ok((Step::Quit, format!("итераций: {done}"))),
                Exit::Export(values) => return Ok((Step::Export(values), format!("итераций: {done}"))),
            }
        }
        Ok((Step::Port("next"), format!("итераций: {done}")))
    }

    async fn group(&mut self, node: &Node) -> Result<(Step, String), NodeError> {
        let inner = node.inner();
        let entry = inner
            .nodes
            .iter()
            .find(|n| !inner.edges.iter().any(|e| e.to == n.id && e.to_port != "loop"))
            .map(|n| n.id.as_str());
        let pushed = self.push_policy(node);
        let exit = self.chain(inner, entry).await;
        if pushed {
            self.policies.pop();
        }
        Ok((step_of(own_catch(node, exit)?), String::new()))
    }

    /// New Browser: тело — в браузере другого профиля (своя текущая вкладка), затем `after` и возврат в свой браузер.
    /// Ошибки тела — по политикам его узлов, затем этого узла (как у группы); закрытый второй браузер — ошибка узла,
    /// а не конец запуска.
    async fn new_browser<'g>(&mut self, graph: Graph<'g>, node: &'g Node, cfg: &Cfg<'_>) -> Result<(Step, String), NodeError> {
        let serial = cfg.int(&self.scope, "serial")?;
        let quit = cfg.string("after") != "keep";
        let host = self.host.clone().ok_or_else(|| "у запуска нет профилей".to_string())?;
        // Запуск браузера не прерывается, как шаг 2 запуска: «Стоп» заметит первый узел тела.
        let opened = host.open(serial, quit).await?;
        let mut inner = Tabs::new(opened.browser.clone(), Some(opened.tab.clone())).with_relay(Some(opened.relay_port));
        if let Some(outer) = self.tabs.as_mut() {
            inner.put_elements(outer.take_elements());
        }
        let outer = self.tabs.replace(inner);
        let humanize = std::mem::replace(&mut self.humanize, opened.humanize);
        let body = graph.next(&node.id, "body").map(|edge| edge.to.as_str());
        let pushed = self.push_policy(node);
        // Закрытый второй браузер прерывает тело, как «Стоп» (основной браузер запуска об этом не узнает): узлы
        // выходят своим путём и возвращают подменённое (переменные подпроцесса, политики) — брошенный future их
        // потерял бы.
        let listeners = self.listeners.mark();
        let stop = self.cancel.child_token();
        let run_cancel = std::mem::replace(&mut self.cancel, stop.clone());
        let watch = async {
            opened.browser.closed().await;
            stop.cancel();
            std::future::pending::<Result<Exit, Fail>>().await
        };
        let exit = tokio::select! {
            exit = self.chain(graph, body) => exit,
            never = watch => never,
        };
        self.cancel = run_cancel;
        // Слушатели тела слушали второй браузер: кончаются вместе с телом.
        self.listeners.stop_since(listeners);
        let exit = match exit {
            Err(Fail::Cancelled) if !self.cancel.is_cancelled() => Err(Fail::BrowserClosed),
            other => other,
        };
        if pushed {
            self.policies.pop();
        }
        self.humanize = humanize;
        // Вкладки второго браузера отцепляются до того, как он гаснет.
        let elements = self.tabs.as_mut().map(Tabs::take_elements);
        self.tabs = outer;
        if let (Some(tabs), Some(elements)) = (self.tabs.as_mut(), elements) {
            tabs.put_elements(elements);
        }
        let launched = opened.launched;
        host.close(opened, quit).await;
        let exit = match exit {
            Err(Fail::BrowserClosed) => return Err("браузер профиля закрыт".to_string().into()),
            other => other,
        };
        // Break в теле кончает только его: цикл снаружи и запуск идут дальше.
        let step = match own_catch(node, exit)? {
            Exit::Done | Exit::Break => Step::Port("next"),
            Exit::Quit => Step::Quit,
            Exit::Export(values) => Step::Export(values),
        };
        Ok((step, if quit && launched { "браузер профиля закрыт после тела".into() } else { "браузер профиля оставлен".into() }))
    }

    /// Apply Created Process: шаги подпроцесса в том же браузере, своё пространство переменных, настройки подпроцесса
    /// не действуют.
    async fn call(&mut self, node: &Node, cfg: &Cfg<'_>) -> Result<(Step, String), NodeError> {
        let id = cfg.int(&self.scope, "process")?;
        if self.calls.contains(&id) {
            return Err(format!("Сценарий {id} вызывает сам себя. Удалите повторный вызов.").into());
        }
        let sub = (self.lookup)(id).ok_or_else(|| format!("Сценарий {id} не найден"))?;
        // Подпроцесс берётся из базы в момент вызова — проверка запуска его не видела (или он успел измениться).
        // ponytail: проверка на каждый вызов; для небольших процессов дёшево, в горячем цикле кэшировать по id.
        let report = crate::flow::validate::validate(&sub, &*self.lookup);
        if !report.is_ok() {
            return Err(format!("Во вложенном сценарии есть ошибки: {}", report.summary()).into());
        }
        let mut inputs = Map::new();
        if let Value::Object(fields) = cfg.raw("inputs") {
            for (name, template) in fields {
                inputs.insert(name, self.scope.resolve(&template)?);
            }
        }
        let mut scope = self.scope.child();
        vars::declare(&mut scope, &sub.variables, inputs)?;
        let outer = std::mem::replace(&mut self.scope, scope);
        let pushed = self.push_policy(node);
        self.calls.push(id);
        self.journal.depth = self.calls.len() as u32;
        let listeners = self.listeners.mark();
        let exit = self.run(&sub).await;
        self.listeners.stop_since(listeners);
        self.calls.pop();
        self.journal.depth = self.calls.len() as u32;
        if pushed {
            self.policies.pop();
        }
        self.scope = outer;
        let exported = match own_catch(node, exit)? {
            Exit::Export(values) => values,
            // Quit Browser в подпроцессе кончает только подпроцесс.
            _ => Map::new(),
        };
        let note = self.save(cfg, Value::Object(exported))?;
        Ok((Step::Port("next"), note))
    }

    /// Manage Variables: `add` задаёт переменные; `export` завершает текущий процесс и отдаёт значения.
    fn manage(&mut self, cfg: &Cfg<'_>) -> Result<(Step, String), NodeError> {
        if cfg.string("mode") == "export" {
            let names = cfg.raw("names");
            let mut values = Map::new();
            for name in names.as_array().into_iter().flatten().filter_map(Value::as_str) {
                let value = self.scope.get(name).cloned().ok_or_else(|| format!("переменная {name} не задана"))?;
                values.insert(name.to_string(), value);
            }
            let note = format!("Возвращены значения: {}", values.keys().cloned().collect::<Vec<_>>().join(", "));
            return Ok((Step::Export(values), note));
        }
        let rows = cfg.raw("vars");
        let mut names = Vec::new();
        for row in rows.as_array().into_iter().flatten() {
            let name = row["name"].as_str().unwrap_or_default();
            let ty: VarType = serde_json::from_value(row["type"].clone()).map_err(|_| format!("переменная {name}: неизвестный тип"))?;
            let value = vars::coerce(ty, self.scope.resolve(&row["value"])?).map_err(|e| format!("переменная {name}: {e}"))?;
            self.scope.set(name, value)?;
            names.push(name);
        }
        Ok((Step::Port("next"), format!("заданы: {}", names.join(", "))))
    }

    fn push_policy(&mut self, node: &Node) -> bool {
        let inherited = match node.on_error {
            None => return false,
            Some(NodePolicy::Interrupt) => Inherited::Interrupt,
            Some(NodePolicy::Skip) => Inherited::Skip,
            Some(NodePolicy::Branch) => Inherited::Catch,
        };
        self.policies.push(inherited);
        true
    }

    /// Ошибка узла по политике: своя → ближайшей группы или вызова → процесса.
    fn failed(&mut self, spec: &OpSpec, node: &Node, ms: u64, message: String) -> Result<Step, Fail> {
        let policy = match node.on_error {
            Some(NodePolicy::Branch) => {
                self.scope.set_last_error(&message);
                self.journal.node(node, Status::Branch, ms, &message);
                return Ok(Step::Port("error"));
            }
            Some(NodePolicy::Interrupt) => Inherited::Interrupt,
            Some(NodePolicy::Skip) => Inherited::Skip,
            None => self.policies.last().cloned().unwrap_or(match self.base {
                ErrorPolicy::Interrupt => Inherited::Interrupt,
                ErrorPolicy::Skip => Inherited::Skip,
            }),
        };
        match policy {
            Inherited::Skip => {
                self.journal.node(node, Status::Skip, ms, &message);
                Ok(Step::Port(spec.shape.main_exit()))
            }
            Inherited::Interrupt => {
                self.journal.node(node, Status::Error, ms, &message);
                Err(Fail::Node { node: node.id.clone(), message })
            }
            Inherited::Catch => {
                self.journal.node(node, Status::Error, ms, &message);
                Err(Fail::Catch { node: node.id.clone(), message })
            }
        }
    }

    /// Результат в переменную поля `save_to`; хвост журнала — имя, тип и длина, без значения.
    pub fn save(&mut self, cfg: &Cfg<'_>, value: Value) -> Result<String, OpError> {
        let Some(name) = cfg.var_out("save_to") else { return Ok(String::new()) };
        let about = shape_of(&value);
        self.scope.set(&name, value)?;
        Ok(format!("→ {name} ({about})"))
    }
}

/// Catch, дошедший до узла с `branch`, — его собственный: внутренний контейнер со skip/interrupt не даёт ему
/// образоваться, а внутренний с branch забирает его себе.
fn own_catch(node: &Node, exit: Result<Exit, Fail>) -> Result<Exit, NodeError> {
    match exit {
        Err(Fail::Catch { message, .. }) if node.on_error == Some(NodePolicy::Branch) => Err(NodeError::Caught(message)),
        other => other.map_err(NodeError::Run),
    }
}

/// Включать ли сеть перед узлом: его выход `next` ведёт в Request to finish (не выключенный и не во вход `loop`).
/// Не включается перед самим Request to finish — второе ожидание кончилось бы на запросе первого, — и перед
/// контейнером (циклы, группа, подпроцесс, New Browser): сеть была бы включена всё его тело.
fn arms_wait(graph: Graph<'_>, node: &Node) -> bool {
    const NEVER: [&str; 8] = [
        "wait.request",
        "control.for_times",
        "control.for_data",
        "control.while",
        "control.for_elements",
        "control.group",
        "control.call",
        "control.new_browser",
    ];
    !NEVER.contains(&node.op.as_str())
        && graph
            .next(&node.id, "next")
            .filter(|edge| edge.to_port != "loop")
            .and_then(|edge| graph.node(&edge.to))
            .is_some_and(|next| next.op == "wait.request" && !next.disabled)
}

fn step_of(exit: Exit) -> Step {
    match exit {
        Exit::Done => Step::Port("next"),
        Exit::Break => Step::Break,
        Exit::Quit => Step::Quit,
        Exit::Export(values) => Step::Export(values),
    }
}

fn shape_of(value: &Value) -> String {
    if vars::element_index(value).is_some() {
        return "element".into();
    }
    match value {
        Value::Null => "null".into(),
        Value::Bool(_) => "boolean".into(),
        Value::Number(_) => "number".into(),
        Value::String(text) => format!("string, {}", text.chars().count()),
        Value::Array(list) => format!("array, {}", list.len()),
        Value::Object(map) => format!("object, {}", map.len()),
    }
}

fn join(first: &str, second: &str) -> String {
    match (first.is_empty(), second.is_empty()) {
        (true, _) => second.to_string(),
        (_, true) => first.to_string(),
        _ => format!("{first} {second}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::journal::Status;
    use serde_json::json;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    fn flow(vars: Value, nodes: Value, edges: Value) -> Flow {
        serde_json::from_value(json!({ "format": "ombra-flow/1", "name": "t", "variables": vars, "nodes": nodes, "edges": edges })).unwrap()
    }

    fn ctx(flow: &Flow, subflows: Vec<(i64, Flow)>) -> Ctx {
        let mut scope = Scope::new(HashMap::from([("acc_id", json!("k1")), ("password", json!("S3cr3t"))]));
        vars::declare(&mut scope, &flow.variables, Map::new()).unwrap();
        let subflows: HashMap<i64, Flow> = subflows.into_iter().collect();
        Ctx::new(scope, flow.settings.on_error, Box::new(move |id| subflows.get(&id).cloned()), Journal::memory(), CancellationToken::new())
    }

    async fn run(flow: &Flow) -> (Result<Exit, Fail>, Ctx) {
        let mut ctx = ctx(flow, Vec::new());
        let result = ctx.run(flow).await;
        (result, ctx)
    }

    fn trace(ctx: &Ctx) -> Vec<String> {
        ctx.journal.entries().iter().filter(|e| e.status == Status::Ok).map(|e| e.node.clone()).collect()
    }

    fn status(ctx: &Ctx, node: &str) -> Vec<Status> {
        ctx.journal.entries().iter().filter(|e| e.node == node).map(|e| e.status).collect()
    }

    fn start() -> Value {
        json!({ "id": "s", "op": "start" })
    }

    /// Узел без действия — для следа обхода.
    fn w(id: &str) -> Value {
        json!({ "id": id, "op": "wait.time", "config": { "ms": 0 } })
    }

    fn words() -> Value {
        json!([{ "name": "word", "type": "string", "default": "q" }])
    }

    /// Statement if, который падает: «меньше» для не-чисел.
    fn bad(id: &str, policy: Option<&str>) -> Value {
        let mut node = json!({ "id": id, "op": "control.if", "config": { "variable": "word", "condition": "lt", "value": "abc" } });
        if let Some(policy) = policy {
            node["on_error"] = json!(policy);
        }
        node
    }

    #[tokio::test]
    async fn run_from_top_level_node_skips_earlier_nodes() {
        let f = flow(json!([]), json!([start(), w("a"), w("b"), w("c")]), json!([{ "from": "s", "to": "a" }, { "from": "a", "to": "b" }, { "from": "b", "to": "c" }]));
        let mut c = ctx(&f, Vec::new());
        assert_eq!(c.run_from(&f, &["b".into()]).await, Ok(Exit::Done));
        assert_eq!(trace(&c), ["b", "c"]);
    }

    #[tokio::test]
    async fn run_from_inside_group_continues_after_group() {
        let f = flow(
            json!([]),
            json!([start(), { "id": "g", "op": "control.group", "nodes": [w("g1"), w("g2")], "edges": [{ "from": "g1", "to": "g2" }] }, w("z")]),
            json!([{ "from": "s", "to": "g" }, { "from": "g", "to": "z" }]),
        );
        let mut c = ctx(&f, Vec::new());
        assert_eq!(c.run_from(&f, &["g".into(), "g2".into()]).await, Ok(Exit::Done));
        assert_eq!(trace(&c), ["g2", "g", "z"]);
    }

    #[tokio::test]
    async fn run_from_inside_group_keeps_group_policy() {
        let f = flow(
            words(),
            json!([start(), { "id": "g", "op": "control.group", "on_error": "branch", "nodes": [bad("x", None)] }, w("caught")]),
            json!([{ "from": "s", "to": "g" }, { "from": "g", "port": "error", "to": "caught" }]),
        );
        let mut c = ctx(&f, Vec::new());
        assert_eq!(c.run_from(&f, &["g".into(), "x".into()]).await, Ok(Exit::Done));
        assert_eq!(status(&c, "g"), [Status::Branch]);
        assert_eq!(trace(&c), ["caught"]);
    }

    #[tokio::test]
    async fn run_from_nested_group_error_is_caught_by_outer_branch_group() {
        let f = flow(
            words(),
            json!([
                start(),
                { "id": "g1", "op": "control.group", "on_error": "branch", "nodes": [
                    { "id": "g2", "op": "control.group", "nodes": [bad("x", None)] }
                ] },
                w("caught")
            ]),
            json!([{ "from": "s", "to": "g1" }, { "from": "g1", "port": "error", "to": "caught" }]),
        );
        let mut c = ctx(&f, Vec::new());
        assert_eq!(c.run_from(&f, &["g1".into(), "g2".into(), "x".into()]).await, Ok(Exit::Done));
        assert_eq!(status(&c, "g1"), [Status::Branch]);
        let (normal, n) = run(&f).await;
        assert_eq!(normal, Ok(Exit::Done));
        assert_eq!(trace(&c), trace(&n)[1..], "после x след тот же, что при обходе от Start");
    }

    #[tokio::test]
    async fn run_from_loop_body_stops_at_iteration_end() {
        let f = flow(
            json!([]),
            json!([start(), { "id": "l", "op": "control.for_times", "config": { "times": 2 } }, w("b1"), w("b2")]),
            json!([{ "from": "s", "to": "l" }, { "from": "l", "port": "body", "to": "b1" }, { "from": "b1", "to": "b2" }, { "from": "b2", "to": "l", "to_port": "loop" }]),
        );
        let mut c = ctx(&f, Vec::new());
        assert_eq!(c.run_from(&f, &["b1".into()]).await, Ok(Exit::Done));
        assert_eq!(trace(&c), ["b1", "b2"]);
    }

    #[tokio::test]
    async fn run_from_leaving_a_group_by_quit_writes_the_group_line() {
        let f = flow(
            json!([]),
            json!([start(), { "id": "g", "op": "control.group", "nodes": [w("x"), { "id": "q", "op": "control.quit_browser" }], "edges": [{ "from": "x", "to": "q" }] }, w("z")]),
            json!([{ "from": "s", "to": "g" }, { "from": "g", "to": "z" }]),
        );
        let mut c = ctx(&f, Vec::new());
        assert_eq!(c.run_from(&f, &["g".into(), "x".into()]).await, Ok(Exit::Quit));
        let (normal, n) = run(&f).await;
        assert_eq!(normal, Ok(Exit::Quit));
        assert_eq!(status(&n, "g"), [Status::Ok], "обычный обход пишет строку группы");
        assert_eq!(status(&c, "g"), [Status::Ok]);
        assert_eq!(trace(&c), trace(&n)[1..]);
    }

    #[tokio::test]
    async fn run_from_inside_a_new_browser_body_is_refused() {
        let f = flow(
            json!([]),
            json!([
                start(),
                { "id": "b", "op": "control.new_browser", "config": { "serial": 2 } },
                w("b1"),
                { "id": "g", "op": "control.group", "nodes": [w("x")] },
                { "id": "l", "op": "control.for_times", "config": { "times": 1 } },
                w("l1"),
                w("after")
            ]),
            json!([
                { "from": "s", "to": "b" }, { "from": "b", "port": "body", "to": "b1" }, { "from": "b1", "to": "g" }, { "from": "g", "to": "l" },
                { "from": "l", "port": "body", "to": "l1" }, { "from": "l1", "to": "l", "to_port": "loop" }, { "from": "l", "to": "b", "to_port": "loop" },
                { "from": "b", "to": "after" }
            ]),
        );
        for path in [vec!["b1"], vec!["g", "x"], vec!["l1"]] {
            let path: Vec<String> = path.into_iter().map(String::from).collect();
            let mut c = ctx(&f, Vec::new());
            let result = c.run_from(&f, &path).await;
            assert!(
                matches!(&result, Err(Fail::Node { node, message }) if node == path.last().unwrap() && message == "Начать проверку с блока внутри «Нового браузера» нельзя. Запустите сценарий с начала."),
                "{path:?}: {result:?}"
            );
            assert!(c.journal.entries().is_empty(), "{path:?}: узлы не шли");
        }
        // Узел за New Browser — не тело.
        let mut c = ctx(&f, Vec::new());
        assert_eq!(c.run_from(&f, &["after".into()]).await, Ok(Exit::Done));
        assert_eq!(trace(&c), ["after"]);
    }

    #[tokio::test]
    async fn run_from_missing_node_is_an_error() {
        let f = flow(json!([]), json!([start()]), json!([]));
        let mut c = ctx(&f, Vec::new());
        let result = c.run_from(&f, &["nope".into()]).await;
        assert!(matches!(result, Err(Fail::Node { message, .. }) if message.starts_with("нет узла")));
    }

    #[tokio::test]
    async fn on_node_is_called_before_each_node() {
        let f = flow(
            json!([]),
            json!([start(), w("a"), { "id": "off", "op": "wait.time", "config": { "ms": 0 }, "disabled": true }, w("b")]),
            json!([{ "from": "s", "to": "a" }, { "from": "a", "to": "off" }, { "from": "off", "to": "b" }]),
        );
        let seen: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let mut c = ctx(&f, Vec::new());
        let sink = seen.clone();
        c.on_node = Some(Arc::new(move |id: &str| sink.lock().unwrap().push(id.to_string())));
        c.run_from(&f, &["a".into()]).await.unwrap();
        assert_eq!(*seen.lock().unwrap(), ["a", "off", "b"]);
    }

    #[tokio::test]
    async fn sub_process_nodes_skip_on_node_and_carry_their_depth() {
        // Id узлов уникальны только в своём процессе: `a` подпроцесса — не `a` вызывающего.
        let sub = flow(json!([]), json!([start(), w("a")]), json!([{ "from": "s", "to": "a" }]));
        let main = flow(
            json!([]),
            json!([start(), { "id": "c", "op": "control.call", "config": { "process": 7 } }, w("a")]),
            json!([{ "from": "s", "to": "c" }, { "from": "c", "to": "a" }]),
        );
        let seen: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let mut c = ctx(&main, vec![(7, sub)]);
        let sink = seen.clone();
        c.on_node = Some(Arc::new(move |id: &str| sink.lock().unwrap().push(id.to_string())));
        assert_eq!(c.run(&main).await, Ok(Exit::Done));
        assert_eq!(*seen.lock().unwrap(), ["c", "a"], "узлы подпроцесса не подсвечивают узлы вызывающего");
        let rows: Vec<(&str, u32)> = c.journal.entries().iter().map(|e| (e.node.as_str(), e.depth)).collect();
        assert_eq!(rows, [("s", 0), ("s", 1), ("a", 1), ("c", 0), ("a", 0)]);
        let line = serde_json::to_value(&c.journal.entries()[0]).unwrap();
        assert!(line.get("depth").is_none(), "строки самого процесса — как раньше, без depth: {line}");
        assert_eq!(serde_json::to_value(&c.journal.entries()[1]).unwrap()["depth"], 1);
    }

    #[tokio::test]
    async fn chain_runs_in_order_until_a_dead_end() {
        let f = flow(json!([]), json!([start(), w("a"), w("b"), w("never")]), json!([{ "from": "s", "to": "a" }, { "from": "a", "to": "b" }]));
        let (result, ctx) = run(&f).await;
        assert_eq!(result, Ok(Exit::Done));
        assert_eq!(trace(&ctx), ["s", "a", "b"]);
    }

    #[tokio::test]
    async fn quit_browser_ends_the_run() {
        let f = flow(
            json!([]),
            json!([start(), w("a"), { "id": "q", "op": "control.quit_browser" }]),
            json!([{ "from": "s", "to": "a" }, { "from": "a", "to": "q" }]),
        );
        let (result, ctx) = run(&f).await;
        assert_eq!((result, trace(&ctx)), (Ok(Exit::Quit), vec!["s".to_string(), "a".into(), "q".into()]));
    }

    #[tokio::test]
    async fn statement_if_picks_a_branch_and_branches_merge() {
        for (word, expected) in [("go", ["s", "i", "yes", "m"]), ("stop", ["s", "i", "no", "m"])] {
            let f = flow(
                json!([{ "name": "word", "type": "string", "default": word }]),
                json!([start(), { "id": "i", "op": "control.if", "config": { "variable": "word", "condition": "eq", "value": "go" } }, w("yes"), w("no"), w("m")]),
                json!([
                    { "from": "s", "to": "i" },
                    { "from": "i", "port": "true", "to": "yes" },
                    { "from": "i", "port": "false", "to": "no" },
                    { "from": "yes", "to": "m" },
                    { "from": "no", "to": "m" }
                ]),
            );
            let (_, ctx) = run(&f).await;
            assert_eq!(trace(&ctx), expected);
        }
    }

    #[tokio::test]
    async fn disabled_nodes_pass_control_to_their_main_exit() {
        let f = flow(
            json!([]),
            json!([
                start(),
                { "id": "a", "op": "wait.time", "config": { "ms": 0 }, "disabled": true },
                { "id": "i", "op": "control.if", "config": { "variable": "acc_id", "condition": "exist" }, "disabled": true },
                w("yes"),
                w("no")
            ]),
            json!([{ "from": "s", "to": "a" }, { "from": "a", "to": "i" }, { "from": "i", "port": "true", "to": "yes" }, { "from": "i", "port": "false", "to": "no" }]),
        );
        let (_, ctx) = run(&f).await;
        assert_eq!(trace(&ctx), ["s", "no"], "отключённое условие идёт в ветку «нет»");
        assert_eq!(status(&ctx, "a"), [Status::Disabled]);
    }

    #[tokio::test]
    async fn group_runs_its_chain_then_continues() {
        let f = flow(
            json!([]),
            json!([start(), { "id": "g", "op": "control.group", "nodes": [w("g1"), w("g2")], "edges": [{ "from": "g1", "to": "g2" }] }, w("z")]),
            json!([{ "from": "s", "to": "g" }, { "from": "g", "to": "z" }]),
        );
        let (_, ctx) = run(&f).await;
        assert_eq!(trace(&ctx), ["s", "g1", "g2", "g", "z"]);
    }

    #[tokio::test]
    async fn loops_iterate_and_write_item_and_index() {
        let f = flow(
            json!([{ "name": "list", "type": "array", "default": ["x", "y"] }, { "name": "map", "type": "object", "default": { "k": "v" } }]),
            json!([
                start(),
                { "id": "t", "op": "control.for_times", "config": { "times": 3 } },
                w("tb"),
                { "id": "d", "op": "control.for_data", "config": { "data": "${list}", "item_to": "item", "index_to": "i" } },
                w("db"),
                { "id": "o", "op": "control.for_data", "config": { "data": "${map}", "item_to": "val", "index_to": "key" } }
            ]),
            json!([
                { "from": "s", "to": "t" },
                { "from": "t", "port": "body", "to": "tb" },
                { "from": "tb", "to": "t", "to_port": "loop" },
                { "from": "t", "to": "d" },
                { "from": "d", "port": "body", "to": "db" },
                { "from": "db", "to": "d", "to_port": "loop" },
                { "from": "d", "to": "o" }
            ]),
        );
        let (result, ctx) = run(&f).await;
        assert_eq!(result, Ok(Exit::Done));
        assert_eq!(trace(&ctx), ["s", "tb", "tb", "tb", "t", "db", "db", "d", "o"]);
        assert_eq!(ctx.scope.get("for_times_index"), Some(&json!(2)), "индекс с 0, имя по умолчанию");
        assert_eq!((ctx.scope.get("item"), ctx.scope.get("i")), (Some(&json!("y")), Some(&json!(1))));
        assert_eq!((ctx.scope.get("val"), ctx.scope.get("key")), (Some(&json!("v")), Some(&json!("k"))), "объект: значение и ключ");
    }

    #[tokio::test]
    async fn while_checks_the_condition_before_each_iteration() {
        let f = flow(
            json!([{ "name": "flag", "type": "string", "default": "go" }]),
            json!([
                start(),
                { "id": "l", "op": "control.while", "config": { "variable": "flag", "condition": "eq", "value": "go" } },
                { "id": "v", "op": "proc.vars", "config": { "vars": [{ "name": "flag", "type": "string", "value": "stop" }] } }
            ]),
            json!([{ "from": "s", "to": "l" }, { "from": "l", "port": "body", "to": "v" }, { "from": "v", "to": "l", "to_port": "loop" }]),
        );
        let (_, ctx) = run(&f).await;
        assert_eq!(trace(&ctx), ["s", "v", "l"]);
    }

    #[tokio::test]
    async fn exit_loop_leaves_only_the_nearest_loop() {
        let f = flow(
            json!([]),
            json!([
                start(),
                { "id": "outer", "op": "control.for_times", "config": { "times": 2, "index_to": "oi" } },
                { "id": "inner", "op": "control.for_times", "config": { "times": 5, "index_to": "ii" } },
                { "id": "b", "op": "control.break" },
                w("after")
            ]),
            json!([
                { "from": "s", "to": "outer" },
                { "from": "outer", "port": "body", "to": "inner" },
                { "from": "inner", "port": "body", "to": "b" },
                { "from": "inner", "to": "after" },
                { "from": "after", "to": "outer", "to_port": "loop" }
            ]),
        );
        let (result, ctx) = run(&f).await;
        assert_eq!(result, Ok(Exit::Done));
        assert_eq!(trace(&ctx), ["s", "b", "inner", "after", "b", "inner", "after", "outer"]);
    }

    #[tokio::test]
    async fn interrupt_is_the_default_policy() {
        let f = flow(words(), json!([start(), bad("x", None), w("z")]), json!([{ "from": "s", "to": "x" }, { "from": "x", "port": "true", "to": "z" }]));
        let (result, ctx) = run(&f).await;
        let Err(Fail::Node { node, message }) = result else { panic!("{result:?}") };
        assert_eq!(node, "x");
        assert!(message.starts_with("Statement if: "), "{message}");
        assert_eq!(status(&ctx, "x"), [Status::Error]);
    }

    #[tokio::test]
    async fn skip_and_branch_policies() {
        let f = flow(
            words(),
            json!([start(), bad("x", Some("skip")), w("no"), bad("y", Some("branch")), w("e")]),
            json!([{ "from": "s", "to": "x" }, { "from": "x", "port": "false", "to": "no" }, { "from": "no", "to": "y" }, { "from": "y", "port": "error", "to": "e" }]),
        );
        let (result, ctx) = run(&f).await;
        assert_eq!(result, Ok(Exit::Done));
        assert_eq!(trace(&ctx), ["s", "no", "e"]);
        assert_eq!((status(&ctx, "x"), status(&ctx, "y")), (vec![Status::Skip], vec![Status::Branch]));
        assert!(ctx.scope.get("last_error").and_then(Value::as_str).is_some_and(|e| e.contains("числами")));
    }

    #[tokio::test]
    async fn process_setting_skip_applies_without_a_node_policy() {
        let mut f = flow(words(), json!([start(), bad("x", None), w("no")]), json!([{ "from": "s", "to": "x" }, { "from": "x", "port": "false", "to": "no" }]));
        f.settings.on_error = ErrorPolicy::Skip;
        let (result, ctx) = run(&f).await;
        assert_eq!((result, trace(&ctx)), (Ok(Exit::Done), vec!["s".to_string(), "no".into()]));
    }

    #[tokio::test]
    async fn groups_pass_their_policy_to_inner_nodes() {
        let f = flow(
            words(),
            json!([
                start(),
                { "id": "g", "op": "control.group", "on_error": "skip", "nodes": [bad("x", None), w("g2")], "edges": [{ "from": "x", "port": "false", "to": "g2" }] },
                { "id": "h", "op": "control.group", "on_error": "branch", "nodes": [bad("y", None), w("c")], "edges": [{ "from": "y", "port": "false", "to": "c" }] },
                w("caught")
            ]),
            json!([{ "from": "s", "to": "g" }, { "from": "g", "to": "h" }, { "from": "h", "port": "error", "to": "caught" }]),
        );
        let (result, ctx) = run(&f).await;
        assert_eq!(result, Ok(Exit::Done));
        assert_eq!(trace(&ctx), ["s", "g2", "g", "caught"]);
        assert_eq!(status(&ctx, "h"), [Status::Branch], "группа с branch уходит в свой выход error");
        let strict = flow(
            words(),
            json!([start(), { "id": "g", "op": "control.group", "on_error": "skip", "nodes": [bad("x", Some("interrupt"))] }]),
            json!([{ "from": "s", "to": "g" }]),
        );
        assert!(matches!(run(&strict).await.0, Err(Fail::Node { node, .. }) if node == "x"), "своя политика узла сильнее группы");
    }

    #[tokio::test]
    async fn branch_catch_belongs_to_the_container_not_to_an_equal_id() {
        let sub = flow(
            words(),
            json!([start(), { "id": "c", "op": "control.group", "nodes": [bad("x", None)] }]),
            json!([{ "from": "s", "to": "c" }]),
        );
        let main = flow(
            json!([]),
            json!([start(), { "id": "c", "op": "control.call", "config": { "process": 7 }, "on_error": "branch" }, w("caught")]),
            json!([{ "from": "s", "to": "c" }, { "from": "c", "port": "error", "to": "caught" }]),
        );
        let mut c = ctx(&main, vec![(7, sub)]);
        assert_eq!(c.run(&main).await, Ok(Exit::Done));
        assert_eq!(trace(&c).last().map(String::as_str), Some("caught"));
        assert!(c.scope.get("last_error").is_some());
    }

    fn exporting() -> Flow {
        flow(
            json!([{ "name": "q", "type": "number", "default": 1 }]),
            json!([start(), { "id": "e", "op": "proc.vars", "config": { "mode": "export", "names": ["q", "acc_id"] } }]),
            json!([{ "from": "s", "to": "e" }]),
        )
    }

    #[tokio::test]
    async fn apply_created_process_passes_inputs_and_returns_exports() {
        let main = flow(
            json!([{ "name": "n", "type": "string", "default": "5" }]),
            json!([start(), { "id": "c", "op": "control.call", "config": { "process": 7, "inputs": { "q": "${n}" }, "save_to": "out" } }]),
            json!([{ "from": "s", "to": "c" }]),
        );
        let mut ctx = ctx(&main, vec![(7, exporting())]);
        assert_eq!(ctx.run(&main).await, Ok(Exit::Done));
        assert_eq!(ctx.scope.get("out"), Some(&json!({ "q": 5, "acc_id": "k1" })), "строка приведена к number подпроцесса");
        assert!(ctx.scope.get("q").is_none(), "переменные подпроцесса не протекают");
    }

    #[tokio::test]
    async fn sub_process_errors_follow_the_call_policy() {
        let failing = flow(words(), json!([start(), bad("x", None)]), json!([{ "from": "s", "to": "x" }]));
        let main = |policy: Value| {
            flow(
                json!([]),
                json!([start(), { "id": "c", "op": "control.call", "config": { "process": 7 }, "on_error": policy }, w("caught")]),
                json!([{ "from": "s", "to": "c" }, { "from": "c", "port": "error", "to": "caught" }]),
            )
        };
        let branch = main(json!("branch"));
        let mut c = ctx(&branch, vec![(7, failing.clone())]);
        assert_eq!(c.run(&branch).await, Ok(Exit::Done));
        assert_eq!(trace(&c).last().map(String::as_str), Some("caught"));
        let plain = main(Value::Null);
        let mut c = ctx(&plain, vec![(7, failing)]);
        assert!(matches!(c.run(&plain).await, Err(Fail::Node { node, .. }) if node == "x"));
    }

    #[tokio::test]
    async fn recursive_and_invalid_sub_processes_are_node_errors() {
        let call = |id: i64| flow(json!([]), json!([start(), { "id": "c", "op": "control.call", "config": { "process": id } }]), json!([{ "from": "s", "to": "c" }]));
        let main = call(7);
        let mut c = ctx(&main, vec![(7, call(7))]);
        let result = c.run(&main).await;
        assert!(matches!(result, Err(Fail::Node { ref message, .. }) if message.contains("вызывает сам себя")), "{result:?}");
        let broken = flow(json!([]), json!([start(), { "id": "t", "op": "web.teleport" }]), json!([{ "from": "s", "to": "t" }]));
        let mut c = ctx(&main, vec![(7, broken)]);
        let result = c.run(&main).await;
        assert!(
            matches!(result, Err(Fail::Node { ref message, .. }) if message.starts_with("Apply Created Process: Во вложенном сценарии есть ошибки")),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn missing_sub_process_is_a_node_error() {
        let main = flow(json!([]), json!([start(), { "id": "c", "op": "control.call", "config": { "process": 9 } }]), json!([{ "from": "s", "to": "c" }]));
        let (result, _) = run(&main).await;
        assert!(matches!(result, Err(Fail::Node { message, .. }) if message == "Apply Created Process: Сценарий 9 не найден"));
    }

    #[tokio::test]
    async fn quit_inside_a_sub_process_ends_only_the_sub_process() {
        let quits = flow(json!([]), json!([start(), { "id": "q", "op": "control.quit_browser" }]), json!([{ "from": "s", "to": "q" }]));
        let main = flow(
            json!([]),
            json!([start(), { "id": "c", "op": "control.call", "config": { "process": 7, "save_to": "out" } }, w("z")]),
            json!([{ "from": "s", "to": "c" }, { "from": "c", "to": "z" }]),
        );
        let mut c = ctx(&main, vec![(7, quits)]);
        assert_eq!(c.run(&main).await, Ok(Exit::Done));
        assert_eq!(trace(&c).last().map(String::as_str), Some("z"));
        assert_eq!(c.scope.get("out"), Some(&json!({})));
    }

    #[tokio::test]
    async fn export_at_the_top_ends_the_run_with_values() {
        let (result, _) = run(&exporting()).await;
        assert_eq!(result, Ok(Exit::Export(json!({ "q": 1, "acc_id": "k1" }).as_object().unwrap().clone())));
    }

    #[tokio::test]
    async fn stop_interrupts_waits_and_tight_loops() {
        let waits = flow(json!([]), json!([start(), { "id": "w", "op": "wait.time", "config": { "ms": 60000 } }]), json!([{ "from": "s", "to": "w" }]));
        let spin = flow(
            json!([{ "name": "flag", "type": "string", "default": "go" }]),
            json!([start(), { "id": "l", "op": "control.while", "config": { "variable": "flag", "condition": "eq", "value": "go" } }]),
            json!([{ "from": "s", "to": "l" }]),
        );
        for f in [waits, spin] {
            let mut c = ctx(&f, Vec::new());
            let cancel = c.cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                cancel.cancel();
            });
            let started = Instant::now();
            assert_eq!(c.run(&f).await, Err(Fail::Cancelled));
            assert!(started.elapsed() < Duration::from_secs(5));
        }
    }

    #[tokio::test]
    async fn wait_time_takes_a_fixed_or_random_pause() {
        let f = flow(
            json!([]),
            json!([start(), { "id": "a", "op": "wait.time", "config": { "ms": { "min": 10, "max": 30 } } }, { "id": "b", "op": "wait.time", "config": { "ms": -5 }, "on_error": "skip" }]),
            json!([{ "from": "s", "to": "a" }, { "from": "a", "to": "b" }]),
        );
        let started = Instant::now();
        let (result, ctx) = run(&f).await;
        assert_eq!(result, Ok(Exit::Done));
        assert!(started.elapsed() >= Duration::from_millis(10));
        assert_eq!(status(&ctx, "b"), [Status::Skip]);
    }

    #[tokio::test]
    async fn journal_keeps_templates_not_values() {
        let f = flow(
            json!([]),
            json!([
                start(),
                { "id": "v", "op": "proc.vars", "config": { "vars": [{ "name": "p", "type": "string", "value": "${password}" }] } },
                { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["p"] } }
            ]),
            json!([{ "from": "s", "to": "v" }, { "from": "v", "to": "x" }]),
        );
        let (result, ctx) = run(&f).await;
        let Ok(Exit::Export(values)) = result else { panic!("{result:?}") };
        assert_eq!(values["p"], json!("S3cr3t"));
        let journal = serde_json::to_string(ctx.journal.entries()).unwrap();
        assert!(journal.contains("${password}"));
        assert!(!journal.contains("S3cr3t"), "{journal}");
    }

    #[test]
    fn journal_names_element_values() {
        assert_eq!(shape_of(&vars::element_value(0)), "element");
    }

    #[tokio::test]
    async fn new_browser_without_an_application_is_a_node_error() {
        let f = flow(
            json!([]),
            json!([{ "id": "s", "op": "start" }, { "id": "b", "op": "control.new_browser", "config": { "serial": 1 } }]),
            json!([{ "from": "s", "to": "b" }]),
        );
        let (result, _) = run(&f).await;
        assert!(matches!(result, Err(Fail::Node { ref message, .. }) if message == "New Browser: у запуска нет профилей"), "{result:?}");
    }

    #[tokio::test]
    async fn new_browser_body_types_in_the_second_profiles_input_mode() {
        use crate::engine::page::Browser;
        use crate::engine::page::tests::{Mock, base};
        use crate::flow::host::{Opened, tests::FakeHost};
        let mock = Mock::spawn(|method, _, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.createIsolatedWorld" => json!({ "executionContextId": 1 }),
                "Runtime.callFunctionOn" => json!({ "result": { "type": "object", "objectId": "A" } }),
                "DOM.describeNode" => json!({ "node": { "backendNodeId": 1 } }),
                _ => json!({}),
            })
        })
        .await;
        let browser = Browser::connect(&mock.url).await.unwrap();
        let opened = Opened { profile_id: "p2".into(), browser, tab: "T1".into(), relay_port: 0, launched: false, busy: None, humanize: false };
        let f = flow(
            json!([]),
            json!([
                start(),
                { "id": "b", "op": "control.new_browser", "config": { "serial": 2 } },
                { "id": "i", "op": "web.input", "config": { "target": { "by": "css", "selector": "#q" }, "content": "ab", "interval_ms": 0 } }
            ]),
            json!([{ "from": "s", "to": "b" }, { "from": "b", "port": "body", "to": "i" }]),
        );
        let mut c = ctx(&f, Vec::new());
        c.humanize = true;
        c.host = Some(Arc::new(FakeHost { opened: std::sync::Mutex::new(Some(opened)), ..Default::default() }));
        assert_eq!(c.run(&f).await, Ok(Exit::Done));
        assert!(mock.sent("Input.insertText").is_empty(), "у второго профиля очеловечивание выключено");
        assert_eq!(mock.sent("Input.dispatchKeyEvent").len(), 4, "посимвольно: нажатие и отпускание");
        assert!(c.humanize, "после тела — снова основного профиля");
    }
}
