//! Codex as a second harness with real processes (SPEC-451): supervisor → detached
//! `yulius ptyhost` → fake agent writing a Codex CLI 0.167 shaped rollout. A Codex task runs on a
//! Codex account in its own Codex home, its rollout is found and watched, its usage reaches its
//! account, it moves to another Codex account at a usage limit by a native resume, a restart
//! reattaches it, and the credentials Codex refreshed are kept. No test calls OpenAI.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::supervision::{
    build_full, runtime, screen_contains, seed_records, seed_repo, task, wait_for,
};
use crate::support;
use crate::support::TestResult;
use crate::support::agents::FakeAgentOptions;
use yulius_app::context::App;
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{AccountRecord, AccountStatus, LaunchKind, TaskOrigin};
use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;
use yulius_ptyhost::test_support::locate_fake_agent;

/// A synthetic Codex `auth.json` refreshed at `last_refresh`.
fn codex_auth(last_refresh: &str) -> String {
    format!(
        r#"{{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{{"id_token":"h.e30.s","access_token":"SYNTHETIC-access-{last_refresh}","refresh_token":"SYNTHETIC-refresh","account_id":"acc-synthetic"}},"last_refresh":"{last_refresh}"}}"#
    )
}

async fn add_codex_account(
    app: &App,
    id: &str,
    auth: &str,
) -> Result<AccountId, Box<dyn std::error::Error>> {
    let account = AccountId::parse(id)?;
    app.deps
        .store
        .insert_account(&AccountRecord {
            id: account.clone(),
            provider: Provider::OpenAi,
            label: format!("Codex {id}"),
            credential: Some(app.deps.crypto.seal(auth.as_bytes())?),
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
            email: None,
            email_checked_at: None,
        })
        .await?;
    Ok(account)
}

async fn new_codex_task(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    title: &str,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    let record = app
        .create_task_internal(NewTask {
            sandbox: sandbox.clone(),
            created_by: user.clone(),
            prompt: format!("{title}: answer with an echo"),
            harness: Harness::Codex,
            title: Some(title.into()),
            workflow_id: None,
            steps: Vec::new(),
            model: None,
            effort: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            start: true,
            mode: yulius_app::model::TaskMode::Prompt,
            supertask: false,
        })
        .await?;
    Ok(record.id)
}

async fn stored_credential(
    app: &App,
    account: &AccountId,
) -> Result<String, Box<dyn std::error::Error>> {
    let sealed = app
        .deps
        .store
        .account(account)
        .await?
        .ok_or("account")?
        .credential
        .ok_or("credential")?;
    Ok(String::from_utf8(app.deps.crypto.open(&sealed)?)?)
}

/// An RFC 3339 time `ms_ago` milliseconds before now (a refresh Codex just made).
fn ago(ms_ago: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0));
    yulius_app::auth::rfc3339(Timestamp(now - ms_ago))
}

/// The output tokens stored for `attempt`.
fn saved_output(data: &Path, attempt: &str) -> Option<i64> {
    let db = rusqlite::Connection::open(data.join("yulius.db")).ok()?;
    db.query_row(
        "SELECT output_tokens FROM metrics WHERE attempt_id = ?1",
        [attempt],
        |row| row.get(0),
    )
    .ok()
    .flatten()
}

fn temp() -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    // A short base: run sockets live at <data>/runs/<uuid>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    Ok((dir, data))
}

