//! Расширения Chrome: идентификатор, пакет CRX3, распаковка и манифест (спецификация 3Б). Без сети и базы.
pub mod crx;
pub mod id;
pub mod package;

/// Почему пакет не подошёл; `reason` уходит в поле ошибки `ExtensionFormat` без содержимого файла.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FormatError {
    #[error("не пакет CRX3")]
    Crx,
    #[error("не архив zip")]
    Zip,
    #[error("нет манифеста или он неверен")]
    Manifest,
    #[error("движок не загружает расширения Manifest V2")]
    Mv2,
    #[error("пакет больше допустимого")]
    TooLarge,
    #[error("идентификатор расширения не совпал")]
    IdMismatch,
}

impl FormatError {
    pub fn reason(self) -> &'static str {
        match self {
            Self::Crx => "crx",
            Self::Zip => "zip",
            Self::Manifest => "manifest",
            Self::Mv2 => "mv2",
            Self::TooLarge => "too_large",
            Self::IdMismatch => "id_mismatch",
        }
    }
}

/// Ошибка распаковки: пакет плохой — `Format`, диск — `Io`.
#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error(transparent)]
    Format(#[from] FormatError),
    #[error("файл: {0}")]
    Io(#[from] std::io::Error),
}
