//! Разбор аргументов `ombra flow …` (спецификация А2, «CLI»). Свой код: команд шесть, флагов — десяток.
use crate::flow::params::{Distribution, MissingRow, Order};
use crate::flow::tasks::ProfilePick;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Справка: общая или по команде.
    Help(Option<String>),
    Run(RunArgs),
    Retry { task_id: i64, failed: bool, json: bool },
    Validate { file: PathBuf, json: bool },
    List { json: bool },
    Import { file: PathBuf },
    Export { id: i64, file: Option<PathBuf> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunArgs {
    pub distribution: Distribution,
    /// Путь к файлу, id или имя процесса в базе.
    pub flow: String,
    pub profiles: ProfilePick,
    pub set: Vec<(String, String)>,
    pub data: Option<PathBuf>,
    pub threads: u32,
    pub order: Order,
    pub missing_row: MissingRow,
    pub name: Option<String>,
    pub json: bool,
}

const COMMANDS: [&str; 6] = ["run", "retry", "validate", "list", "import", "export"];

/// Флаги со значением; остальные — переключатели.
const VALUED: [&str; 10] = ["--profile", "--serial", "--group", "--set", "--data", "--threads", "--order", "--missing-row", "--name", "--distribution"];

/// Аргументы после `flow`. Ошибка — текст для stderr (код выхода 2).
pub fn parse(args: &[String]) -> Result<Command, String> {
    let Some((name, rest)) = args.split_first() else { return Ok(Command::Help(None)) };
    if matches!(name.as_str(), "--help" | "-h" | "help") {
        return Ok(Command::Help(None));
    }
    if !COMMANDS.contains(&name.as_str()) {
        return Err(format!("неизвестная команда «{name}»; команды: {}", COMMANDS.join(", ")));
    }
    let mut tokens = Tokens::new(rest)?;
    if tokens.flag("--help") || tokens.flag("-h") {
        return Ok(Command::Help(Some(name.clone())));
    }
    let command = match name.as_str() {
        "run" => Command::Run(run(&mut tokens)?),
        "retry" => {
            let task_id = number(&tokens.positional("task_id")?, "task_id")?;
            Command::Retry { task_id, failed: tokens.flag("--failed"), json: tokens.flag("--json") }
        }
        "validate" => Command::Validate { file: tokens.positional("файл сценария")?.into(), json: tokens.flag("--json") },
        "list" => Command::List { json: tokens.flag("--json") },
        "import" => Command::Import { file: tokens.positional("файл сценария")?.into() },
        _ => {
            let id = number(&tokens.positional("идентификатор сценария")?, "идентификатор сценария")?;
            Command::Export { id, file: tokens.optional_positional().map(PathBuf::from) }
        }
    };
    tokens.finish()?;
    Ok(command)
}

fn run(tokens: &mut Tokens) -> Result<RunArgs, String> {
    let flow = tokens.positional("сценарий (файл, идентификатор или название)")?;
    let ids = split(tokens.values("--profile"));
    let serials = split(tokens.values("--serial"));
    let group = tokens.value("--group")?;
    let profiles = match (ids.is_empty(), serials.is_empty(), group) {
        (false, true, None) => ProfilePick::Ids(ids),
        (true, false, None) => ProfilePick::Serials(serials.iter().map(|s| number(s, "номер профиля")).collect::<Result<_, _>>()?),
        (true, true, Some(group)) => ProfilePick::Group(group),
        (true, true, None) => return Err("укажите профили: --profile, --serial или --group".into()),
        _ => return Err("профили — одним способом: --profile, --serial или --group".into()),
    };
    let set = tokens
        .values("--set")
        .into_iter()
        .map(|pair| match pair.split_once('=') {
            Some((name, value)) if !name.trim().is_empty() => Ok((name.trim().to_string(), value.to_string())),
            // Значение `--set` в stderr не попадает: в нём бывают пароли.
            _ => Err("После --set укажите имя=значение".to_string()),
        })
        .collect::<Result<_, String>>()?;
    let threads = match tokens.value("--threads")? {
        Some(text) => text.trim().parse::<u32>().map_err(|_| format!("--threads — число: «{text}»"))?,
        None => 1,
    };
    let order = match tokens.value("--order")? {
        Some(text) => Order::parse(&text).ok_or_else(|| format!("--order — ordered или random: «{text}»"))?,
        None => Order::Ordered,
    };
    let missing_row = match tokens.value("--missing-row")? {
        Some(text) => MissingRow::parse(&text).ok_or_else(|| format!("--missing-row — skip или random: «{text}»"))?,
        None => MissingRow::Skip,
    };
    let data = tokens.value("--data")?.map(PathBuf::from);
    let distribution = match tokens.value("--distribution")? {
        Some(text) => Distribution::parse(&text).ok_or_else(|| "--distribution — queue или per_profile".to_string())?,
        None => Distribution::PerProfile,
    };
    let name = tokens.value("--name")?;
    Ok(RunArgs { distribution, flow, profiles, set, data, threads, order, missing_row, name, json: tokens.flag("--json") })
}

/// Значения через запятую из всех повторов флага; пустые части отбрасываются.
fn split(values: Vec<String>) -> Vec<String> {
    values.iter().flat_map(|value| value.split(',')).map(str::trim).filter(|part| !part.is_empty()).map(str::to_string).collect()
}

fn number(text: &str, what: &str) -> Result<i64, String> {
    text.trim().parse::<i64>().ok().filter(|n| *n > 0).ok_or_else(|| format!("{what} — положительное число: «{text}»"))
}

/// Аргументы команды: флаги (`--name значение`, `--name=значение`, переключатели) и позиционные — по порядку.
struct Tokens {
    options: Vec<(String, Option<String>)>,
    positionals: Vec<String>,
}

impl Tokens {
    fn new(args: &[String]) -> Result<Self, String> {
        let (mut options, mut positionals) = (Vec::new(), Vec::new());
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            if !arg.starts_with('-') || arg == "-" {
                positionals.push(arg.clone());
                continue;
            }
            let (flag, inline) = match arg.split_once('=') {
                Some((flag, value)) => (flag.to_string(), Some(value.to_string())),
                None => (arg.clone(), None),
            };
            let value = if VALUED.contains(&flag.as_str()) {
                Some(match inline {
                    Some(value) => value,
                    None => iter.next().cloned().ok_or_else(|| format!("у {flag} нет значения"))?,
                })
            } else {
                if inline.is_some() {
                    return Err(format!("у {flag} не бывает значения"));
                }
                None
            };
            options.push((flag, value));
        }
        Ok(Self { options, positionals })
    }

    /// Был ли переключатель.
    fn flag(&mut self, name: &str) -> bool {
        let before = self.options.len();
        self.options.retain(|(flag, _)| flag != name);
        self.options.len() != before
    }

    /// Все значения флага по порядку: флаг может повторяться.
    fn values(&mut self, name: &str) -> Vec<String> {
        let (taken, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.options).into_iter().partition(|(flag, _)| flag == name);
        self.options = kept;
        taken.into_iter().filter_map(|(_, value)| value).collect()
    }

    /// Одно значение; повтор — ошибка.
    fn value(&mut self, name: &str) -> Result<Option<String>, String> {
        let mut values = self.values(name);
        if values.len() > 1 {
            return Err(format!("{name} указан дважды"));
        }
        Ok(values.pop())
    }

    fn positional(&mut self, what: &str) -> Result<String, String> {
        if self.positionals.is_empty() {
            return Err(format!("не указан {what}"));
        }
        Ok(self.positionals.remove(0))
    }

    fn optional_positional(&mut self) -> Option<String> {
        (!self.positionals.is_empty()).then(|| self.positionals.remove(0))
    }

    /// Всё ли разобрано: остаток — ошибка.
    fn finish(self) -> Result<(), String> {
        if let Some((flag, _)) = self.options.first() {
            return Err(format!("неизвестный флаг {flag}"));
        }
        if let Some(extra) = self.positionals.first() {
            return Err(format!("лишний аргумент «{extra}»"));
        }
        Ok(())
    }
}

