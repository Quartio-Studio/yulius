//! The pinned Antigravity CLI Yulius provides to Antigravity runs (SPEC-505 AGY-007, SPEC-519
//! NEO-003): a stub of Google's release storage serves a tarball laid out as agy 1.5.12's is (one
//! file, `antigravity`), whose binary is a synthetic ELF header. No test reaches Google, except
//! the ignored one that installs the real release.

use std::os::unix::fs::PermissionsExt as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::http::{StatusCode, Uri};
use axum::response::IntoResponse;
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha512};
use yulius::wiring::harness::HarnessAdapter;
use yulius::wiring::pinned_cli::PinnedCli;
use yulius_app::error::AppError;
use yulius_app::ports::harness::HarnessPort;
use yulius_domain::agent_cli::antigravity::{self, Platform};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const TARBALL: &str =
    "/antigravity-public/antigravity-cli/1.5.12-5281398513336320/linux-x64/cli_linux_x64.tar.gz";

/// An ELF64 header for `machine`, then filler.
fn elf(machine: u16) -> Vec<u8> {
    let mut bytes = vec![0x7f, b'E', b'L', b'F', 2, 1, 1, 0];
    bytes.resize(16, 0);
    bytes.extend_from_slice(&3u16.to_le_bytes());
    bytes.extend_from_slice(&machine.to_le_bytes());
    bytes.resize(4096, b'.');
    bytes
}

/// A tarball of `entries` (path, mode, body), each a regular file, or a link when its body is
/// `None`.
fn tarball_of(entries: &[(&str, u32, Option<&[u8]>)]) -> std::io::Result<Vec<u8>> {
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    for (path, mode, body) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_mode(*mode);
        match body {
            Some(body) => {
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(body.len() as u64);
                builder.append_data(&mut header, path, *body)?;
            }
            None => {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                builder.append_link(&mut header, path, "/etc/passwd")?;
            }
        }
    }
    builder.into_inner().and_then(GzEncoder::finish)
}

/// A tarball of one regular file whose name is written raw, so it can be a name the tar
/// builder refuses (`../x`, an absolute path).
fn raw_named(name: &str, body: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(body.len() as u64);
    header.set_mode(0o755);
    let gnu = header
        .as_gnu_mut()
        .ok_or_else(|| std::io::Error::other("gnu"))?;
    gnu.name[..name.len()].copy_from_slice(name.as_bytes());
    header.set_cksum();
    builder.append(&header, body)?;
    builder.into_inner().and_then(GzEncoder::finish)
}

/// The release as Google publishes its linux-x64 tarball (1.5.10): one executable.
fn release() -> std::io::Result<Vec<u8>> {
    tarball_of(&[("antigravity", 0o755, Some(&elf(62)))])
}

/// The linux-x64 platform pinned to `tarball`'s sha512 (the source pins the real one).
fn pinned(tarball: &[u8]) -> Result<Platform, Box<dyn std::error::Error>> {
    let mut platform = antigravity::platform("linux", "x86_64").ok_or("linux-x64")?;
    let hex: String = Sha512::digest(tarball)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    platform.sha512 = Box::leak(hex.into_boxed_str());
    Ok(platform)
}

#[derive(Default)]
struct Stub {
    tarball: Mutex<Vec<u8>>,
    downloads: AtomicUsize,
}

struct Setup {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    stub: Arc<Stub>,
    server: String,
}

async fn setup(tarball: Vec<u8>) -> Result<Setup, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().join("antigravity-cli");
    let stub = Arc::new(Stub {
        tarball: Mutex::new(tarball),
        downloads: AtomicUsize::new(0),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let server = format!("http://{}", listener.local_addr()?);
    let served = Arc::clone(&stub);
    let router = Router::new().fallback(move |uri: Uri| {
        let stub = Arc::clone(&served);
        async move {
            if uri.path() != TARBALL {
                return StatusCode::NOT_FOUND.into_response();
            }
            stub.downloads.fetch_add(1, Ordering::SeqCst);
            stub.tarball
                .lock()
                .map(|t| t.clone())
                .unwrap_or_default()
                .into_response()
        }
    });
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(Setup {
        _dir: dir,
        root,
        stub,
        server,
    })
}

fn cli(s: &Setup, platform: Platform) -> Result<PinnedCli, Box<dyn std::error::Error>> {
    Ok(PinnedCli::antigravity_from(
        s.root.clone(),
        s.server.clone(),
        Some(platform),
        false,
    )?)
}

/// The pinned release is downloaded once for callers at the same time, checked
/// against its pinned sha512 and installed as `<root>/1.5.12-linux-x64/antigravity`, runnable by
/// every sandbox user, with nothing else left in the root.
#[tokio::test]
async fn the_pinned_antigravity_cli_installs_once() -> TestResult {
    let tarball = release()?;
    let s = setup(tarball.clone()).await?;
    let cli = Arc::new(cli(&s, pinned(&tarball)?)?);
    let (a, b) = tokio::join!(cli.ensure(), cli.ensure());
    let (a, b) = (a?, b?);
    assert_eq!(a, b);
    assert_eq!(a.program, s.root.join("1.5.12-linux-x64/antigravity"));
    assert_eq!(a.dir, s.root.join("1.5.12-linux-x64"));
    assert_eq!(s.stub.downloads.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::metadata(&a.program)?.permissions().mode() & 0o777,
        0o755
    );
    let names: Vec<String> = std::fs::read_dir(&s.root)?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["1.5.12-linux-x64"]);
    // Installed: no second download.
    assert_eq!(cli.ensure().await?, a);
    assert_eq!(s.stub.downloads.load(Ordering::SeqCst), 1);
    Ok(())
}

