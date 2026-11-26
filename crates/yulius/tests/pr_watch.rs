//! SPEC-444: a live task with an open pull request and no `yulius wait` running is told, by one
//! watch pass each, of a red run at its head (jobs, failing tests, where to read the log), of a
//! conflict with the integration branch and of its merge, each once and with its next step. Real
//! repositories, the production Git adapter and a fake agent that echoes what it is typed.

use crate::support::{eventually, git};
use std::path::Path;
use std::time::Duration;

use yulius_app::ci::{CiJobRecord, CiRunRecord};
use yulius_app::delivery::DeliveryRecord;
use yulius_app::pr_watch::WatchBounds;
use yulius_domain::ci::{CiRef, GateSource, RunState};
use yulius_domain::delivery::DeliveryStage;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::resources::Need;
use yulius_domain::task::Activity;

use crate::pool_supervision::{build_tuned, new_task, runtime, screen, seed, stop, temp};

type TestResult = Result<(), Box<dyn std::error::Error>>;

pub(crate) async fn leave_verification_step(
    app: &yulius_app::context::App,
    task: &TaskId,
) -> TestResult {
    app.deps
        .store
        .replace_steps(
            task,
            &[yulius_app::model::StepRecord {
                id: yulius_domain::ids::StepId::parse(&format!("verify-{task}"))?,
                task_id: task.clone(),
                ordinal: 1,
                title: "Verify delivery".to_owned(),
                status: "in_progress".to_owned(),
                note: None,
                updated_by: None,
                updated_at: None,
                effort: 1,
            }],
            "person",
            app.now(),
        )
        .await?;
    Ok(())
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
        eventually("the first answer", Duration::from_secs(30), || {
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
        let failures = "FAIL [   0.2s] (3/9) phone::resume_signout\n  stderr ───\n    boom\n\n";
        let (red, job) = run(&sandbox, "r1", &head, RunState::Failed, Some(failures), 1);
        let sibling = CiJobRecord {
            id: "r1-onboarding".into(),
            name: "onboarding".into(),
            key: "onboarding".into(),
            ordinal: 1,
            state: RunState::Running,
            exit_code: None,
            failures: None,
            ..job.clone()
        };
        app.deps
            .store
            .insert_ci_run(&red, &[job.clone(), sibling.clone()])
            .await?;
        // The runner persists both sets of blocks when each job ends.
        let ended = yulius_app::ci::CiJobUpdate {
            exit_code: Some(100),
            failures: job.failures.clone(),
            ..yulius_app::ci::CiJobUpdate::default()
        };
        app.deps
            .store
            .transition_ci_job(&job.id, &[RunState::Failed], RunState::Failed, &ended)
            .await?;
        let stopped = yulius_app::ci::CiJobUpdate {
            reason: Some("stopped early: job quality failed".into()),
            failures: Some("---- onboarding::checkbox stdout ----\nassertion detail\n\n".into()),
            ..yulius_app::ci::CiJobUpdate::default()
        };
        app.deps
            .store
            .transition_ci_job(
                &sibling.id,
                &[RunState::Running],
                RunState::Cancelled,
                &stopped,
            )
            .await?;
        let typed = typed_within(&app, &bounds, &id)
            .await?
            .ok_or("the failure was not typed")?;
        assert!(
            typed.starts_with("Yulius, about your pull request #7: Its Yulius CI run r1 at ")
                && typed.contains(
                    "failed: `quality` exited 100 (failing tests: phone::resume_signout)"
                )
                && typed.contains(
                    "`onboarding` cancelled (partial failing tests: onboarding::checkbox)"
                )
                && typed.contains("`ci_status` with run_id r1")
                && typed.contains("Fix it test-first"),
            "{typed}"
        );
        echoed(
            &app,
            &id,
            "ECHO: Yulius, about your pull request #7: Its Yulius CI run r1",
        )
        .await?;
        assert!(watch(&app, &bounds, &id).await?.is_none(), "told once");

        // The next head's run supersedes the previously reported head and stopped its
        // sibling before it recorded any test failure.
        git(worktree, &["commit", "-q", "--allow-empty", "-m", "Retry repair"])?;
        let head = git(worktree, &["rev-parse", "HEAD"])?;
        let (ordinary, primary) = run(&sandbox, "ordinary", &head, RunState::Failed, None, 2);
        let empty = CiJobRecord {
            id: "ordinary-onboarding".into(),
            run_id: ordinary.id.clone(),
            state: RunState::Cancelled,
            reason: stopped.reason.clone(),
            ..sibling
        };
        app.deps
            .store
            .insert_ci_run(&ordinary, &[primary, empty])
            .await?;
        let notice = typed_within(&app, &bounds, &id)
            .await?
            .ok_or("missing ordinary failure")?;
        assert!(
            notice.contains("run ordinary") && notice.contains("`quality` exited 100"),
            "{notice}"
        );
        assert!(
            !notice.contains("onboarding") && !notice.contains("partial"),
            "{notice}"
        );
        echoed(&app, &id, "Its Yulius CI run ordinary").await?;
        assert!(watch(&app, &bounds, &id).await?.is_none());

        // `dev` moves in a way that conflicts, and the head's next run passed.
        std::fs::write(seed_dir.join("README.md"), "# dev's line\n")?;
        git(&seed_dir, &["commit", "-q", "-am", "dev"])?;
        git(&seed_dir, &["push", "-q", "origin", "HEAD:dev"])?;
        git(&repo, &["fetch", "-q", "origin"])?;
        let (green, job) = run(&sandbox, "r2", &head, RunState::Passed, None, 3);
        app.deps.store.insert_ci_run(&green, &[job]).await?;
        // The owner's rerun is outside the recent sandbox window. Another PR also
        // tested this SHA, so the remembered-head lookup must still select PR #7.
        for queued in 4..=104 {
            let sha = if queued == 104 { &head } else { "other-head" };
            let (mut other, job) =
                run(&sandbox, &format!("other-{queued}"), sha, RunState::Failed, None, queued);
            other.reference = CiRef::PullRequest(99);
            app.deps.store.insert_ci_run(&other, &[job]).await?;
        }
        // A permission prompt or question waits for a person: nothing is typed over it, though
        // the session takes input (it was just typed into).
        crate::parking::ask(&app, &id).await?;
        assert!(watch(&app, &bounds, &id).await?.is_none(), "a person must act first");
        app.deps.store.set_task_needs_you(&id, None, app.now()).await?;
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

        // The stored delivery says it merged. Without a closed PR read, only that fact is known.
        let merged = DeliveryRecord {
            stage: DeliveryStage::MergedIntegration,
            updated_at: app.now(),
            ..open
        };
        app.deps
            .store
            .transition_delivery(&DeliveryStage::PrOpen, &merged)
            .await?;
        assert!(app.watch_prs(&bounds).await.is_empty(), "the completed merged task stays quiet");
        assert!(!screen(&app, &id).contains("Pull request #7 is merged"));
        leave_verification_step(&app, &id).await?;
        // Not over a prompt that waits for a person either (review round 2).
        let notices = data.join(yulius_app::pr_watch::PR_NOTICES_FILE);
        let before: serde_json::Value = serde_json::from_slice(&std::fs::read(&notices)?)?;
        let activity = app.deps.store.task(&id).await?.ok_or("task")?.activity;
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
        let held: serde_json::Value = serde_json::from_slice(&std::fs::read(&notices)?)?;
        assert_eq!(held[id.as_str()]["progress"], before[id.as_str()]["progress"]);
        assert_eq!(held[id.as_str()]["progress_ms"], before[id.as_str()]["progress_ms"]);
        app.deps
            .store
            .set_activity(&id, activity, app.now())
            .await?;
        let typed = typed_within(&app, &bounds, &id)
            .await?
            .ok_or("the merge was not typed")?;
        let heard: serde_json::Value = serde_json::from_slice(&std::fs::read(&notices)?)?;
        let typed_at = heard[id.as_str()]["typed_ms"].as_i64().ok_or("merge notice time")?;
        assert!((merged.updated_at.0..=app.now().0).contains(&typed_at));
        assert_eq!(heard[id.as_str()]["progress_ms"], typed_at);
        assert!(
            typed.starts_with(
                "Yulius, about your pull request #7: Pull request #7 is merged. Yulius could not read its merged head"
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

/// SPEC-491 DON-003: a pull request that conflicts with the integration branch is not tested,
/// so its task is not Done, and its agent hears of the conflict also where no run exists at its
/// head (a project without Yulius CI): the branch's pushed head, the pull request's, is asked.
/// Told once per head.
#[test]
fn a_conflict_is_told_without_a_run_at_the_head() -> TestResult {
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
        let seed_dir = data.join("seed");
        git(&seed_dir, &["push", "-q", "origin", "main:dev"])?;
        let repo = data.join("sandboxes").join(sandbox.as_str()).join("repo");
        git(&repo, &["fetch", "-q", "origin"])?;
        let id = new_task(&app, &sandbox, &user, "Watched").await?;
        eventually("the first answer", Duration::from_secs(30), || {
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
        // Pushed: the pull request's head.
        let branch = task.branch.clone().ok_or("no branch")?;
        git(
            worktree,
            &["push", "-q", "origin", &format!("HEAD:{branch}")],
        )?;
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
        assert!(
            watch(&app, &bounds, &id).await?.is_none(),
            "no run and no conflict: nothing to say"
        );

        // `dev` moves in a way that conflicts; no run ever comes.
        std::fs::write(seed_dir.join("README.md"), "# dev's line\n")?;
        git(&seed_dir, &["commit", "-q", "-am", "dev"])?;
        git(&seed_dir, &["push", "-q", "origin", "HEAD:dev"])?;
        git(&repo, &["fetch", "-q", "origin"])?;
        let typed = typed_within(&app, &bounds, &id)
            .await?
            .ok_or("the conflict was not typed")?;
        assert!(
            typed.contains(&format!(
                "Its head {} conflicts with `dev` as Yulius last fetched it",
                &head[..9]
            )) && typed.contains("git merge origin/dev"),
            "{typed}"
        );
        echoed(&app, &id, "Never rebase or force-push").await?;
        assert!(watch(&app, &bounds, &id).await?.is_none(), "told once");
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-553 REP-002, REP-004: with a real live agent PTY and FakeGitHub:
/// 1. An actionable failed publication (`DirtyWorktree`) is delivered once by `watch_prs` and not
///    repeated on subsequent passes.
/// 2. Resolving a newer `DirtyWorktree` failure by committing cleanly and pushing `repair2` never
///    emits a stale `"publish"` notice for the superseded commit.
/// 3. After `repair2` is pushed past a failing `pr_head` with no new CI run yet, repeated
///    `watch_prs` passes (Pass 1 and Pass 2) never emit a stale failed-run notice for `pr_head`,
///    while a fresh failure at `repair2` itself IS delivered.
#[test]
fn failed_publication_and_superseded_repair_notices_in_pr_watch() -> TestResult {
    use std::sync::Arc;
    use yulius_app::model::{GitHubTokenRecord, Secret};
    use yulius_app::ports::github::{ChecksState, GitHubPort, NewPull};
    use yulius_domain::time::Timestamp;
    use yulius_ptyhost::test_support::locate_fake_agent;

    struct BareOrigin(std::path::PathBuf);
    impl crate::support::FakeRemote for BareOrigin {
        fn branch_sha(&self, branch: &str) -> Option<String> {
            git(
                &self.0,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{branch}"),
                ],
            )
            .ok()
        }
        fn merge(&self, _: &str, _: &str, _: &str, _: &str) -> Result<String, String> {
            Err("merge not used in this test".into())
        }
        fn fast_forward(&self, _: &str, _: &str) -> Result<(), String> {
            Err("fast_forward not used in this test".into())
        }
    }

    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let github = Arc::new(crate::support::FakeGitHub::default());
    github.set_remote(Arc::new(BareOrigin(data.join("origin.git"))));
    github.add_user("code-admin", 1, "admin");
    github.set_perm("admin", "acme/pool", crate::support::perms("write"));
    let agent = crate::support::agents::FakeAgentOptions {
        transcript: true,
        answers_first_message: true,
        ..crate::support::agents::FakeAgentOptions::default()
    };
    let rt = runtime()?;
    rt.block_on(async {
        let app = crate::pool_supervision::build_served_tuned(
            &data,
            &fake,
            Arc::new(crate::pool_supervision::ScriptedUsage::default()),
            agent,
            github.clone(),
            "http://127.0.0.1:9",
            |config| config.goal_ready_ms = 2_000,
        )?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        yulius_app::ports::store::PermissionStore::put_access(
            app.deps.store.as_ref(),
            &user,
            &sandbox,
            yulius_domain::permission::AccessLevel::Write,
            app.now(),
        )
        .await?;
        let profile_json = serde_json::to_string(&yulius_app::delivery::profile_to_contract(
            &yulius_domain::delivery::DeliveryProfile::standard(),
        ))?;
        app.deps.store.set_profile(&sandbox, &profile_json).await?;
        let access = app.deps.crypto.seal(b"token-admin")?;
        app.deps
            .store
            .save_github_token(
                &GitHubTokenRecord {
                    user_id: user.clone(),
                    access,
                    refresh: None,
                    expires_at: None,
                    refresh_expires_at: None,
                },
                Timestamp(1),
            )
            .await?;
        let seed_dir = data.join("seed");
        git(&seed_dir, &["push", "-q", "origin", "main:dev"])?;
        let repo = data.join("sandboxes").join(sandbox.as_str()).join("repo");
        git(&repo, &["fetch", "-q", "origin"])?;
        let id = new_task(&app, &sandbox, &user, "Watched repair").await?;
        eventually("the first answer", Duration::from_secs(30), || {
            let shown = screen(&app, &id);
            async move { shown.contains("ECHO: Watched repair: answer with an echo") }
        })
        .await?;
        let task = app.deps.store.task(&id).await?.ok_or("task")?;
        let branch = task.branch.clone().ok_or("branch")?;
        let worktree = Path::new(task.worktree_path.as_deref().ok_or("worktree")?).to_path_buf();
        let spec_dir = worktree.join("docs/specs");
        std::fs::create_dir_all(&spec_dir)?;
        std::fs::write(
            spec_dir.join("2026-10-07-repair.md"),
            format!(
                "---\nyulius-task: {}\nstatus: in-progress\n---\n\n# Repair\n\n## Goal\nFix CI.\n\n## Context\nOpen PR.\n\n## Requirements\n- R1\n\n## Plan\n- [x] Fix\n- [ ] Verify\n\n## Acceptance criteria\n- Green\n\n## Test plan\n- Unit\n\n## Decisions\n- None.\n",
                id.as_str()
            ),
        )?;
        git(&worktree, &["add", "docs/specs/2026-10-07-repair.md"])?;
        git(&worktree, &["commit", "-q", "-m", "Initial spec"])?;
        git(&worktree, &["push", "-q", "origin", &branch])?;
        let pr_head = git(&worktree, &["rev-parse", "HEAD"])?;
        let pull = github
            .create_pull(
                &Secret("token-admin".into()),
                "acme",
                "pool",
                &NewPull {
                    title: "feat: repair".into(),
                    body: String::new(),
                    head: branch.clone(),
                    base: "dev".into(),
                },
            )
            .await?;
        let open = DeliveryRecord {
            task_id: id.clone(),
            stage: DeliveryStage::PrOpen,
            pr_number: Some(pull.number),
            updated_at: app.now(),
        };
        app.deps
            .store
            .transition_delivery(&DeliveryStage::None, &open)
            .await?;
        github.set_checks(&pr_head, ChecksState::Failure);
        let steps = [("Fix", "done"), ("Verify", "in_progress")]
            .into_iter()
            .enumerate()
            .map(|(i, (title, status))| -> Result<_, Box<dyn std::error::Error>> {
                Ok(yulius_app::model::StepRecord {
                    id: yulius_domain::ids::StepId::parse(&format!("s-pw-{i}"))?,
                    task_id: id.clone(),
                    ordinal: u32::try_from(i)?,
                    title: title.to_owned(),
                    status: status.to_owned(),
                    note: None,
                    updated_by: None,
                    updated_at: None,
                    effort: 1,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        app.deps
            .store
            .replace_steps(&id, &steps, "agent", app.now())
            .await?;
        let bounds = WatchBounds::default();

        // 1. Dirty worktree on `repair1`: `auto_open_pr` records actionable `push_problem`, and
        // `watch_prs` delivers it into the real PTY once (never duplicated on pass 2).
        std::fs::write(worktree.join("app.txt"), "repair1\n")?;
        git(&worktree, &["add", "app.txt"])?;
        git(&worktree, &["commit", "-q", "-m", "Repair 1"])?;
        std::fs::write(worktree.join("dirty.tmp"), "uncommitted\n")?;
        app.auto_open_pr(&id).await;
        let owner_ctx = yulius_app::auth::AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let dlv_view = app.task_delivery(&owner_ctx, id.as_str()).await?;
        assert!(
            dlv_view
                .notes
                .iter()
                .any(|n| n.contains("uncommitted changes")),
            "expected dirty-worktree push_problem after auto_open_pr: {dlv_view:?}"
        );
        let typed = typed_within(&app, &bounds, &id)
            .await?
            .ok_or("publish failure notice was not typed by watch_prs")?;
        assert!(
            typed.contains("uncommitted changes"),
            "expected dirty-worktree publish notice: {typed}"
        );
        echoed(&app, &id, "uncommitted changes").await?;
        assert!(
            watch(&app, &bounds, &id).await?.is_none(),
            "publish notice must be told once"
        );

        // 2. Record a brand-new `DirtyWorktree` failure on `repair1b` (never delivered to
        // `pr_watch`), then resolve it cleanly by committing `dirty.tmp` in `repair2` and pushing
        // `repair2` past failing `pr_head` (which also has an untold `RunState::Failed` run).
        std::fs::remove_file(worktree.join("dirty.tmp"))?;
        std::fs::write(worktree.join("app.txt"), "repair1b\n")?;
        git(&worktree, &["add", "app.txt"])?;
        git(&worktree, &["commit", "-q", "-m", "Repair 1b"])?;
        std::fs::write(worktree.join("dirty2.tmp"), "new dirty file\n")?;
        app.auto_open_pr(&id).await;
        // Now resolve the dirty worktree with a clean commit `repair2` and publish it.
        git(&worktree, &["add", "dirty2.tmp"])?;
        git(&worktree, &["commit", "-q", "-m", "Repair 2 clean"])?;
        let repair2 = git(&worktree, &["rev-parse", "HEAD"])?;
        app.auto_open_pr(&id).await;
        let origin_head = git(
            &data.join("origin.git"),
            &["rev-parse", &format!("refs/heads/{branch}")],
        )?;
        assert_eq!(origin_head, repair2, "repair2 published cleanly");

        // Record an untold failed CI run at the superseded `pr_head` with NO CI run yet for
        // `repair2`: BOTH Pass 1 and Pass 2 of `watch_prs` must emit neither a stale `"publish"`
        // notice for `repair1b` nor a stale `"run"` failure notice for `pr_head`.
        let (old_red, old_job) = run(
            &sandbox,
            "r-old",
            &pr_head,
            RunState::Failed,
            Some("FAIL old\n"),
            10,
        );
        let old_red = CiRunRecord {
            reference: CiRef::PullRequest(pull.number),
            ..old_red
        };
        app.deps
            .store
            .insert_ci_run(&old_red, std::slice::from_ref(&old_job))
            .await?;
        assert!(
            watch(&app, &bounds, &id).await?.is_none(),
            "pass 1 must emit neither stale publish nor stale failed-run notice"
        );
        assert!(
            watch(&app, &bounds, &id).await?.is_none(),
            "pass 2 must also emit neither stale publish nor stale failed-run notice"
        );

        // 3. Negative control: a real failure at `repair2` itself IS delivered into the PTY.
        let (new_red, new_job) = run(
            &sandbox,
            "r-new",
            &repair2,
            RunState::Failed,
            Some("FAIL new\n"),
            20,
        );
        let new_red = CiRunRecord {
            reference: CiRef::PullRequest(pull.number),
            ..new_red
        };
        app.deps
            .store
            .insert_ci_run(&new_red, std::slice::from_ref(&new_job))
            .await?;
        let red_typed = typed_within(&app, &bounds, &id)
            .await?
            .ok_or("failed run at current repair2 head was not typed")?;
        assert!(
            red_typed.contains("failed"),
            "expected current-head failure notice: {red_typed}"
        );

        // A later pass at repair2 is green. The delivery is recorded as merged before
        // GitHub's PR read catches up. The watcher still knows the plan for the next slice.
        let (green, job) = run(&sandbox, "r-green", &repair2, RunState::Passed, None, 30);
        let green = CiRunRecord { reference: CiRef::PullRequest(pull.number), ..green };
        app.deps.store.insert_ci_run(&green, &[job]).await?;
        assert!(!github.pull(pull.number).ok_or("open PR")?.merged);
        let merged = DeliveryRecord { stage: DeliveryStage::MergedIntegration, ..open };
        app.deps.store.transition_delivery(&DeliveryStage::PrOpen, &merged).await?;
        let notice = typed_within(&app, &bounds, &id).await?.ok_or("merge notice")?;
        assert!(notice.contains("could not read its merged head"), "{notice}");
        assert!(!notice.contains("into `dev`") && !notice.contains(&repair2[..9]), "{notice}");
        assert!(notice.contains("1 step(s) of your plan are still ahead (next: \"Verify\")"), "{notice}");

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
    eventually("the echo", Duration::from_secs(30), || {
        let shown = screen(app, id);
        async move { shown.contains(line) }
    })
    .await
}
