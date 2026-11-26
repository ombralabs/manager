//! Правка профиля по частям для Local API (спецификация 2Б, «Профили»): меняются только переданные поля.

use super::proxies::ProxyEditForm;
use super::forms::{name_error, SecretInput, clean_lines, launch_line_errors, validate_proxy_form};
use super::profiles::not_found;
use super::{CommandError, Core};
use crate::cookies::Cookie;
use crate::engine::Engine;
use crate::fingerprint::{self, Arch, Catalog, FieldError, Fingerprint, OsRandom, generate};
use crate::store::profiles::{ProfileInput, ProfileRecord};
use crate::store::proxies::{ProxyInput, ProxyKind};
use crate::store::secrets::{Secret, SecretField};
use std::fmt;

/// Имя профиля, созданного через Local API без `name`: к нему добавляется выданный номер.
pub const DEFAULT_NAME: &str = "Профиль";
/// Прокси из `user_proxy_config` Local API.
#[derive(Clone)]
pub struct ProxyConfig {
    pub kind: ProxyKind,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<Secret>,
}

// Логин и пароль прокси в `Debug` не попадают.
impl fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyConfig")
            .field("kind", &self.kind)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username.as_ref().map(|_| "***"))
            .field("password", &self.password.as_ref().map(|_| "***"))
            .finish()
    }
}

/// Что сделать со шлюзом профиля.
#[derive(Debug, Clone)]
pub enum ProxyAssign {
    /// Снять закрепление: при запуске профиль получит случайный свободный прокси.
    Unpin,
    /// Закрепить прокси общего списка по id.
    Existing(i64),
    /// Найти такой прокси в общем списке или добавить его и закрепить.
    Config(ProxyConfig),
}

/// Поля, которые меняет Local API; `None` — не менять.
#[derive(Clone, Default)]
pub struct ProfilePatch {
    pub name: Option<String>,
    pub note: Option<String>,
    pub platform_domain: Option<String>,
    pub start_urls: Option<Vec<String>>,
    /// Дополнительные аргументы запуска профиля (v2 Update Profile, `launch_args`); пустой список — снять все.
    pub extra_args: Option<Vec<String>>,
    /// Пустая строка — снять логин.
    pub username: Option<String>,
    pub password: Option<Secret>,
    pub totp: Option<Secret>,
    /// `Some(None)` — «Без группы».
    pub group_id: Option<Option<i64>>,
    /// Набор расширений (`sys_app_cate_id` / `category_id` AdsPower); номер уже проверен.
    pub ext_category_id: Option<i64>,
    pub fingerprint: Option<Fingerprint>,
    /// Новое содержимое `api_extra` — JSON-объект строкой.
    pub api_extra: Option<String>,
    pub proxy: Option<ProxyAssign>,
    /// Уже разобранные cookies: добавить или заменить по ключу (домен, имя, путь).
    pub cookies: Option<Vec<Cookie>>,
    /// `ombra.humanize`: переданные поля меняют настройки очеловечивания, остальные — как были.
    pub humanize: Option<HumanizePatch>,
}

/// Поля `ombra.humanize` Local API; `None` — не менять (при создании — значение по умолчанию).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HumanizePatch {
    pub enabled: Option<bool>,
    pub preset: Option<crate::humanize::Preset>,
    pub block_input: Option<bool>,
    pub paste_key: Option<String>,
}

impl HumanizePatch {
    fn apply(&self, h: &mut crate::humanize::Humanize) {
        h.enabled = self.enabled.unwrap_or(h.enabled);
        h.preset = self.preset.unwrap_or(h.preset);
        h.block_input = self.block_input.unwrap_or(h.block_input);
        if let Some(key) = &self.paste_key {
            h.paste_key = key.trim().to_string();
        }
    }
}

