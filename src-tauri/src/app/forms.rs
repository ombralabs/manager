//! Форма редактора профиля, какой её присылает интерфейс, и перевод в записи хранилища.
use crate::engine::args::{self, ArgsError};
use crate::fingerprint::{self, Arch, FieldError, Fingerprint};
use super::proxies::ProxyEditForm;
use crate::store::profiles::{ProfileInput, ProfileRecord};
use crate::store::proxies::ProxyInput;
use crate::store::secrets::{Secret, SecretField};
use crate::store::bookmarks::{BOOKMARKS_MAX, BOOKMARK_TITLE_MAX, Bookmark};
use crate::store::tags::TAG_MAX;
use serde::Deserialize;
use std::fmt;

/// Как менять сохранённый пароль: оставить, удалить или задать новый.
#[derive(Clone, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum SecretInput {
    Keep,
    Clear,
    Set { value: String },
}

impl SecretInput {
    pub fn to_field(&self) -> SecretField {
        match self {
            Self::Keep => SecretField::Keep,
            Self::Clear => SecretField::Clear,
            Self::Set { value } => SecretField::Set(Secret::new(value.clone())),
        }
    }
}

impl fmt::Debug for SecretInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Keep => f.write_str("Keep"),
            Self::Clear => f.write_str("Clear"),
            Self::Set { .. } => f.write_str("Set(***)"),
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct ProfileForm {
    pub name: String,
    pub note: String,
    pub platform_domain: String,
    pub start_urls: Vec<String>,
    pub extra_args: Vec<String>,
    pub username: String,
    pub password: SecretInput,
    /// Ключ 2FA (Base32). Поля нет — не меняется.
    #[serde(default = "keep")]
    pub totp: SecretInput,
    pub fingerprint: Fingerprint,
    /// Закреплённый прокси — запись общего списка. Поля нет (Local API, старый редактор) — шлюз не меняется,
    /// при создании закрепления нет; `null` — «Случайный при первом запуске»: закрепление и отметка о замене
    /// снимаются; число — закрепить эту запись (неизвестная — ошибка поля `proxy_id`).
    #[serde(default, deserialize_with = "present")]
    pub proxy_id: Option<Option<i64>>,
    /// Группа профиля. Поля нет (редактор этапа 1) — оставить прежнюю, при создании без группы;
    /// `null` — «Без группы»; число — эта группа. Код, собирающий форму, пишет `Some(Some(id))` / `Some(None)`.
    #[serde(default, deserialize_with = "present")]
    pub group_id: Option<Option<i64>>,
    /// Восстанавливать вкладки прошлой сессии. Поля нет — оставить прежнее, при создании «да», как в AdsPower.
    #[serde(default)]
    pub restore_tabs: Option<bool>,
    /// Теги профиля. Поля нет (редактор этапа 1, Local API) — не менять, при создании без тегов;
    /// список — набор тегов профиля заменяется им (пустой снимает все).
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Закладки профиля. Поля нет — не менять, при создании без закладок; список заменяет прежний.
    #[serde(default)]
    pub bookmarks: Option<Vec<Bookmark>>,
    /// Набор расширений. Поля нет — не менять, при создании «Общий» (0).
    #[serde(default)]
    pub ext_category_id: Option<i64>,
    /// Очеловечивание ввода. Поля нет — не менять, при создании значения по умолчанию (включено).
    #[serde(default)]
    pub humanize: Option<crate::humanize::Humanize>,
    #[serde(default)]
    pub network: Option<crate::network::ProfileNetwork>,
    #[serde(default)]
    pub inline_proxy: Option<ProxyEditForm>,
    #[serde(default)]
    pub inline_proxy_source: Option<i64>,
}

fn keep() -> SecretInput {
    SecretInput::Keep
}

fn mask(login: &str) -> &'static str {
    if login.is_empty() { "" } else { "***" }
}

/// Поле есть в JSON (в том числе `null`) → `Some(..)`; отсутствие даёт `None` через `#[serde(default)]`.
fn present<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Option<i64>>, D::Error> {
    Option::<i64>::deserialize(deserializer).map(Some)
}

