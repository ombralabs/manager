use super::*;
use super::super::install::tests::fake_app;
use axum::{Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use ring::signature::KeyPair;
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

fn signer() -> ring::signature::Ed25519KeyPair {
    ring::signature::Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap()
}

fn signed(value: serde_json::Value) -> Vec<u8> {
    let text = serde_json::to_string(&value).unwrap();
    serde_json::to_vec(&serde_json::json!({
        "signed": text,
        "sig": STANDARD.encode(signer().sign(text.as_bytes()).as_ref()),
    }))
    .unwrap()
}

fn asset_filename(version: &str) -> String {
    format!("ombra-engine-{version}-{}.{}", platform(), extension(platform()).unwrap())
}

fn manifest_json(version: &str, archive: &[u8]) -> serde_json::Value {
    let chromium = version.split('-').next().unwrap();
    serde_json::json!({
        "schema": 1,
        "type": "release",
        "version": version,
        "chromium": chromium,
        "minManager": "1.2.0",
        "platforms": {
            platform(): {
                "file": asset_filename(version),
                "sha256": format!("{:x}", Sha256::digest(archive)),
                "size": archive.len(),
            }
        }
    })
}

fn github_release(base: &str, version: &str, manifest_len: usize, archive_len: usize) -> GithubRelease {
    let file = asset_filename(version);
    GithubRelease {
        tag_name: format!("v{version}"),
        draft: false,
        prerelease: false,
        assets: vec![
            GithubAsset {
                name: "release.json".into(),
                size: manifest_len as u64,
                browser_download_url: format!("{base}/download/v{version}/release.json"),
            },
            GithubAsset {
                name: file.clone(),
                size: archive_len as u64,
                browser_download_url: format!("{base}/download/v{version}/{file}"),
            },
        ],
    }
}

fn source(base: &str) -> Source {
    Source {
        releases_url: Url::parse(&format!("{base}/releases")).unwrap(),
        download_url: Url::parse(&format!("{base}/download")).unwrap(),
        public_key: signer().public_key().as_ref().to_vec(),
        platform: platform().into(),
        manager: "1.2.3".into(),
        allow_local_http: true,
    }
}

fn archive(version: &str) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let chromium = version.split('-').next().unwrap();
    let app = fake_app(dir.path(), Some(chromium));
    let metadata = catalog::metadata_path(&app);
    std::fs::create_dir_all(metadata.parent().unwrap()).unwrap();
    std::fs::write(
        metadata,
        serde_json::json!({ "version": version, "chromium": chromium, "platform": platform() }).to_string(),
    )
    .unwrap();
    if cfg!(target_os = "macos") {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        tar.append_dir_all(install::APP_NAME, &app).unwrap();
        tar.into_inner().unwrap().finish().unwrap()
    } else {
        let xz = xz2::write::XzEncoder::new(Vec::new(), 1);
        let mut tar = tar::Builder::new(xz);
        tar.append_dir_all(".", &app).unwrap();
        tar.into_inner().unwrap().finish().unwrap()
    }
}

#[derive(Clone)]
struct Data {
    latest: Option<GithubRelease>,
    releases: Vec<GithubRelease>,
    manifest: Vec<u8>,
    archive: Vec<u8>,
    downloads: Arc<AtomicUsize>,
    slow: bool,
    redirect_to: Option<String>,
}

