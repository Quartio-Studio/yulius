//! Exact-value redaction of agent secrets with real processes (SPEC-002 ADR-20, AC-VLT-4): the
//! `yulius --scrub-exec` Bash wrapper and the `PostToolUse` hook as Claude Code runs them, and a
//! fake agent in the real ptyhost whose terminal people watch.

mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use support::agents::{FakeAgentHarness, FakeAgentOptions};
use support::{FakeGitHub, TestResult};
use yulius::wiring::clock::SystemClock;
use yulius::wiring::git::GitAdapter;
use yulius::wiring::session::PtySessionHost;
use yulius_app::auth::AuthContext;
use yulius_app::context::{App, AppConfig, Deps};
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{AccountRecord, AccountStatus, SandboxRecord, TaskOrigin, UserRecord};
use yulius_contracts::api::secrets::AgentSecretInput;
use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::time::Timestamp;
use yulius_ptyhost::test_support::locate_fake_agent;
use yulius_store::SqliteStore;
use yulius_store::crypto::MasterKeyCrypto;

const VALUE: &str = "sk_test_51NzQ9redactValue8f3a";
const LABEL: &str = "[secret:STRIPE_KEY]";

fn exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_yulius"))
}

fn admin() -> Result<UserRecord, Box<dyn std::error::Error>> {
    Ok(UserRecord {
        id: UserId::parse("u-admin")?,
        github_login: Some("admin".into()),
        github_id: None,
        display_name: "Admin".into(),
        avatar_url: None,
        is_superadmin: true,
        is_instance_admin: true,
        password_hash: None,
        created_at: Timestamp(1),
        disabled_at: None,
    })
}

fn sandbox_record(user: &UserRecord) -> Result<SandboxRecord, Box<dyn std::error::Error>> {
    Ok(SandboxRecord {
        id: SandboxId::parse("s-vault")?,
        slug: "vault".into(),
        repo_owner: "acme".into(),
        repo_name: "vault".into(),
        github_repo_id: Some(1),
        default_branch: "main".into(),
        profile_json: "{}".into(),
        port_base: 41100,
        port_count: 10,
        onboarding_state: "not_needed".into(),
        onboarding_task_id: None,
        created_by: user.id.clone(),
        created_at: Timestamp(1),
        removed_at: None,
    })
}

async fn add_stripe_key(app: &App, user: &UserRecord, sandbox: &SandboxId) -> TestResult {
    let ctx = AuthContext {
        user: user.clone(),
        csrf_token: None,
        session_hash: None,
    };
    app.create_agent_secret(
        &ctx,
        sandbox.as_str(),
        &AgentSecretInput {
            name: "STRIPE_KEY".into(),
            hosts: vec!["api.stripe.test".into()],
            value: VALUE.into(),
            ..AgentSecretInput::default()
        },
    )
    .await?;
    Ok(())
}

/// A run's fingerprint file as Yulius writes it at launch (made by the server from a vault).
async fn fingerprint_file(dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let h = support::harness()?;
    let user = admin()?;
    use yulius_app::ports::store::{SandboxStore as _, UserStore as _};
    h.store.insert_user(&user).await?;
    let sandbox = sandbox_record(&user)?;
    h.store.insert_sandbox(&sandbox).await?;
    add_stripe_key(&h.app, &user, &sandbox.id).await?;
    let set = h.app.scrub_fingerprints_for(&sandbox.id).await?;
    let path = dir.join("scrub.json");
    let text = serde_json::to_string(&set)?;
    assert!(!text.contains(VALUE), "fingerprints hold no value");
    std::fs::write(&path, text)?;
    Ok(path)
}

