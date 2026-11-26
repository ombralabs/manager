//! Публичные подписанные выпуски браузера в GitHub Releases (`ombralabs/ombra`) → проверенный архив → новая неизменяемая папка браузера.
use super::{catalog, install, version::{Version, platform}};
use crate::engine::EngineError;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

const DEFAULT_RELEASES_URL: &str = "https://api.github.com/repos/ombralabs/ombra/releases";
const DEFAULT_DOWNLOAD_URL: &str = "https://github.com/ombralabs/ombra/releases/download";
const MAX_METADATA: usize = 2 * 1024 * 1024;
const MAX_ARCHIVE: u64 = 8 * 1024 * 1024 * 1024;
const INVALID: &str = "EngineUpdateInvalid";
const UNAVAILABLE: &str = "EngineUpdateUnavailable";
const CANCELLED: &str = "EngineUpdateCancelled";

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub phase: &'static str,
    pub version: Option<String>,
    pub downloaded: u64,
    pub total: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct Outcome {
    pub state: &'static str,
    pub catalog: catalog::Catalog,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct GithubRelease {
    pub tag_name: String,
    pub draft: bool,
    pub prerelease: bool,
    pub assets: Vec<GithubAsset>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct GithubAsset {
    pub name: String,
    pub size: u64,
    pub browser_download_url: String,
}

#[derive(Deserialize)]
struct Envelope {
    signed: String,
    sig: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseManifest {
    version: String,
    min_manager: String,
    chromium: String,
    #[serde(default)]
    commit: Option<String>,
    platforms: BTreeMap<String, PlatformAsset>,
}

#[derive(Deserialize)]
struct PlatformAsset {
    file: String,
    sha256: String,
    size: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct VerifiedRelease {
    pub version: String,
    pub sha256: String,
    pub size: u64,
    pub url: Url,
    pub commit: Option<String>,
}

pub struct Source {
    releases_url: Url,
    download_url: Url,
    public_key: Vec<u8>,
    platform: String,
    manager: String,
    allow_local_http: bool,
}

impl Source {
    pub fn from_build() -> Result<Self, EngineError> {
        let releases = option_env!("OMBRA_ENGINE_RELEASES_URL").unwrap_or(DEFAULT_RELEASES_URL);
        let download = option_env!("OMBRA_ENGINE_DOWNLOAD_URL").unwrap_or(DEFAULT_DOWNLOAD_URL);
        Ok(Self {
            releases_url: Url::parse(releases).map_err(|_| EngineError::Update(INVALID))?,
            download_url: Url::parse(download).map_err(|_| EngineError::Update(INVALID))?,
            public_key: STANDARD.decode(include_str!("../../../engine-signing.pub").trim()).map_err(|_| EngineError::Update(INVALID))?,
            platform: platform().into(),
            manager: env!("CARGO_PKG_VERSION").into(),
            allow_local_http: false,
        })
    }

    fn allowed_url(&self, url: &Url) -> bool {
        if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
            return false;
        }
        if self.allow_local_http
            && url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
        {
            return true;
        }
        url.scheme() == "https"
            && url.port_or_known_default() == Some(443)
            && (matches!(
                url.host_str(),
                Some(
                    "api.github.com"
                        | "github.com"
                        | "release-assets.githubusercontent.com"
                        | "objects.githubusercontent.com"
                )
            ) || url.host_str() == self.releases_url.host_str()
                || url.host_str() == self.download_url.host_str())
    }

    fn asset_url(&self, release: &GithubRelease, version: &str, filename: &str, size: Option<u64>) -> Result<Url, EngineError> {
        let invalid = || EngineError::Update(INVALID);
        let mut matches = release.assets.iter().filter(|asset| asset.name == filename);
        let asset = matches.next().ok_or_else(invalid)?;
        let base = self.download_url.as_str().trim_end_matches('/');
        let expected = format!("{base}/v{version}/{filename}");
        if matches.next().is_some()
            || asset.browser_download_url != expected
            || asset.size == 0
            || size.is_some_and(|n| asset.size != n)
        {
            return Err(invalid());
        }
        let url = Url::parse(&expected).map_err(|_| invalid())?;
        if !self.allowed_url(&url) {
            return Err(invalid());
        }
        Ok(url)
    }

    pub(crate) fn verify(&self, bytes: &[u8], release: &GithubRelease) -> Result<VerifiedRelease, EngineError> {
        let invalid = || EngineError::Update(INVALID);
        let ext = extension(&self.platform).ok_or(EngineError::Update("EngineUpdatePlatform"))?;
        if bytes.len() > MAX_METADATA {
            return Err(invalid());
        }
        let version = release_version(release).ok_or_else(invalid)?;
        self.asset_url(release, version, "release.json", Some(bytes.len() as u64))?;
        let envelope: Envelope = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        let signature = STANDARD.decode(envelope.sig).map_err(|_| invalid())?;
        if self.public_key.len() != 32 || signature.len() != 64 {
            return Err(invalid());
        }
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, &self.public_key)
            .verify(envelope.signed.as_bytes(), &signature)
            .map_err(|_| invalid())?;
        let payload: serde_json::Value = serde_json::from_str(&envelope.signed).map_err(|_| invalid())?;
        if payload.get("schema").is_some_and(|s| s != 1) || payload.get("type").is_some_and(|k| k != "release") {
            return Err(invalid());
        }
        let manifest: ReleaseManifest = serde_json::from_value(payload).map_err(|_| invalid())?;
        if manifest.version != version
            || manifest.chromium != version.split('-').next().unwrap_or("")
            || manifest.commit.as_deref().is_some_and(|s| s.len() != 40 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(invalid());
        }
        let minimum = manager_version(&manifest.min_manager).ok_or_else(invalid)?;
        if minimum > manager_version(&self.manager).ok_or_else(invalid)? {
            return Err(EngineError::Update("EngineUpdateIncompatible"));
        }
        let asset = manifest.platforms.get(&self.platform).ok_or(EngineError::Update("EngineUpdatePlatform"))?;
        let filename = format!("ombra-engine-{version}-{}.{ext}", self.platform);
        if asset.file != filename
            || asset.size == 0
            || asset.size > MAX_ARCHIVE
            || asset.sha256.len() != 64
            || !asset.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(invalid());
        }
        let url = self.asset_url(release, version, &filename, Some(asset.size))?;
        Ok(VerifiedRelease {
            version: version.into(),
            sha256: asset.sha256.to_ascii_lowercase(),
            size: asset.size,
            url,
            commit: manifest.commit,
        })
    }
}

fn release_version(release: &GithubRelease) -> Option<&str> {
    if release.draft || release.prerelease {
        return None;
    }
    let version = release.tag_name.strip_prefix('v')?;
    Version::stable(version)?;
    Some(version)
}

fn manager_version(value: &str) -> Option<[u32; 3]> {
    let parts: Vec<u32> = value.split('.').map(str::parse).collect::<Result<_, _>>().ok()?;
    parts.try_into().ok()
}

fn extension(platform: &str) -> Option<&'static str> {
    match platform {
        "darwin-arm64" | "darwin-x64" => Some("tar.gz"),
        "windows-x64" => Some("zip"),
        "linux-x64" => Some("tar.xz"),
        _ => None,
    }
}

