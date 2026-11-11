//! Claude Code kept current in the data directory (SPEC-421): a registry stub serves the
//! dist-tags, the platform package's version document and a tarball whose `claude` is a script
//! printing its version.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha512};
use yulius::wiring::claude_cli::{
    ClaudeUpdater, Outcome, agent_cli_binds, agent_cli_source, describe, start_updates_every,
};
use yulius::wiring::sandbox::parse_channel;
use yulius_app::ports::session::Bind;
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
    /// Sent in chunks without a `Content-Length`.
    chunked: bool,
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
        if let Ok(mut tags) = self.tags.lock() {
            tags.insert(tag.into(), version.into());
        }
    }

    fn release(&self, version: &str, release: Release) {
        if let Ok(mut releases) = self.releases.lock() {
            releases.insert(version.into(), release);
        }
    }
}

/// ELF's `e_machine` of x86-64.
const X86_64: u16 = 62;

/// The models the composer offers, the default first.
const OFFERED: [&str; 5] = yulius_domain::harness::OFFERED_CLAUDE_MODELS;

/// A stand-in for the native binary: an ELF64 header for `machine`, then filler, then the
/// version as the real binary embeds it (`VERSION:"X.Y.Z"`), across a 64 KiB boundary, then the
/// model ids it knows as its model tables name them.
fn binary_knowing(machine: u16, version: &str, models: &[&str]) -> Vec<u8> {
    let mut bytes = vec![0x7f, b'E', b'L', b'F', 2, 1, 1, 0];
    bytes.resize(16, 0);
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&machine.to_le_bytes());
    bytes.resize(65_536 - 7, b'.');
    bytes.extend_from_slice(format!("VERSION:\"{version}\"").as_bytes());
    bytes.extend_from_slice(b"...");
    for model in models {
        bytes.extend_from_slice(format!("{{id:\"{model}\",family:\"x\"}},").as_bytes());
    }
    bytes
}

/// A stand-in that knows every offered model.
fn binary(machine: u16, version: &str) -> Vec<u8> {
    binary_knowing(machine, version, &OFFERED)
}

/// A release of `version` whose binary knows only `models`.
fn knowing(version: &str, models: &[&str]) -> std::io::Result<Release> {
    Ok(Release {
        tarball: tarball_of(&[("claude", binary_knowing(X86_64, version, models))])?,
        integrity: None,
        url: None,
        chunked: false,
    })
}

/// A tarball as npm publishes a platform package, with `files` under `package/`.
fn tarball_of(files: &[(&str, Vec<u8>)]) -> std::io::Result<Vec<u8>> {
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    for (path, body) in files {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(body.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append_data(&mut header, format!("package/{path}"), body.as_slice())?;
    }
    builder.into_inner().and_then(GzEncoder::finish)
}

fn tarball(version: &str) -> std::io::Result<Vec<u8>> {
    tarball_of(&[
        ("package.json", b"{}".to_vec()),
        ("README.md", b"readme".to_vec()),
        ("claude", binary(X86_64, version)),
    ])
}

fn good(version: &str) -> std::io::Result<Release> {
    Ok(Release {
        tarball: tarball(version)?,
        integrity: None,
        url: None,
        chunked: false,
    })
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
        let Ok(tags) = stub.tags.lock().map(|t| t.clone()) else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        return axum::Json(tags).into_response();
    }
    let Ok(releases) = stub.releases.lock().map(|r| r.clone()) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
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
        if release.chunked {
            let chunks: Vec<Result<Vec<u8>, std::io::Error>> =
                release.tarball.chunks(16).map(|c| Ok(c.to_vec())).collect();
            return axum::body::Body::from_stream(futures::stream::iter(chunks)).into_response();
        }
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
    // A host install that knows every offered model, as one installed from npm today does.
    std::fs::write(dir.join("bin/claude.exe"), binary(X86_64, version))?;
    Ok(dir)
}

