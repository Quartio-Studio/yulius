//! The pinned Codex CLI Yulius provides to Codex runs: a registry stub serves
//! a platform tarball laid out as `@openai/codex@0.170.0-linux-x64` is, whose `bin/codex` is a
//! synthetic ELF header. No test reaches npm.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::http::{StatusCode, Uri};
use axum::response::IntoResponse;
use base64::Engine as _;
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha512};
use yulius::wiring::harness::HarnessAdapter;
use yulius::wiring::pinned_cli::CodexCli;
use yulius_app::error::AppError;
use yulius_app::ports::harness::HarnessPort;
use yulius_domain::agent_cli::codex::{self, Platform};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const TRIPLE: &str = "x86_64-unknown-linux-musl";
const TARBALL: &str = "/@openai/codex/-/codex-0.170.0-linux-x64.tgz";

/// An ELF64 header for `machine`, then filler.
fn elf(machine: u16) -> Vec<u8> {
    let mut bytes = vec![0x7f, b'E', b'L', b'F', 2, 1, 1, 0];
    bytes.resize(16, 0);
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&machine.to_le_bytes());
    bytes.resize(4096, b'.');
    bytes
}

/// One tarball entry: its path, kind, mode and body.
struct Entry {
    path: String,
    kind: tar::EntryType,
    mode: u32,
    body: Vec<u8>,
}

fn file(path: &str, mode: u32, body: &[u8]) -> Entry {
    Entry {
        path: path.into(),
        kind: tar::EntryType::Regular,
        mode,
        body: body.to_vec(),
    }
}

fn tarball_of(entries: &[Entry]) -> std::io::Result<Vec<u8>> {
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    for entry in entries {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(entry.kind);
        header.set_size(entry.body.len() as u64);
        header.set_mode(entry.mode);
        if entry.kind == tar::EntryType::Symlink {
            header.set_link_name("/etc/passwd")?;
        }
        // Written raw, so a test can name a path the builder would refuse.
        let name = entry.path.as_bytes();
        let gnu = header
            .as_gnu_mut()
            .ok_or_else(|| std::io::Error::other("gnu"))?;
        gnu.name[..name.len()].copy_from_slice(name);
        header.set_cksum();
        builder.append(&header, entry.body.as_slice())?;
    }
    builder.into_inner().and_then(GzEncoder::finish)
}

/// The release as npm publishes its linux-x64 package (the layout of 0.170.0).
fn release_entries() -> Vec<Entry> {
    let vendor = format!("package/vendor/{TRIPLE}");
    vec![
        file("package/package.json", 0o644, b"{}"),
        file("package/README.md", 0o644, b"readme"),
        file(&format!("{vendor}/bin/codex"), 0o755, &elf(62)),
        file(&format!("{vendor}/codex-path/rg"), 0o755, &elf(62)),
        file(
            &format!("{vendor}/codex-package.json"),
            0o644,
            br#"{"layoutVersion":1,"version":"0.170.0","entrypoint":"bin/codex"}"#,
        ),
        file(&format!("{vendor}/codex-resources/bwrap"), 0o755, &elf(62)),
    ]
}

fn integrity_of(bytes: &[u8]) -> String {
    format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode(Sha512::digest(bytes))
    )
}

/// The linux-x64 platform pinned to `tarball`'s sha512 (the source pins the real one).
fn pinned(tarball: &[u8]) -> Result<Platform, Box<dyn std::error::Error>> {
    let mut platform = codex::platform("linux", "x86_64").ok_or("linux-x64")?;
    platform.integrity = Box::leak(integrity_of(tarball).into_boxed_str());
    Ok(platform)
}

#[derive(Default)]
struct Stub {
    tarball: Mutex<Vec<u8>>,
    downloads: AtomicUsize,
    /// Downloads wait for a permit (none: they answer at once).
    gate: Option<Arc<tokio::sync::Semaphore>>,
    /// The tarball is streamed in chunks without a `Content-Length`.
    streamed: std::sync::atomic::AtomicBool,
}