// Логин, пароль, ключ 2FA и значения cookies в `Debug` не попадают; от cookies — только количество.
impl fmt::Debug for ProfilePatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProfilePatch")
            .field("name", &self.name)
            .field("note", &self.note)
            .field("platform_domain", &self.platform_domain)
            .field("start_urls", &self.start_urls)
            .field("extra_args", &self.extra_args)
            .field("username", &self.username.as_ref().map(|_| "***"))
            .field("password", &self.password.as_ref().map(|_| "***"))
            .field("totp", &self.totp.as_ref().map(|_| "***"))
            .field("group_id", &self.group_id)
            .field("ext_category_id", &self.ext_category_id)
            .field("fingerprint", &self.fingerprint)
            .field("api_extra", &self.api_extra)
            .field("proxy", &self.proxy)
            .field("cookies", &self.cookies.as_ref().map(Vec::len))
            .field("humanize", &self.humanize)
            .finish()
    }
}

/// Форма прокси для общих проверок `validate_proxy_form`. Пароль не передан, но логин есть — `Keep`:
/// у совпадающей записи общего списка пароль может уже быть.
fn proxy_form(config: &ProxyConfig) -> ProxyEditForm {
    let username = config.username.as_deref().map(str::trim).unwrap_or("").to_string();
    let password = match (config.password.as_ref().filter(|p| !p.expose().is_empty()), username.is_empty()) {
        (Some(password), _) => SecretInput::Set { value: password.expose().to_string() },
        (None, true) => SecretInput::Clear,
        (None, false) => SecretInput::Keep,
    };
    ProxyEditForm {
        kind: config.kind,
        host: config.host.trim().to_string(),
        port: config.port,
        username,
        password,
        note: String::new(),
        enabled: true,
    }
}

fn is_json_object(text: &str) -> bool {
    matches!(serde_json::from_str::<serde_json::Value>(text), Ok(serde_json::Value::Object(_)))
}

/// Запись хранилища из профиля: секреты не трогаются (`Keep`).
fn input_from(record: &ProfileRecord) -> ProfileInput {
    ProfileInput {
        name: record.name.clone(),
        note: record.note.clone(),
        group_id: record.group_id,
        platform_domain: record.platform_domain.clone(),
        start_urls: record.start_urls.clone(),
        restore_tabs: record.restore_tabs,
        extra_args: record.extra_args.clone(),
        username: record.username.clone(),
        password: SecretField::Keep,
        totp_secret: SecretField::Keep,
        fingerprint_json: record.fingerprint_json.clone(),
        proxy_id: record.proxy_id,
        bookmarks: None,
        ext_category_id: record.ext_category_id,
        humanize: Some(record.humanize.clone()),
        humanize_seed: None,
        network: None,
        inline_proxy: None,
    }
}

/// Переданные поля — в запись хранилища; секреты шифрует хранилище (`SecretField::Set`).
fn apply_fields(input: &mut ProfileInput, patch: &ProfilePatch) {
    if let Some(name) = &patch.name {
        input.name = name.trim().to_string();
    }
    if let Some(note) = &patch.note {
        input.note = note.clone();
    }
    if let Some(domain) = &patch.platform_domain {
        input.platform_domain = domain.trim().to_string();
    }
    if let Some(urls) = &patch.start_urls {
        input.start_urls = clean_lines(urls);
    }
    if let Some(args) = &patch.extra_args {
        input.extra_args = clean_lines(args);
    }
    if let Some(username) = &patch.username {
        let username = username.trim();
        input.username = (!username.is_empty()).then(|| username.to_string());
    }
    if let Some(password) = &patch.password {
        input.password = SecretField::Set(password.clone());
    }
    if let Some(totp) = &patch.totp {
        // Пустой `fakey` — ключа нет, а не пустой ключ в базе.
        input.totp_secret = if totp.expose().is_empty() { SecretField::Clear } else { SecretField::Set(totp.clone()) };
    }
    if let Some(group) = patch.group_id {
        input.group_id = group;
    }
    if let Some(category) = patch.ext_category_id {
        input.ext_category_id = category;
    }
    if let Some(fingerprint) = &patch.fingerprint {
        input.fingerprint_json = fingerprint.to_json();
    }
    if let Some(humanize) = &patch.humanize {
        let mut h = input.humanize.clone().unwrap_or_default();
        humanize.apply(&mut h);
        input.humanize = Some(h);
    }
}

