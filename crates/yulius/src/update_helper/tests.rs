//! The root update helper against real Git repositories and fake `cargo`, `systemctl` and
//! server health (SPEC-002 ADR-18, AC-OPS-8).

use std::net::TcpListener;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const OLD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn git_in(dir: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()?;
    if !output.status.success() {
        return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn script(path: &Path, body: &str) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n"))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

/// A data directory, a helper state directory, an origin with `main` (A → B → C) and a side
/// branch (D), fake `cargo` and `systemctl`, and an installed binary.
struct Env {
    dir: tempfile::TempDir,
    args: HelperArgs,
    a: String,
    b: String,
    c: String,
    d: String,
}

impl Env {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        let data = root.join("data");
        std::fs::create_dir_all(&data)?;
        if nix::unistd::getuid().is_root() {
            // The helper reads only requests of the (unprivileged) data directory's owner.
            let nobody = nix::unistd::Uid::from_raw(65534);
            nix::unistd::chown(&data, Some(nobody), None)?;
        }
        let origin = root.join("origin.git");
        let work = root.join("work");
        std::fs::create_dir_all(&work)?;
        git_in(&root, &["init", "--quiet", "--bare", "origin.git"])?;
        git_in(&work, &["init", "--quiet", "-b", "main"])?;
        let mut commits = Vec::new();
        for title in ["feat: a", "fix: b", "feat: c"] {
            git_in(&work, &["commit", "--quiet", "--allow-empty", "-m", title])?;
            commits.push(git_in(&work, &["rev-parse", "HEAD"])?);
        }
        git_in(&work, &["checkout", "--quiet", "-b", "side", &commits[1]])?;
        git_in(
            &work,
            &["commit", "--quiet", "--allow-empty", "-m", "feat: d"],
        )?;
        commits.push(git_in(&work, &["rev-parse", "HEAD"])?);
        let url = format!("file://{}", origin.display());
        git_in(&work, &["push", "--quiet", &url, "main", "side"])?;
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin)?;
        script(
            &bin.join("cargo"),
            r#"echo "$@" >> "$(dirname "$0")/cargo.log"
printf '   Compiling yulius-domain v0.1.0\n' >&2
printf '    Building [=>   ] 1/3: yulius-domain\r' >&2
printf '    Building [==>  ] 2/3: yulius-app\r' >&2
if [ -f "$(dirname "$0")/cargo.fail" ]; then printf 'error[E0425]: cannot find value `x`\n' >&2; exit 101; fi
mkdir -p "$CARGO_TARGET_DIR/release"
printf '%s' "$YULIUS_BUILD_COMMIT" > "$CARGO_TARGET_DIR/release/yulius"
printf '    Building [===> ] 3/3: yulius\r' >&2
printf '    Finished `release` profile\n' >&2"#,
        )?;
        script(
            &bin.join("systemctl"),
            r#"d="$(dirname "$0")"
echo "$*" >> "$d/systemctl.log"
case "$1" in
  stop) rm -f "$d/active" ;;
  start) touch "$d/active" ;;
  is-active) [ -f "$d/active" ] || exit 3 ;;
  *) exit 1 ;;