/// A tarball of another sha512, one without the binary, one whose binary is no ELF for
/// this machine, and one with a link are refused, the reason naming the Antigravity CLI, and
/// nothing is installed.
#[tokio::test]
async fn a_tarball_that_is_not_the_pinned_release_is_refused() -> TestResult {
    let real = release()?;
    let outside = tempfile::tempdir()?;
    let absolute = outside.path().join("escape");
    let absolute_name = absolute.to_string_lossy().into_owned();
    let up = raw_named("../escape", &elf(62))?;
    let rooted = raw_named(&absolute_name, &elf(62))?;
    let cases: Vec<(Vec<u8>, Platform, &str)> = vec![
        (real.clone(), pinned(b"another tarball")?, "sha512"),
        // The release lies at the tarball's root: a name may not leave it.
        (up.clone(), pinned(&up)?, "outside its release"),
        (rooted.clone(), pinned(&rooted)?, "outside its release"),
        (
            tarball_of(&[("README", 0o644, Some(b"x"))])?,
            pinned(&tarball_of(&[("README", 0o644, Some(b"x"))])?)?,
            "holds no antigravity",
        ),
        (
            tarball_of(&[("antigravity", 0o755, Some(&elf(183)))])?,
            pinned(&tarball_of(&[("antigravity", 0o755, Some(&elf(183)))])?)?,
            "not a 64-bit ELF",
        ),
        (
            tarball_of(&[("antigravity", 0o777, None)])?,
            pinned(&tarball_of(&[("antigravity", 0o777, None)])?)?,
            "not a regular file",
        ),
    ];
    for (tarball, platform, why) in cases {
        let s = setup(tarball).await?;
        let cli = Arc::new(cli(&s, platform)?);
        match cli.ensure().await {
            Err(error) => {
                let text = error.to_string();
                assert!(text.contains(why), "{why}: {text}");
                assert!(text.contains("Antigravity CLI"), "{text}");
            }
            Ok(provided) => return Err(format!("{why}: installed {provided:?}").into()),
        }
        assert_eq!(cli.installed(), None, "{why}");
        let left: Vec<String> = std::fs::read_dir(&s.root)?
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(left.is_empty(), "{why}: {left:?} left in the root");
        assert!(
            !s.root.parent().ok_or("parent")?.join("escape").exists(),
            "{why}"
        );
    }
    assert!(!absolute.exists(), "nothing written at an absolute name");
    Ok(())
}

/// Antigravity runs get the provided CLI; without it, the host's `agy`; with neither,
/// a reason naming the release. A caller that must not wait gets only what is ready.
#[tokio::test]
async fn antigravity_runs_get_the_provided_cli_or_a_reason() -> TestResult {
    let tarball = release()?;
    let s = setup(tarball.clone()).await?;
    let plain = HarnessAdapter::new("/bin/yulius".into(), "http://127.0.0.1:1");
    assert_eq!(plain.provide_antigravity(true).await?, None);

    let mut adapter = HarnessAdapter::new("/bin/yulius".into(), "http://127.0.0.1:1")
        .with_antigravity_cli(Arc::new(cli(&s, pinned(&tarball)?)?));
    adapter.host_antigravity = None;
    match adapter.provide_antigravity(false).await {
        Err(AppError::Upstream(reason)) => {
            assert!(
                reason.contains("not installed yet; an Antigravity start installs it"),
                "{reason}"
            )
        }
        other => return Err(format!("expected a reason, got {other:?}").into()),
    }
    let provided = adapter.provide_antigravity(true).await?.ok_or("provided")?;
    assert_eq!(
        provided.program,
        s.root.join("1.5.12-linux-x64/antigravity")
    );
    assert_eq!(
        adapter.provide_antigravity(false).await?.map(|c| c.program),
        Some(provided.program)
    );

    let broken = setup(b"not a tarball".to_vec()).await?;
    let mut failing = HarnessAdapter::new("/bin/yulius".into(), "http://127.0.0.1:1")
        .with_antigravity_cli(Arc::new(cli(&broken, pinned(&tarball)?)?));
    failing.host_antigravity = None;
    match failing.provide_antigravity(true).await {
        Err(AppError::Upstream(reason)) => {
            assert!(reason.contains("could not be installed"), "{reason}");
            assert!(reason.contains(antigravity::VERSION), "{reason}");
            assert!(reason.contains("`agy`"), "{reason}");
        }
        other => return Err(format!("expected a reason, got {other:?}").into()),
    }
    failing.host_antigravity = Some("/usr/local/bin/agy".into());
    assert_eq!(
        failing.provide_antigravity(true).await?,
        None,
        "the host's agy"
    );
    Ok(())
}

/// AGY-007, NEO-003, by hand: the real pinned release installs from Google and is the 1.5.12
/// binary.
#[tokio::test]
#[ignore = "downloads about 100 MB from Google's release storage"]
async fn the_real_pinned_release_installs_from_google() -> TestResult {
    let dir = tempfile::tempdir()?;
    let cli = Arc::new(PinnedCli::antigravity(dir.path().join("antigravity-cli"))?);
    let provided = cli.ensure().await?;
    let output = std::process::Command::new(&provided.program)
        .arg("--version")
        .env("HOME", dir.path())
        .env("AGY_CLI_DISABLE_AUTO_UPDATE", "true")
        .output()?;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        antigravity::VERSION
    );
    Ok(())
}