/// Runs `yulius --scrub-exec <command>` as Claude Code does, inside a run (fingerprint file
/// only: Yulius is not reachable, so the file is the source).
fn scrub_exec(
    scrub_file: Option<&Path>,
    cwd: &Path,
    command: &str,
) -> std::io::Result<std::process::Output> {
    let mut child = Command::new(exe());
    child
        .arg("--scrub-exec")
        .arg(command)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("SHELL", "/bin/sh")
        .stdin(Stdio::null());
    if let Some(file) = scrub_file {
        child.env("YULIUS_SCRUB_FILE", file);
    }
    child.output()
}

/// AC-VLT-4: a value echoed by a command, written to a file and read back, base64-encoded or
/// split across output chunks never reaches the agent; the command's status is kept; without
/// fingerprints a run's command does not run.
#[tokio::test]
async fn the_bash_wrapper_redacts_every_form_of_a_value() -> TestResult {
    let dir = tempfile::tempdir()?;
    let scrub = fingerprint_file(dir.path()).await?;
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work)?;
    // A value someone left in the repository (the agent never got it from Yulius).
    std::fs::write(work.join(".env"), format!("STRIPE_KEY={VALUE}\nOTHER=1\n"))?;

    let cases: Vec<(&str, String)> = vec![
        ("echoed", format!("echo {VALUE}")),
        ("a file read back", "cat .env".into()),
        ("grep", "grep STRIPE .env".into()),
        ("base64", "base64 < .env".into()),
        (
            "base64 at another offset",
            "(printf 'xy'; cat .env) | base64".into(),
        ),
        (
            "split across writes",
            format!(
                "printf '{}'; sleep 0.3; printf '{}\\n'",
                &VALUE[..9],
                &VALUE[9..]
            ),
        ),
        (
            "byte by byte",
            format!("for c in $(echo {VALUE} | sed 's/./& /g'); do printf '%s' \"$c\"; done; echo"),
        ),
        ("stderr", format!("echo {VALUE} >&2")),
    ];
    for (what, command) in &cases {
        let output = scrub_exec(Some(&scrub), &work, command)?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{what}: {stderr}");
        for text in [&stdout, &stderr] {
            assert!(
                !text.contains(VALUE),
                "{what}: the value reached the agent: {text}"
            );
            assert!(
                !text.contains(&VALUE[4..20]),
                "{what}: a long part reached the agent: {text}"
            );
        }
        assert!(
            stdout.contains(LABEL) || stderr.contains(LABEL),
            "{what}: no label in {stdout:?} / {stderr:?}"
        );
    }
    let decoded = scrub_exec(
        Some(&scrub),
        &work,
        "base64 < .env | base64 -d 2>/dev/null || base64 -D < /dev/null",
    )?;
    assert!(
        !String::from_utf8_lossy(&decoded.stdout).contains(VALUE),
        "decoding redacted output gives no value"
    );

    // Ordinary output is untouched and the status is the command's.
    let plain = scrub_exec(Some(&scrub), &work, "echo hello; exit 7")?;
    assert_eq!(plain.status.code(), Some(7));
    assert_eq!(String::from_utf8_lossy(&plain.stdout), "hello\n");

    // A run without its fingerprints fails closed: the command does not run.
    let missing = dir.path().join("missing.json");
    let closed = scrub_exec(Some(&missing), &work, "touch ran")?;
    assert_eq!(closed.status.code(), Some(126));
    assert!(!work.join("ran").exists(), "the command must not run");
    // Outside a Yulius run the wrapper is transparent.
    let outside = scrub_exec(None, &work, "echo outside")?;
    assert_eq!(String::from_utf8_lossy(&outside.stdout), "outside\n");

    // Agent commands run 5 nice levels below the agent (CI jobs keep the CPU first).
    if Path::new("/usr/bin/nice").exists() {
        let level = |out: &[u8]| String::from_utf8_lossy(out).trim().parse::<i32>();
        let own = level(&Command::new("/usr/bin/nice").output()?.stdout)?;
        let wrapped = level(&scrub_exec(Some(&scrub), &work, "nice")?.stdout)?;
        assert_eq!(wrapped, (own + 5).min(19));
    }
    Ok(())
}