async fn chunk(response: &mut reqwest::Response, cancel: &CancellationToken) -> Result<Option<Vec<u8>>, EngineError> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(EngineError::Update(CANCELLED)),
        data = tokio::time::timeout(Duration::from_secs(30), response.chunk()) =>
            data.map_err(|_| EngineError::Update(UNAVAILABLE))?
                .map(|part| part.map(|bytes| bytes.to_vec()))
                .map_err(|_| EngineError::Update(UNAVAILABLE)),
    }
}

async fn fetch(
    client: &reqwest::Client,
    source: &Source,
    start: Url,
    timeout: Duration,
    cancel: &CancellationToken,
) -> Result<reqwest::Response, EngineError> {
    let mut url = start;
    for _ in 0..=5 {
        if cancel.is_cancelled() {
            return Err(EngineError::Update(CANCELLED));
        }
        if !source.allowed_url(&url) {
            return Err(EngineError::Update(INVALID));
        }
        let response = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(EngineError::Update(CANCELLED)),
            res = client.get(url.clone()).timeout(timeout).send() => res.map_err(|_| EngineError::Update(UNAVAILABLE))?,
        };
        if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or(EngineError::Update(INVALID))?;
            url = url.join(location).map_err(|_| EngineError::Update(INVALID))?;
            continue;
        }
        if response.status() != reqwest::StatusCode::OK {
            return Err(EngineError::Update(UNAVAILABLE));
        }
        return Ok(response);
    }
    Err(EngineError::Update(INVALID))
}