async fn serve(stub: Arc<Stub>) -> Result<String, Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let router = Router::new().fallback(move |uri: Uri| {
        let stub = Arc::clone(&stub);
        async move {
            if uri.path() != TARBALL {
                return StatusCode::NOT_FOUND.into_response();
            }
            stub.downloads.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = &stub.gate {
                let _permit = gate.acquire().await;
            }
            let body = stub.tarball.lock().map(|t| t.clone()).unwrap_or_default();
            if stub.streamed.load(Ordering::SeqCst) {
                let chunks: Vec<Result<Vec<u8>, std::io::Error>> =
                    body.chunks(16).map(|c| Ok(c.to_vec())).collect();
                return axum::body::Body::from_stream(futures::stream::iter(chunks))
                    .into_response();
            }
            body.into_response()
        }
    });
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(base)
}

struct Setup {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    stub: Arc<Stub>,
    registry: String,
}

async fn setup(tarball: Vec<u8>, gated: bool) -> Result<Setup, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().join("codex-cli");
    let stub = Arc::new(Stub {
        tarball: Mutex::new(tarball),
        downloads: AtomicUsize::new(0),
        gate: gated.then(|| Arc::new(tokio::sync::Semaphore::new(0))),
        streamed: std::sync::atomic::AtomicBool::new(false),
    });
    let registry = serve(Arc::clone(&stub)).await?;
    Ok(Setup {
        _dir: dir,
        root,
        stub,
        registry,
    })
}

fn cli(s: &Setup, platform: Platform) -> Result<CodexCli, Box<dyn std::error::Error>> {
    Ok(CodexCli::codex_from(
        s.root.clone(),
        s.registry.clone(),
        Some(platform),
        false,
    )?)
}

fn mode(path: &Path) -> Result<u32, Box<dyn std::error::Error>> {
    Ok(std::fs::metadata(path)?.permissions().mode() & 0o777)
}