esac"#,
        )?;
        std::fs::write(bin.join("active"), "")?;
        let binary = bin.join("yulius");
        std::fs::write(&binary, OLD)?;
        std::fs::write(data.join("yulius.db"), "old database")?;
        std::fs::write(data.join("yulius.db-wal"), "old wal")?;
        if nix::unistd::getuid().is_root() {
            for name in ["yulius.db", "yulius.db-wal"] {
                let nobody = nix::unistd::Uid::from_raw(65534);
                nix::unistd::chown(&data.join(name), Some(nobody), None)?;
            }
        }
        let args = HelperArgs {
            data_dir: data,
            state_dir: root.join("state"),
            repo: url,
            binary,
            service: "yqa-test.service".into(),
            health_url: "http://127.0.0.1:9/internal/version".into(),
            health_secs: 4,
            cargo: bin.join("cargo"),
            git: PathBuf::from("git"),
            systemctl: bin.join("systemctl"),
            host_check: None,
        };
        Ok(Self {
            a: commits[0].clone(),
            b: commits[1].clone(),
            c: commits[2].clone(),
            d: commits[3].clone(),
            dir,
            args,
        })
    }

    fn bin(&self, name: &str) -> PathBuf {
        self.args.cargo.with_file_name(name)
    }

    fn request(&self, id: &str, action: HelperAction, commit: &str) -> TestResult {
        let request = HelperRequest {
            version: VERSION,
            id: id.into(),
            action,
            commit: commit.into(),
            token: None,
            requested_at: now_ms(),
        };
        self.raw_request(&serde_json::to_vec(&request)?)
    }

    fn raw_request(&self, bytes: &[u8]) -> TestResult {
        let dir = self.args.data_dir.join("update");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(REQUEST_FILE);
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, bytes)?;
        if nix::unistd::getuid().is_root() {
            nix::unistd::chown(&path, Some(nix::unistd::Uid::from_raw(65534)), None)?;
        }
        Ok(())
    }

    fn run(&self) -> Result<HelperStatus, Box<dyn std::error::Error>> {
        Helper::new(self.args.clone()).run()?;
        Ok(serde_json::from_slice(&std::fs::read(
            self.args.state_dir.join(STATUS_FILE),
        )?)?)
    }

    /// A health endpoint that reports the installed binary's content as its commit while the
    /// fake service is active; `broken` makes the build of that commit fail its check (and
    /// "migrate" the database first, as a new version would).
    fn health(&mut self, broken: Option<String>) -> TestResult {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        self.args.health_url = format!("http://{}/internal/version", listener.local_addr()?);
        let binary = self.args.binary.clone();
        let active = self.bin("active");
        let db = self.args.data_dir.join("yulius.db");
        let broken = Arc::new(broken);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                // Read the whole request head before answering (closing with unread data
                // would reset the connection).
                let mut head = Vec::new();
                let mut buf = [0u8; 512];
                while !head.ends_with(b"\r\n\r\n") {
                    match std::io::Read::read(&mut stream, &mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let commit = std::fs::read_to_string(&binary).unwrap_or_default();
                let (status, body) = if !active.exists() {
                    (503, String::new())
                } else if broken.as_deref() == Some(commit.as_str()) {
                    let _ = std::fs::write(&db, "migrated by the broken version");
                    (500, String::new())
                } else {
                    (
                        200,
                        format!("{{\"commit\":\"{commit}\",\"version\":\"0.1.0\"}}"),
                    )
                };
                let _ = write!(
                    stream,
                    "HTTP/1.0 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Ok(())
    }

    fn stage(&self, commit: &str) -> TestResult {
        let staged = self.args.state_dir.join("staged");
        std::fs::create_dir_all(&staged)?;
        std::fs::write(staged.join("yulius"), commit)?;
        std::fs::write(staged.join("commit"), commit)?;
        Ok(())
    }
}

fn request_json(overrides: &[(&str, serde_json::Value)]) -> Vec<u8> {
    let mut value = serde_json::json!({
        "version": 1, "id": "0123456789-build", "action": "build",
        "commit": "0123456789abcdef0123456789abcdef01234567", "requested_at": 1_000_000,
    });
    for (key, field) in overrides {
        if field.is_null() {
            value.as_object_mut().map(|o| o.remove(*key));
        } else {
            value[*key] = field.clone();
        }
    }
    value.to_string().into_bytes()
}

#[test]
fn requests_are_parsed_strictly() {
    use serde_json::json;
    let now = 1_000_000;
    assert!(parse_request(&request_json(&[]), now).is_ok());
    assert!(parse_request(&request_json(&[("token", json!("ghs_A1b2.c-d_e"))]), now).is_ok());
    let refused: &[(&str, serde_json::Value)] = &[
        ("version", json!(2)),
        ("id", json!("short")),
        ("id", json!("has a space in it")),
        ("action", json!("install-and-run")),
        ("commit", json!("0123456")),
        ("commit", json!("0123456789ABCDEF0123456789ABCDEF01234567")),
        ("token", json!("ghs_x\nX-Injected: 1")),
        ("token", json!("ghs x")),
        ("token", json!("")),
        ("token", json!("ghs_a:b")),
        ("requested_at", json!(now - REQUEST_TTL_MS - 1)),
        ("requested_at", json!(now + 120_000)),
        ("extra", json!("unknown fields are refused")),
        ("commit", serde_json::Value::Null),
    ];
    for (key, value) in refused {
        assert!(
            parse_request(&request_json(&[(key, value.clone())]), now).is_err(),
            "{key} = {value}"
        );
    }
    assert!(parse_request(b"not json", now).is_err());
    assert!(parse_request(b"[]", now).is_err());
}

/// A stateless GitHub App installation token (`ghs_<APPID>_<JWT>`, about 520 characters since
/// 2026-10-02), synthetic and `len` bytes long.
fn stateless_installation_token(len: usize) -> String {
    let head = "ghs_1234567_eyJhbGciOiJSUzI1NiJ9.";
    let tail = ".c2lnbmF0dXJl-_";
    format!("{head}{}{tail}", "x".repeat(len - head.len() - tail.len()))
}

#[test]
fn stateless_installation_tokens_are_accepted() {
    use serde_json::json;
    let now = 1_000_000;
    for len in [520, 1024, MAX_TOKEN_BYTES] {
        let token = stateless_installation_token(len);
        assert!(
            parse_request(&request_json(&[("token", json!(token))]), now).is_ok(),
            "a {len}-byte installation token is refused"
        );
    }
    let oversized = stateless_installation_token(MAX_TOKEN_BYTES + 1);
    assert!(parse_request(&request_json(&[("token", json!(oversized))]), now).is_err());
    let injected = format!("{}\r\nX-Injected: 1", stateless_installation_token(520));
    assert!(parse_request(&request_json(&[("token", json!(injected))]), now).is_err());
}

#[test]
fn cargo_progress_lines_are_parsed() {
    assert_eq!(
        parse_progress("    Building [=====>      ] 120/341: serde, tokio"),
        Some((120, 341))
    );
    assert_eq!(
        parse_progress("    Building [=> ] 3/3: yulius(bin)"),
        Some((3, 3))
    );
    assert_eq!(parse_progress("   Compiling serde v1.0.229"), None);
    assert_eq!(parse_progress("    Building [=> ] 9/3: x"), None);
    assert_eq!(parse_progress("    Building [=> ] 0/0: x"), None);
}

#[test]
fn only_regular_files_of_the_owner_are_read() -> TestResult {
    let dir = tempfile::tempdir()?;
    let me = nix::unistd::getuid().as_raw();
    let file = dir.path().join("request.json");
    std::fs::write(&file, "{}")?;
    assert_eq!(read_owned_file(&file, me, 16)?, Some(b"{}".to_vec()));
    assert!(
        read_owned_file(&file, me.wrapping_add(1), 16).is_err(),
        "another owner"
    );
    assert!(read_owned_file(&file, me, 1).is_err(), "too large");
    assert_eq!(read_owned_file(&dir.path().join("missing"), me, 16)?, None);
    let link = dir.path().join("link.json");
    symlink(&file, &link)?;
    assert!(read_owned_file(&link, me, 16).is_err(), "a link");
    assert!(read_owned_file(dir.path(), me, 16).is_err(), "a directory");
    let fifo = dir.path().join("fifo");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRWXU)?;
    assert!(
        read_owned_file(&fifo, me, 16).is_err(),
        "a FIFO, without blocking"
    );
    Ok(())
}

#[test]
fn a_main_commit_is_built_exactly_once_with_progress() -> TestResult {
    let env = Env::new()?;
    env.request("run-0001-build", HelperAction::Build, &env.b)?;
    let status = env.run()?;
    assert_eq!(status.state, HelperState::Done, "{status:?}");
    assert_eq!(status.step, "built");
    assert_eq!(status.id, "run-0001-build");
    assert_eq!(status.commit, env.b);
    let staged = env.args.state_dir.join("staged");
    assert_eq!(std::fs::read_to_string(staged.join("commit"))?, env.b);
    assert_eq!(
        std::fs::read_to_string(staged.join("yulius"))?,
        env.b,
        "built with YULIUS_BUILD_COMMIT"
    );
    let checkout = env.args.state_dir.join("src");
    assert_eq!(git_in(&checkout, &["rev-parse", "HEAD"])?, env.b);
    assert_eq!(
        std::fs::read_to_string(env.bin("cargo.log"))?.trim(),
        "build --release --locked -p yulius"
    );
    // The same request (a re-trigger, a reboot) is not acted on again.
    let again = env.run()?;
    assert_eq!(again, status);
    assert_eq!(
        std::fs::read_to_string(env.bin("cargo.log"))?
            .lines()
            .count(),
        1
    );
    // The service was never touched by a build.
    assert!(!env.bin("systemctl.log").exists());
    Ok(())
}

#[test]
fn a_failed_build_reports_its_error_and_stages_nothing() -> TestResult {
    let env = Env::new()?;
    std::fs::write(env.bin("cargo.fail"), "")?;
    env.request("run-0002-build", HelperAction::Build, &env.c)?;
    let status = env.run()?;
    assert_eq!(status.state, HelperState::Failed);
    assert!(
        status
            .error
            .as_deref()
            .is_some_and(|e| e.contains("cannot find value")),
        "{status:?}"
    );
    assert!(
        status
            .log_tail
            .iter()
            .any(|l| l.starts_with("error[E0425]"))
    );
    assert!(!env.args.state_dir.join("staged").join("commit").exists());
    Ok(())
}

#[test]
fn commits_off_main_older_than_installed_or_unknown_are_refused() -> TestResult {
    let env = Env::new()?;
    let refuse = |id: &str, commit: &str, expected: &str| -> TestResult {
        env.request(id, HelperAction::Build, commit)?;
        let status = env.run()?;
        assert_eq!(status.state, HelperState::Failed, "{commit}: {status:?}");
        let error = status.error.unwrap_or_default();
        assert!(error.contains(expected), "{commit}: {error}");
        Ok(())
    };
    // Only main is fetched: a commit of another branch is not even there.
    refuse("run-0003-build", &env.d, "was not found on main")?;
    refuse(
        "run-0004-build",
        "0123456789abcdef0123456789abcdef01234567",
        "was not found on main",
    )?;
    // Present (fetched earlier, e.g. before a force push) but not reachable from main.
    git_in(
        &env.args.state_dir.join("src"),
        &["fetch", "--quiet", &env.args.repo, "side"],
    )?;
    refuse("run-0014-build", &env.d, "is not on main")?;
    std::fs::write(env.args.state_dir.join("installed-commit"), &env.c)?;
    refuse("run-0005-build", &env.b, "older than the installed")?;
    refuse("run-0006-build", &env.c, "already installed")?;
    refuse("run-0007-build", &env.a, "older than the installed")?;
    assert!(!env.bin("cargo.log").exists(), "nothing was built");
    Ok(())
}

#[test]
fn refused_requests_are_reported_under_their_id() -> TestResult {
    let env = Env::new()?;
    env.raw_request(&request_json(&[
        ("id", serde_json::json!("run-0008-build")),
        ("requested_at", serde_json::json!(1)),
    ]))?;
    let status = env.run()?;
    assert_eq!(status.id, "run-0008-build");
    assert_eq!(status.state, HelperState::Failed);
    assert!(status.error.unwrap_or_default().contains("stale"));
    // A request that is a link is not read at all.
    let path = env.args.data_dir.join("update").join(REQUEST_FILE);
    std::fs::remove_file(&path)?;
    let elsewhere = env.dir.path().join("elsewhere.json");
    std::fs::write(&elsewhere, "{}")?;
    symlink(&elsewhere, &path)?;
    Helper::new(env.args.clone()).run()?;
    assert!(!env.bin("cargo.log").exists());
    Ok(())
}

#[test]
fn the_read_token_reaches_git_as_a_header_only() -> TestResult {
    let mut env = Env::new()?;
    let log = env.bin("git.log");
    script(
        &env.bin("git"),
        &format!(
            r#"echo "ARGS $*" >> "{log}"
case "$*" in
  *fetch*) echo "HEADER $GIT_CONFIG_KEY_0 $GIT_CONFIG_VALUE_0" >> "{log}"; exit 1 ;;
esac
[ -n "$GIT_CONFIG_VALUE_0" ] && echo "LEAK $*" >> "{log}"
exit 0"#,
            log = log.display()
        ),
    )?;
    env.args.git = env.bin("git");
    let token = stateless_installation_token(520);
    let request = HelperRequest {
        version: VERSION,
        id: "run-0009-build".into(),
        action: HelperAction::Build,
        commit: env.b.clone(),
        token: Some(token.clone()),
        requested_at: now_ms(),
    };
    env.raw_request(&serde_json::to_vec(&request)?)?;
    std::fs::create_dir_all(env.args.state_dir.join("src").join(".git"))?;
    let _ = env.run()?;
    let text = std::fs::read_to_string(&log)?;
    let calls = text
        .lines()
        .filter(|l| l.starts_with("ARGS"))
        .collect::<Vec<_>>();
    assert!(calls.iter().any(|l| l.contains(" fetch ")), "{text}");
    assert!(calls.iter().all(|l| !l.contains(&token)));
    assert!(
        !text.contains("LEAK"),
        "only the fetch gets the header: {text}"
    );
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
    assert!(
        text.contains(&format!(
            "HEADER http.extraHeader Authorization: Basic {basic}"
        )),
        "{text}"
    );
    let status: HelperStatus =
        serde_json::from_slice(&std::fs::read(env.args.state_dir.join(STATUS_FILE))?)?;
    assert!(!format!("{status:?}").contains(&token));
    Ok(())
}