async fn fetch_metadata(
    client: &reqwest::Client,
    source: &Source,
    url: Url,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, EngineError> {
    let mut response = fetch(client, source, url, Duration::from_secs(30), cancel).await?;
    if response.content_length().is_some_and(|n| n > MAX_METADATA as u64) {
        return Err(EngineError::Update(INVALID));
    }
    let mut bytes = Vec::new();
    while let Some(part) = chunk(&mut response, cancel).await? {
        if bytes.len() + part.len() > MAX_METADATA {
            return Err(EngineError::Update(INVALID));
        }
        bytes.extend_from_slice(&part);
    }
    Ok(bytes)
}

async fn resolve_release(
    client: &reqwest::Client,
    source: &Source,
    cancel: &CancellationToken,
) -> Result<VerifiedRelease, EngineError> {
    let base = source.releases_url.as_str().trim_end_matches('/');
    let latest_url = Url::parse(&format!("{base}/latest")).map_err(|_| EngineError::Update(INVALID))?;
    let mut release = match fetch_metadata(client, source, latest_url, cancel).await {
        Ok(bytes) => serde_json::from_slice::<GithubRelease>(&bytes)
            .ok()
            .filter(|r| release_version(r).is_some()),
        Err(EngineError::Update(UNAVAILABLE)) => None,
        Err(error) => return Err(error),
    };
    for page in 1..=20 {
        if release.is_some() {
            break;
        }
        let page_url = Url::parse(&format!("{base}?per_page=20&page={page}")).map_err(|_| EngineError::Update(INVALID))?;
        let bytes = fetch_metadata(client, source, page_url, cancel).await?;
        let list: Vec<GithubRelease> = serde_json::from_slice(&bytes).map_err(|_| EngineError::Update(INVALID))?;
        let last_page = list.len() < 20;
        release = list.into_iter().find(|r| release_version(r).is_some());
        if last_page {
            break;
        }
    }
    let release = release.ok_or(EngineError::Update(UNAVAILABLE))?;
    let version = release_version(&release).ok_or(EngineError::Update(INVALID))?;
    let manifest_url = source.asset_url(&release, version, "release.json", None)?;
    let raw = fetch_metadata(client, source, manifest_url, cancel).await?;
    source.verify(&raw, &release)
}

pub async fn ensure_latest(
    engines: &Path,
    source: &Source,
    background: bool,
    cancel: CancellationToken,
    progress: impl Fn(Progress) + Send + Sync,
) -> Result<Outcome, EngineError> {
    let current = catalog::list(engines)?;
    if background && !current.automatic {
        return Ok(Outcome { state: "pinned", catalog: current });
    }
    let active = current.versions.iter().find(|v| v.active);
    let current_version = active.and_then(|v| v.version.parse::<Version>().ok().or_else(|| v.chromium.parse().ok()));
    let emit = |phase, release: Option<&VerifiedRelease>, downloaded| {
        progress(Progress {
            phase,
            version: release.map(|r| r.version.clone()),
            downloaded,
            total: release.map(|r| r.size),
        });
    };
    emit("checking", None, 0);
    if !source.allowed_url(&source.releases_url) || !source.allowed_url(&source.download_url) {
        return Err(EngineError::Update(INVALID));
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(20))
        .user_agent(concat!("Ombra-Desktop/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| EngineError::Update(UNAVAILABLE))?;
    let release = resolve_release(&client, source, &cancel).await?;
    if current_version.is_some_and(|installed| installed >= Version::stable(&release.version).unwrap()) {
        emit("current", Some(&release), 0);
        return Ok(Outcome { state: "current", catalog: catalog::list(engines)? });
    }
    // Повторное использование уже скачанного выпуска не требует сети и не меняет его файлы.
    if current.versions.iter().any(|v| v.id == release.version) {
        let receipt = std::fs::read_to_string(engines.join("versions").join(&release.version).join("archive.sha256"))?;
        if receipt.trim() != release.sha256 {
            return Err(EngineError::Update(INVALID));
        }
        catalog::select(engines, &release.version, current.automatic)?;
        emit("complete", Some(&release), release.size);
        return Ok(Outcome { state: "updated", catalog: catalog::list(engines)? });
    }
    std::fs::create_dir_all(engines)?;
    let ext = extension(&source.platform).ok_or(EngineError::Update("EngineUpdatePlatform"))?;
    let temporary = tempfile::Builder::new()
        .prefix(".download-")
        .suffix(&format!(".{ext}"))
        .tempfile_in(engines)?;
    let mut output = tokio::fs::File::from_std(temporary.reopen()?);
    emit("downloading", Some(&release), 0);
    let mut response = fetch(&client, source, release.url.clone(), Duration::from_secs(30 * 60), &cancel).await?;
    if response.content_length().is_some_and(|n| n != release.size) {
        return Err(EngineError::Update(INVALID));
    }
    let mut hash = Sha256::new();
    let mut downloaded = 0u64;
    let mut last = Instant::now();
    while let Some(part) = chunk(&mut response, &cancel).await? {
        downloaded += part.len() as u64;
        if downloaded > release.size {
            return Err(EngineError::Update(INVALID));
        }
        hash.update(&part);
        output.write_all(&part).await?;
        if last.elapsed() >= Duration::from_millis(200) {
            emit("downloading", Some(&release), downloaded);
            last = Instant::now();
        }
    }
    emit("verifying", Some(&release), downloaded);
    if downloaded != release.size || format!("{:x}", hash.finalize()) != release.sha256 {
        return Err(EngineError::Update(INVALID));
    }
    output.flush().await?;
    output.sync_all().await?;
    drop(output);
    if cancel.is_cancelled() {
        return Err(EngineError::Update(CANCELLED));
    }
    emit("installing", Some(&release), downloaded);
    let dir = engines.to_owned();
    let version = release.version.clone();
    let commit = release.commit.clone();
    let automatic = current.automatic;
    let install_cancel = cancel.clone();
    // Файл принадлежит блокирующей задаче и удаляется после распаковки, в том числе при ошибке.
    tokio::task::spawn_blocking(move || {
        install::install_archive(&dir, temporary.path(), Some(&version), commit.as_deref(), automatic, &install_cancel)
    })
    .await
    .map_err(|_| EngineError::Install("установка прервана".into()))??;
    emit("complete", Some(&release), downloaded);
    Ok(Outcome { state: "updated", catalog: catalog::list(engines)? })
}

#[cfg(all(test, unix))]
#[path = "update_tests.rs"]
mod tests;