pub fn help(command: Option<&str>) -> &'static str {
    match command {
        Some("run") => RUN_HELP,
        Some("retry") => RETRY_HELP,
        Some("validate") => "ombra flow validate <файл.json> [--json]\n\nПроверяет сценарий из файла. Коды завершения: 0 — ошибок нет; 1 — есть ошибки; 2 — не удалось прочитать файл.",
        Some("list") => "ombra flow list [--json]\n\nПоказывает сохранённые сценарии: идентификатор, название, группу и наличие ошибок.",
        Some("import") => "ombra flow import <файл.json>\n\nИмпортирует сценарий и вложенные сценарии из файла. Выводит идентификатор созданного сценария.",
        Some("export") => "ombra flow export <id> [файл.json]\n\nЭкспортирует сценарий вместе с вложенными сценариями. Если файл не указан, результат выводится в stdout. Существующий файл не перезаписывается.",
        _ => HELP,
    }
}

const HELP: &str = "ombra flow <команда> [аргументы]

Команды для запуска сценариев без графического интерфейса. Их можно выполнять одновременно с приложением Ombra.

  run       запустить сценарий на профилях
  retry     повторить задачу: все профили или только неудачные запуски
  validate  проверить файл сценария
  list      сценарии базы
  import    создать сценарий из файла
  export    выгрузить сценарий в файл

