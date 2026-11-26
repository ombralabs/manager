//! Таблица профилей CSV (спецификация 3А): экспорт списка по фильтрам и импорт с отчётом по строкам.
use super::forms::{clean_lines, launch_line_errors, name_error, tag_errors};
use super::import_support::{GROUP_NAME_MAX, group_by_name, parse_proxy_line, proxy_line, resolve_proxy};
use super::proxy_import::ParsedProxy;
use super::profiles::ListQueryDto;
use super::proxies::LineError;
use super::{CommandError, Core};
use crate::engine::Engine;
use crate::fingerprint::{self, Arch, Catalog, FieldError, OsRandom, generate};
use crate::store::profiles::{ProfileInput, ProfileRecord};
use crate::store::secrets::{Secret, SecretField};
use serde::Serialize;
use std::io::Read;
use std::path::Path;

const COLUMNS: [&str; 11] =
    ["name", "group", "tags", "note", "platform_domain", "start_urls", "username", "password", "totp", "proxy", "extra_args"];
/// Как у массового создания.
const IMPORT_MAX: usize = 1000;

/// Итог импорта: созданные профили и строки файла, которые не прошли проверку (номер с 1, заголовок — строка 1).
/// `code` — `name_required`, `name_invalid`, `tags_invalid`, `start_urls_invalid`, `extra_args_invalid`, `proxy_invalid`, `group_invalid`, `totp_invalid`, `format`.
#[derive(Debug, Clone, Serialize)]
pub struct CsvImportReport {
    pub created: Vec<ProfileRecord>,
    pub errors: Vec<LineError>,
}

fn join(list: &[String]) -> String {
    list.join("|")
}

fn split(cell: &str) -> Vec<String> {
    clean_lines(&cell.split('|').map(str::to_string).collect::<Vec<_>>())
}

fn field_code(e: &FieldError) -> &'static str {
    match (e.field, e.code) {
        ("name", "required") => "name_required",
        ("name", _) => "name_invalid",
        ("tags", _) => "tags_invalid",
        ("start_urls", _) => "start_urls_invalid",
        ("totp", _) => "totp_invalid",
        ("extra_args", _) => "extra_args_invalid",
        _ => "format",
    }
}

/// Файл таблицы больше этого не читается (спецификация 3В-1).
const CSV_FILE_MAX: u64 = 10 * 1024 * 1024;

fn import_failed(message: impl Into<String>) -> CommandError {
    CommandError::new("ImportFailed", message)
}

impl<E: Engine> Core<E> {
    /// Профили списка (те же фильтры, без страниц; корзины нет) в CSV. Без `with_secrets` логин, пароль, 2FA и пароль прокси пусты.
    pub fn export_csv(&self, query: &ListQueryDto, with_secrets: bool) -> Result<String, CommandError> {
        Ok(self.render_csv(query, with_secrets)?.0)
    }

    /// Таблица в файл целиком или никак (как экспорт cookies); возвращает число профилей в файле.
    pub fn export_csv_file(&self, query: &ListQueryDto, with_secrets: bool, path: &Path) -> Result<usize, CommandError> {
        let (text, count) = self.render_csv(query, with_secrets)?;
        super::cookies::write_export_file(path, &text)?;
        Ok(count)
    }