impl<E: Engine> Core<E> {
    /// Прокси общего списка с теми же типом, хостом (без учёта регистра), портом и логином — его id;
    /// переданный пароль заменяет сохранённый. Иначе — новая запись. Неверная конфигурация → `Validation`.
    pub fn resolve_proxy(&self, config: &ProxyConfig) -> Result<i64, CommandError> {
        let form = proxy_form(config);
        let errors = validate_proxy_form(&form, true);
        if !errors.is_empty() {
            return Err(CommandError::validation(errors));
        }
        let username = (!form.username.is_empty()).then_some(form.username.as_str());
        let password = config.password.as_ref().filter(|p| !p.expose().is_empty());
        if let Some(id) = self.store.find_proxy(config.kind, &form.host, config.port, username)? {
            if let Some(password) = password {
                let saved = self.store.get_proxy(id)?.ok_or_else(|| CommandError::new("NotFound", "прокси не найден"))?;
                self.store.update_proxy(
                    id,
                    &ProxyInput {
                        kind: saved.kind,
                        host: saved.host,
                        port: saved.port,
                        username: saved.username,
                        password: SecretField::Set(password.clone()),
                        note: saved.note,
                        enabled: saved.enabled,
                    },
                )?;
            }
            return Ok(id);
        }
        // Новой записи логин и пароль нужны вместе: иначе авторизация уйдёт наполовину.
        let errors = validate_proxy_form(&form, false);
        if !errors.is_empty() {
            return Err(CommandError::validation(errors));
        }
        let record = self.store.create_proxy(&ProxyInput {
            kind: config.kind,
            host: form.host.clone(),
            port: config.port,
            username: username.map(str::to_string),
            password: password.cloned().map_or(SecretField::Clear, SecretField::Set),
            note: String::new(),
            enabled: true,
        })?;
        Ok(record.id)
    }

    /// Создание через Local API: без имени — «Профиль <номер>»; без отпечатка — от генератора;
    /// группа по умолчанию — «Без группы». Все проверки — до записи: при ошибке ничего не создаётся.
    pub async fn create_profile_patch(&self, patch: ProfilePatch, now: i64) -> Result<ProfileRecord, CommandError> {
        let errors = self.patch_errors(&patch)?;
        if !errors.is_empty() {
            return Err(CommandError::validation(errors));
        }
        let fingerprint = match &patch.fingerprint {
            Some(fingerprint) => fingerprint.clone(),
            None => generate(Catalog::builtin(), Arch::host(), &mut OsRandom)?,
        };
        let proxy_id = match &patch.proxy {
            None | Some(ProxyAssign::Unpin) => None,
            Some(ProxyAssign::Existing(id)) => Some(*id),
            Some(ProxyAssign::Config(config)) => Some(self.resolve_proxy(config)?),
        };
        let mut input = ProfileInput::new(DEFAULT_NAME, fingerprint.to_json());
        apply_fields(&mut input, &patch);
        input.proxy_id = proxy_id;
        let record = self.store.create_profile(&input, now)?;
        if patch.name.is_none() {
            // Номер выдаётся при записи: имя по умолчанию дописывается той же операцией.
            input.name = format!("{DEFAULT_NAME} {}", record.serial);
            input.password = SecretField::Keep;
            input.totp_secret = SecretField::Keep;
            self.store.update_profile(&record.id, &input)?;
        }
        self.apply_side_fields(&record.id, patch).await
    }

    /// Правка через Local API: меняются только переданные поля. Работающему профилю cookies уходят и в браузер.
    pub async fn patch_profile(&self, id: &str, patch: ProfilePatch) -> Result<ProfileRecord, CommandError> {
        let existing = self.store.get_profile(id)?.ok_or_else(not_found)?;
        let errors = self.patch_errors(&patch)?;
        if !errors.is_empty() {
            return Err(CommandError::validation(errors));
        }
        let proxy = match &patch.proxy {
            None => None,
            Some(ProxyAssign::Unpin) => Some(None),
            Some(ProxyAssign::Existing(proxy_id)) => Some(Some(*proxy_id)),
            Some(ProxyAssign::Config(config)) => Some(Some(self.resolve_proxy(config)?)),
        };
        let mut input = input_from(&existing);
        apply_fields(&mut input, &patch);
        self.store.update_profile(id, &input)?;
        if let Some(proxy_id) = proxy {
            // Как ручное назначение в интерфейсе: отметка «шлюз заменён при запуске» снимается.
            let ids = [id.to_string()];
            self.store.set_profiles_proxy(&ids, proxy_id)?;
        }
        self.apply_side_fields(id, patch).await
    }