Справка по команде: ombra flow <команда> --help
Коды выхода: 0 — все запуски завершены успешно, 1 — есть ошибка, отмена или пропуск, 2 — ошибка аргументов, сценария,
параметров, таблицы или профилей, 3 — Ombra недоступна, 130 — прерван сигналом.";

const RUN_HELP: &str = "ombra flow run <файл.json | id | имя> (--profile <id>[,…] | --serial <№>[,…] | --group <имя>)
                [--set имя=значение]… [--data строки.csv|.xlsx|.json]
                [--threads N] [--order ordered|random] [--missing-row skip|random]
                [--distribution queue|per_profile] [--name <имя задачи>] [--json]

Сценарий: существующий путь — файл; иначе число — id в базе; иначе имя в базе.
--profile, --serial   профили по id или номерам; через запятую, флаг повторяется
--group               все профили группы, по возрастанию номера
--distribution        queue — общая очередь без повторения готовых заданий; per_profile — на каждом профиле (по умолчанию)
--set                 значение переменной сценария; повторяется
--data                таблица на профиль: строка по колонке serial_number или acc_id, иначе i-я строка i-му профилю
--threads             сколько профилей одновременно, 1–100 (по умолчанию 1)
--order               порядок профилей: ordered (по умолчанию) или random
--missing-row         нет строки для профиля: skip — пропустить (по умолчанию), random — случайная строка
--name                имя задачи (по умолчанию RPA_TASK_<дата_время>)
--json                итог в stdout JSON; ход — в stderr

Ctrl+C завершает текущие запуски с действиями «После задачи». Повторное нажатие немедленно закрывает браузеры.";

const RETRY_HELP: &str = "ombra flow retry <task_id> [--failed] [--json]