fn entries(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// The pinned release is downloaded once, checked and unpacked as its own tree into
/// `<root>/0.170.0-linux-x64` (the binary, its `rg` and resources, nothing outside `vendor/`),
/// readable and runnable by every sandbox user; callers at the same time wait for one install;
/// an earlier release and an interrupted install's leftovers go.
#[tokio::test]
async fn the_pinned_release_is_installed_once_as_its_own_tree() -> TestResult {
    let tarball = tarball_of(&release_entries())?;
    let platform = pinned(&tarball)?;
    let s = setup(tarball, true).await?;
    std::fs::create_dir_all(s.root.join("0.160.1-linux-x64/bin"))?;
    std::fs::write(s.root.join("0.160.1-linux-x64/bin/codex"), elf(62))?;
    std::fs::create_dir_all(s.root.join(".staging-1-2"))?;
    let cli = Arc::new(cli(&s, platform)?);
    assert_eq!(cli.installed(), None);
    let callers: Vec<_> = (0..3)
        .map(|_| {
            let cli = Arc::clone(&cli);
            tokio::spawn(async move { cli.ensure().await })
        })
        .collect();
    // All three wait on the one download under way.
    tokio::time::timeout(Duration::from_secs(10), async {
        while s.stub.downloads.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    if let Some(gate) = &s.stub.gate {
        gate.add_permits(10);
    }
    let mut provided = Vec::new();
    for caller in callers {
        provided.push(caller.await??);
    }
    assert_eq!(s.stub.downloads.load(Ordering::SeqCst), 1, "one download");
    let dir = s.root.join("0.170.0-linux-x64");
    for one in &provided {
        assert_eq!(one.dir, dir);
        assert_eq!(one.program, dir.join("bin/codex"));
    }
    assert_eq!(std::fs::read(dir.join("bin/codex"))?, elf(62));
    assert_eq!(mode(&dir.join("bin/codex"))?, 0o755);
    assert_eq!(mode(&dir.join("codex-path/rg"))?, 0o755);
    assert_eq!(mode(&dir.join("codex-package.json"))?, 0o644);
    assert_eq!(mode(&dir)?, 0o755);
    assert_eq!(mode(&dir.join("codex-resources"))?, 0o755);
    assert!(dir.join("codex-resources/bwrap").is_file());
    assert!(!dir.join("package.json").exists(), "only the vendor tree");
    assert_eq!(entries(&s.root), ["0.170.0-linux-x64"]);
    // Installed: no download again.
    assert_eq!(cli.ensure().await?, provided[0]);
    assert_eq!(s.stub.downloads.load(Ordering::SeqCst), 1);
    Ok(())
}

/// A refused install: its name, the tarball served, the bytes the sha512 is pinned to (`None`:
/// the tarball's) and a smaller unpacked bound.
type RefusedCase = (&'static str, Vec<u8>, Option<Vec<u8>>, Option<u64>);

/// The release is readable and runnable by every sandbox user whatever the service's
/// umask (here 077, on a thread with its own): in an isolated sandbox root maps to another user.
#[cfg(target_os = "linux")]
#[test]
fn the_release_is_open_to_sandbox_users_whatever_the_umask() -> TestResult {
    let tarball = tarball_of(&release_entries())?;
    let platform = pinned(&tarball)?;
    let provided = std::thread::spawn(move || -> Result<_, String> {
        nix::sched::unshare(nix::sched::CloneFlags::CLONE_FS).map_err(|e| e.to_string())?;
        nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(0o077));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        rt.block_on(async {
            let s = setup(tarball, false).await.map_err(|e| e.to_string())?;
            let cli = Arc::new(cli(&s, platform).map_err(|e| e.to_string())?);
            let provided = cli.ensure().await.map_err(|e| e.to_string())?;
            Ok((s, provided))
        })
    })
    .join()
    .map_err(|_| "the umask thread panicked")??;
    let (s, provided) = provided;
    for path in [
        s.root.clone(),
        provided.dir.clone(),
        provided.dir.join("bin"),
        provided.dir.join("codex-path"),
        provided.dir.join("codex-resources"),
        provided.program.clone(),
    ] {
        assert_eq!(mode(&path)?, 0o755, "{}", path.display());
    }
    assert_eq!(mode(&provided.dir.join("codex-package.json"))?, 0o644);
    Ok(())
}

/// An earlier release left by a process that ran it at the last install goes once this
/// process looks again, without a download.
#[tokio::test]
async fn an_earlier_release_goes_once_nothing_runs_it() -> TestResult {
    let tarball = tarball_of(&release_entries())?;
    let s = setup(tarball.clone(), false).await?;
    let current = s.root.join("0.170.0-linux-x64/bin");
    std::fs::create_dir_all(&current)?;
    std::fs::write(current.join("codex"), elf(62))?;
    std::fs::create_dir_all(s.root.join("0.160.1-linux-x64/bin"))?;
    std::fs::write(s.root.join("0.160.1-linux-x64/bin/codex"), elf(62))?;
    let cli = Arc::new(cli(&s, pinned(&tarball)?)?);
    cli.ensure().await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while s.root.join("0.160.1-linux-x64").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    assert_eq!(entries(&s.root), ["0.170.0-linux-x64"]);
    assert_eq!(s.stub.downloads.load(Ordering::SeqCst), 0);
    Ok(())
}

/// A platform without a provided CLI says so to a caller that must not wait, not that
/// the CLI is still being installed.
#[tokio::test]
async fn an_unsupported_platform_gives_its_reason() -> TestResult {
    let dir = tempfile::tempdir()?;
    let cli = Arc::new(CodexCli::codex_from(
        dir.path().join("codex-cli"),
        "http://127.0.0.1:1".into(),
        None,
        false,
    )?);
    let refused = cli.ensure().await.err().ok_or("no CLI expected")?;
    assert!(
        refused.to_string().contains("provides no Codex CLI"),
        "{refused}"
    );
    assert_eq!(cli.ready().err(), Some(refused));
    Ok(())
}

/// A tarball that is not the pinned one, that names a path outside its release or a
/// link, that holds no binary or one for another machine, or that is too large is refused, and
/// nothing is left installed or behind.
#[tokio::test]
async fn a_refused_tarball_installs_nothing() -> TestResult {
    let vendor = format!("package/vendor/{TRIPLE}");
    let good = tarball_of(&release_entries())?;
    let with = |extra: Entry| -> std::io::Result<Vec<u8>> {
        let mut entries = release_entries();
        entries.push(extra);
        tarball_of(&entries)
    };
    let link = Entry {
        path: format!("{vendor}/codex-path/sh"),
        kind: tar::EntryType::Symlink,
        mode: 0o777,
        body: Vec::new(),
    };
    let cases: Vec<RefusedCase> = vec![
        (
            "not the pinned sha512",
            good.clone(),
            Some(b"other".to_vec()),
            None,
        ),
        (
            "a path outside",
            with(file(&format!("{vendor}/../../escape"), 0o644, b"x"))?,
            None,
            None,
        ),
        ("a link", with(link)?, None, None),
        (
            "no binary",
            tarball_of(&[file(&format!("{vendor}/codex-path/rg"), 0o755, &elf(62))])?,
            None,
            None,
        ),
        (
            "another machine",
            tarball_of(&[file(&format!("{vendor}/bin/codex"), 0o755, &elf(183))])?,
            None,
            None,
        ),
        ("too large", good.clone(), None, Some(1024)),
    ];
    for (case, tarball, pin, max) in cases {
        let platform = pinned(pin.as_deref().unwrap_or(&tarball))?;
        let s = setup(tarball, false).await?;
        let mut cli = cli(&s, platform)?;
        if let Some(max) = max {
            cli = cli.with_max_unpacked(max);
        }
        let cli = Arc::new(cli);
        let refused = cli.ensure().await;
        assert!(refused.is_err(), "{case}: {refused:?}");
        assert_eq!(cli.installed(), None, "{case}");
        assert!(
            entries(&s.root).is_empty(),
            "{case}: {:?}",
            entries(&s.root)
        );
        assert!(
            !s.root.parent().ok_or("parent")?.join("escape").exists(),
            "{case}"
        );
    }
    Ok(())
}

/// A tarball larger than the download bound installs nothing, whether the registry says
/// its size (`Content-Length`) or streams it without one: the download stops at the bound.
#[tokio::test]
async fn a_tarball_over_the_download_bound_installs_nothing() -> TestResult {
    let tarball = tarball_of(&release_entries())?;
    for streamed in [false, true] {
        let s = setup(tarball.clone(), false).await?;
        s.stub.streamed.store(streamed, Ordering::SeqCst);
        let cli = Arc::new(cli(&s, pinned(&tarball)?)?.with_max_tarball(64));
        let refused = cli.ensure().await;
        assert!(
            refused
                .as_ref()
                .is_err_and(|e| e.to_string().contains("larger than allowed")),
            "streamed {streamed}: {refused:?}"
        );
        assert!(cli.installed().is_none());
        assert!(entries(&s.root).is_empty(), "{:?}", entries(&s.root));
    }
    Ok(())
}

/// An earlier release a process still runs (a Codex session from before a pin bump) is
/// kept by the install, and goes at the next start once nothing runs it.
#[tokio::test]
async fn an_earlier_release_a_process_runs_is_kept() -> TestResult {
    let tarball = tarball_of(&release_entries())?;
    let s = setup(tarball.clone(), false).await?;
    let earlier = s.root.join("0.160.1-linux-x64");
    std::fs::create_dir_all(earlier.join("bin"))?;
    // A real executable stands in for the earlier release's binary.
    std::fs::copy("/bin/sleep", earlier.join("bin/codex"))?;
    let mut running = tokio::process::Command::new(earlier.join("bin/codex"))
        .arg("600")
        .kill_on_drop(true)
        .spawn()?;
    let cli_now = Arc::new(cli(&s, pinned(&tarball)?)?);
    cli_now.ensure().await?;
    assert_eq!(entries(&s.root), ["0.160.1-linux-x64", "0.170.0-linux-x64"]);
    running.kill().await?;
    let _ = running.wait().await;
    // The next process looks once for earlier releases, now that nothing runs it.
    let next = Arc::new(cli(&s, pinned(&tarball)?)?);
    next.ensure().await?;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while earlier.exists() {
        if std::time::Instant::now() >= deadline {
            return Err("the earlier release stayed".into());
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(entries(&s.root), ["0.170.0-linux-x64"]);
    Ok(())
}

/// The download lies in a staging folder only Yulius can enter, whatever the umask (here
/// a common 022, on a thread with its own), so nobody swaps the hashed tarball before it is
/// unpacked.
#[cfg(target_os = "linux")]
#[test]
fn the_staging_folder_is_private() -> TestResult {
    let tarball = tarball_of(&release_entries())?;
    let platform = pinned(&tarball)?;
    let staging_mode = std::thread::spawn(move || -> Result<u32, String> {
        nix::sched::unshare(nix::sched::CloneFlags::CLONE_FS).map_err(|e| e.to_string())?;
        nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(0o022));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        rt.block_on(async {
            let s = setup(tarball, true).await.map_err(|e| e.to_string())?;
            let cli = Arc::new(cli(&s, platform).map_err(|e| e.to_string())?);
            let install = tokio::spawn({
                let cli = Arc::clone(&cli);
                async move { cli.ensure().await }
            });
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while s.stub.downloads.load(Ordering::SeqCst) == 0 {
                if std::time::Instant::now() >= deadline {
                    return Err("the download never started".to_owned());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let staging = entries(&s.root)
                .into_iter()
                .find(|name| name.starts_with(".staging-"))
                .ok_or("a staging folder")?;
            let staging_mode = mode(&s.root.join(staging)).map_err(|e| e.to_string())?;
            if let Some(gate) = &s.stub.gate {
                gate.add_permits(1);
            }
            install
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            Ok(staging_mode)
        })
    })
    .join()
    .map_err(|_| "the umask thread panicked")??;
    assert_eq!(staging_mode, 0o700);
    Ok(())
}

/// A failed install answers the next callers at once with its reason instead of each
/// waiting for a download, and is tried again after a while.
#[tokio::test]
async fn a_failed_install_is_not_retried_by_every_start() -> TestResult {
    let tarball = tarball_of(&release_entries())?;
    let s = setup(b"not a tarball".to_vec(), false).await?;
    let cli = Arc::new(cli(&s, pinned(&tarball)?)?);
    let first = cli.ensure().await;
    assert!(first.is_err());
    assert_eq!(cli.ensure().await.err(), first.err());
    assert_eq!(
        s.stub.downloads.load(Ordering::SeqCst),
        1,
        "answered at once"
    );
    // After the wait it is tried again, and the registry now has the release.
    if let Ok(mut served) = s.stub.tarball.lock() {
        *served = tarball;
    }
    let cli = Arc::new(
        Arc::try_unwrap(cli)
            .map_err(|_| "shared")?
            .with_retry_after(Duration::ZERO),
    );
    let provided = cli.ensure().await?;
    assert!(provided.program.is_file());
    assert_eq!(s.stub.downloads.load(Ordering::SeqCst), 2);
    Ok(())
}

/// The adapter gives Codex runs the provided CLI; while it cannot be installed it falls
/// back to the host's `codex`, and with neither the start fails with the reason (never
/// `codex: not found`). Without a provider runs keep `codex` from `PATH`.
#[tokio::test]
async fn codex_runs_get_the_provided_cli_or_a_reason() -> TestResult {
    let tarball = tarball_of(&release_entries())?;
    let s = setup(tarball.clone(), false).await?;
    let plain = HarnessAdapter::new("/bin/yulius".into(), "http://127.0.0.1:1");
    assert_eq!(plain.provide_codex(true).await?, None);

    let adapter = HarnessAdapter::new("/bin/yulius".into(), "http://127.0.0.1:1")
        .with_codex_cli(Arc::new(cli(&s, pinned(&tarball)?)?));
    let provided = adapter.provide_codex(true).await?.ok_or("provided")?;
    assert_eq!(provided.program, s.root.join("0.170.0-linux-x64/bin/codex"));

    let broken = setup(b"not a tarball".to_vec(), false).await?;
    let mut failing = HarnessAdapter::new("/bin/yulius".into(), "http://127.0.0.1:1")
        .with_codex_cli(Arc::new(cli(&broken, pinned(&tarball)?)?));
    failing.host_codex = None;
    match failing.provide_codex(true).await {
        Err(AppError::Upstream(reason)) => {
            assert!(reason.contains("could not be installed"), "{reason}");
            assert!(reason.contains(codex::VERSION), "{reason}");
        }
        other => return Err(format!("expected a reason, got {other:?}").into()),
    }
    failing.host_codex = Some("/usr/local/bin/codex".into());
    assert_eq!(failing.provide_codex(true).await?, None, "the host's codex");

    // A caller that must not wait (`install` false) gets only what is ready: the installed
    // release, the recent failure's reason, or, before any install, that it is not installed.
    assert_eq!(
        adapter.provide_codex(false).await?.map(|cli| cli.program),
        Some(s.root.join("0.170.0-linux-x64/bin/codex"))
    );
    failing.host_codex = None;
    assert!(
        matches!(failing.provide_codex(false).await, Err(AppError::Upstream(ref r)) if r.contains("tarball")),
        "the recent failure's reason"
    );
    let untried = setup(tarball.clone(), false).await?;
    let mut waiting = HarnessAdapter::new("/bin/yulius".into(), "http://127.0.0.1:1")
        .with_codex_cli(Arc::new(cli(&untried, pinned(&tarball)?)?));
    waiting.host_codex = None;
    match waiting.provide_codex(false).await {
        Err(AppError::Upstream(reason)) => {
            assert!(reason.contains("not installed yet"), "{reason}")
        }
        other => return Err(format!("expected a reason, got {other:?}").into()),
    }
    assert_eq!(
        untried.stub.downloads.load(Ordering::SeqCst),
        0,
        "nothing downloaded"
    );
    waiting.host_codex = Some("/usr/local/bin/codex".into());
    assert_eq!(
        waiting.provide_codex(false).await?,
        None,
        "the host's codex"
    );
    Ok(())
}

/// At start, Yulius installs the Codex CLI in the background
/// only when a Codex subscription exists, and the Antigravity CLI only when a Google one does:
/// an instance with none, or with Claude subscriptions only, downloads nothing.
#[tokio::test]
async fn the_clis_are_installed_at_start_only_for_their_subscriptions() -> TestResult {
    use yulius_app::model::{AccountRecord, AccountStatus};
    use yulius_app::ports::store::AccountStore as _;
    use yulius_domain::harness::Provider;
    use yulius_domain::ids::AccountId;
    use yulius_domain::time::Timestamp;
    let installs = Arc::new(AtomicUsize::new(0));
    let antigravity = Arc::new(AtomicUsize::new(0));
    let h = crate::support::harness_with(crate::support::HarnessOptions {
        harness: Some(Arc::new(crate::support::agents::FakeAgentHarness::new(
            "/bin/yulius".into(),
            "/bin/false".into(),
            crate::support::agents::FakeAgentOptions {
                codex_installs: Some(Arc::clone(&installs)),
                antigravity_installs: Some(Arc::clone(&antigravity)),
                ..Default::default()
            },
        ))),
        ..crate::support::HarnessOptions::default()
    })?;
    let account = |id: &str, provider: Provider| AccountRecord {
        id: AccountId::parse(id).unwrap_or_else(|_| unreachable!("a valid id")),
        provider,
        label: id.into(),
        credential: None,
        status: AccountStatus::Ok,
        usage_json: None,
        last_error: None,
        email: None,
        email_checked_at: None,
        email_typed: false,
        created_at: Timestamp(1),
        updated_at: Timestamp(1),
    };
    h.app.warm_agent_clis().await?;
    h.store
        .insert_account(&account("claude-1", Provider::Anthropic))
        .await?;
    h.app.warm_agent_clis().await?;
    assert_eq!(installs.load(Ordering::SeqCst), 0, "no Codex subscription");
    assert_eq!(
        antigravity.load(Ordering::SeqCst),
        0,
        "no Google subscription"
    );
    h.store
        .insert_account(&account("codex-1", Provider::OpenAi))
        .await?;
    h.app.warm_agent_clis().await?;
    assert_eq!(installs.load(Ordering::SeqCst), 1);
    assert_eq!(
        antigravity.load(Ordering::SeqCst),
        0,
        "no Google subscription"
    );
    h.store
        .insert_account(&account("google-1", Provider::Google))
        .await?;
    h.app.warm_agent_clis().await?;
    assert_eq!(installs.load(Ordering::SeqCst), 2);
    assert_eq!(antigravity.load(Ordering::SeqCst), 1);
    Ok(())
}

/// CLI-001 against npm itself (needs the network; run with `--ignored`): the pinned sha512 is
/// the published one, and the real release unpacks into a `bin/codex` that names its version.
#[tokio::test]
#[ignore = "downloads 167 MB from registry.npmjs.org"]
async fn the_real_pinned_release_installs_from_npm() -> TestResult {
    let dir = tempfile::tempdir()?;
    let cli = Arc::new(CodexCli::codex(dir.path().join("codex-cli"))?);
    let provided = cli.ensure().await?;
    let output = std::process::Command::new(&provided.program)
        .arg("--version")
        .output()?;
    let version = String::from_utf8_lossy(&output.stdout);
    assert_eq!(version.trim(), format!("codex-cli {}", codex::VERSION));
    assert!(provided.dir.join("codex-path/rg").is_file());
    Ok(())
}