/// The `PostToolUse` hook as Claude Code calls it: a Read result holding the value comes back as
/// `updatedToolOutput` without it.
#[tokio::test]
async fn the_tool_hook_rewrites_results_that_hold_a_value() -> TestResult {
    let dir = tempfile::tempdir()?;
    let scrub = fingerprint_file(dir.path()).await?;
    let input = json!({
        "session_id": "s", "hook_event_name": "PostToolUse", "tool_name": "Read",
        "tool_input": {"file_path": "/w/.env"},
        "tool_response": {"type": "text", "file": {"filePath": "/w/.env", "content": format!("STRIPE_KEY={VALUE}\n"), "numLines": 1}}
    });
    let mut child = Command::new(exe())
        .args(["hook", "claude", "PostToolUse"])
        .env_clear()
        .env("YULIUS_SCRUB_FILE", &scrub)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    {
        use std::io::Write as _;
        child
            .stdin
            .take()
            .ok_or("stdin")?
            .write_all(input.to_string().as_bytes())?;
    }
    let output = child.wait_with_output()?;
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout)?;
    assert!(!text.contains(VALUE), "{text}");
    let answer: serde_json::Value = serde_json::from_str(&text)?;
    assert_eq!(
        answer["hookSpecificOutput"]["updatedToolOutput"]["file"]["content"],
        format!("STRIPE_KEY={LABEL}\n")
    );
    Ok(())
}

fn git(dir: &Path, args: &[&str]) -> TestResult {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()?;
    if !out.status.success() {
        return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&out.stderr)).into());
    }
    Ok(())
}

fn seed_repo(data: &Path, sandbox: &SandboxId) -> TestResult {
    let origin = data.join("origin.git");
    std::fs::create_dir_all(&origin)?;
    git(&origin, &["init", "--bare", "-q", "-b", "main"])?;
    let seed = data.join("seed");
    std::fs::create_dir_all(&seed)?;
    git(&seed, &["init", "-q", "-b", "main"])?;
    std::fs::write(seed.join("README.md"), "# fixture\n")?;
    git(&seed, &["add", "README.md"])?;
    git(&seed, &["commit", "-q", "-m", "init"])?;
    git(
        &seed,
        &["remote", "add", "origin", &origin.to_string_lossy()],
    )?;
    git(&seed, &["push", "-q", "origin", "main"])?;
    let repo = data.join("sandboxes").join(sandbox.as_str()).join("repo");
    std::fs::create_dir_all(repo.parent().ok_or("no parent")?)?;
    git(
        data,
        &[
            "clone",
            "-q",
            &origin.to_string_lossy(),
            &repo.to_string_lossy(),
        ],
    )?;
    Ok(())
}

async fn wait_for(app: &App, task: &TaskId, needle: &str) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if app.runtime(task).hub.screen_text().contains(needle) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!(
        "timed out waiting for {needle:?}; screen:\n{}",
        app.runtime(task).hub.screen_text()
    )
    .into())
}