    /// Таблица из файла: не больше 10 МБ, UTF-8 (BOM допускается — его снимает `import_csv`).
    pub fn import_csv_file(&self, path: &Path) -> Result<CsvImportReport, CommandError> {
        // Читаем не больше лимита + 1 байт: файл, выросший после проверки, и FIFO не растягивают чтение.
        let mut bytes = Vec::new();
        std::fs::File::open(path)?.take(CSV_FILE_MAX + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > CSV_FILE_MAX {
            return Err(import_failed("файл больше 10 МБ"));
        }
        let text = String::from_utf8(bytes).map_err(|_| import_failed("файл не в кодировке UTF-8"))?;
        self.import_csv(&text)
    }

    /// CSV и число строк профилей в нём.
    fn render_csv(&self, query: &ListQueryDto, with_secrets: bool) -> Result<(String, usize), CommandError> {
        let mut count = 0usize;
        let mut out = ::csv::Writer::from_writer(Vec::new());
        out.write_record(COLUMNS).map_err(|e| CommandError::new("Internal", e.to_string()))?;
        for row in self.list_profiles(query)?.rows {
            let Some(rec) = self.store.get_profile(&row.id)? else { continue };
            let group = row.group.map(|g| g.name).unwrap_or_default();
            let (password, totp) = if with_secrets {
                let (password, totp) = self.store.profile_secrets_each(&rec.id)?;
                (password?, totp?)
            } else {
                (None, None)
            };
            let proxy = match rec.proxy_id {
                // Без секретов ничего не расшифровывается.
                Some(pid) if with_secrets => {
                    let p = self.store.proxy_credentials(pid)?;
                    let pass = p.password.as_ref().map(|s| s.expose().to_string());
                    proxy_line(p.kind, p.username.as_deref(), pass.as_deref(), &p.host, p.port)
                }
                Some(pid) => match self.store.get_proxy(pid)? {
                    Some(p) => proxy_line(p.kind, p.username.as_deref(), None, &p.host, p.port),
                    None => String::new(),
                },
                None => String::new(),
            };
            let username = if with_secrets { rec.username.clone().unwrap_or_default() } else { String::new() };
            let expose = |s: Option<Secret>| s.map(|s| s.expose().to_string()).unwrap_or_default();
            out.write_record([
                rec.name.as_str(),
                &group,
                &join(&row.tags),
                &rec.note,
                &rec.platform_domain,
                &join(&rec.start_urls),
                &username,
                &expose(password),
                &expose(totp),
                &proxy,
                &join(&rec.extra_args),
            ])
            .map_err(|e| CommandError::new("Internal", e.to_string()))?;
            count += 1;
        }
        let bytes = out.into_inner().map_err(|e| CommandError::new("Internal", e.to_string()))?;
        let text = String::from_utf8(bytes).map_err(|e| CommandError::new("Internal", e.to_string()))?;
        Ok((text, count))
    }

    /// Хорошие строки создаются одной транзакцией вместе с тегами, плохие попадают в отчёт. Нужна колонка `name`.
    pub fn import_csv(&self, text: &str) -> Result<CsvImportReport, CommandError> {
        // CRLF приводится к LF: иначе номера строк в отчёте считаются неверно.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text).replace("\r\n", "\n");
        let mut reader = ::csv::ReaderBuilder::new().flexible(true).from_reader(text.as_bytes());
        let headers: Vec<String> = reader
            .headers()
            .map_err(|e| import_failed(format!("файл не читается как CSV: {e}")))?
            .iter()
            .map(|h| h.trim().to_lowercase())
            .collect();
        let mut report = CsvImportReport { created: Vec::new(), errors: Vec::new() };
        if headers.iter().all(String::is_empty) {
            return Ok(report);
        }
        let col = |name: &str| headers.iter().position(|h| h == name);
        let Some(name_col) = col("name") else {
            return Err(CommandError::validation(vec![FieldError { field: "name", code: "required" }]));
        };
        // Этап 1: все строки читаются и проверяются без единой записи; этап 2 — прокси, группы, профили.
        let mut rows: Vec<Row> = Vec::new();
        let mut count = 0;
        for record in reader.records() {
            count += 1;
            if count > IMPORT_MAX {
                return Err(import_failed(format!("в файле больше {IMPORT_MAX} строк: разбейте его на части")));
            }
            let record = match record {
                Ok(r) => r,
                Err(e) => {
                    let line = e.position().map_or(count + 1, |p| p.line() as usize);
                    report.errors.push(LineError { line, code: "format" });
                    continue;
                }
            };
            let line = record.position().map_or(count + 1, |p| p.line() as usize);
            let cell = |name: &str| col(name).and_then(|i| record.get(i)).unwrap_or("").trim().to_string();
            let name = record.get(name_col).unwrap_or("").trim().to_string();
            let (row_tags, start_urls, extra_args) = (split(&cell("tags")), split(&cell("start_urls")), split(&cell("extra_args")));
            let mut errors = Vec::new();
            errors.extend(name_error(&name));
            errors.extend(tag_errors(&row_tags));
            errors.extend(launch_line_errors(&start_urls, &extra_args));
            let totp = cell("totp");
            if !totp.is_empty() && super::totp::decode_base32(&totp).is_none() {
                errors.push(FieldError { field: "totp", code: "invalid" });
            }
            if let Some(first) = errors.first() {
                report.errors.push(LineError { line, code: field_code(first) });
                continue;
            }
            let proxy = match cell("proxy") {
                p if p.is_empty() => None,
                p => match parse_proxy_line(&p) {
                    Some(parsed) => Some(parsed),
                    None => {
                        report.errors.push(LineError { line, code: "proxy_invalid" });
                        continue;
                    }
                },
            };
            let group = Some(cell("group")).filter(|g| !g.is_empty());
            if group.as_ref().is_some_and(|g| g.chars().count() > GROUP_NAME_MAX) {
                report.errors.push(LineError { line, code: "group_invalid" });
                continue;
            }
            let fp = generate(Catalog::builtin(), Arch::host(), &mut OsRandom)?;
            if !fingerprint::validate(&fp, Arch::host()).is_empty() {
                return Err(CommandError::new("Internal", "сгенерированный отпечаток не прошёл проверку"));
            }
            let mut input = ProfileInput::new(name, fp.to_json());
            input.note = cell("note");
            input.platform_domain = cell("platform_domain");
            input.start_urls = start_urls;
            input.extra_args = extra_args;
            input.username = Some(cell("username")).filter(|u| !u.is_empty());
            // Пустая ячейка — секрета нет; пробелы по краям пароля — часть пароля, поэтому без trim.
            let raw = |name: &str| col(name).and_then(|i| record.get(i)).unwrap_or("").to_string();
            input.password = Some(raw("password")).filter(|p| !p.is_empty()).map_or(SecretField::Keep, |p| SecretField::Set(Secret::new(p)));
            input.totp_secret = Some(cell("totp")).filter(|t| !t.is_empty()).map_or(SecretField::Keep, |t| SecretField::Set(Secret::new(t)));
            rows.push(Row { input, tags: row_tags, proxy, group });
        }
        if rows.is_empty() {
            return Ok(report);
        }
        let (mut inputs, mut tags) = (Vec::new(), Vec::new());
        for Row { mut input, tags: row_tags, proxy, group } in rows {
            if let Some(proxy) = proxy {
                let enabled = proxy.username.is_none() || proxy.password.is_some();
                input.proxy_id = Some(resolve_proxy(&self.store, proxy, enabled)?);
            }
            if let Some(name) = group {
                input.group_id = Some(group_by_name(&self.store, &name)?);
            }
            inputs.push(input);
            tags.push(row_tags);
        }
        report.created = self.store.create_profiles_tagged(&inputs, &tags, crate::sys::unix_now())?;
        Ok(report)
    }
}

/// Проверенная строка файла, ещё не записанная.
struct Row {
    input: ProfileInput,
    tags: Vec<String>,
    proxy: Option<ParsedProxy>,
    group: Option<String>,
}