/// SPEC-451 CDX-002, CDX-003, CDX-004, CDX-006, AC-451-1/3/5: a Codex task runs Codex on a Codex
/// account (never the Claude one) in `<sandbox>/codex/<account>`, whose `auth.json`, `config.toml`
/// and `hooks.json` are private and whose `sessions` is the sandbox's; its session is learnt from
/// the rollout; its turns, usage and rate limits are read; a restart reattaches the same process
/// and reads the rollout again; and when the run ends, the credentials Codex refreshed meanwhile
/// replace the stored ones.
#[test]
fn a_codex_task_runs_in_its_home_survives_a_restart_and_keeps_refreshed_credentials() -> TestResult
{
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let agent = FakeAgentOptions {
        transcript: true,
        ..FakeAgentOptions::default()
    };
    let first = runtime()?;
    let (task_id, account, child_pid, home) = first.block_on(async {
        let app = build_full(&data, Some(&fake), agent.clone(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let account =
            add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_codex_task(&app, &sandbox, &user, "Codex echo").await?;
        wait_for("the fake agent banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let attempt = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("no live attempt")?;
        assert_eq!(attempt.harness, Harness::Codex);
        assert_eq!(
            attempt.account_id.as_ref(),
            Some(&account),
            "a Codex account"
        );
        assert_eq!(attempt.provider_session_id, None, "Codex names its session");

        let sandbox_dir = data.join("sandboxes").join(sandbox.as_str());
        let home = sandbox_dir.join("codex").join(account.as_str());
        let auth = std::fs::read_to_string(home.join("auth.json"))?;
        assert_eq!(auth, codex_auth("2026-11-01T10:00:00Z"));
        for file in ["auth.json", "config.toml", "hooks.json"] {
            let mode = std::fs::metadata(home.join(file))?.permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{file}");
        }
        let worktree = task(&app, &id).await?.worktree_path.ok_or("worktree")?;
        let config = std::fs::read_to_string(home.join("config.toml"))?;
        assert!(
            config.contains(&format!(
                "[projects.\"{worktree}\"]\ntrust_level = \"trusted\""
            )),
            "{config}"
        );
        assert_eq!(
            std::fs::read_link(home.join("sessions"))?,
            PathBuf::from("../sessions")
        );
        let hooks: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(home.join("hooks.json"))?)?;
        assert!(
            hooks["hooks"]["Stop"][0]["hooks"][0]["command"]
                .as_str()
                .is_some_and(|c| c.ends_with("hook codex Stop")),
            "{hooks}"
        );

        app.send_input(&id, b"hello codex\r").await?;
        wait_for("the echo", Duration::from_secs(10), || {
            screen_contains(&app, &id, "ECHO: hello codex")
        })
        .await?;
        // The rollout is in the sandbox's shared sessions, found by the id Codex gave it.
        let session = attempt.id.as_str().to_owned();
        wait_for(
            "the session learnt from the rollout",
            Duration::from_secs(30),
            || async {
                app.deps
                    .store
                    .attempt(&attempt.id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|a| a.provider_session_id)
                    .as_deref()
                    == Some(session.as_str())
            },
        )
        .await?;
        // The lineage records it in the same watch round, right after the attempt.
        wait_for(
            "the session in the lineage",
            Duration::from_secs(10),
            || async {
                app.deps
                    .store
                    .provider_sessions(&id)
                    .await
                    .unwrap_or_default()
                    .iter()
                    .any(|l| l.session_id == session && l.harness == Harness::Codex)
            },
        )
        .await?;
        assert!(
            sandbox_dir
                .join("codex/sessions/2026/11/09")
                .join(format!("rollout-2026-11-09T00-00-00-{session}.jsonl"))
                .is_file()
        );
        // Its usage and the account's rate limits come from the rollout.
        wait_for("the run's tokens", Duration::from_secs(30), || async {
            saved_output(&data, attempt.id.as_str()).is_some_and(|n| n > 0)
        })
        .await?;
        wait_for("the account's window", Duration::from_secs(30), || async {
            app.deps
                .store
                .account(&account)
                .await
                .ok()
                .flatten()
                .and_then(|a| a.usage_json)
                .is_some_and(|json| {
                    yulius_app::usage_json::from_json(&json)
                        .primary
                        .and_then(|w| w.used_percent)
                        == Some(10.0)
                })
        })
        .await?;
        Ok::<_, Box<dyn std::error::Error>>((id, account, attempt.child_pid, home))
    })?;
    // The server "crashes"; the run goes on in its host.
    first.shutdown_timeout(Duration::from_secs(5));

    let second = runtime()?;
    second.block_on(async {
        let app = build_full(&data, Some(&fake), agent, 8)?;
        app.reconcile().await?;
        let attempt = app
            .deps
            .store
            .live_attempt(&task_id)
            .await?
            .ok_or("attempt lost on restart")?;
        assert_eq!(attempt.child_pid, child_pid, "same process, not respawned");
        app.send_input(&task_id, b"again\r").await?;
        wait_for("the echo after restart", Duration::from_secs(10), || {
            screen_contains(&app, &task_id, "ECHO: again")
        })
        .await?;
        // Codex refreshed its tokens during the run: the newer file is kept when the run ends,
        // and the account, limited meanwhile, keeps its state.
        let refreshed = codex_auth(&ago(60_000));
        std::fs::write(home.join("auth.json"), &refreshed)?;
        app.deps
            .store
            .set_account_state(&account, AccountStatus::Limited, None, None, app.now())
            .await?;
        app.send_input(&task_id, b"/exit\r").await?;
        wait_for("the run's end", Duration::from_secs(30), || async {
            task(&app, &task_id)
                .await
                .is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        wait_for(
            "the written-back credentials",
            Duration::from_secs(10),
            || async {
                stored_credential(&app, &account)
                    .await
                    .is_ok_and(|stored| stored == refreshed)
            },
        )
        .await?;
        let stored = app.deps.store.account(&account).await?.ok_or("account")?;
        assert_eq!(
            stored.status,
            AccountStatus::Limited,
            "the account keeps its state"
        );
        // No run of the account is left in the sandbox: its credential does not stay there.
        assert!(
            !home.join("auth.json").exists(),
            "auth.json left in the sandbox"
        );
        assert!(home.join("config.toml").exists());
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    second.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-451 CDX-006: the Codex home's `auth.json` is the agent's to write, so only a genuine
/// refresh goes back into the vault. A newer one another run of the account left in the sandbox
/// is taken at the next launch instead of being overwritten with the vault's older copy; one the
/// agent forged (another ChatGPT account) is never taken.
#[test]
fn only_a_genuine_codex_refresh_reaches_the_vault() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let account =
            add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        seed_repo(&data, &sandbox)?;
        // Another run of the account refreshed its tokens and is still going: its file is newer.
        let home = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("codex")
            .join(account.as_str());
        std::fs::create_dir_all(&home)?;
        let left = codex_auth(&ago(120_000));
        std::fs::write(home.join("auth.json"), &left)?;
        let id = new_codex_task(&app, &sandbox, &user, "Codex refresh").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        assert_eq!(std::fs::read_to_string(home.join("auth.json"))?, left);
        assert_eq!(stored_credential(&app, &account).await?, left);
        // The agent writes a sign-in of its own: it is not taken when the run ends.
        let forged = codex_auth(&ago(1_000)).replace("acc-synthetic", "acc-attacker");
        std::fs::write(home.join("auth.json"), &forged)?;
        app.send_input(&id, b"/exit\r").await?;
        wait_for("the run's end", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        assert_eq!(stored_credential(&app, &account).await?, left);
        // A file that did not reach the vault is not removed: were it a genuine refresh that
        // could not be stored, it is the only copy of the rotated tokens.
        assert_eq!(std::fs::read_to_string(home.join("auth.json"))?, forged);
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// An administrator's context for `user`.
async fn admin_ctx(
    app: &App,
    user: &UserId,
) -> Result<yulius_app::auth::AuthContext, Box<dyn std::error::Error>> {
    Ok(yulius_app::auth::AuthContext {
        user: app.deps.store.user(user).await?.ok_or("user")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    })
}

/// Runs a Codex task to its end: started, `refreshed` written into its Codex home by "Codex"
/// when given, then `/exit`.
async fn run_codex_task(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    title: &str,
    home: &Path,
    seen: &str,
    refreshed: Option<&str>,
) -> TestResult {
    let id = new_codex_task(app, sandbox, user, title).await?;
    wait_for("the banner", Duration::from_secs(30), || {
        screen_contains(app, &id, "FAKE-AGENT READY")
    })
    .await?;
    assert_eq!(std::fs::read_to_string(home.join("auth.json"))?, seen);
    if let Some(refreshed) = refreshed {
        std::fs::write(home.join("auth.json"), refreshed)?;
    }
    app.send_input(&id, b"/exit\r").await?;
    wait_for("the run's end", Duration::from_secs(30), || async {
        task(app, &id).await.is_ok_and(|t| !t.status.is_live())
    })
    .await?;
    Ok(())
}

/// SPEC-451 CDX-011, CDX-006: a sign-in imported as an `auth.json` (from `codex login` on a
/// laptop) is what a Codex task's home gets; the tokens Codex refreshes during the run go back
/// into the account; and a later import replaces them for the next run.
#[test]
fn an_imported_codex_sign_in_reaches_the_run_and_its_refresh_is_kept() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        seed_repo(&data, &sandbox)?;
        let ctx = admin_ctx(&app, &user).await?;
        let imported = codex_auth(&ago(3_600_000));
        let created = app
            .create_account(
                &ctx,
                &yulius_contracts::api::admin::CreateAccount {
                    provider: "openai".into(),
                    label: "Codex laptop".into(),
                    api_key: None,
                    auth_json: Some(imported.clone()),
                },
            )
            .await?;
        assert_eq!(created.login_id, None, "no sign-in runs");
        let account = AccountId::parse(&created.account.id)?;
        let home = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("codex")
            .join(account.as_str());

        let refreshed = codex_auth(&ago(60_000));
        run_codex_task(
            &app,
            &sandbox,
            &user,
            "Codex import",
            &home,
            &imported,
            Some(&refreshed),
        )
        .await?;
        wait_for(
            "the written-back credentials",
            Duration::from_secs(10),
            || async {
                stored_credential(&app, &account)
                    .await
                    .is_ok_and(|stored| stored == refreshed)
            },
        )
        .await?;

        // Signed in again on the laptop and imported: the next run gets the new file.
        let again = codex_auth(&ago(1_000)).replace("SYNTHETIC-refresh", "SYNTHETIC-refresh-2");
        app.import_account_auth(
            &ctx,
            account.as_str(),
            &yulius_contracts::api::admin::ImportAccountAuth {
                auth_json: again.clone(),
            },
        )
        .await?;
        assert_eq!(stored_credential(&app, &account).await?, again);
        run_codex_task(&app, &sandbox, &user, "Codex reimport", &home, &again, None).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-451 CLI-002, AC-451-6: a Codex task starts the Codex CLI Yulius provides by its absolute
/// path, never `codex` from a `PATH` that may not have it.
#[test]
fn a_codex_task_starts_the_provided_codex_cli() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let dir = data.join("codex-cli").join("0.167.0-linux-x64");
    let program = dir.join("bin/codex");
    let agent = FakeAgentOptions {
        provided_codex: Some(yulius_app::ports::harness::ProvidedCli {
            program: program.clone(),
            dir,
        }),
        ..FakeAgentOptions::default()
    };
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), agent, 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_codex_task(&app, &sandbox, &user, "Codex provided").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let attempt = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("no live attempt")?;
        let launch = std::fs::read_to_string(Path::new(&attempt.run_dir).join("fake-launch.txt"))?;
        assert!(
            launch.contains(&format!("program=Some({:?})", program.to_string_lossy())),
            "{launch}"
        );
        app.send_input(&id, b"/exit\r").await?;
        wait_for("the run's end", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-451 CLI-002, AC-451-6 (Linux): a contained Codex run sees the provided Codex CLI's
/// directory, bound read-only at its own path, though the rest of the data directory stays hidden.
/// Skipped with a message where bubblewrap cannot create namespaces.
#[cfg(target_os = "linux")]
#[test]
fn a_contained_codex_run_sees_the_provided_codex_cli() -> TestResult {
    let probe = std::process::Command::new("bwrap")
        .args([
            "--die-with-parent",
            "--unshare-pid",
            "--ro-bind",
            "/",
            "/",
            "--proc",
            "/proc",
            "--",
            "/bin/true",
        ])
        .output();
    if !probe.as_ref().is_ok_and(|out| out.status.success()) {
        eprintln!(
            "SKIPPED a_contained_codex_run_sees_the_provided_codex_cli: bwrap cannot create namespaces"
        );
        return Ok(());
    }
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let fake_dir = fake.parent().ok_or("fake-agent dir")?.to_path_buf();
    let (_dir, data) = temp()?;
    let dir = data.join("codex-cli").join("0.167.0-linux-x64");
    std::fs::create_dir_all(dir.join("bin"))?;
    std::fs::write(dir.join("bin/codex"), b"provided")?;
    let agent = FakeAgentOptions {
        provided_codex: Some(yulius_app::ports::harness::ProvidedCli {
            program: dir.join("bin/codex"),
            dir: dir.clone(),
        }),
        ..FakeAgentOptions::default()
    };
    let rt = runtime()?;
    rt.block_on(async {
        let app = crate::supervision::build_tuned(&data, Some(&fake), agent, move |c| {
            c.contained = true;
            c.contained_readonly = vec![fake_dir];
        })?;
        let (sandbox, user) = seed_records(&app).await?;
        add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_codex_task(&app, &sandbox, &user, "Codex contained").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        for (path, expected) in [
            (dir.join("bin/codex"), "READ codex OK"),
            (data.join("yulius.db"), "READ yulius.db ERR"),
        ] {
            app.send_input(&id, format!("/read {}\r", path.display()).as_bytes())
                .await?;
            wait_for(expected, Duration::from_secs(10), || {
                screen_contains(&app, &id, expected)
            })
            .await?;
        }
        crate::support::stop_tasks(&app, &[&id]).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-451 CLI-002: a Codex CLI that cannot be provided fails the start (its reason goes to the
/// terminal's "Could not start" notice) before anything is spawned, instead of a run that dies
/// with `codex: not found`.
#[test]
fn a_codex_start_without_a_codex_cli_says_why() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let agent = FakeAgentOptions {
        codex_unavailable: Some("the Codex CLI 0.167.0 could not be installed (offline)".into()),
        ..FakeAgentOptions::default()
    };
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), agent, 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_codex_task(&app, &sandbox, &user, "Codex offline").await?;
        wait_for("the failed start", Duration::from_secs(30), || async {
            task(&app, &id)
                .await
                .is_ok_and(|t| matches!(t.status, TaskStatus::Failed(_)))
        })
        .await?;
        assert!(app.deps.store.live_attempt(&id).await?.is_none(), "no run");
        assert!(
            app.deps.store.attempts(&id).await?.is_empty(),
            "nothing was spawned"
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-451 CDX-009, AC-451-2: a Codex task at its usage limit moves to another Codex account by
/// a native resume of the same session, never to the Claude account, and the limited account is
/// marked; Codex does not go on by itself, so the resume asks it to.
#[test]
fn a_codex_task_at_its_limit_moves_to_another_codex_account() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: Some(1),
            ..FakeAgentOptions::default()
        };
        let app = build_full(&data, Some(&fake), agent, 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let auth = codex_auth("2026-11-01T10:00:00Z");
        let one = add_codex_account(&app, "a-codex-1", &auth).await?;
        let two = add_codex_account(&app, "a-codex-2", &auth).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_codex_task(&app, &sandbox, &user, "Codex rotation").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        app.send_input(&id, b"hello\r").await?;
        wait_for(
            "the switch to another account",
            Duration::from_secs(90),
            || async {
                let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
                attempts.len() >= 2 && attempts.last().is_some_and(|a| !a.state.is_terminal())
            },
        )
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        let (first, second) = (&attempts[0], &attempts[1]);
        let accounts = [Some(one.clone()), Some(two.clone())];
        assert!(accounts.contains(&first.account_id), "{first:?}");
        assert!(accounts.contains(&second.account_id), "{second:?}");
        assert_ne!(first.account_id, second.account_id, "another Codex account");
        assert_eq!(second.harness, Harness::Codex, "the task keeps its harness");
        assert_eq!(second.launch_kind, LaunchKind::Resume);
        assert!(first.provider_session_id.is_some());
        assert_eq!(
            first.provider_session_id, second.provider_session_id,
            "same session"
        );
        let launch = std::fs::read_to_string(Path::new(&second.run_dir).join("fake-launch.txt"))?;
        assert!(
            launch.contains("Resume {") && launch.contains("continue_interrupted: true"),
            "{launch}"
        );
        wait_for("the resumed run", Duration::from_secs(30), || async {
            task(&app, &id)
                .await
                .is_ok_and(|t| t.status == TaskStatus::Running)
        })
        .await?;
        let limited = app
            .deps
            .store
            .account(first.account_id.as_ref().ok_or("first account")?)
            .await?
            .ok_or("the first account")?;
        assert_eq!(limited.status, AccountStatus::Limited);
        let claude = app
            .deps
            .store
            .account(&AccountId::parse("a-claude")?)
            .await?
            .ok_or("claude")?;
        assert_eq!(
            claude.status,
            AccountStatus::Ok,
            "the Claude account is untouched"
        );
        support::stop_tasks(&app, &[&id]).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-451 CDX-009: with no Codex account left with capacity, a Codex task waits instead of
/// running on the Claude account.
#[test]
fn a_codex_task_waits_while_no_codex_account_has_capacity() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let account =
            add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        let full = yulius_domain::usage::AccountUsage {
            primary: Some(yulius_domain::usage::UsageWindow {
                used_percent: Some(100.0),
                window_minutes: Some(300),
                resets_at: Some(Timestamp(4_000_000_000_000)),
            }),
            limit_reached: true,
            limit_resets_at: Some(Timestamp(4_000_000_000_000)),
            ..yulius_domain::usage::AccountUsage::default()
        };
        app.deps
            .store
            .set_account_state(
                &account,
                AccountStatus::Limited,
                Some(&yulius_app::usage_json::to_json(&full)),
                None,
                app.now(),
            )
            .await?;
        seed_repo(&data, &sandbox)?;
        let id = new_codex_task(&app, &sandbox, &user, "Codex waits").await?;
        wait_for("the pause", Duration::from_secs(30), || async {
            task(&app, &id)
                .await
                .is_ok_and(|t| t.status == TaskStatus::WaitingCapacity)
        })
        .await?;
        assert!(
            app.deps.store.attempts(&id).await?.is_empty(),
            "no run on the Claude account"
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The notices said so far on a watch of the task's terminal.
fn notices(watch: &mut tokio::sync::broadcast::Receiver<yulius_app::hub::HubFrame>) -> Vec<String> {
    let mut said = Vec::new();
    while let Ok(frame) = watch.try_recv() {
        if let yulius_app::hub::HubFrame::Notice(text) = frame {
            said.push(text);
        }
    }
    said
}

/// SPEC-451 CDX-013, AC-451-7, with SPEC-447 (MOV-001, MOV-003, MOV-006): a Codex task moves only
/// between Codex subscriptions. A person's move to the Claude subscription is refused naming why;
/// removing its Codex subscription moves it to the other Codex one in the same session, never to
/// the Claude one that has room; removing that one too leaves it waiting, asking for a Codex
/// subscription.
#[test]
fn a_codex_task_moves_only_between_codex_subscriptions() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            ..FakeAgentOptions::default()
        };
        let app = build_full(&data, Some(&fake), agent, 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let ctx = admin_ctx(&app, &user).await?;
        let auth = codex_auth("2026-11-01T10:00:00Z");
        add_codex_account(&app, "a-codex-1", &auth).await?;
        add_codex_account(&app, "a-codex-2", &auth).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_codex_task(&app, &sandbox, &user, "Codex moves").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let first = app.deps.store.attempts(&id).await?;
        let on = first[0].account_id.clone().ok_or("the first account")?;
        let other = if on.as_str() == "a-codex-1" {
            "a-codex-2"
        } else {
            "a-codex-1"
        };

        let refused = app
            .switch_task(
                &ctx,
                id.as_str(),
                &yulius_contracts::api::tasks::ContinueTask {
                    account_id: Some("a-claude".into()),
                },
            )
            .await;
        match refused {
            Err(yulius_app::error::AppError::Capacity(text)) => {
                assert!(text.contains("is not a Codex subscription"), "{text}");
            }
            other => return Err(format!("expected a refusal, got {other:?}").into()),
        }
        assert_eq!(
            app.deps.store.attempts(&id).await?.len(),
            1,
            "nothing moved"
        );

        let mut watch = app.runtime(&id).hub.watch();
        app.delete_account(&ctx, on.as_str()).await?;
        let mut said = Vec::new();
        wait_for(
            "the move to the other Codex subscription",
            Duration::from_secs(60),
            || {
                said.extend(notices(&mut watch));
                let app = app.clone();
                let id = id.clone();
                async move {
                    let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
                    attempts.len() == 2 && !attempts[1].state.is_terminal()
                }
            },
        )
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(
            attempts[1].account_id.as_ref().map(AccountId::as_str),
            Some(other)
        );
        assert_eq!(
            attempts[1].harness,
            Harness::Codex,
            "the task keeps its harness"
        );
        assert_eq!(
            attempts[1].provider_session_id,
            attempts[0].provider_session_id
        );
        wait_for("the removal's notice", Duration::from_secs(10), || {
            said.extend(notices(&mut watch));
            let done = said
                .iter()
                .any(|n| n.contains("was removed: this session continues on Codex"));
            async move { done }
        })
        .await?;
        assert!(
            !said.iter().any(|n| n.contains("Claude")),
            "a Codex task's notices never speak of Claude: {said:?}"
        );

        app.delete_account(&ctx, other).await?;
        wait_for("the wait", Duration::from_secs(60), || async {
            task(&app, &id)
                .await
                .is_ok_and(|t| t.status == TaskStatus::WaitingCapacity)
        })
        .await?;
        let runs = app.deps.store.attempts(&id).await?;
        assert!(
            runs.iter()
                .all(|a| a.account_id.as_ref().map(AccountId::as_str) != Some("a-claude")),
            "never on the Claude subscription: {runs:?}"
        );
        let request = app
            .deps
            .store
            .task_needs_you(&id)
            .await?
            .ok_or("Needs you")?;
        assert!(
            request.action.contains("Codex subscription"),
            "{}",
            request.action
        );
        wait_for("the stranded notice", Duration::from_secs(10), || {
            said.extend(notices(&mut watch));
            let done = said
                .iter()
                .any(|n| n.contains("No Codex subscription is left"));
            async move { done }
        })
        .await?;
        assert!(
            !said.iter().any(|n| n.contains("Claude")),
            "a Codex task's notices never speak of Claude: {said:?}"
        );
        support::stop_tasks(&app, &[&id]).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-451 CDX-013 and CDX-006: Continue on a Claude subscription is refused up front for a
/// Codex task, which keeps resting instead of failing; and a Codex start whose subscription is
/// removed just after it was chosen (SPEC-447's race point) starts on the other Codex one without
/// leaving the removed one's credentials in the sandbox.
#[test]
fn a_codex_start_that_loses_its_subscription_leaves_no_credentials() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let ctx = admin_ctx(&app, &user).await?;
        let auth = codex_auth("2026-11-01T10:00:00Z");
        add_codex_account(&app, "a-codex-1", &auth).await?;
        add_codex_account(&app, "a-codex-2", &auth).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_codex_task(&app, &sandbox, &user, "Codex race").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let on = app.deps.store.attempts(&id).await?[0]
            .account_id
            .clone()
            .ok_or("the first account")?;
        let other = if on.as_str() == "a-codex-1" {
            "a-codex-2"
        } else {
            "a-codex-1"
        };
        support::stop_tasks(&app, &[&id]).await?;
        let home = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("codex")
            .join(on.as_str());
        wait_for(
            "the run's credentials removed",
            Duration::from_secs(10),
            || async { !home.join("auth.json").exists() },
        )
        .await?;

        let resting = task(&app, &id).await?.status;
        let refused = app
            .continue_task(
                &ctx,
                id.as_str(),
                &yulius_contracts::api::tasks::ContinueTask {
                    account_id: Some("a-claude".into()),
                },
            )
            .await;
        match refused {
            Err(yulius_app::error::AppError::Capacity(text)) => {
                assert!(text.contains("is not a Codex subscription"), "{text}");
            }
            other => return Err(format!("expected a refusal, got {other:?}").into()),
        }
        assert_eq!(
            task(&app, &id).await?.status,
            resting,
            "the task still rests"
        );
        assert_eq!(
            app.deps.store.attempts(&id).await?.len(),
            1,
            "nothing started"
        );

        let (reached_tx, reached_rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        yulius_app::race::on(
            yulius_app::race::Point::AccountChosen,
            Path::new(id.as_str()),
            move || {
                let _ = reached_tx.send(());
                let _ = go_rx.recv_timeout(Duration::from_secs(60));
            },
        );
        let continuing = {
            let (app, ctx, id, on) = (app.clone(), ctx.clone(), id.clone(), on.clone());
            tokio::spawn(async move {
                app.continue_task(
                    &ctx,
                    id.as_str(),
                    &yulius_contracts::api::tasks::ContinueTask {
                        account_id: Some(on.as_str().to_owned()),
                    },
                )
                .await
            })
        };
        tokio::task::spawn_blocking(move || reached_rx.recv_timeout(Duration::from_secs(60)))
            .await?
            .map_err(|_| "the launch never chose its subscription")?;
        app.delete_account(&ctx, on.as_str()).await?;
        go_tx.send(())?;
        continuing.await??;
        wait_for(
            "the run on the other Codex subscription",
            Duration::from_secs(60),
            || async {
                app.deps
                    .store
                    .live_attempt(&id)
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|a| a.account_id.as_ref().map(AccountId::as_str) == Some(other))
            },
        )
        .await?;
        wait_for(
            "the removed subscription's credentials gone",
            Duration::from_secs(10),
            || async { !home.join("auth.json").exists() },
        )
        .await?;
        support::stop_tasks(&app, &[&id]).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Marks `account` limited until `resets_at` (100 % of its 5-hour window).
async fn limit_until(
    app: &App,
    account: &AccountId,
    resets_at: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    let full = yulius_domain::usage::AccountUsage {
        primary: Some(yulius_domain::usage::UsageWindow {
            used_percent: Some(100.0),
            window_minutes: Some(300),
            resets_at: Some(Timestamp(resets_at)),
        }),
        limit_reached: true,
        limit_resets_at: Some(Timestamp(resets_at)),
        ..yulius_domain::usage::AccountUsage::default()
    };
    app.deps
        .store
        .set_account_state(
            account,
            AccountStatus::Limited,
            Some(&yulius_app::usage_json::to_json(&full)),
            None,
            app.now(),
        )
        .await?;
    Ok(())
}

/// SPEC-451 CDX-010: the capacity view (the Dashboard's Subscriptions card) names each
/// subscription's provider, and with nothing available it resumes when the first subscription
/// of any provider does, a Codex one included.
#[test]
fn the_capacity_view_names_providers_and_resumes_with_codex_too() -> TestResult {
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, None, FakeAgentOptions::default(), 8)?;
        let (_sandbox, user) = seed_records(&app).await?;
        let ctx = admin_ctx(&app, &user).await?;
        let codex = add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        let soon = app.now().0 + 3_600_000;
        limit_until(&app, &AccountId::parse("a-claude")?, soon + 3_600_000).await?;
        limit_until(&app, &codex, soon).await?;
        let capacity = app.capacity_view(&ctx).await?;
        let provider = |id: &str| {
            capacity
                .accounts
                .iter()
                .find(|a| a.id.as_deref() == Some(id))
                .map(|a| a.provider.clone())
        };
        assert_eq!(provider("a-claude").as_deref(), Some("anthropic"));
        assert_eq!(provider("a-codex").as_deref(), Some("openai"));
        assert_eq!(
            capacity.resumes_at.as_deref(),
            Some(yulius_app::auth::rfc3339(Timestamp(soon)).as_str()),
            "{capacity:?}"
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-451 CDX-006: when the account is gone by the run's end (removed or
/// signed out meanwhile), its credentials do not stay in the sandbox for later tasks to read.
#[test]
fn a_gone_accounts_codex_credentials_leave_the_sandbox() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let account =
            add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_codex_task(&app, &sandbox, &user, "Codex gone").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let home = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("codex")
            .join(account.as_str());
        // Codex refreshed meanwhile, so the file differs from what the vault held.
        std::fs::write(home.join("auth.json"), codex_auth(&ago(1_000)))?;
        app.deps.store.delete_account(&account).await?;
        app.send_input(&id, b"/exit\r").await?;
        wait_for("the run's end", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        wait_for(
            "the credentials removed",
            Duration::from_secs(10),
            || async { !home.join("auth.json").exists() },
        )
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
