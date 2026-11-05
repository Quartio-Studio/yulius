//! Claude Code kept current in the data directory (SPEC-421): a registry stub serves the
//! dist-tags, the platform package's version document and a tarball whose `claude` is a script
//! printing its version.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha512};
use yulius::wiring::claude_cli::{ClaudeUpdater, Outcome, agent_cli_source};
use yulius_domain::agent_cli::{Channel, Version};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const PLATFORM: &str = "@anthropic-ai/claude-code-linux-x64";

/// One release the stub offers.
#[derive(Clone)]
struct Release {
    tarball: Vec<u8>,
    /// What the version document says the tarball's sha512 is (`None`: the real one).
    integrity: Option<String>,
    /// Where the version document says the tarball is (`None`: on the stub).
    url: Option<String>,
}

#[derive(Default)]
struct Stub {
    tags: Mutex<HashMap<String, String>>,
    releases: Mutex<HashMap<String, Release>>,
    requests: AtomicUsize,
    downloads: AtomicUsize,
}

impl Stub {
    fn tag(&self, tag: &str, version: &str) {
        self.tags
            .lock()
            .expect("tags")
            .insert(tag.into(), version.into());
    }

    fn release(&self, version: &str, release: Release) {
        self.releases
            .lock()
            .expect("releases")
            .insert(version.into(), release);
    }
}

/// A tarball as npm publishes a platform package: `package/claude` and friends.
fn tarball(printed: &str) -> Vec<u8> {
    let script = format!("#!/bin/sh\necho '{printed} (Claude Code)'\n");
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    for (path, body, mode) in [
        ("package/package.json", "{}".to_owned(), 0o644),
        ("package/README.md", "readme".to_owned(), 0o644),
        ("package/claude", script, 0o755),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(body.len() as u64);
        header.set_mode(mode);
        header.set_cksum();
        builder
            .append_data(&mut header, path, body.as_bytes())
            .expect("entry");
    }
    builder
        .into_inner()
        .and_then(GzEncoder::finish)
        .expect("tarball")
}

fn good(version: &str) -> Release {
    Release {
        tarball: tarball(version),
        integrity: None,
        url: None,
    }
}

fn integrity_of(bytes: &[u8]) -> String {
    format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode(Sha512::digest(bytes))
    )
}

async fn serve(stub: Arc<Stub>) -> Result<String, Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let origin = base.clone();
    let router = Router::new().fallback(move |uri: Uri| {
        let stub = Arc::clone(&stub);
        let origin = origin.clone();
        async move { answer(&stub, &origin, uri.path()) }
    });
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(base)
}

fn answer(stub: &Stub, origin: &str, path: &str) -> Response {
    stub.requests.fetch_add(1, Ordering::SeqCst);
    if path == "/-/package/@anthropic-ai%2Fclaude-code/dist-tags" {
        let tags = stub.tags.lock().expect("tags").clone();
        return axum::Json(tags).into_response();
    }
    let releases = stub.releases.lock().expect("releases").clone();
    if let Some(version) = path.strip_prefix("/@anthropic-ai%2Fclaude-code-linux-x64/") {
        let Some(release) = releases.get(version) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        return axum::Json(serde_json::json!({
            "name": PLATFORM,
            "version": version,
            "dist": {
                "tarball": release.url.clone().unwrap_or_else(|| format!("{origin}/t/{version}.tgz")),
                "integrity": release.integrity.clone().unwrap_or_else(|| integrity_of(&release.tarball)),
            },
        }))
        .into_response();
    }
    if let Some(version) = path
        .strip_prefix("/t/")
        .and_then(|p| p.strip_suffix(".tgz"))
        && let Some(release) = releases.get(version)
    {
        stub.downloads.fetch_add(1, Ordering::SeqCst);
        return release.tarball.clone().into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

/// A host install of `version` as npm leaves it.
fn host_install(root: &Path, version: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = root.join("usr/local/lib/node_modules/@anthropic-ai/claude-code");
    std::fs::create_dir_all(dir.join("bin"))?;
    std::fs::write(
        dir.join("package.json"),
        format!(
            r#"{{"name":"@anthropic-ai/claude-code","version":"{version}","bin":{{"claude":"bin/claude.exe"}}}}"#
        ),
    )?;
    std::fs::write(dir.join("bin/claude.exe"), "host")?;
    Ok(dir)
}

struct Setup {
    _dir: tempfile::TempDir,
    stub: Arc<Stub>,
    root: PathBuf,
    host: PathBuf,
    updater: ClaudeUpdater,
}

async fn setup(host_version: &str) -> Result<Setup, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let stub = Arc::new(Stub::default());
    let registry = serve(Arc::clone(&stub)).await?;
    let host = host_install(dir.path(), host_version)?;
    let root = dir.path().join("data/claude-code");
    let updater = ClaudeUpdater::with_registry(
        root.clone(),
        Some(host.clone()),
        registry,
        Some(PLATFORM.to_owned()),
        false,
    )?;
    Ok(Setup {
        _dir: dir,
        stub,
        root,
        host,
        updater,
    })
}

fn v(text: &str) -> Version {
    Version::parse(text).expect("version")
}

/// Every entry of the updater's directory but `current` and `versions`.
fn leftovers(root: &Path) -> Vec<String> {
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|name| name != "current" && name != "versions")
                .collect()
        })
        .unwrap_or_default()
}

