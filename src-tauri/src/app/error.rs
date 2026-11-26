use crate::engine::EngineError;
use crate::fingerprint::{FieldError, GenerateError};
use crate::store::StoreError;
use crate::supervisor::SupervisorError;
use serde::Serialize;

/// Все коды ошибок команд: интерфейс переводит их по `errors.<код>`, Local API этапа 2 отдаёт как есть.
pub const ERROR_CODES: &[&str] = &[
    "SecurityCredentials", "SecurityConfigured", "SecurityStorage", "SecurityUnsupported", "SecurityBusy", "SecurityMigration",
    "SecurityRecoveryRequired", "SecurityRequired", "SecurityRateLimited", "SecurityDevice", "SecurityLocked", "SecurityExpired", "SecurityPassword",
    "SecurityLegacyKey",
    "NotFound",
    "AlreadyRunning",
    "NotRunning",
    "LicenseWaiting",
    "LicenseHydraUnavailable", "LicenseSignInCancelled", "LicenseSignInTimeout", "LicenseSignInFailed",
    "LicenseNotFound", "LicenseServerUnavailable", "LicenseSignedOut", "LicenseKeyInvalid",
    "BrowserAccessRequired", "BrowserAccessInvalid",
    "FreeDeviceBound", "DeviceIdentityUnavailable",
    "AccountUnavailable", "AccountBadCredentials", "AccountCodeInvalid", "AccountCodeReused", "AccountChallengeExpired",
    "AccountRateLimited", "AccountSignedOut", "AccountAlreadyEnabled", "AccountStorage", "AccountMfaRequired",
    "ProfileRunning",
    "ProfileLocked",
    "EngineMissing",
    "EngineInstall",
    "EngineBusy",
    "EngineUpdateUnavailable", "EngineUpdateInvalid", "EngineUpdateIncompatible", "EngineUpdatePlatform",
    "EngineUpdateCancelled", "EngineUpdateBusy",
    "Engine",
    "NoFreeProxy",
    "ProxyInUse",
    "ProxyUnavailable",
    "GeoUnavailable",
    "BadFingerprint",
    "BadArgs",
    "LaunchFailed",
    "StopFailed",
    "CookiesFailed",
    "Cdp",
    "SecretsUnreadable",
    "Keychain",
    "Storage",
    "Io",
    "Validation",
    "Fingerprint",
    "GroupExists",
    "ImportFailed",
    "BadParams",
    "TaskRunning",
    "ProfileBusy",
    "SyncRunning",
    "TaskChanged",
    "ArchivePassword",
    "ArchiveFormat",
    "ArchiveTooLarge",
    "ExtensionDownload",
    "ExtensionFormat",
    "Internal",
];

/// Ответ команды при ошибке. `code` — стабильная строка; в `message` не попадают логины, пароли и ключи.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<FieldError>>,
    #[serde(rename = "ownerHint", skip_serializing_if = "Option::is_none")]
    pub owner_hint: Option<String>,
}

impl CommandError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into(), fields: None, owner_hint: None }
    }

    pub fn validation(fields: Vec<FieldError>) -> Self {
        Self { code: "Validation".into(), message: "проверьте поля формы".into(), fields: Some(fields), owner_hint: None }
    }
}

impl From<StoreError> for CommandError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound(_) => Self::new("NotFound", e.to_string()),
            StoreError::Invalid(field) => Self::validation(vec![FieldError { field, code: "invalid" }]),
            StoreError::Duplicate("group") => Self::new("GroupExists", "группа с таким названием уже есть"),
            StoreError::Duplicate("ext_category") => Self::validation(vec![FieldError { field: "name", code: "taken" }]),
            StoreError::Duplicate("profile_label") => Self::validation(vec![FieldError { field: "name", code: "taken" }]),
            StoreError::Duplicate(_) => Self::new("Storage", e.to_string()),
            StoreError::Crypto => Self::new(
                "SecretsUnreadable",
                "Не удалось открыть сохранённые пароли: ключ не подходит. Проверьте прежний ключ в резервной копии данных.",
            ),
            StoreError::Keychain(_) => Self::new("Keychain", e.to_string()),
            StoreError::Db(_) => Self::new("Storage", e.to_string()),
            StoreError::Io(_) => Self::new("Io", e.to_string()),
            StoreError::TaskRunning(_) => Self::new("TaskRunning", e.to_string()),
            StoreError::TaskChanged => Self::new("TaskChanged", e.to_string()),
        }
    }
}

impl From<SupervisorError> for CommandError {
    fn from(e: SupervisorError) -> Self {
        match e {
            SupervisorError::Store(inner) => inner.into(),
            SupervisorError::Launch(inner @ EngineError::FreeDeviceBound { .. }) => inner.into(),
            other => Self::new(other.code(), other.to_string()),
        }
    }
}