/// The host's Claude Code replaced by one that knows only `models` (same version).
fn host_knows(host: &Path, version: &str, models: &[&str]) -> std::io::Result<()> {
    std::fs::write(
        host.join("bin/claude.exe"),
        binary_knowing(X86_64, version, models),
    )
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
    Version::parse(text).unwrap_or_else(|| panic!("{text} is a version"))
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
    s.stub.release("2.1.311", good("2.1.311")?);

    let outcome = s.updater.check(Channel::Stable).await?;
    assert_eq!(outcome, Outcome::Installed(v("2.1.311")));
    let installed_binary = s.root.join("versions/2.1.311/bin/claude.exe");
    assert_eq!(std::fs::read(&installed_binary)?, binary(X86_64, "2.1.311"));
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(&installed_binary)?.permissions().mode() & 0o777,
            0o755
        );
        for dir in ["", "versions", "versions/2.1.311", "versions/2.1.311/bin"] {
            assert_eq!(
                std::fs::metadata(s.root.join(dir))?.permissions().mode() & 0o777,
                0o755,
                "{dir:?}"
            );
        }
    }
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
    s.stub.release("2.1.311", good("2.1.311")?);
    s.updater.check(Channel::Stable).await?;
    s.stub.tag("stable", "2.1.318");

    // The same tarball on another origin than the registry's.
    let elsewhere = serve(Arc::clone(&s.stub)).await?;
    let symlink = {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_cksum();
        builder.append_link(&mut header, "package/claude", "/bin/sh")?;
        builder.into_inner()?.finish()?
    };
    let refusals = [
        (
            "sha512",
            Release {
                integrity: Some(integrity_of(b"something else")),
                ..good("2.1.318")?
            },
        ),
        ("does not name 2.1.318", good("2.1.300")?),
        (
            "not a 64-bit ELF for",
            Release {
                tarball: tarball_of(&[("claude", b"#!/bin/sh\necho 2.1.318\n".to_vec())])?,
                ..good("2.1.318")?
            },
        ),
        (
            "not a 64-bit ELF for",
            Release {
                tarball: tarball_of(&[("claude", binary(183, "2.1.318"))])?,
                ..good("2.1.318")?
            },
        ),
        (
            "holds no `package/claude`",
            Release {
                tarball: tarball_of(&[("cli.js", binary(X86_64, "2.1.318"))])?,
                ..good("2.1.318")?
            },
        ),
        (
            "is not on",
            Release {
                url: Some(format!("{elsewhere}/t/2.1.318.tgz")),
                ..good("2.1.318")?
            },
        ),
        (
            "not a regular file",
            Release {
                tarball: symlink,
                ..good("2.1.318")?
            },
        ),
    ];
    for (why, release) in refusals {
        s.stub.release("2.1.318", release);
        let refused = s.updater.check(Channel::Stable).await;
        assert!(
            refused.as_ref().is_err_and(|e| e.to_string().contains(why)),
            "{why}: {refused:?}"
        );
        assert_eq!(s.updater.current(), Some(v("2.1.311")), "{why}");
        assert_eq!(installed(&s.root), ["2.1.311"], "{why}");
        assert!(
            leftovers(&s.root).is_empty(),
            "{why}: {:?}",
            leftovers(&s.root)
        );
    }

    // A tarball over the bound is not read to its end, with or without a `Content-Length`.
    let small = ClaudeUpdater::with_registry(
        s.root.clone(),
        Some(s.host.clone()),
        s.updater.registry().to_owned(),
        Some(PLATFORM.to_owned()),
        false,
    )?
    .with_max_tarball(64);
    for chunked in [false, true] {
        s.stub.release(
            "2.1.318",
            Release {
                chunked,
                ..good("2.1.318")?
            },
        );
        let refused = small.check(Channel::Stable).await;
        assert!(
            refused
                .as_ref()
                .is_err_and(|e| e.to_string().contains("larger than allowed")),
            "chunked {chunked}: {refused:?}"
        );
        assert_eq!(s.updater.current(), Some(v("2.1.311")));
        assert!(leftovers(&s.root).is_empty());
    }

    // A binary over its bound is not written to its end.
    let small = ClaudeUpdater::with_registry(
        s.root.clone(),
        Some(s.host.clone()),
        s.updater.registry().to_owned(),
        Some(PLATFORM.to_owned()),
        false,
    )?
    .with_max_binary(1024);
    s.stub.release("2.1.318", good("2.1.318")?);
    let refused = small.check(Channel::Stable).await;
    assert!(
        refused
            .as_ref()
            .is_err_and(|e| e.to_string().contains("binary is larger than allowed")),
        "{refused:?}"
    );
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
    s.stub.release("2.1.311", good("2.1.311")?);

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
        s.stub.release(version, good(version)?);
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
    s.stub.release("2.1.311", good("2.1.311")?);
    s.updater.check(Channel::Stable).await?;
    host_install(s.host.ancestors().nth(6).ok_or("root")?, "2.1.400")?;
    assert_eq!(agent_cli_source(&s.root, &s.host), s.host);
    assert_eq!(
        s.updater.check(Channel::Stable).await?,
        Outcome::UpToDate(v("2.1.400"))
    );
    Ok(())
}