fn installed(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root.join("versions"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// AC1: a newer `stable` is installed, verified and made current; runs then bind it.
#[tokio::test]
async fn a_newer_stable_release_is_installed_and_becomes_current() -> TestResult {
    let s = setup("2.1.292").await?;
    s.stub.tag("stable", "2.1.311");
    s.stub.tag("latest", "2.1.318");
    s.stub.release("2.1.311", good("2.1.311"));

    let outcome = s.updater.check(Channel::Stable).await?;
    assert_eq!(outcome, Outcome::Installed(v("2.1.311")));
    let binary = s.root.join("versions/2.1.311/bin/claude.exe");
    let out = std::process::Command::new(&binary)
        .arg("--version")
        .output()?;
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("2.1.311 "));
    assert_eq!(
        std::fs::read_link(s.root.join("current"))?,
        Path::new("versions/2.1.311")
    );
    assert_eq!(s.updater.current(), Some(v("2.1.311")));
    assert!(leftovers(&s.root).is_empty(), "{:?}", leftovers(&s.root));
    assert_eq!(
        agent_cli_source(&s.root, &s.host),
        s.root.join("versions/2.1.311").canonicalize()?
    );

    // Nothing newer: nothing downloaded again.
    assert_eq!(
        s.updater.check(Channel::Stable).await?,
        Outcome::UpToDate(v("2.1.311"))
    );
    assert_eq!(s.stub.downloads.load(Ordering::SeqCst), 1);
    Ok(())
}

/// AC2: a refused release installs nothing and leaves the current version and no leftovers.
#[tokio::test]
async fn a_refused_release_leaves_the_current_version() -> TestResult {
    let s = setup("2.1.292").await?;
    s.stub.tag("stable", "2.1.311");
    s.stub.release("2.1.311", good("2.1.311"));
    s.updater.check(Channel::Stable).await?;
    s.stub.tag("stable", "2.1.318");

    let refusals = [
        (
            "integrity",
            Release {
                integrity: Some(integrity_of(b"something else")),
                ..good("2.1.318")
            },
        ),
        ("version", good("2.1.300")),
        (
            "url",
            Release {
                url: Some("https://example.invalid/claude-code-linux-x64-2.1.318.tgz".into()),
                ..good("2.1.318")
            },
        ),
        (
            "binary",
            Release {
                tarball: {
                    let mut builder =
                        tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
                    let mut header = tar::Header::new_gnu();
                    header.set_entry_type(tar::EntryType::Symlink);
                    header.set_size(0);
                    header.set_cksum();
                    builder.append_link(&mut header, "package/claude", "/bin/sh")?;
                    builder.into_inner()?.finish()?
                },
                integrity: None,
                url: None,
            },
        ),
    ];
    for (why, release) in refusals {
        s.stub.release("2.1.318", release);
        let refused = s.updater.check(Channel::Stable).await;
        assert!(refused.is_err(), "{why}: {refused:?}");
        assert_eq!(s.updater.current(), Some(v("2.1.311")), "{why}");
        assert_eq!(installed(&s.root), ["2.1.311"], "{why}");
        assert!(
            leftovers(&s.root).is_empty(),
            "{why}: {:?}",
            leftovers(&s.root)
        );
    }

    // A tarball over the bound is not read to its end.
    s.stub.release("2.1.318", good("2.1.318"));
    let small = ClaudeUpdater::with_registry(
        s.root.clone(),
        Some(s.host.clone()),
        s.updater.registry().to_owned(),
        Some(PLATFORM.to_owned()),
        false,
    )?
    .with_max_tarball(64);
    assert!(small.check(Channel::Stable).await.is_err());
    assert_eq!(s.updater.current(), Some(v("2.1.311")));
    assert!(leftovers(&s.root).is_empty());

    // The same release, intact, goes through.
    assert_eq!(
        s.updater.check(Channel::Stable).await?,
        Outcome::Installed(v("2.1.318"))
    );
    Ok(())
}

/// AC3: off asks nothing; a host as new as the channel downloads nothing.
#[tokio::test]
async fn off_or_an_up_to_date_host_downloads_nothing() -> TestResult {
    let s = setup("2.1.311").await?;
    s.stub.tag("stable", "2.1.311");
    s.stub.tag("latest", "2.1.318");
    s.stub.release("2.1.311", good("2.1.311"));

    assert_eq!(s.updater.check(Channel::Off).await?, Outcome::Off);
    assert_eq!(s.stub.requests.load(Ordering::SeqCst), 0);

    assert_eq!(
        s.updater.check(Channel::Stable).await?,
        Outcome::UpToDate(v("2.1.311"))
    );
    assert_eq!(s.stub.downloads.load(Ordering::SeqCst), 0);
    assert!(s.updater.current().is_none());
    assert_eq!(agent_cli_source(&s.root, &s.host), s.host);

    // Without a host install to bind over, nothing is asked either (CCU-005).
    let bare = ClaudeUpdater::with_registry(
        s.root.clone(),
        None,
        s.updater.registry().to_owned(),
        Some(PLATFORM.to_owned()),
        false,
    )?;
    let asked = s.stub.requests.load(Ordering::SeqCst);
    assert_eq!(bare.check(Channel::Latest).await?, Outcome::NoHostInstall);
    assert_eq!(s.stub.requests.load(Ordering::SeqCst), asked);
    Ok(())
}

/// AC4: the current version and the one before it are kept.
#[tokio::test]
async fn the_current_and_the_previous_version_stay() -> TestResult {
    let s = setup("2.1.292").await?;
    for version in ["2.1.300", "2.1.311", "2.1.318"] {
        s.stub.tag("latest", version);
        s.stub.release(version, good(version));
        assert_eq!(
            s.updater.check(Channel::Latest).await?,
            Outcome::Installed(v(version))
        );
    }
    assert_eq!(installed(&s.root), ["2.1.311", "2.1.318"]);
    // A temporary directory an interrupted install left is removed by the next check.
    std::fs::create_dir_all(s.root.join(".staging-2.1.320-1"))?;
    s.updater.check(Channel::Latest).await?;
    assert!(leftovers(&s.root).is_empty());
    Ok(())
}

/// AC5: a host updated by hand past Yulius's version is the one runs get.
#[tokio::test]
async fn a_newer_host_install_wins_over_yulius_s() -> TestResult {
    let s = setup("2.1.292").await?;
    s.stub.tag("stable", "2.1.311");
    s.stub.release("2.1.311", good("2.1.311"));
    s.updater.check(Channel::Stable).await?;
    host_install(s.host.ancestors().nth(6).ok_or("root")?, "2.1.400")?;
    assert_eq!(agent_cli_source(&s.root, &s.host), s.host);
    assert_eq!(
        s.updater.check(Channel::Stable).await?,
        Outcome::UpToDate(v("2.1.400"))
    );
    Ok(())
}