impl From<EngineError> for CommandError {
    fn from(e: EngineError) -> Self {
        let code = match &e {
            EngineError::NotInstalled => "EngineMissing",
            EngineError::Install(_) => "EngineInstall",
            EngineError::Update(code) => code,
            EngineError::AccessRequired => "BrowserAccessRequired",
            EngineError::AccessInvalid => "BrowserAccessInvalid",
            EngineError::FreeDeviceBound { .. } => "FreeDeviceBound",
            EngineError::DeviceIdentityUnavailable => "DeviceIdentityUnavailable",
            _ => "Engine",
        };
        let mut error = Self::new(code, e.to_string());
        if let EngineError::FreeDeviceBound { owner_hint } = e {
            error.owner_hint = crate::engine::masked_owner_hint(owner_hint.as_deref());
        }
        error
    }
}

impl From<GenerateError> for CommandError {
    fn from(e: GenerateError) -> Self {
        Self::new("Fingerprint", e.to_string())
    }
}

impl From<crate::cookies::FormatError> for CommandError {
    fn from(e: crate::cookies::FormatError) -> Self {
        Self::new("ImportFailed", e.to_string())
    }
}

impl From<std::io::Error> for CommandError {
    fn from(e: std::io::Error) -> Self {
        Self::new("Io", e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_have_stable_codes() {
        assert_eq!(CommandError::from(SupervisorError::AlreadyRunning).code, "AlreadyRunning");
        assert_eq!(CommandError::from(SupervisorError::Store(StoreError::Crypto)).code, "SecretsUnreadable");
        assert_eq!(
            CommandError::from(StoreError::Invalid("name")).fields,
            Some(vec![FieldError { field: "name", code: "invalid" }])
        );
        assert_eq!(CommandError::from(EngineError::NotInstalled).code, "EngineMissing");
        let json = serde_json::to_value(CommandError::new("NoProxy", "у профиля нет прокси")).unwrap();
        assert_eq!(json, serde_json::json!({"code": "NoProxy", "message": "у профиля нет прокси"}));
    }

    #[test]
    fn every_supervisor_code_is_listed() {
        let samples = [
            SupervisorError::NotFound,
            SupervisorError::AlreadyRunning,
            SupervisorError::NotRunning,
            SupervisorError::Cancelled,
            SupervisorError::StillStarting,
            SupervisorError::LicenseWaiting,
            SupervisorError::ProfileRunning,
            SupervisorError::ProfileLocked,
            SupervisorError::EngineMissing,
            SupervisorError::NoFreeProxy,
            SupervisorError::ProxyInUse,
            SupervisorError::ProxyUnavailable,
            SupervisorError::GeoUnavailable,
            SupervisorError::BadFingerprint,
            SupervisorError::Args(crate::engine::args::ArgsError::NotAFlag("x".into())),
            SupervisorError::Launch(EngineError::ReadyTimeout),
            SupervisorError::Stop(EngineError::ReadyTimeout),
            SupervisorError::Cookies("x".into()),
            SupervisorError::Store(StoreError::Crypto),
            SupervisorError::Store(StoreError::Invalid("x")),
            SupervisorError::Io(std::io::Error::other("x")),
        ];
        for error in samples {
            assert!(ERROR_CODES.contains(&error.code()), "{} нет в ERROR_CODES", error.code());
        }
    }

    #[test]
    fn every_error_code_is_translated() {
        let dictionaries =
            [("ru", include_str!("../../../src/i18n/ru.json")), ("en", include_str!("../../../src/i18n/en.json"))];
        for (language, text) in dictionaries {
            let dictionary: serde_json::Value = serde_json::from_str(text).unwrap();
            for code in ERROR_CODES {
                assert!(dictionary["errors"][code].is_string(), "нет перевода errors.{code} в {language}.json");
            }
        }
    }

    #[test]
    fn duplicate_group_is_group_exists() {
        assert_eq!(CommandError::from(StoreError::Duplicate("group")).code, "GroupExists");
        assert_eq!(
            CommandError::from(StoreError::Duplicate("ext_category")).fields,
            Some(vec![FieldError { field: "name", code: "taken" }])
        );
        assert_eq!(CommandError::from(StoreError::Duplicate("other")).code, "Storage");
    }

    #[test]
    fn format_errors_are_import_failed() {
        for error in [
            crate::cookies::FormatError::Empty,
            crate::cookies::FormatError::Json("expected value".into()),
            crate::cookies::FormatError::Unrecognized,
        ] {
            assert_eq!(CommandError::from(error).code, "ImportFailed");
        }
    }
}