#[test]
fn a_staged_build_is_installed_after_a_backup() -> TestResult {
    let mut env = Env::new()?;
    env.health(None)?;
    env.stage(&env.c)?;
    env.request("run-0010-install", HelperAction::Install, &env.c)?;
    let status = env.run()?;
    assert_eq!(status.state, HelperState::Done, "{status:?}");
    assert_eq!(status.step, "succeeded");
    assert_eq!(status.previous.as_deref(), Some(OLD));
    assert_eq!(std::fs::read_to_string(&env.args.binary)?, env.c);
    assert_eq!(
        std::fs::read_to_string(env.args.state_dir.join("installed-commit"))?,
        env.c
    );
    let backup = env.args.state_dir.join("backups").join("run-0010-install");
    assert_eq!(std::fs::read_to_string(backup.join("yulius"))?, OLD);
    assert_eq!(
        std::fs::read_to_string(backup.join("yulius.db"))?,
        "old database"
    );
    assert_eq!(
        std::fs::read_to_string(backup.join("yulius.db-wal"))?,
        "old wal"
    );
    let calls = std::fs::read_to_string(env.bin("systemctl.log"))?;
    let verbs: Vec<&str> = calls
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    assert_eq!(verbs.first(), Some(&"stop"));
    assert!(verbs.contains(&"start"));
    assert!(calls.lines().all(|l| l.ends_with("yqa-test.service")));
    Ok(())
}