impl fmt::Debug for ProfileForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProfileForm")
            .field("name", &self.name)
            .field("note", &self.note)
            .field("platform_domain", &self.platform_domain)
            .field("start_urls", &self.start_urls)
            .field("extra_args", &self.extra_args)
            .field("username", &mask(&self.username))
            .field("password", &self.password)
            .field("totp", &self.totp)
            .field("fingerprint", &self.fingerprint)
            .field("proxy_id", &self.proxy_id)
            .field("group_id", &self.group_id)
            .field("restore_tabs", &self.restore_tabs)
            .field("tags", &self.tags)
            .field("bookmarks", &self.bookmarks)
            .field("ext_category_id", &self.ext_category_id)
            .finish()
    }
}

pub const NAME_MAX: usize = 100;

/// Ошибка поля `name` для уже обрезанного по краям имени: пустое или длиннее `NAME_MAX`.
pub fn name_error(name: &str) -> Option<FieldError> {
    if name.is_empty() {
        Some(FieldError { field: "name", code: "required" })
    } else if name.chars().count() > NAME_MAX {
        Some(FieldError { field: "name", code: "too_long" })
    } else {
        None
    }
}

/// Тег длиннее `TAG_MAX` — ошибка поля `tags`; теги уже очищены `clean_lines`.
pub fn tag_errors(tags: &[String]) -> Vec<FieldError> {
    if tags.iter().any(|t| t.chars().count() > TAG_MAX) {
        vec![FieldError { field: "tags", code: "invalid" }]
    } else {
        Vec::new()
    }
}

/// Закладки: `http(s)://`, непустое название до `BOOKMARK_TITLE_MAX`, не больше `BOOKMARKS_MAX`.
pub fn bookmark_errors(list: &[Bookmark]) -> Vec<FieldError> {
    let ok = |b: &Bookmark| {
        let title = b.title.trim().chars().count();
        (1..=BOOKMARK_TITLE_MAX).contains(&title) && (b.url.starts_with("http://") || b.url.starts_with("https://"))
    };
    if list.len() > BOOKMARKS_MAX || !list.iter().all(ok) {
        vec![FieldError { field: "bookmarks", code: "invalid" }]
    } else {
        Vec::new()
    }
}

/// Название без пробелов по краям — как хранится.
pub fn clean_bookmarks(list: &[Bookmark]) -> Vec<Bookmark> {
    list.iter().map(|b| Bookmark { title: b.title.trim().to_string(), url: b.url.trim().to_string() }).collect()
}