/// CCU-001: the loop checks on its own, again after each period; `off` starts no loop.
#[tokio::test]
async fn the_loop_installs_without_anyone_asking() -> TestResult {
    let s = setup("2.1.292").await?;
    s.stub.tag("stable", "2.1.311");
    s.stub.release("2.1.311", good("2.1.311")?);
    let root = s.root.clone();
    let updater = ClaudeUpdater::with_registry(
        root.clone(),
        Some(s.host.clone()),
        s.updater.registry().to_owned(),
        Some(PLATFORM.to_owned()),
        false,
    )?;
    let looping = start_updates_every(
        updater,
        Channel::Stable,
        Duration::ZERO,
        Duration::from_millis(50),
    )
    .ok_or("a loop")?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while s.updater.current() != Some(v("2.1.311")) {
        assert!(tokio::time::Instant::now() < deadline, "never installed");
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // A later release is picked up by a later round.
    s.stub.tag("stable", "2.1.318");
    s.stub.release("2.1.318", good("2.1.318")?);
    while s.updater.current() != Some(v("2.1.318")) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "never installed the next"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    looping.abort();

    let off = ClaudeUpdater::with_registry(
        root,
        Some(s.host.clone()),
        s.updater.registry().to_owned(),
        Some(PLATFORM.to_owned()),
        false,
    )?;
    assert!(start_updates_every(off, Channel::Off, Duration::ZERO, Duration::ZERO).is_none());
    assert_eq!(parse_channel("off"), Ok(Channel::Off));
    assert_eq!(parse_channel("latest"), Ok(Channel::Latest));
    assert!(parse_channel("nightly").is_err());
    Ok(())
}

/// CCU-003, CCU-005: a run binds the chosen directory at the host package's path; with updates
/// off the host's own, and nothing without a host package.
#[tokio::test]
async fn runs_bind_the_chosen_version_at_the_host_package_path() -> TestResult {
    let s = setup("2.1.292").await?;
    s.stub.tag("stable", "2.1.311");
    s.stub.release("2.1.311", good("2.1.311")?);
    let bind = |source: PathBuf| Bind {
        source,
        dest: s.host.clone(),
        writable: false,
    };
    // Before any install, the host's.
    assert_eq!(
        agent_cli_binds(Some(&s.host), Some(&s.root)),
        [bind(s.host.clone())]
    );
    s.updater.check(Channel::Stable).await?;
    assert_eq!(
        agent_cli_binds(Some(&s.host), Some(&s.root)),
        [bind(s.root.join("versions/2.1.311").canonicalize()?)]
    );
    assert_eq!(agent_cli_binds(Some(&s.host), None), [bind(s.host.clone())]);
    assert!(agent_cli_binds(None, Some(&s.root)).is_empty());
    Ok(())
}

/// CCU-006: the doctor says which Claude Code isolated runs get and where it comes from.
#[tokio::test]
async fn the_doctor_names_the_version_runs_get() -> TestResult {
    let s = setup("2.1.292").await?;
    let text = describe(&s.root, Some(&s.host), Channel::Stable);
    assert!(text.starts_with("2.1.292 from the host"), "{text}");
    s.stub.tag("stable", "2.1.311");
    s.stub.release("2.1.311", good("2.1.311")?);
    s.updater.check(Channel::Stable).await?;
    let text = describe(&s.root, Some(&s.host), Channel::Stable);
    assert!(
        text.starts_with("2.1.311, kept current by Yulius in "),
        "{text}"
    );
    assert!(text.contains("the host has 2.1.292"), "{text}");
    // Off: runs bind the host's, whatever was installed before (review finding 7).
    let text = describe(&s.root, Some(&s.host), Channel::Off);
    assert!(text.starts_with("2.1.292 from the host"), "{text}");
    assert!(text.contains("YULIUS_CLAUDE_UPDATES=off"), "{text}");
    let text = describe(&s.root, None, Channel::Stable);
    assert!(text.contains("no npm install of Claude Code"), "{text}");
    Ok(())
}

/// Review finding 1: a host package laid out otherwise (its `claude` is not `bin/claude.exe`)
/// is neither updated nor bound over: runs keep the host's.
#[tokio::test]
async fn a_host_package_of_another_layout_is_left_alone() -> TestResult {
    let s = setup("2.1.292").await?;
    s.stub.tag("stable", "2.1.311");
    s.stub.release("2.1.311", good("2.1.311")?);
    s.updater.check(Channel::Stable).await?;
    std::fs::write(
        s.host.join("package.json"),
        r#"{"name":"@anthropic-ai/claude-code","version":"2.1.292","bin":{"claude":"cli.js"}}"#,
    )?;
    assert_eq!(agent_cli_source(&s.root, &s.host), s.host);
    s.stub.tag("stable", "2.1.318");
    s.stub.release("2.1.318", good("2.1.318")?);
    let asked = s.stub.requests.load(Ordering::SeqCst);
    assert_eq!(s.updater.check(Channel::Stable).await?, Outcome::HostLayout);
    assert_eq!(s.stub.requests.load(Ordering::SeqCst), asked);
    assert_eq!(s.updater.current(), Some(v("2.1.311")));
    Ok(())
}

/// Review finding 2: a version a running process executes is kept past the switches, and removed
/// by the first check after it ended.
#[tokio::test]
async fn a_version_a_process_runs_is_not_removed() -> TestResult {
    let s = setup("2.1.292").await?;
    s.stub.tag("latest", "2.1.300");
    s.stub.release("2.1.300", good("2.1.300")?);
    s.updater.check(Channel::Latest).await?;
    // A session still running 2.1.300's binary (a real executable stands in for it).
    let old = s.root.join("versions/2.1.300/bin/claude.exe");
    std::fs::remove_file(&old)?;
    std::fs::copy("/bin/sleep", &old)?;
    let mut running = tokio::process::Command::new(&old)
        .arg("600")
        .kill_on_drop(true)
        .spawn()?;
    for version in ["2.1.311", "2.1.318"] {
        s.stub.tag("latest", version);
        s.stub.release(version, good(version)?);
        s.updater.check(Channel::Latest).await?;
    }
    assert_eq!(installed(&s.root), ["2.1.300", "2.1.311", "2.1.318"]);
    running.kill().await?;
    assert_eq!(
        s.updater.check(Channel::Latest).await?,
        Outcome::UpToDate(v("2.1.318"))
    );
    assert_eq!(installed(&s.root), ["2.1.311", "2.1.318"]);
    Ok(())
}

/// SPEC-461 AC1 (the incident of 2026-11-11): a release whose binary does not know the default
/// model never becomes current, and the refusal names the release and the model.
#[tokio::test]
async fn a_release_that_lacks_the_default_model_is_refused() -> TestResult {
    let s = setup("2.1.292").await?;
    s.stub.tag("latest", "2.1.300");
    s.stub.release("2.1.300", good("2.1.300")?);
    s.updater.check(Channel::Latest).await?;
    // 2.1.311 knows Fable 5.1 and the rest, not Fable 5.2.
    s.stub.tag("latest", "2.1.311");
    s.stub
        .release("2.1.311", knowing("2.1.311", &OFFERED[1..])?);
    let refused = s.updater.check(Channel::Latest).await;
    assert!(
        refused.as_ref().is_err_and(|e| e
            .to_string()
            .contains("2.1.311 does not know claude-fable-5-2")),
        "{refused:?}"
    );
    assert_eq!(s.updater.current(), Some(v("2.1.300")));
    assert_eq!(installed(&s.root), ["2.1.300"]);
    assert!(leftovers(&s.root).is_empty(), "{:?}", leftovers(&s.root));
    // The same release with the model is taken.
    s.stub.release("2.1.311", good("2.1.311")?);
    assert_eq!(
        s.updater.check(Channel::Latest).await?,
        Outcome::Installed(v("2.1.311"))
    );
    Ok(())
}

/// SPEC-461 AC3: on `stable`, when neither the version in use nor stable's release knows the
/// default model, `latest`'s release is taken; when it does not know it either, nothing is, and
/// both are named.
#[tokio::test]
async fn stable_falls_forward_to_latest_for_the_default_model() -> TestResult {
    // The incident: the host and npm's `stable` are 2.1.311, which knows Fable 5.1 only.
    let s = setup("2.1.311").await?;
    host_knows(&s.host, "2.1.311", &OFFERED[1..])?;
    s.stub.tag("stable", "2.1.311");
    s.stub.tag("latest", "2.1.322");
    s.stub
        .release("2.1.311", knowing("2.1.311", &OFFERED[1..])?);

    // `latest` does not know it either: nothing is installed, and the release is named.
    s.stub
        .release("2.1.322", knowing("2.1.322", &OFFERED[1..])?);
    let refused = s.updater.check(Channel::Stable).await;
    let error = refused.as_ref().err().ok_or("a refusal")?;
    assert!(
        error
            .to_string()
            .contains("2.1.322 does not know claude-fable-5-2"),
        "{error}"
    );
    assert_eq!(error.refused().len(), 1);
    assert_eq!(error.refused()[0].version, v("2.1.322"));
    assert_eq!(error.refused()[0].missing, [OFFERED[0]]);
    assert!(s.updater.current().is_none());
    assert!(leftovers(&s.root).is_empty(), "{:?}", leftovers(&s.root));

    // `latest` knows it: it is taken, though the channel is `stable`.
    s.stub.release("2.1.322", good("2.1.322")?);
    assert_eq!(
        s.updater.check(Channel::Stable).await?,
        Outcome::Installed(v("2.1.322"))
    );
    assert_eq!(
        agent_cli_source(&s.root, &s.host),
        s.root.join("versions/2.1.322").canonicalize()?
    );
    // From then on stable's older release changes nothing.
    assert_eq!(
        s.updater.check(Channel::Stable).await?,
        Outcome::UpToDate(v("2.1.322"))
    );
    Ok(())
}

/// SPEC-461 AC3: a newer stable release without the default model is refused, then `latest`
/// too, and both are named; a version in use that knows the default model keeps `stable` on
/// stable's releases.
#[tokio::test]
async fn stable_names_both_refusals_and_stays_stable_while_the_default_is_known() -> TestResult {
    let s = setup("2.1.292").await?;
    host_knows(&s.host, "2.1.292", &OFFERED[1..])?;
    s.stub.tag("stable", "2.1.311");
    s.stub.tag("latest", "2.1.318");
    s.stub
        .release("2.1.311", knowing("2.1.311", &OFFERED[1..])?);
    s.stub
        .release("2.1.318", knowing("2.1.318", &OFFERED[1..])?);
    let refused = s.updater.check(Channel::Stable).await;
    let error = refused.as_ref().err().ok_or("a refusal")?;
    let text = error.to_string();
    assert!(
        text.contains("2.1.311 does not know claude-fable-5-2")
            && text.contains("2.1.318 does not know claude-fable-5-2"),
        "{text}"
    );
    let versions: Vec<_> = error.refused().iter().map(|r| r.version).collect();
    assert_eq!(versions, [v("2.1.311"), v("2.1.318")]);
    assert!(s.updater.current().is_none());
    assert!(leftovers(&s.root).is_empty());
    // On `latest` there is nothing newer to take: its own refusal is the answer.
    let refused = s.updater.check(Channel::Latest).await;
    assert!(
        refused
            .as_ref()
            .is_err_and(|e| e.refused().len() == 1 && e.refused()[0].version == v("2.1.318")),
        "{refused:?}"
    );

    // A version in use that knows the default model: `stable` takes stable's release and never
    // asks for `latest`'s.
    host_knows(&s.host, "2.1.292", &OFFERED)?;
    s.stub.release("2.1.311", good("2.1.311")?);
    s.stub.release("2.1.318", good("2.1.318")?);
    assert_eq!(
        s.updater.check(Channel::Stable).await?,
        Outcome::Installed(v("2.1.311"))
    );
    let downloads = s.stub.downloads.load(Ordering::SeqCst);
    assert_eq!(
        s.updater.check(Channel::Stable).await?,
        Outcome::UpToDate(v("2.1.311"))
    );
    assert_eq!(s.stub.downloads.load(Ordering::SeqCst), downloads);
    Ok(())
}

/// SPEC-461 AC4: a release that knows the default model but not another offered one is taken;
/// what it lacks is what the version in use lacks from then on.
#[tokio::test]
async fn a_release_lacking_another_offered_model_is_taken_and_the_gap_known() -> TestResult {
    let s = setup("2.1.292").await?;
    s.stub.tag("latest", "2.1.400");
    // Haiku 4.5 left the binary; the composer still lists it.
    s.stub
        .release("2.1.400", knowing("2.1.400", &OFFERED[..4])?);
    assert_eq!(s.updater.in_use_missing(&s.host).await, Some(Vec::new()));
    assert_eq!(
        s.updater.check(Channel::Latest).await?,
        Outcome::Installed(v("2.1.400"))
    );
    assert_eq!(
        s.updater.in_use_missing(&s.host).await,
        Some(vec!["claude-haiku-4-5".to_owned()])
    );
    Ok(())
}

/// SPEC-461 AC2: an instance follows `latest` unless told otherwise, on `serve` and `doctor`
/// (both declare [`DEFAULT_CHANNEL`]).
#[test]
fn the_channel_defaults_to_latest() -> TestResult {
    use yulius::wiring::sandbox::{DEFAULT_CHANNEL, IsolationArgs};
    assert_eq!(DEFAULT_CHANNEL, "latest");
    assert_eq!(parse_channel(DEFAULT_CHANNEL), Ok(Channel::default()));
    assert_eq!(IsolationArgs::default().claude_updates, Channel::Latest);
    // The declared default, not a parse: parsing reads the environment (YULIUS_CLAUDE_UPDATES).
    let command = <IsolationArgs as clap::Args>::augment_args(clap::Command::new("serve"));
    let declared = command
        .get_arguments()
        .find(|arg| arg.get_long() == Some("claude-updates"))
        .ok_or("--claude-updates")?;
    assert_eq!(declared.get_default_values(), ["latest"]);
    Ok(())
}

/// SPEC-461 CCM-002: the composer's list (`models.js`) and the server's are the same ids, the
/// default first.
#[test]
fn the_composer_offers_the_models_the_server_checks() -> TestResult {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../yulius-server/web/assets/models.js");
    let text = std::fs::read_to_string(path)?;
    let list = text
        .split_once("export const PROVIDERS")
        .and_then(|(_, rest)| rest.split_once("export const EFFORTS"))
        .map(|(providers, _)| providers)
        .ok_or("the PROVIDERS list")?;
    let ids: Vec<&str> = list
        .split("{ id: '")
        .skip(1)
        .filter_map(|rest| rest.split('\'').next())
        .filter(|id| id.starts_with("claude-"))
        .collect();
    assert_eq!(ids, OFFERED, "models.js and OFFERED_CLAUDE_MODELS differ");
    assert_eq!(OFFERED[0], yulius_domain::harness::DEFAULT_CLAUDE_MODEL);
    Ok(())
}