    /// Все ошибки переданных полей сразу; хранилище при этом не меняется.
    fn patch_errors(&self, patch: &ProfilePatch) -> Result<Vec<FieldError>, CommandError> {
        let mut errors = Vec::new();
        if let Some(name) = &patch.name {
            errors.extend(name_error(name.trim()));
        }
        // Как в редакторе: запрещённые аргументы и строки не с `--` — ошибка поля, профиль не меняется.
        let start_urls = patch.start_urls.as_deref().map(clean_lines).unwrap_or_default();
        let extra_args = patch.extra_args.as_deref().map(clean_lines).unwrap_or_default();
        errors.extend(launch_line_errors(&start_urls, &extra_args));
        if let Some(fingerprint) = &patch.fingerprint {
            errors.extend(fingerprint::validate(fingerprint, Arch::host()));
        }
        if let Some(Some(group)) = patch.group_id
            && self.store.get_group(group)?.is_none()
        {
            errors.push(FieldError { field: "group_id", code: "invalid" });
        }
        match &patch.proxy {
            Some(ProxyAssign::Existing(proxy_id)) if self.store.get_proxy(*proxy_id)?.is_none() => {
                errors.push(FieldError { field: "proxy_id", code: "invalid" });
            }
            Some(ProxyAssign::Config(config)) => errors.extend(validate_proxy_form(&proxy_form(config), true)),
            _ => {}
        }
        if let Some(extra) = &patch.api_extra
            && !is_json_object(extra)
        {
            errors.push(FieldError { field: "api_extra", code: "invalid" });
        }
        let paste_key = patch.humanize.as_ref().and_then(|h| h.paste_key.as_deref());
        errors.extend(paste_key.and_then(|k| crate::humanize::paste_key_error(k.trim())));
        Ok(errors)
    }