#[test]
fn a_failed_check_restores_the_previous_binary_and_database() -> TestResult {
    let mut env = Env::new()?;
    env.health(Some(env.c.clone()))?;
    std::fs::write(env.args.data_dir.join("yulius.db-shm"), "shm")?;
    env.stage(&env.c)?;
    env.request("run-0011-install", HelperAction::Install, &env.c)?;
    let status = env.run()?;
    assert_eq!(status.state, HelperState::RolledBack, "{status:?}");
    assert!(
        status
            .error
            .as_deref()
            .is_some_and(|e| e.contains("did not become healthy")),
        "{status:?}"
    );
    assert_eq!(std::fs::read_to_string(&env.args.binary)?, OLD);
    let data = &env.args.data_dir;
    assert_eq!(
        std::fs::read_to_string(data.join("yulius.db"))?,
        "old database"
    );
    assert_eq!(
        std::fs::read_to_string(data.join("yulius.db-wal"))?,
        "old wal"
    );
    assert!(!data.join("yulius.db-shm").exists());
    assert_eq!(
        std::fs::metadata(data.join("yulius.db"))?
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(!env.args.state_dir.join("installed-commit").exists());
    assert!(
        env.bin("active").exists(),
        "the previous version runs again"
    );
    Ok(())
}

#[test]
fn a_linked_database_is_never_backed_up_or_replaced() -> TestResult {
    let mut env = Env::new()?;
    env.health(None)?;
    let secret = env.dir.path().join("not-the-database");
    std::fs::write(&secret, "root only")?;
    let db = env.args.data_dir.join("yulius.db");
    std::fs::remove_file(&db)?;
    symlink(&secret, &db)?;
    env.stage(&env.c)?;
    env.request("run-0012-install", HelperAction::Install, &env.c)?;
    let status = env.run()?;
    assert_eq!(status.state, HelperState::Failed);
    assert!(
        status
            .error
            .unwrap_or_default()
            .contains("nothing was installed")
    );
    assert_eq!(std::fs::read_to_string(&env.args.binary)?, OLD);
    assert!(env.bin("active").exists(), "the service runs again");
    let backup = env.args.state_dir.join("backups").join("run-0012-install");
    assert!(!backup.join("yulius.db").exists());
    Ok(())
}

#[test]
fn install_needs_the_staged_build_of_that_commit() -> TestResult {
    let mut env = Env::new()?;
    env.health(None)?;
    env.stage(&env.b)?;
    env.request("run-0013-install", HelperAction::Install, &env.c)?;
    let status = env.run()?;
    assert_eq!(status.state, HelperState::Failed);
    assert!(status.error.unwrap_or_default().contains("No build of"));
    assert!(
        !env.bin("systemctl.log").exists(),
        "the service was not touched"
    );
    assert_eq!(std::fs::read_to_string(&env.args.binary)?, OLD);
    Ok(())
}

/// A fake `yulius host check` that records its arguments and prints `report` (exit `code`).
fn host_check(env: &mut Env, report: &str, code: i32) -> TestResult {
    let program = env.bin("host-check");
    script(
        &program,
        &format!(
            "echo \"$@\" > \"$(dirname \"$0\")/host-check.args\"\necho 'progress noise'\necho '{report}'\nexit {code}"
        ),
    )?;
    env.args.host_check = Some(program);
    Ok(())
}

/// SPEC-203 HST-012: after a healthy install the new version re-checks the host; the report is
/// in the status and kept in `host-check.json`.
#[test]
fn a_healthy_install_re_checks_the_host_with_the_new_version() -> TestResult {
    let mut env = Env::new()?;
    env.health(None)?;
    host_check(
        &mut env,
        r#"{"ok":true,"lines":[{"name":"sandbox Docker","outcome":"ok","detail":"each isolated sandbox gets its own engine"}]}"#,
        0,
    )?;
    env.stage(&env.c)?;
    env.request("run-0020-install", HelperAction::Install, &env.c)?;
    let status = env.run()?;
    assert_eq!(status.state, HelperState::Done, "{status:?}");
    let report = status.host_check.ok_or("no host check in the status")?;
    assert!(report.ok, "{report:?}");
    assert_eq!(report.commit.as_deref(), Some(env.c.as_str()));
    assert!(report.checked_at > 0);
    let args = std::fs::read_to_string(env.bin("host-check.args"))?;
    assert_eq!(
        args.trim(),
        format!(
            "host check --json --data-dir {} --service yqa-test.service --update-state {}",
            env.args.data_dir.display(),
            env.args.state_dir.display()
        )
    );
    let kept: HostCheckReport =
        serde_json::from_slice(&std::fs::read(env.args.state_dir.join(HOST_CHECK_FILE))?)?;
    assert_eq!(kept, report);
    Ok(())
}

/// SPEC-203 HST-012: a re-check that finds sandboxes without Docker is a regression in the
/// status, with its reason, but the update stands (no rollback).
#[test]
fn a_docker_regression_is_reported_not_rolled_back() -> TestResult {
    let mut env = Env::new()?;
    env.health(None)?;
    host_check(
        &mut env,
        r#"{"ok":false,"lines":[{"name":"Docker Engine","outcome":"failed","detail":"no Docker on the host; run `sudo yulius host setup`"},{"name":"sandbox Docker","outcome":"failed","detail":"the host has no dockerd"}]}"#,
        1,
    )?;
    env.stage(&env.c)?;
    env.request("run-0021-install", HelperAction::Install, &env.c)?;
    let status = env.run()?;
    assert_eq!(status.state, HelperState::Done, "{status:?}");
    assert_eq!(std::fs::read_to_string(&env.args.binary)?, env.c);
    let report = status.host_check.ok_or("no host check")?;
    assert!(!report.ok);
    let problems = report.problems().join("\n");
    assert!(
        problems.contains("Docker Engine: no Docker on the host"),
        "{problems}"
    );
    assert!(
        problems.contains("sandbox Docker: the host has no dockerd"),
        "{problems}"
    );
    Ok(())
}

/// A version without `host check` (or one that answers nothing) is "not re-checked", said
/// plainly, never passed.
#[test]
fn a_version_without_host_check_is_not_checked() -> TestResult {
    let mut env = Env::new()?;
    env.health(None)?;
    let program = env.bin("old-yulius");
    script(
        &program,
        "echo \"error: unrecognized subcommand 'host'\" >&2\nexit 2",
    )?;
    env.args.host_check = Some(program);
    env.stage(&env.c)?;
    env.request("run-0022-install", HelperAction::Install, &env.c)?;
    let status = env.run()?;
    assert_eq!(status.state, HelperState::Done, "{status:?}");
    let report = status.host_check.ok_or("no host check")?;
    assert!(!report.ok);
    let error = report.error.unwrap_or_default();
    assert!(error.contains("not re-checked"), "{error}");
    assert!(error.contains("unrecognized subcommand 'host'"), "{error}");
    assert!(error.contains("sudo yulius host check"), "{error}");
    Ok(())
}

/// Review of SPEC-203: whatever the new version prints, the status stays readable by the server
/// (under `MAX_STATUS_BYTES`): every field of the report is bounded.
#[test]
fn an_oversized_report_is_bounded() -> TestResult {
    let mut env = Env::new()?;
    env.health(None)?;
    // Under the 1 MiB the helper reads, so the report is parsed and must be bounded itself.
    let huge = "x".repeat(100_000);
    let lines: Vec<String> = (0..60)
        .map(|_| format!(r#"{{"name":"{huge}","outcome":"failed","detail":"{huge}"}}"#))
        .collect();
    host_check(
        &mut env,
        &format!(
            r#"{{"ok":false,"error":"{huge}","lines":[{}]}}"#,
            lines[..3].join(",")
        ),
        1,
    )?;
    env.stage(&env.c)?;
    env.request("run-0023-install", HelperAction::Install, &env.c)?;
    let status = env.run()?;
    let bytes = std::fs::read(env.args.state_dir.join(STATUS_FILE))?;
    assert!(
        (bytes.len() as u64) < yulius_contracts::update::MAX_STATUS_BYTES,
        "{} bytes",
        bytes.len()
    );
    let report = status.host_check.ok_or("no host check")?;
    assert!(!report.ok);
    assert!(report.lines.iter().all(|l| l.name.len() <= 200));
    assert!(report.error.as_deref().is_some_and(|e| e.len() <= 2000));
    Ok(())
}
