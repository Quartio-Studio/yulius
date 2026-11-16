//! SPEC-493 SCO-001 to SCO-003: a claim that overlaps a live task's claim is told to both. The
//! claiming agent's answer names the holding task, what it changes and where its pull request
//! stands; the holding task's session gets one message naming the claiming task and the paths.
//! A fake agent echoes what it is typed.

use std::time::Duration;

use serde_json::json;
use yulius_app::coordination::AgentContext;

use crate::pool_supervision::{build_tuned, new_task, runtime, screen, seed, stop, temp, wait_for};

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
            wait_for("the first answer", Duration::from_secs(30), || {
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
        wait_for("the holder told", Duration::from_secs(30), || {
            let shown = screen(&app, &a);
            async move { shown.contains(told) }
        })
        .await?;
        // The same claim again types nothing more.
        app.tool_scope_claim(&claimer, &json!({"pattern": "crates/a/**"}))
            .await?;
        // A second message would be typed at once (the check that stops it runs before any
        // typing): none comes within a bounded look.
        // Each message shows once as its paste.
        let echoed = "200~Yulius: task #";
        assert_eq!(
            screen(&app, &a).matches(echoed).count(),
            1,
            "{}",
            screen(&app, &a)
        );
        let again = wait_for("a second message", Duration::from_secs(5), || {
            let shown = screen(&app, &a);
            async move { shown.matches(echoed).count() > 1 }
        })
        .await;
        assert!(again.is_err(), "told twice: {}", screen(&app, &a));
        stop(&app, &a).await?;
        stop(&app, &b).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