Повторно запускает задачу на всех её профилях. С --failed повторяет только запуски, завершённые с ошибкой или отменённые в прошлый раз.
Используются параметры и таблица задачи, а для сохранённого сценария — его текущая версия. Если повторять нечего, код завершения — 0.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_is_explicit_and_legacy_cli_keeps_its_default() {
        let args = |extra: &[&str]| {
            let mut command: Vec<String> = ["run", "flow.json", "--serial", "1,2"].into_iter().map(str::to_string).collect();
            command.extend(extra.iter().map(|s| s.to_string()));
            parse(&command)
        };
        let Command::Run(default) = args(&[]).unwrap() else { panic!() };
        assert_eq!(default.distribution, Distribution::PerProfile);
        let Command::Run(queue) = args(&["--distribution", "queue"]).unwrap() else { panic!() };
        assert_eq!(queue.distribution, Distribution::Queue);
        assert!(args(&["--distribution", "unknown"]).unwrap_err().contains("--distribution"));
    }

    fn parsed(line: &str) -> Result<Command, String> {
        parse(&line.split_whitespace().map(str::to_string).collect::<Vec<_>>())
    }

    fn run(line: &str) -> RunArgs {
        match parsed(line).unwrap() {
            Command::Run(args) => args,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn run_takes_profiles_params_and_defaults() {
        let args = run("run login.json --serial 3,5 --serial 7 --set login=u1 --set=pass=a=b --data rows.csv --json");
        assert_eq!(args.flow, "login.json");
        assert_eq!(args.profiles, ProfilePick::Serials(vec![3, 5, 7]));
        assert_eq!(args.set, [("login".to_string(), "u1".to_string()), ("pass".to_string(), "a=b".to_string())]);
        assert_eq!(args.data, Some(PathBuf::from("rows.csv")));
        assert_eq!((args.threads, args.order, args.missing_row, args.name, args.json), (1, Order::Ordered, MissingRow::Skip, None, true));
        let args = run("run 12 --profile k7f2q9,a1b2c3 --threads 5 --order random --missing-row random --name Ночная");
        assert_eq!(args.profiles, ProfilePick::Ids(vec!["k7f2q9".into(), "a1b2c3".into()]));
        assert_eq!((args.threads, args.order, args.missing_row, args.name.as_deref()), (5, Order::Random, MissingRow::Random, Some("Ночная")));
        assert_eq!(run("run Вход --group Ночные").profiles, ProfilePick::Group("Ночные".into()));
    }

    #[test]
    fn other_commands() {
        assert_eq!(parsed("retry 7 --failed --json").unwrap(), Command::Retry { task_id: 7, failed: true, json: true });
        assert_eq!(parsed("retry 7").unwrap(), Command::Retry { task_id: 7, failed: false, json: false });
        assert_eq!(parsed("validate f.json --json").unwrap(), Command::Validate { file: "f.json".into(), json: true });
        assert_eq!(parsed("list").unwrap(), Command::List { json: false });
        assert_eq!(parsed("import f.json").unwrap(), Command::Import { file: "f.json".into() });
        assert_eq!(parsed("export 4").unwrap(), Command::Export { id: 4, file: None });
        assert_eq!(parsed("export 4 out.json").unwrap(), Command::Export { id: 4, file: Some("out.json".into()) });
    }

    #[test]
    fn help_is_asked_for() {
        assert_eq!(parsed("").unwrap(), Command::Help(None));
        assert_eq!(parsed("--help").unwrap(), Command::Help(None));
        assert_eq!(parsed("run --help").unwrap(), Command::Help(Some("run".into())));
        assert!(help(Some("run")).contains("--missing-row"));
        assert!(help(Some("run")).contains("--data строки.csv|.xlsx|.json"));
        assert!(help(None).contains("retry"));
    }

    #[test]
    fn mistakes_are_named() {
        let cases = [
            ("go", "неизвестная команда"),
            ("run f.json", "укажите профили"),
            ("run f.json --serial 1 --group G", "одним способом"),
            ("run f.json --serial x", "номер профиля"),
            ("run f.json --serial 0", "номер профиля"),
            ("run f.json --serial 1 --set login", "имя=значение"),
            ("run f.json --serial 1 --threads many", "--threads"),
            ("run f.json --serial 1 --order shuffle", "--order"),
            ("run f.json --serial 1 --missing-row drop", "--missing-row"),
            ("run f.json --serial 1 --name A --name B", "дважды"),
            ("run f.json --serial 1 --verbose", "неизвестный флаг --verbose"),
            ("run f.json extra --serial 1", "лишний аргумент"),
            ("run f.json --serial", "нет значения"),
            ("run --serial 1", "не указан"),
            ("retry", "не указан"),
            ("retry seven", "task_id"),
            ("list --json=yes", "не бывает значения"),
            ("export", "не указан"),
        ];
        for (line, expected) in cases {
            let err = parsed(line).unwrap_err();
            assert!(err.contains(expected), "{line}: {err}");
        }
        assert_eq!(parsed("run f.json --serial 1 --set =s3cret").unwrap_err(), "После --set укажите имя=значение", "без значения");
    }
}
