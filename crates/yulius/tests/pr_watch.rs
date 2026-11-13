//! SPEC-444: a live task with an open pull request and no `yulius wait` running is told, by one
//! watch pass each, of a red run at its head (jobs, failing tests, where to read the log), of a
//! conflict with the integration branch and of its merge, each once and with its next step. Real
//! repositories, the production Git adapter and a fake agent that echoes what it is typed.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use yulius_app::ci::{CiJobRecord, CiRunRecord};
use yulius_app::delivery::DeliveryRecord;
use yulius_app::pr_watch::WatchBounds;
use yulius_domain::ci::{CiRef, GateSource, RunState};
use yulius_domain::delivery::DeliveryStage;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::resources::Need;
use yulius_domain::task::Activity;

use crate::pool_supervision::{build_tuned, new_task, runtime, screen, seed, stop, temp, wait_for};

type TestResult = Result<(), Box<dyn std::error::Error>>;

pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned().into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// A run of pull request 7 at `head` that ended in `state`, with one job.
pub(crate) fn run(
    sandbox: &SandboxId,
    id: &str,
    head: &str,
    state: RunState,
    failures: Option<&str>,
    queued: i64,
) -> (CiRunRecord, CiJobRecord) {
    let at = yulius_domain::time::Timestamp(1);
    let run = CiRunRecord {
        id: id.to_owned(),
        sandbox_id: sandbox.clone(),
        reference: CiRef::PullRequest(7),
        sha: head.to_owned(),
        source: GateSource::ProjectConfig,
        state,
        reason: None,
        requested_by: "yulius".into(),
        status_for: None,
        status_note: None,
        status_attempts: 0,
        queued_at: yulius_domain::time::Timestamp(queued),
        started_at: Some(at),
        ended_at: Some(at),
    };
    let job = CiJobRecord {
        id: format!("{id}-quality"),
        run_id: id.to_owned(),
        ordinal: 0,
        name: "quality".into(),
        key: "quality".into(),
        command: "scripts/dev ci".into(),
        matrix: None,
        state,
        reason: None,
        exit_code: Some(if state == RunState::Failed { 100 } else { 0 }),
        output: String::new(),
        failures: failures.map(str::to_owned),
        started_at: Some(at),
        ended_at: Some(at),
        need: Need::default(),
        timeout_minutes: None,
        exclusive: false,
        waited_ms: None,
    };
    (run, job)
}

