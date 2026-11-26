//! Числовой порядок Chromium и номера выпуска Ombra; строковое сравнение здесь неверно.
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub chromium: [u32; 4],
    pub release: u32,
}

impl FromStr for Version {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (chromium, release) = text.split_once('-').unwrap_or((text, "0"));
        let numbers = chromium.split('.').map(|part| {
            if part.is_empty() || !part.bytes().all(|c| c.is_ascii_digit()) { return Err(()); }
            part.parse::<u32>().map_err(|_| ())
        }).collect::<Result<Vec<_>, _>>()?;
        if numbers.len() != 4 || release.is_empty() || !release.bytes().all(|c| c.is_ascii_digit()) { return Err(()); }
        Ok(Self { chromium: numbers.try_into().map_err(|_| ())?, release: release.parse().map_err(|_| ())? })
    }
}

impl Version {
    pub fn stable(text: &str) -> Option<Self> {
        let version = text.parse::<Self>().ok()?;
        (version.release > 0).then_some(version)
    }
}

pub fn platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-x64",
        ("windows", "x86_64") => "windows-x64",
        ("linux", "x86_64") => "linux-x64",
        _ => "unsupported",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_chromium_then_numeric_release() {
        assert!(Version::stable("151.0.7922.108-10") > Version::stable("151.0.7922.108-9"));
        assert!(Version::stable("152.0.1.1-1") > Version::stable("151.0.7922.108-99"));
        assert!(Version::stable("151.0.7922.108-1").unwrap() > "151.0.7922.108".parse().unwrap());
    }

    #[test]
    fn stable_rejects_development_versions_and_paths() {
        for text in ["151.0.7922.108", "151.0.7922.108-0", "151.0.7922.108-0+dev.abc", "../151.0.1.1-1", "151.0.1-1", "151.0.1.1-+1"] {
            assert!(Version::stable(text).is_none(), "{text}");
        }
    }
}