struct Server {
    source: Source,
    downloads: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn spawn_server(
    version: &str,
    manifest: Vec<u8>,
    archive: Vec<u8>,
    slow: bool,
    latest_is_sdk: bool,
    redirect_to: Option<String>,
) -> Server {
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", socket.local_addr().unwrap());
    let release = github_release(&base, version, manifest.len(), archive.len());
    let sdk_release = GithubRelease {
        tag_name: "sdk-v0.1.0".into(),
        draft: false,
        prerelease: false,
        assets: vec![],
    };
    let data = Data {
        latest: Some(if latest_is_sdk { sdk_release.clone() } else { release.clone() }),
        releases: vec![sdk_release, release],
        manifest,
        archive,
        downloads: Arc::new(AtomicUsize::new(0)),
        slow,
        redirect_to,
    };
    let downloads = data.downloads.clone();
    let router = Router::new()
        .route("/releases/latest", get(|State(d): State<Data>| async move {
            match d.latest {
                Some(r) => (StatusCode::OK, serde_json::to_vec(&r).unwrap()).into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }))
        .route("/releases", get(|State(d): State<Data>| async move {
            (StatusCode::OK, serde_json::to_vec(&d.releases).unwrap())
        }))
        .route("/download/{tag}/{file}", get(|State(d): State<Data>, axum::extract::Path((_tag, file)): axum::extract::Path<(String, String)>| async move {
            if file == "release.json" {
                return (StatusCode::OK, d.manifest).into_response();
            }
            if let Some(target) = d.redirect_to {
                return (StatusCode::FOUND, [(axum::http::header::LOCATION, target)]).into_response();
            }
            d.downloads.fetch_add(1, Ordering::SeqCst);
            if !d.slow {
                return (StatusCode::OK, axum::body::Body::from(d.archive)).into_response();
            }
            (
                StatusCode::OK,
                axum::body::Body::from_stream(futures_util::stream::unfold((d.archive, 0), |(bytes, position)| async move {
                    if position >= bytes.len() {
                        return None;
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    let end = (position + bytes.len().div_ceil(2)).min(bytes.len());
                    Some((Ok::<_, std::io::Error>(bytes[position..end].to_vec()), (bytes, end)))
                })),
            )
                .into_response()
        }))
        .route("/asset-blob", get(|State(d): State<Data>| async move {
            d.downloads.fetch_add(1, Ordering::SeqCst);
            (StatusCode::OK, axum::body::Body::from(d.archive))
        }))
        .with_state(data);
    let task = tokio::spawn(async move { axum::serve(socket, router).await.unwrap() });
    Server { source: source(&base), downloads, task }
}

#[test]
fn checks_signature_tag_compatibility_and_asset_identity() {
    let base = "http://127.0.0.1:1234";
    let src = source(base);
    let value = manifest_json("158.0.8127.37-3", b"archive");
    let raw = signed(value.clone());
    let gh = github_release(base, "158.0.8127.37-3", raw.len(), 7);
    assert!(src.verify(&raw, &gh).is_ok());

    let mut forged: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    forged["signed"] = serde_json::Value::String(forged["signed"].as_str().unwrap().replace("158.0.8127.37-3", "158.0.8127.37-4"));
    let forged_raw = serde_json::to_vec(&forged).unwrap();
    let gh_forged = github_release(base, "158.0.8127.37-3", forged_raw.len(), 7);
    assert!(src.verify(&forged_raw, &gh_forged).is_err());

    let mut bad_manager = value.clone();
    bad_manager["minManager"] = "2.0.0".into();
    let raw_bad_mgr = signed(bad_manager);
    let gh_mgr = github_release(base, "158.0.8127.37-3", raw_bad_mgr.len(), 7);
    assert!(matches!(src.verify(&raw_bad_mgr, &gh_mgr), Err(EngineError::Update("EngineUpdateIncompatible"))));

    for (field, changed) in [("file", "wrong.tar.gz"), ("sha256", "bad")] {
        let mut bad = value.clone();
        bad["platforms"][platform()][field] = changed.into();
        let bad_raw = signed(bad);
        let gh_bad = github_release(base, "158.0.8127.37-3", bad_raw.len(), 7);
        assert!(src.verify(&bad_raw, &gh_bad).is_err(), "{field}");
    }

    let mut draft = gh.clone();
    draft.draft = true;
    assert!(src.verify(&raw, &draft).is_err());

    let mut wrong_url = gh;
    wrong_url.assets[1].browser_download_url = "https://evil.example/engine.tar.gz".into();
    assert!(src.verify(&raw, &wrong_url).is_err());
}

#[tokio::test]
async fn downloads_newer_stable_skips_sdk_latest_and_reuses_cached_archive() {
    let dir = tempfile::tempdir().unwrap();
    fake_app(dir.path(), Some("150.0.1.1"));
    let bytes = archive("158.0.8127.37-3");
    let manifest = signed(manifest_json("158.0.8127.37-3", &bytes));
    let server = spawn_server("158.0.8127.37-3", manifest, bytes, false, true, None).await;
    let outcome = ensure_latest(dir.path(), &server.source, true, CancellationToken::new(), |_| {}).await.unwrap();
    assert_eq!(outcome.state, "updated");
    assert_eq!(outcome.catalog.versions.len(), 2);
    assert_eq!(catalog::active(dir.path()).unwrap().version, "158.0.8127.37");
    assert_eq!(install::read_version(&install::app_dir(dir.path())).as_deref(), Some("150.0.1.1"));

    let same = ensure_latest(dir.path(), &server.source, false, CancellationToken::new(), |_| {}).await.unwrap();
    assert_eq!(same.state, "current");
    assert_eq!(server.downloads.load(Ordering::SeqCst), 1);

    catalog::select(dir.path(), "legacy", false).unwrap();
    let pinned = ensure_latest(dir.path(), &server.source, true, CancellationToken::new(), |_| {}).await.unwrap();
    assert_eq!(pinned.state, "pinned");
    assert_eq!(catalog::active(dir.path()).unwrap().version, "150.0.1.1");

    let again = ensure_latest(dir.path(), &server.source, false, CancellationToken::new(), |_| {}).await.unwrap();
    assert_eq!(again.state, "updated");
    assert!(!again.catalog.automatic, "ручная проверка не включает выключенное автообновление");
    assert_eq!(server.downloads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn follows_allowed_redirect_and_rejects_disallowed_redirect_host() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = archive("158.0.8127.37-3");
    let manifest = signed(manifest_json("158.0.8127.37-3", &bytes));
    let evil = spawn_server("158.0.8127.37-3", manifest.clone(), bytes.clone(), false, false, Some("https://evil.example/blob".into())).await;
    assert!(matches!(ensure_latest(dir.path(), &evil.source, false, CancellationToken::new(), |_| {}).await, Err(EngineError::Update(INVALID))));

    let ok_redirect = spawn_server("158.0.8127.37-3", manifest, bytes, false, false, Some("/asset-blob".into())).await;
    let outcome = ensure_latest(dir.path(), &ok_redirect.source, false, CancellationToken::new(), |_| {}).await.unwrap();
    assert_eq!(outcome.state, "updated");
    assert_eq!(ok_redirect.downloads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn never_downloads_or_downgrades_when_installed_chromium_is_newer() {
    let dir = tempfile::tempdir().unwrap();
    fake_app(dir.path(), Some("159.0.1.1"));
    let bytes = archive("158.0.8127.37-3");
    let manifest = signed(manifest_json("158.0.8127.37-3", &bytes));
    let server = spawn_server("158.0.8127.37-3", manifest, bytes, false, false, None).await;
    let result = ensure_latest(dir.path(), &server.source, true, CancellationToken::new(), |_| {}).await.unwrap();
    assert_eq!(result.state, "current");
    assert_eq!(server.downloads.load(Ordering::SeqCst), 0);
    assert_eq!(catalog::active(dir.path()).unwrap().version, "159.0.1.1");
}

#[tokio::test]
async fn bad_hash_size_or_internal_version_never_changes_active_browser() {
    for problem in ["hash", "size", "version"] {
        let dir = tempfile::tempdir().unwrap();
        fake_app(dir.path(), Some("150.0.1.1"));
        let bytes = archive(if problem == "version" { "158.0.8127.37-2" } else { "158.0.8127.37-3" });
        let mut manifest = manifest_json("158.0.8127.37-3", &bytes);
        if problem == "hash" {
            manifest["platforms"][platform()]["sha256"] = "0".repeat(64).into();
        }
        if problem == "size" {
            manifest["platforms"][platform()]["size"] = ((bytes.len() + 5) as u64).into();
        }
        let server = spawn_server("158.0.8127.37-3", signed(manifest), bytes, false, false, None).await;
        assert!(ensure_latest(dir.path(), &server.source, true, CancellationToken::new(), |_| {}).await.is_err(), "{problem}");
        assert_eq!(catalog::active(dir.path()).unwrap().version, "150.0.1.1");
        assert_eq!(catalog::list(dir.path()).unwrap().versions.len(), 1);
        assert!(!std::fs::read_dir(dir.path()).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".download-")));
    }
}

#[tokio::test]
async fn cancellation_removes_partial_download_and_keeps_previous_browser() {
    let dir = tempfile::tempdir().unwrap();
    fake_app(dir.path(), Some("150.0.1.1"));
    let bytes = archive("158.0.8127.37-3");
    let manifest = signed(manifest_json("158.0.8127.37-3", &bytes));
    let server = spawn_server("158.0.8127.37-3", manifest, bytes, true, false, None).await;
    let cancel = CancellationToken::new();
    let result = ensure_latest(dir.path(), &server.source, true, cancel.clone(), |event| {
        if event.phase == "downloading" && event.downloaded > 0 {
            assert!(event.downloaded < event.total.unwrap());
            cancel.cancel();
        }
    })
    .await;
    assert!(matches!(result, Err(EngineError::Update(CANCELLED))));
    assert_eq!(server.downloads.load(Ordering::SeqCst), 1);
    assert_eq!(catalog::active(dir.path()).unwrap().version, "150.0.1.1");
    assert!(!std::fs::read_dir(dir.path()).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".download-")));
}

#[tokio::test]
async fn installs_the_first_browser_and_emits_all_stages() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = archive("158.0.8127.37-3");
    let manifest = signed(manifest_json("158.0.8127.37-3", &bytes));
    let server = spawn_server("158.0.8127.37-3", manifest, bytes, false, false, None).await;
    let phases = std::sync::Mutex::new(Vec::new());
    let outcome = ensure_latest(dir.path(), &server.source, true, CancellationToken::new(), |event| {
        phases.lock().unwrap().push(event.phase);
    })
    .await
    .unwrap();
    assert_eq!(outcome.state, "updated");
    assert_eq!(outcome.catalog.versions.len(), 1);
    assert!(outcome.catalog.versions[0].active);
    assert!(outcome.catalog.automatic);
    let mut phases = phases.into_inner().unwrap();
    phases.dedup();
    assert_eq!(phases, ["checking", "downloading", "verifying", "installing", "complete"]);
}

#[tokio::test]
async fn manual_download_preserves_disabled_automatic_updates() {
    let dir = tempfile::tempdir().unwrap();
    catalog::set_automatic(dir.path(), false).unwrap();
    let bytes = archive("158.0.8127.37-3");
    let manifest = signed(manifest_json("158.0.8127.37-3", &bytes));
    let server = spawn_server("158.0.8127.37-3", manifest, bytes, false, false, None).await;
    let outcome = ensure_latest(dir.path(), &server.source, false, CancellationToken::new(), |_| {}).await.unwrap();
    assert_eq!(outcome.state, "updated");
    assert!(!outcome.catalog.automatic);
    assert!(outcome.catalog.versions[0].active);
}