/// PRN-001, PRN-002, PRN-004, PRN-005, PRN-007.
#[test]
fn a_task_with_no_wait_is_told_each_event_once() -> TestResult {
    use yulius_ptyhost::test_support::locate_fake_agent;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let agent = crate::support::agents::FakeAgentOptions {
        transcript: true,
        answers_first_message: true,
        ..crate::support::agents::FakeAgentOptions::default()
    };
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_tuned(&data, &fake, agent, |config| config.goal_ready_ms = 2_000)?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        // The integration branch, `dev`, on the origin and in Yulius's clone.
        let seed_dir = data.join("seed");
        git(&seed_dir, &["push", "-q", "origin", "main:dev"])?;
        let repo = data.join("sandboxes").join(sandbox.as_str()).join("repo");
        git(&repo, &["fetch", "-q", "origin"])?;
        let id = new_task(&app, &sandbox, &user, "Watched").await?;
        wait_for("the first answer", Duration::from_secs(30), || {
            let shown = screen(&app, &id);
            async move { shown.contains("ECHO: Watched: answer with an echo") }
        })
        .await?;
        let task = app.deps.store.task(&id).await?.ok_or("task")?;
        let worktree = task.worktree_path.clone().ok_or("no worktree")?;
        let worktree = Path::new(&worktree);
        std::fs::write(worktree.join("README.md"), "# the task's line\n")?;
        git(worktree, &["commit", "-q", "-am", "task"])?;
        let head = git(worktree, &["rev-parse", "HEAD"])?;
        let open = DeliveryRecord {
            task_id: id.clone(),
            stage: DeliveryStage::PrOpen,
            pr_number: Some(7),
            updated_at: app.now(),
        };
        app.deps
            .store
            .transition_delivery(&DeliveryStage::None, &open)
            .await?;
        let bounds = WatchBounds::default();
        // Before any run: nothing to say.
        assert!(watch(&app, &bounds, &id).await?.is_none());

        // A red run at the head, with no wait running: told within one pass.
        let failures = "FAIL [   0.2s] (3/9) app tests::bad\n  stderr ───\n    boom\n\n";
        let (red, job) = run(&sandbox, "r1", &head, RunState::Failed, Some(failures), 1);
        app.deps
            .store
            .insert_ci_run(&red, std::slice::from_ref(&job))
            .await?;
        // A job's failing tests are recorded with its end, as a real run's are.
        let ended = yulius_app::ci::CiJobUpdate {
            exit_code: Some(100),
            failures: job.failures.clone(),
            ..yulius_app::ci::CiJobUpdate::default()
        };
        app.deps
            .store
            .transition_ci_job(&job.id, &[RunState::Failed], RunState::Failed, &ended)
            .await?;
        let typed = typed_within(&app, &bounds, &id)
            .await?
            .ok_or("the failure was not typed")?;
        assert!(
            typed.starts_with("Yulius, about your pull request #7: Its Yulius CI run r1 at ")
                && typed.contains("failed: `quality` exited 100 (failing tests: app tests::bad)")
                && typed.contains("`ci_status` with run_id r1 and job `quality`")
                && typed.contains("fix it test-first"),
            "{typed}"
        );
        echoed(
            &app,
            &id,
            "ECHO: Yulius, about your pull request #7: Its Yulius CI run r1",
        )
        .await?;
        assert!(watch(&app, &bounds, &id).await?.is_none(), "told once");

        // `dev` moves in a way that conflicts, and the head's next run passed.
        std::fs::write(seed_dir.join("README.md"), "# dev's line\n")?;
        git(&seed_dir, &["commit", "-q", "-am", "dev"])?;
        git(&seed_dir, &["push", "-q", "origin", "HEAD:dev"])?;
        git(&repo, &["fetch", "-q", "origin"])?;
        let (green, job) = run(&sandbox, "r2", &head, RunState::Passed, None, 2);
        app.deps.store.insert_ci_run(&green, &[job]).await?;
        // A permission prompt or question waits for a person: nothing is typed over it, though
        // the session takes input (it was just typed into).
        let activity = app.deps.store.task(&id).await?.ok_or("task")?.activity;
        app.deps
            .store
            .set_activity(&id, Some(Activity::NeedsAttention), app.now())
            .await?;
        assert!(
            watch(&app, &bounds, &id).await?.is_none(),
            "a person must act first"
        );
        app.deps
            .store
            .set_activity(&id, activity, app.now())
            .await?;
        let typed = typed_within(&app, &bounds, &id)
            .await?
            .ok_or("the conflict was not typed")?;
        assert!(
            typed.contains(&format!(
                "Its head {} conflicts with `dev` as Yulius last fetched it",
                &head[..9]
            )) && typed.contains("git merge origin/dev")
                && typed.contains("Never rebase or force-push"),
            "{typed}"
        );
        assert!(
            typed.contains("passed its own Yulius CI run, but no merge is armed"),
            "the green head says why it is not merging: {typed}"
        );
        echoed(&app, &id, "Never rebase or force-push").await?;
        assert!(watch(&app, &bounds, &id).await?.is_none(), "told once");

        // Nothing moves while CI is idle: stalled (PRN-009), told once and shown with the task.
        let stalling = WatchBounds {
            stall_after_ms: 0,
            stall_quiet_ms: 0,
            ..WatchBounds::default()
        };
        assert_eq!(app.pr_stalled(&id), None);
        let typed = typed_within(&app, &stalling, &id)
            .await?
            .ok_or("the stall was not typed")?;
        assert!(
            typed.starts_with(
                "Yulius, about your pull request #7: It has made no progress for 0 min"
            ),
            "{typed}"
        );
        let task = app.deps.store.task(&id).await?.ok_or("task")?;
        let shown = app
            .task_summary(&task)
            .await?
            .pr_stalled
            .unwrap_or_default();
        assert!(
            shown.starts_with("Pull request #7 has made no progress for 0 min: Its head"),
            "{shown}"
        );
        echoed(&app, &id, "It has made no progress").await?;
        assert!(watch(&app, &stalling, &id).await?.is_none(), "told once");

        // Merged (by anyone): said once, then the watch is over.
        let merged = DeliveryRecord {
            stage: DeliveryStage::MergedIntegration,
            updated_at: app.now(),
            ..open
        };
        app.deps
            .store
            .transition_delivery(&DeliveryStage::PrOpen, &merged)
            .await?;
        // Not over a prompt that waits for a person either (review round 2).
        app.deps
            .store
            .set_activity(&id, Some(Activity::NeedsAttention), app.now())
            .await?;
        assert!(
            watch(&app, &bounds, &id).await?.is_none(),
            "a person must act first"
        );
        assert_eq!(
            app.pr_stalled(&id),
            None,
            "a merged pull request is no stall"
        );
        app.deps
            .store
            .set_activity(&id, activity, app.now())
            .await?;
        let typed = typed_within(&app, &bounds, &id)
            .await?
            .ok_or("the merge was not typed")?;
        assert!(
            typed.starts_with(
                "Yulius, about your pull request #7: Pull request #7 is merged into `dev`"
            ),
            "{typed}"
        );
        echoed(
            &app,
            &id,
            "ECHO: Yulius, about your pull request #7: Pull request #7 is merged",
        )
        .await?;
        assert!(watch(&app, &bounds, &id).await?.is_none(), "told once");
        assert_eq!(
            app.pr_stalled(&id),
            None,
            "a merged pull request is no stall"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// One watch pass: what it typed into `id`'s session.
async fn watch(
    app: &std::sync::Arc<yulius_app::context::App>,
    bounds: &WatchBounds,
    id: &TaskId,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let typed = app.watch_prs(bounds).await;
    Ok(typed
        .into_iter()
        .find(|(task, _)| task == id)
        .map(|(_, text)| text))
}

/// Watch passes until one types into `id`'s session (it takes input a second after its spawn),
/// within 30 seconds.
async fn typed_within(
    app: &std::sync::Arc<yulius_app::context::App>,
    bounds: &WatchBounds,
    id: &TaskId,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if let Some(typed) = watch(app, bounds, id).await? {
            return Ok(Some(typed));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Ok(None)
}

/// Waits until the session echoed `line`.
async fn echoed(
    app: &std::sync::Arc<yulius_app::context::App>,
    id: &TaskId,
    line: &str,
) -> TestResult {
    wait_for("the echo", Duration::from_secs(30), || {
        let shown = screen(app, id);
        async move { shown.contains(line) }
    })
    .await
}