/// The task terminal people watch (VLT-007) and the run's environment: the agent's environment
/// holds the placeholder and the wrapper, never the value; a value typed or printed into the
/// terminal shows as the label; the run's fingerprint file holds no value.
#[test]
fn the_terminal_and_the_run_environment_hold_no_value() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let dir = tempfile::Builder::new().prefix("yv").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let store = SqliteStore::open(&data.join("yulius.db"))?;
        let mut config = AppConfig::new(
            data.clone(),
            "https://yulius.test",
            "http://127.0.0.1:9",
            exe(),
        );
        config.contained = false;
        let deps = Deps {
            store: Arc::new(store),
            clock: Arc::new(SystemClock),
            crypto: Arc::new(MasterKeyCrypto::from_key(&[7u8; 32])?),
            github: Arc::new(FakeGitHub::default()),
            session_host: Arc::new(PtySessionHost::new(exe())),
            harness: Arc::new(FakeAgentHarness::new(
                exe(),
                fake.clone(),
                FakeAgentOptions {
                    report_env: vec![
                        "STRIPE_KEY".into(),
                        "CLAUDE_CODE_SHELL_PREFIX".into(),
                        "YULIUS_SCRUB_FILE".into(),
                    ],
                    ..FakeAgentOptions::default()
                },
            )),
            git: Arc::new(GitAdapter::new("git")),
            system: Arc::new(support::FixedSystem),
            browser: Arc::new(yulius::wiring::browser::UnavailableBrowser),
            usage: Arc::new(yulius_app::ports::usage::NoUsageProbe),
        };
        let app = App::new(config, deps);
        let user = admin()?;
        let sandbox = sandbox_record(&user)?;
        app.deps.store.insert_user(&user).await?;
        app.deps.store.insert_sandbox(&sandbox).await?;
        app.deps
            .store
            .insert_account(&AccountRecord {
                id: AccountId::parse("a-claude")?,
                provider: Provider::Anthropic,
                label: "Claude test".into(),
                credential: Some(app.deps.crypto.seal(b"sk-ant-oat01-test-credential")?),
                status: AccountStatus::Ok,
                usage_json: None,
                last_error: None,
                created_at: Timestamp(1),
                updated_at: Timestamp(1),
            })
            .await?;
        seed_repo(&data, &sandbox.id)?;
        add_stripe_key(&app, &user, &sandbox.id).await?;
        let task = app
            .create_task_internal(NewTask {
                sandbox: sandbox.id.clone(),
                created_by: user.id.clone(),
                prompt: "echo things".into(),
                harness: Harness::ClaudeCode,
                title: Some("Redaction".into()),
                workflow_id: None,
                steps: Vec::new(),
                model: None,
                effort: None,
                origin: TaskOrigin::Ui,
                automation_run: None,
                start: true,
                mode: yulius_app::model::TaskMode::Prompt,
            })
            .await?;
        wait_for(&app, &task.id, "FAKE-AGENT READY").await?;
        // The placeholder (24 characters), not the value (29), is in the agent's environment;
        // the Bash wrapper and its fingerprints are set.
        wait_for(&app, &task.id, "ENV STRIPE_KEY=set len=24").await?;
        wait_for(&app, &task.id, "ENV CLAUDE_CODE_SHELL_PREFIX=set").await?;
        wait_for(&app, &task.id, "ENV YULIUS_SCRUB_FILE=set").await?;

        // A value typed into the terminal (echoed by the PTY and by the agent) shows as the label.
        app.send_input(&task.id, format!("{VALUE}\r").as_bytes())
            .await?;
        wait_for(&app, &task.id, &format!("ECHO: {LABEL}")).await?;
        let screen = app.runtime(&task.id).hub.screen_text();
        assert!(!screen.contains(VALUE), "{screen}");
        // Split over two writes it is still found.
        app.send_input(&task.id, &VALUE.as_bytes()[..10]).await?;
        tokio::time::sleep(Duration::from_millis(10)).await;
        app.send_input(&task.id, format!("{}\r", &VALUE[10..]).as_bytes())
            .await?;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let screen = app.runtime(&task.id).hub.screen_text();
        assert!(!screen.contains(VALUE), "{screen}");

        // The run's files: fingerprints and launch record hold no value.
        let attempt = app
            .deps
            .store
            .live_attempt(&task.id)
            .await?
            .ok_or("no attempt")?;
        for entry in std::fs::read_dir(&attempt.run_dir)? {
            let path = entry?.path();
            if path.is_file() {
                let bytes = std::fs::read(&path)?;
                assert!(
                    !bytes.windows(VALUE.len()).any(|w| w == VALUE.as_bytes()),
                    "{} holds the value",
                    path.display()
                );
            }
        }
        assert!(Path::new(&attempt.run_dir).join("scrub.json").is_file());
        app.stop_task_now(&task.id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    runtime.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