    /// После записи профиля: неприменяемые поля отпечатка и cookies (под очередью профиля — работающему и в браузер).
    async fn apply_side_fields(&self, id: &str, patch: ProfilePatch) -> Result<ProfileRecord, CommandError> {
        if let Some(extra) = &patch.api_extra {
            self.store.set_api_extra(id, extra)?;
        }
        if let Some(cookies) = patch.cookies.filter(|cookies| !cookies.is_empty()) {
            self.supervisor.import_cookies(id, cookies).await?;
        }
        self.store.get_profile(id)?.ok_or_else(not_found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::env;
    use crate::cookies::tests::cookie;
    use crate::store::groups::GroupInput;

    fn proxy_config(port: u16, password: Option<&str>) -> ProxyConfig {
        ProxyConfig {
            kind: ProxyKind::Http,
            host: "127.0.0.1".into(),
            port,
            username: Some("user".into()),
            password: password.map(Secret::new),
        }
    }

    #[tokio::test]
    async fn create_without_fields_gets_default_name_and_fingerprint() {
        let e = env().await;
        let rec = e.core.create_profile_patch(ProfilePatch::default(), 10).await.unwrap();
        assert_eq!(rec.name, format!("Профиль {}", rec.serial));
        assert_eq!((rec.group_id, rec.proxy_id, rec.created_at), (None, None, 10));
        assert!(rec.restore_tabs && !rec.has_password && !rec.has_totp);
        let fingerprint = Fingerprint::from_json(&rec.fingerprint_json).unwrap();
        assert!(fingerprint::validate(&fingerprint, Arch::host()).is_empty());
        assert_eq!(e.core.store.api_extra(&rec.id).unwrap(), "{}");

        let second = e.core.create_profile_patch(ProfilePatch::default(), 11).await.unwrap();
        assert_ne!(second.serial, rec.serial);
        assert_eq!(second.name, format!("Профиль {}", second.serial));
    }

    #[tokio::test]
    async fn create_with_all_fields() {
        let e = env().await;
        let group = e
            .core
            .store
            .create_group(
                &GroupInput { name: "Магазины".into(), color: "#2447c9".into(), note: String::new(), quick_access: false },
                1,
            )
            .unwrap();
        let fingerprint = generate(Catalog::builtin(), Arch::host(), &mut OsRandom).unwrap();
        let patch = ProfilePatch {
            name: Some("  Магазин 7 ".into()),
            note: Some("заметка".into()),
            platform_domain: Some(" facebook.com ".into()),
            start_urls: Some(vec![" https://example.com/ ".into(), String::new()]),
            extra_args: Some(vec![" --disable-sync ".into(), " ".into()]),
            username: Some("login@example.com".into()),
            password: Some(Secret::new("hunter2")),
            totp: Some(Secret::new("JBSWY3DPEHPK3PXP")),
            group_id: Some(Some(group.id)),
            ext_category_id: None,
            fingerprint: Some(fingerprint.clone()),
            api_extra: Some(r#"{"fonts":["all"]}"#.into()),
            proxy: Some(ProxyAssign::Config(proxy_config(e.proxy_port, Some("pass")))),
            cookies: Some(vec![cookie(".example.com", "sid", "v1")]),
            humanize: None,
        };
        let rec = e.core.create_profile_patch(patch, 1).await.unwrap();
        assert_eq!(rec.name, "Магазин 7");
        assert_eq!(rec.note, "заметка");
        assert_eq!(rec.platform_domain, "facebook.com");
        assert_eq!(rec.start_urls, ["https://example.com/"]);
        assert_eq!(rec.extra_args, ["--disable-sync"]);
        assert_eq!(rec.username.as_deref(), Some("login@example.com"));
        assert!(rec.has_password && rec.has_totp);
        assert_eq!(rec.group_id, Some(group.id));
        assert_eq!(Fingerprint::from_json(&rec.fingerprint_json).unwrap(), fingerprint);

        let (password, totp) = e.core.store.profile_secrets(&rec.id).unwrap();
        assert_eq!(password.unwrap().expose(), "hunter2");
        assert_eq!(totp.unwrap().expose(), "JBSWY3DPEHPK3PXP");
        assert_eq!(e.core.store.api_extra(&rec.id).unwrap(), r#"{"fonts":["all"]}"#);
        let proxy = e.core.store.proxy_credentials(rec.proxy_id.unwrap()).unwrap();
        assert_eq!((proxy.host.as_str(), proxy.port, proxy.username.as_deref()), ("127.0.0.1", e.proxy_port, Some("user")));
        assert_eq!(proxy.password.unwrap().expose(), "pass");
        let names: Vec<String> = e.core.store.load_cookies(&rec.id).unwrap().into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["sid"]);
    }

    #[tokio::test]
    async fn proxy_config_reuses_a_matching_record() {
        let e = env().await;
        let first = e.core.resolve_proxy(&proxy_config(e.proxy_port, Some("old"))).unwrap();
        let spaced = ProxyConfig { host: " 127.0.0.1 ".into(), ..proxy_config(e.proxy_port, Some("new")) };
        assert_eq!(e.core.resolve_proxy(&spaced).unwrap(), first, "тот же тип, хост, порт и логин — та же запись");
        assert_eq!(e.core.store.proxy_credentials(first).unwrap().password.unwrap().expose(), "new", "пароль обновлён");
        assert_eq!(e.core.resolve_proxy(&proxy_config(e.proxy_port, None)).unwrap(), first, "без пароля — прежний остаётся");
        assert_eq!(e.core.store.proxy_credentials(first).unwrap().password.unwrap().expose(), "new");

        let other_login = ProxyConfig { username: Some("other".into()), ..proxy_config(e.proxy_port, Some("x")) };
        let second = e.core.resolve_proxy(&other_login).unwrap();
        assert_ne!(second, first, "другой логин — новая запись");
        let anonymous = ProxyConfig { username: None, ..proxy_config(e.proxy_port, None) };
        let third = e.core.resolve_proxy(&anonymous).unwrap();
        assert!(e.core.store.proxy_credentials(third).unwrap().username.is_none());
        assert_eq!(e.core.store.list_proxies().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn invalid_proxy_config_is_a_field_error() {
        let e = env().await;
        let empty_host = ProxyConfig { host: "  ".into(), ..proxy_config(8080, Some("p")) };
        let err = e.core.resolve_proxy(&empty_host).unwrap_err();
        assert_eq!(err.code, "Validation");
        assert_eq!(err.fields.unwrap()[0].field, "proxy_host");

        let err = e.core.resolve_proxy(&proxy_config(0, Some("p"))).unwrap_err();
        assert_eq!(err.fields.unwrap()[0].field, "proxy_port");

        let err = e.core.resolve_proxy(&proxy_config(8080, None)).unwrap_err();
        assert_eq!(err.fields.unwrap()[0].field, "proxy_password", "новой записи с логином нужен пароль");

        let password_only = ProxyConfig { username: None, ..proxy_config(8080, Some("p")) };
        let err = e.core.resolve_proxy(&password_only).unwrap_err();
        assert_eq!(err.fields.unwrap()[0].field, "proxy_username");
        assert!(e.core.store.list_proxies().unwrap().is_empty(), "ничего не создано");
    }

    #[tokio::test]
    async fn proxy_can_be_pinned_by_id_and_unpinned() {
        let e = env().await;
        let proxy = e.core.resolve_proxy(&proxy_config(e.proxy_port, Some("pass"))).unwrap();
        let patch = ProfilePatch { proxy: Some(ProxyAssign::Existing(proxy)), ..Default::default() };
        let rec = e.core.create_profile_patch(patch, 1).await.unwrap();
        assert_eq!(rec.proxy_id, Some(proxy));

        e.core.store.record_gateway(&rec.id, proxy, Some(5)).unwrap();
        let unpin = ProfilePatch { proxy: Some(ProxyAssign::Unpin), ..Default::default() };
        let rec = e.core.patch_profile(&rec.id, unpin).await.unwrap();
        assert_eq!((rec.proxy_id, rec.gateway_replaced_at), (None, None));
        assert!(e.core.store.get_proxy(proxy).unwrap().is_some(), "запись прокси остаётся в общем списке");
    }

    #[tokio::test]
    async fn invalid_fields_create_nothing() {
        let e = env().await;
        let mut fingerprint = generate(Catalog::builtin(), Arch::host(), &mut OsRandom).unwrap();
        fingerprint.seed = 1;
        let patch = ProfilePatch {
            name: Some("x".repeat(101)),
            start_urls: Some(vec!["--proxy-server=direct://".into()]),
            extra_args: Some(vec!["--proxy-server=direct://".into()]),
            group_id: Some(Some(999)),
            fingerprint: Some(fingerprint),
            api_extra: Some("[]".into()),
            proxy: Some(ProxyAssign::Existing(999)),
            ..Default::default()
        };
        let err = e.core.create_profile_patch(patch, 1).await.unwrap_err();
        assert_eq!(err.code, "Validation");
        let errors = err.fields.unwrap();
        let mut fields: Vec<&str> = errors.iter().map(|f| f.field).collect();
        fields.sort_unstable();
        assert_eq!(fields, ["api_extra", "extra_args", "group_id", "name", "proxy_id", "seed", "start_urls"]);
        assert_eq!(e.core.store.count_profiles().unwrap(), 0, "ничего не создано");

        let blank = ProfilePatch { name: Some("   ".into()), ..Default::default() };
        let err = e.core.create_profile_patch(blank, 1).await.unwrap_err();
        assert_eq!(err.fields.unwrap()[0].code, "required");

        let bad_proxy = ProfilePatch {
            proxy: Some(ProxyAssign::Config(proxy_config(8080, None))),
            ..Default::default()
        };
        let err = e.core.create_profile_patch(bad_proxy, 1).await.unwrap_err();
        assert_eq!(err.fields.unwrap()[0].field, "proxy_password");
        assert_eq!(e.core.store.count_profiles().unwrap(), 0);
        assert!(e.core.store.list_proxies().unwrap().is_empty());
    }

    #[tokio::test]
    async fn patch_changes_only_given_fields() {
        let e = env().await;
        let created = ProfilePatch {
            name: Some("Магазин".into()),
            note: Some("старая".into()),
            username: Some("login".into()),
            password: Some(Secret::new("p1")),
            start_urls: Some(vec!["https://a.example/".into()]),
            ..Default::default()
        };
        let rec = e.core.create_profile_patch(created, 1).await.unwrap();

        let note = ProfilePatch { note: Some("новая".into()), ..Default::default() };
        let patched = e.core.patch_profile(&rec.id, note).await.unwrap();
        assert_eq!(patched.note, "новая");
        assert_eq!((patched.name.as_str(), patched.username.as_deref()), ("Магазин", Some("login")));
        assert_eq!(patched.start_urls, ["https://a.example/"]);
        assert_eq!(patched.fingerprint_json, rec.fingerprint_json);
        assert_eq!(patched.serial, rec.serial);
        assert_eq!(e.core.store.profile_secrets(&rec.id).unwrap().0.unwrap().expose(), "p1", "пароль не тронут");

        let clear = ProfilePatch { group_id: Some(None), username: Some("  ".into()), ..Default::default() };
        let cleared = e.core.patch_profile(&rec.id, clear).await.unwrap();
        assert_eq!((cleared.group_id, cleared.username), (None, None));
        assert_eq!(e.core.patch_profile("nope00", ProfilePatch::default()).await.unwrap_err().code, "NotFound");
    }

    #[tokio::test]
    async fn cookies_are_merged_for_a_stopped_profile() {
        let e = env().await;
        let first = ProfilePatch { cookies: Some(vec![cookie(".example.com", "a", "1")]), ..Default::default() };
        let rec = e.core.create_profile_patch(first, 1).await.unwrap();
        let more = ProfilePatch {
            cookies: Some(vec![cookie(".example.com", "a", "2"), cookie(".example.com", "b", "3")]),
            ..Default::default()
        };
        e.core.patch_profile(&rec.id, more).await.unwrap();
        let mut pairs: Vec<(String, String)> =
            e.core.store.load_cookies(&rec.id).unwrap().into_iter().map(|c| (c.name, c.value)).collect();
        pairs.sort();
        assert_eq!(pairs, [("a".to_string(), "2".to_string()), ("b".to_string(), "3".to_string())]);
    }

    #[tokio::test]
    async fn cookies_reach_a_running_browser() {
        let e = env().await;
        let created = ProfilePatch {
            proxy: Some(ProxyAssign::Config(proxy_config(e.proxy_port, Some("pass")))),
            ..Default::default()
        };
        let rec = e.core.create_profile_patch(created, 1).await.unwrap();
        e.core.supervisor.start(&rec.id).await.unwrap();

        let live = ProfilePatch { cookies: Some(vec![cookie(".example.com", "sid", "live")]), ..Default::default() };
        e.core.patch_profile(&rec.id, live).await.unwrap();
        let in_browser = e.core.engine.cdp(&e.core.paths.profile_dir(&rec.id)).browser_cookies();
        assert!(in_browser.iter().any(|c| c.name == "sid" && c.value == "live"), "cookie ушла в браузер");
        assert_eq!(e.core.store.count_cookies(&rec.id).unwrap(), 1);
        e.core.supervisor.stop(&rec.id).await.unwrap();
    }

    #[test]
    fn debug_hides_secrets() {
        let patch = ProfilePatch {
            username: Some("login-name".into()),
            password: Some(Secret::new("hunter2")),
            totp: Some(Secret::new("TOTPKEY")),
            proxy: Some(ProxyAssign::Config(ProxyConfig {
                kind: ProxyKind::Socks5,
                host: "gw.example.net".into(),
                port: 1080,
                username: Some("proxy-login".into()),
                password: Some(Secret::new("proxy-pass")),
            })),
            cookies: Some(vec![cookie(".example.com", "sid", "cookie-value")]),
            ..Default::default()
        };
        let text = format!("{patch:?}");
        for secret in ["login-name", "hunter2", "TOTPKEY", "proxy-login", "proxy-pass", "cookie-value", "sid"] {
            assert!(!text.contains(secret), "{secret} в {text}");
        }
        assert!(text.contains("gw.example.net") && text.contains("1080") && text.contains("Some(1)"), "{text}");
    }
}
