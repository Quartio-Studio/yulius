//! SPEC-493 SCO-001 to SCO-003: a claim that overlaps a live task's claim is told to both. The
//! claiming agent's answer names the holding task, what it changes and where its pull request
//! stands; the holding task's session gets one message naming the claiming task and the paths.
//! A fake agent echoes what it is typed.

use std::time::Duration;

use serde_json::json;
use yulius_app::coordination::AgentContext;

use crate::support::eventually;
use crate::support::live::pool::{build_tuned, new_task, runtime, screen, seed, stop, temp};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The agent context of `task`'s live run.
async fn agent(
    app: &yulius_app::context::App,
    task: &yulius_domain::ids::TaskId,
) -> Result<AgentContext, Box<dyn std::error::Error>> {
    Ok(AgentContext {
        attempt: app
            .deps
            .store
            .live_attempt(task)
            .await?
            .ok_or("a live run")?,
        task: app.deps.store.task(task).await?.ok_or("the task")?,
    })
}

/// AC-21 (SCO-001): B claims `crates/a/**` while live task A holds `crates/a/src/x.rs`. B's
/// answer names A, its title, its note, its step in progress and its pull request (none yet);
/// A's session gets one message naming B and the paths, and a second claim of the same paths
/// types nothing again.
#[test]
fn an_overlapping_claim_is_told_to_both_tasks() -> TestResult {
    use yulius_ptyhost::test_support::locate_fake_agent;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let agent_options = crate::support::agents::FakeAgentOptions {
        transcript: true,
        answers_first_message: true,
        ..crate::support::agents::FakeAgentOptions::default()
    };
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_tuned(&data, &fake, agent_options, |config| {
            config.goal_ready_ms = 2_000;
        })?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        let a = new_task(&app, &sandbox, &user, "Refactor x").await?;
        let b = new_task(&app, &sandbox, &user, "Feature y").await?;
        for (id, title) in [(&a, "Refactor x"), (&b, "Feature y")] {
            eventually("the first answer", Duration::from_secs(30), || {
                let shown = screen(&app, id);
                let want = format!("ECHO: {title}: answer with an echo");
                async move { shown.contains(&want) }
            })
            .await?;
        }
        let holder = agent(&app, &a).await?;
        app.tool_steps_set(
            &holder,
            &json!({"steps": [{"title": "Split x into two modules", "status": "in_progress"}]}),
        )
        .await?;
        app.tool_scope_claim(
            &holder,
            &json!({"pattern": "crates/a/src/x.rs", "note": "splitting x"}),
        )
        .await?;
        let claimer = agent(&app, &b).await?;
        let answer = app
            .tool_scope_claim(&claimer, &json!({"pattern": "crates/a/**"}))
            .await?;
        let overlap = &answer["conflicts"][0];
        let number = app.deps.store.task(&a).await?.ok_or("a")?.number;
        assert_eq!(overlap["task_number"], number, "{answer}");
        assert_eq!(overlap["task_title"], "Refactor x");
        assert_eq!(overlap["their_note"], "splitting x");
        assert_eq!(overlap["their_step"], "Split x into two modules");
        assert!(overlap["their_pull_request"].is_null(), "{answer}");
        assert!(
            answer["advice"]
                .as_str()
                .is_some_and(|a| a.contains("that task is told too")),
            "{answer}"
        );
        let told =
            "claimed paths that overlap your claims: `crates/a/**` (yours: `crates/a/src/x.rs`)";
        eventually("the holder told", Duration::from_secs(30), || {
            let shown = screen(&app, &a);
            async move { shown.contains(told) }
        })
        .await?;
        // The same claim again types nothing more.
        app.tool_scope_claim(&claimer, &json!({"pattern": "crates/a/**"}))
            .await?;
        // A second message would be typed at once (the check that stops it runs before any
        // typing; a refused paste is tried again 10 s later): none comes within a bounded look.
        // Each message shows once as its paste.
        let echoed = "200~Yulius: task #";
        assert_eq!(
            screen(&app, &a).matches(echoed).count(),
            1,
            "{}",
            screen(&app, &a)
        );
        let again = eventually("a second message", Duration::from_secs(25), || {
            let shown = screen(&app, &a);
            async move { shown.matches(echoed).count() > 1 }
        })
        .await;
        assert!(again.is_err(), "told twice: {}", screen(&app, &a));
        // Review round 2: once A has a pull request, the answer says its stage and the state of
        // Yulius CI's newest run of it.
        let open = yulius_app::delivery::DeliveryRecord {
            task_id: a.clone(),
            stage: yulius_domain::delivery::DeliveryStage::PrOpen,
            pr_number: Some(41),
            updated_at: app.now(),
        };
        app.deps
            .store
            .transition_delivery(&yulius_domain::delivery::DeliveryStage::None, &open)
            .await?;
        let at = yulius_domain::time::Timestamp(1);
        let passed = yulius_app::ci::CiRunRecord {
            id: "r41".into(),
            sandbox_id: sandbox.clone(),
            reference: yulius_domain::ci::CiRef::PullRequest(41),
            sha: "0".repeat(40),
            source: yulius_domain::ci::GateSource::ProjectConfig,
            state: yulius_domain::ci::RunState::Passed,
            reason: None,
            requested_by: "yulius".into(),
            status_for: None,
            status_note: None,
            status_attempts: 0,
            queued_at: at,
            started_at: Some(at),
            ended_at: Some(at),
        };
        app.deps.store.insert_ci_run(&passed, &[]).await?;
        let answer = app
            .tool_scope_claim(&claimer, &json!({"pattern": "crates/a/**"}))
            .await?;
        let pull_request = &answer["conflicts"][0]["their_pull_request"];
        assert_eq!(pull_request["number"], 41, "{answer}");
        assert_eq!(pull_request["stage"], "pr_open", "{answer}");
        assert_eq!(pull_request["checks"], "passed", "{answer}");
        assert_eq!(pull_request["merge_armed"], false, "{answer}");
        // SCO-002: both tasks' views (the page's and the list's) carry the overlap.
        let b_number = app.deps.store.task(&b).await?.ok_or("b")?.number;
        let view = |id: yulius_domain::ids::TaskId| {
            let app = app.clone();
            async move {
                let task = app.deps.store.task(&id).await?.ok_or("task")?;
                Ok::<_, Box<dyn std::error::Error>>(app.task_summary(&task).await?.overlaps)
            }
        };
        let seen_by_a = view(a.clone()).await?;
        assert_eq!(seen_by_a.len(), 1, "{seen_by_a:?}");
        assert_eq!(
            (seen_by_a[0].task_number, seen_by_a[0].task_title.as_str()),
            (b_number, "Feature y")
        );
        assert_eq!(seen_by_a[0].patterns, ["crates/a/src/x.rs"]);
        let seen_by_b = view(b.clone()).await?;
        assert_eq!(
            (seen_by_b.len(), seen_by_b[0].task_number),
            (1, number),
            "{seen_by_b:?}"
        );
        assert_eq!(seen_by_b[0].patterns, ["crates/a/**"]);
        // AC-22 (SCO-004): a task created with paths a live task holds is not made blind: the
        // answer names the holder and offers to run it after that task; with `after_task` it
        // is made, waiting behind it, holding its paths.
        let caller = claimer.caller();
        let refused = app
            .tool_task_create(
                &caller,
                &json!({"prompt": "Tidy x", "paths": ["crates/a/src/x.rs"]}),
            )
            .await?;
        assert_eq!(refused["created"], false, "{refused}");
        assert_eq!(refused["overlaps"][0]["task_number"], number, "{refused}");
        assert!(
            refused["offer"]
                .as_str()
                .is_some_and(|o| o.contains(&format!("`after_task: {number}`"))),
            "{refused}"
        );
        let made = app
            .tool_task_create(
                &caller,
                &json!({"prompt": "Tidy x", "paths": ["crates/a/src/x.rs"], "after_task": number}),
            )
            .await?;
        let new_number = made["task"]["number"].as_u64().ok_or("the new task")?;
        let tasks = app.deps.store.tasks(&sandbox, false).await?;
        let new = tasks
            .iter()
            .find(|t| u64::from(t.number) == new_number)
            .ok_or("the new task")?;
        assert_eq!(
            new.status,
            yulius_domain::task::TaskStatus::Chained,
            "{made}"
        );
        let claims = app.deps.store.active_claims(&sandbox).await?;
        assert!(
            claims
                .iter()
                .any(|c| c.task_id == new.id && c.pattern == "crates/a/src/x.rs"),
            "{claims:?}"
        );
        // Bad paths are refused.
        assert!(
            app.tool_task_create(&caller, &json!({"prompt": "x", "paths": ["/etc"]}))
                .await
                .is_err()
        );
        // Released: the mark is gone from both.
        app.tool_scope_release(&holder, &json!({})).await?;
        assert!(view(a.clone()).await?.is_empty());
        assert!(view(b.clone()).await?.is_empty());
        // Review round 1: claimed again after the release, the holder is told again; a claim
        // widened by a new pair tells only that pair.
        let pastes = |app: &std::sync::Arc<yulius_app::context::App>| {
            screen(app, &a).matches("200~Yulius: task #").count()
        };
        app.tool_scope_claim(&holder, &json!({"pattern": "crates/a/src/x.rs"}))
            .await?;
        app.tool_scope_claim(&claimer, &json!({"pattern": "crates/a/**"}))
            .await?;
        eventually(
            "told again after the release",
            Duration::from_secs(40),
            || {
                let seen = pastes(&app);
                async move { seen == 2 }
            },
        )
        .await?;
        app.tool_scope_claim(
            &claimer,
            &json!({"patterns": ["crates/a/**", "crates/a/src/*.rs"]}),
        )
        .await?;
        eventually("the new pair told", Duration::from_secs(40), || {
            let shown = screen(&app, &a);
            async move {
                shown.contains(
                    "overlap your claims: `crates/a/src/*.rs` (yours: `crates/a/src/x.rs`).",
                )
            }
        })
        .await?;
        assert_eq!(pastes(&app), 3, "{}", screen(&app, &a));
        // An ended holder holds nothing: its claims are no overlap for a new task (review
        // round 1); B and the waiting task still hold those paths.
        stop(&app, &a).await?;
        let answer = app
            .tool_task_create(
                &caller,
                &json!({"prompt": "Tidy x again", "paths": ["crates/a/src/x.rs"]}),
            )
            .await?;
        let holders: Vec<u64> = answer["overlaps"]
            .as_array()
            .map(|o| o.iter().filter_map(|e| e["task_number"].as_u64()).collect())
            .unwrap_or_default();
        assert!(!holders.contains(&u64::from(number)), "{answer}");
        assert!(holders.contains(&u64::from(b_number)), "{answer}");
        stop(&app, &b).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