/// Строки без пробелов по краям; пустые отбрасываются.
pub fn clean_lines(lines: &[String]) -> Vec<String> {
    lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

/// Ошибки стартовых вкладок и дополнительных аргументов; строки уже очищены `clean_lines`.
pub fn launch_line_errors(start_urls: &[String], extra_args: &[String]) -> Vec<FieldError> {
    let mut errors = Vec::new();
    if args::check_start_urls(start_urls).is_err() {
        errors.push(FieldError { field: "start_urls", code: "invalid" });
    }
    match args::check_extra_args(extra_args) {
        Ok(()) => {}
        Err(ArgsError::Forbidden(_)) => errors.push(FieldError { field: "extra_args", code: "forbidden" }),
        Err(_) => errors.push(FieldError { field: "extra_args", code: "not_a_flag" }),
    }
    errors
}

/// Все ошибки формы сразу. Есть ли в базе прокси и группа, проверяет ядро: для этого нужна база.
pub fn validate_form(form: &ProfileForm, arch: Arch) -> Vec<FieldError> {
    let mut errors = Vec::new();
    errors.extend(name_error(form.name.trim()));
    errors.extend(launch_line_errors(&clean_lines(&form.start_urls), &clean_lines(&form.extra_args)));
    errors.extend(fingerprint::validate(&form.fingerprint, arch));
    if let SecretInput::Set { value } = &form.totp
        && super::totp::decode_base32(value).is_none()
    {
        errors.push(FieldError { field: "totp", code: "invalid" });
    }
    if let Some(tags) = &form.tags {
        errors.extend(tag_errors(&clean_lines(tags)));
    }
    if let Some(list) = &form.bookmarks {
        errors.extend(bookmark_errors(&clean_bookmarks(list)));
    }
    errors.extend(form.humanize.as_ref().and_then(|h| crate::humanize::paste_key_error(&h.paste_key)));
    errors
}

/// Логин и пароль прокси задаются только вместе: иначе авторизация уйдёт наполовину.
pub fn validate_proxy_form(proxy: &ProxyEditForm, saved_password: bool) -> Vec<FieldError> {
    let mut errors = Vec::new();
    let host = proxy.host.trim();
    if host.is_empty() {
        errors.push(FieldError { field: "proxy_host", code: "required" });
    } else if host.len() > 253 || host.contains(char::is_whitespace) {
        errors.push(FieldError { field: "proxy_host", code: "invalid" });
    }
    if proxy.port == 0 {
        errors.push(FieldError { field: "proxy_port", code: "out_of_range" });
    }
    let has_login = !proxy.username.trim().is_empty();
    let has_password = match &proxy.password {
        SecretInput::Set { value } => !value.is_empty(),
        SecretInput::Keep => saved_password,
        SecretInput::Clear => false,
    };
    if has_password && !has_login {
        errors.push(FieldError { field: "proxy_username", code: "required" });
    }
    if has_login && !has_password {
        errors.push(FieldError { field: "proxy_password", code: "required" });
    }
    errors
}

/// Запись хранилища из формы редактора. `existing` — профиль при правке: из него берутся значения полей,
/// которых в форме нет (группа, восстановление вкладок, закреплённый прокси).
pub fn profile_input(form: &ProfileForm, existing: Option<&ProfileRecord>) -> ProfileInput {
    let username = form.username.trim();
    ProfileInput {
        name: form.name.trim().to_string(),
        note: form.note.trim().to_string(),
        group_id: form.group_id.unwrap_or_else(|| existing.and_then(|r| r.group_id)),
        platform_domain: form.platform_domain.trim().to_string(),
        start_urls: clean_lines(&form.start_urls),
        restore_tabs: form.restore_tabs.unwrap_or_else(|| existing.is_none_or(|r| r.restore_tabs)),
        extra_args: clean_lines(&form.extra_args),
        username: (!username.is_empty()).then(|| username.to_string()),
        password: form.password.to_field(),
        totp_secret: form.totp.to_field(),
        fingerprint_json: form.fingerprint.to_json(),
        proxy_id: form.proxy_id.unwrap_or_else(|| existing.and_then(|r| r.proxy_id)),
        bookmarks: form.bookmarks.as_deref().map(clean_bookmarks),
        ext_category_id: form
            .ext_category_id
            .unwrap_or_else(|| existing.map_or(crate::store::extensions::DEFAULT_CATEGORY, |r| r.ext_category_id)),
        humanize: form.humanize.clone(),
        humanize_seed: None,
        network: form.network.clone().or_else(|| form.proxy_id.map(|_| crate::network::ProfileNetwork { direct: false, ..existing.map(|record| record.network.clone()).unwrap_or_default() })),
        inline_proxy: form.inline_proxy.as_ref().map(proxy_input),
    }
}

pub fn proxy_input(form: &ProxyEditForm) -> ProxyInput {
    let username = form.username.trim();
    ProxyInput {
        kind: form.kind,
        host: form.host.trim().to_string(),
        port: form.port,
        username: (!username.is_empty()).then(|| username.to_string()),
        // Без логина пароль не хранится: иначе осталась бы авторизация без имени.
        password: if username.is_empty() { SecretField::Clear } else { form.password.to_field() },
        note: form.note.trim().to_string(),
        enabled: form.enabled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::proxies::ProxyKind;
    use crate::fingerprint::{Catalog, OsRandom, generate};

    fn form() -> ProfileForm {
        ProfileForm {
            name: "Магазин".into(),
            note: String::new(),
            platform_domain: String::new(),
            start_urls: vec!["https://example.com/".into()],
            extra_args: vec![],
            username: String::new(),
            password: SecretInput::Keep,
            totp: SecretInput::Keep,
            fingerprint: generate(Catalog::builtin(), Arch::host(), &mut OsRandom).unwrap(),
            proxy_id: Some(Some(3)),
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

    fn proxy() -> ProxyEditForm {
        ProxyEditForm {
            kind: ProxyKind::Socks5,
            host: "gw.example.net".into(),
            port: 1080,
            username: "user".into(),
            password: SecretInput::Set { value: "pass".into() },
            note: String::new(),
            enabled: true,
        }
    }

    fn codes(errors: &[FieldError]) -> Vec<(&'static str, &'static str)> {
        errors.iter().map(|e| (e.field, e.code)).collect()
    }

    #[test]
    fn valid_form_has_no_errors() {
        assert_eq!(validate_form(&form(), Arch::host()), vec![]);
        assert_eq!(validate_proxy_form(&proxy(), false), vec![]);
    }

    #[test]
    fn all_errors_are_reported_at_once() {
        let mut f = form();
        f.name = "   ".into();
        f.start_urls = vec!["  --proxy-server=direct://".into()];
        f.extra_args = vec!["--user-data-dir=/tmp/x".into()];
        f.fingerprint.hardware_concurrency = 64;
        assert_eq!(
            codes(&validate_form(&f, Arch::host())),
            [
                ("name", "required"),
                ("start_urls", "invalid"),
                ("extra_args", "forbidden"),
                ("hardware_concurrency", "out_of_range")
            ]
        );
        f.extra_args = vec!["disable-sync".into()];
        assert!(codes(&validate_form(&f, Arch::host())).contains(&("extra_args", "not_a_flag")));
        f.name = "я".repeat(101);
        assert!(codes(&validate_form(&f, Arch::host())).contains(&("name", "too_long")));
        let mut p = proxy();
        p.host = " ".into();
        p.port = 0;
        assert_eq!(codes(&validate_proxy_form(&p, false)), [("proxy_host", "required"), ("proxy_port", "out_of_range")]);
    }

    #[test]
    fn proxy_login_and_password_go_together() {
        let mut p = proxy();
        p.password = SecretInput::Keep;
        assert_eq!(codes(&validate_proxy_form(&p, false)), [("proxy_password", "required")]);
        assert_eq!(validate_proxy_form(&p, true), vec![], "сохранённый пароль годится");
        p.username = " ".into();
        p.password = SecretInput::Set { value: "pass".into() };
        assert_eq!(codes(&validate_proxy_form(&p, false)), [("proxy_username", "required")]);
        let input = proxy_input(&p);
        assert_eq!(input.username, None);
        assert_eq!(input.password, SecretField::Clear, "без логина пароль не сохраняется");
    }

    #[test]
    fn inputs_are_trimmed_and_secrets_hidden() {
        let mut f = form();
        f.name = "  Магазин  ".into();
        f.start_urls = vec![" https://a.example/ ".into(), "  ".into()];
        f.username = "  ".into();
        let input = profile_input(&f, None);
        assert_eq!(input.name, "Магазин");
        assert_eq!(input.start_urls, ["https://a.example/"]);
        assert_eq!(input.username, None);
        assert_eq!(input.proxy_id, Some(3));
        assert!(!format!("{:?}", SecretInput::Set { value: "hunter2".into() }).contains("hunter2"));
        let parsed: SecretInput = serde_json::from_str(r#"{"action":"set","value":"x"}"#).unwrap();
        assert!(matches!(parsed, SecretInput::Set { .. }));
        let parsed: SecretInput = serde_json::from_str(r#"{"action":"keep"}"#).unwrap();
        assert!(matches!(parsed, SecretInput::Keep));
    }

    #[test]
    fn form_debug_hides_logins_and_passwords() {
        let mut f = form();
        f.username = "bob-account".into();
        f.password = SecretInput::Set { value: "s3cret".into() };
        let text = format!("{f:?}");
        for secret in ["bob-account", "s3cret"] {
            assert!(!text.contains(secret), "{secret} попал в Debug");
        }
        assert!(text.contains("proxy_id"), "{text}");
        let mut p = proxy();
        p.username = "alice-proxy".into();
        p.password = SecretInput::Set { value: "hunter2".into() };
        let text = format!("{p:?}");
        for secret in ["alice-proxy", "hunter2"] {
            assert!(!text.contains(secret), "{secret} попал в Debug");
        }
        assert!(text.contains("gw.example.net"));
    }

    fn json_form(extra: serde_json::Value) -> ProfileForm {
        let fingerprint: serde_json::Value = serde_json::from_str(&form().fingerprint.to_json()).unwrap();
        let mut json = serde_json::json!({
            "name": "a", "note": "", "platform_domain": "", "start_urls": [], "extra_args": [],
            "username": "", "password": {"action": "keep"}, "fingerprint": fingerprint
        });
        json.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn group_and_restore_tabs_distinguish_absent_null_and_value() {
        let absent = json_form(serde_json::json!({}));
        assert_eq!((absent.group_id, absent.restore_tabs), (None, None), "редактор этапа 1 этих полей не шлёт");
        assert_eq!(json_form(serde_json::json!({"group_id": null})).group_id, Some(None));
        let set = json_form(serde_json::json!({"group_id": 4, "restore_tabs": false}));
        assert_eq!((set.group_id, set.restore_tabs), (Some(Some(4)), Some(false)));

        let input = profile_input(&absent, None);
        assert_eq!((input.group_id, input.restore_tabs), (None, true), "создание: без группы, вкладки восстанавливаются");
        let input = profile_input(&set, None);
        assert_eq!((input.group_id, input.restore_tabs), (Some(4), false));
    }

    #[test]
    fn tags_distinguish_absent_and_list_and_are_checked() {
        assert_eq!(json_form(serde_json::json!({})).tags, None, "редактор этапа 1 и Local API тегов не шлют");
        assert_eq!(json_form(serde_json::json!({"tags": []})).tags, Some(vec![]));
        assert_eq!(json_form(serde_json::json!({"tags": ["a"]})).tags, Some(vec!["a".to_string()]));
        let mut f = form();
        f.tags = Some(vec!["я".repeat(30), "  ".into()]);
        assert_eq!(validate_form(&f, Arch::host()), vec![]);
        f.tags = Some(vec!["я".repeat(31)]);
        assert_eq!(codes(&validate_form(&f, Arch::host())), [("tags", "invalid")]);
        assert!(format!("{f:?}").contains("tags"));
    }

    #[test]
    fn bookmarks_are_checked() {
        use crate::store::bookmarks::Bookmark;
        assert_eq!(json_form(serde_json::json!({})).bookmarks, None);
        let mark = |title: &str, url: &str| Bookmark { title: title.into(), url: url.into() };
        let bad = |list: Vec<Bookmark>| {
            let mut f = form();
            f.bookmarks = Some(list);
            codes(&validate_form(&f, Arch::host()))
        };
        assert_eq!(bad(vec![mark("a", "https://a.test/"), mark("b", "http://b.test")]), []);
        for list in [
            vec![mark("a", "ftp://a.test/")],
            vec![mark("a", "javascript:alert(1)")],
            vec![mark("  ", "https://a.test/")],
            vec![mark(&"я".repeat(201), "https://a.test/")],
            (0..501).map(|i| mark("a", &format!("https://a{i}.test/"))).collect(),
        ] {
            assert_eq!(bad(list), [("bookmarks", "invalid")]);
        }
    }

    #[test]
    fn proxy_id_distinguishes_absent_null_and_value() {
        let absent = json_form(serde_json::json!({}));
        assert_eq!(absent.proxy_id, None, "поля нет — шлюз не меняется");
        assert_eq!(profile_input(&absent, None).proxy_id, None, "создание без поля — без закрепления");
        assert_eq!(json_form(serde_json::json!({"proxy_id": null})).proxy_id, Some(None));
        let set = json_form(serde_json::json!({"proxy_id": 7}));
        assert_eq!(set.proxy_id, Some(Some(7)));
        assert_eq!(profile_input(&set, None).proxy_id, Some(7));
        // Форма этапа 1 со встроенным прокси: поле молча игнорируется, закрепление не меняется.
        let legacy = json_form(serde_json::json!({"proxy": {
            "kind": "http", "host": "gw.example.net", "port": 8080, "username": "", "password": {"action": "keep"}
        }}));
        assert_eq!(legacy.proxy_id, None);
        assert_eq!(json_form(serde_json::json!({"proxy": null})).proxy_id, None);
    }
}
