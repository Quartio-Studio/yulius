//! A session whose context is full (SPEC-060, issue #213) with real processes: supervisor →
//! detached `yulius ptyhost` → fake agent that answers "Prompt is too long" like Claude Code
//! 2.1.292. Yulius compacts it once, continues the task in a fresh session when compacting
//! fails, and flags the task when the fresh session is full too.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::pool_supervision::{
    ScriptedUsage, build_with, echoed, new_task, runtime, screen, seed, stop, task, temp, wait_for,
};
use crate::support::TestResult;
use crate::support::agents::FakeAgentOptions;
use tokio::sync::broadcast::Receiver;
use yulius_app::context::App;
use yulius_app::hub::HubFrame;
use yulius_app::model::LaunchKind;
use yulius_domain::ids::TaskId;
use yulius_domain::task::{Activity, AttemptState};
use yulius_ptyhost::test_support::locate_fake_agent;

/// The notices said so far.
fn notices(frames: &mut Receiver<HubFrame>, said: &mut Vec<String>) {
    while let Ok(frame) = frames.try_recv() {
        if let HubFrame::Notice(text) = frame {
            said.push(text);
        }
    }
}

/// The audit details of the task's recovery, oldest first.
fn audited(data: &Path, id: &TaskId) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let db = rusqlite::Connection::open(data.join("yulius.db"))?;
    let mut query = db.prepare(
        "SELECT detail FROM audit_log WHERE action = 'task.context_full' AND target = ?1 \
         ORDER BY id",
    )?;
    let rows = query.query_map([id.as_str()], |row| row.get::<_, Option<String>>(0))?;
    Ok(rows.filter_map(Result::ok).flatten().collect())
}

/// The fake agent's context fills after `full_after` echoed messages; `/compact` fails with
/// `compact_fails`. Every launch answers its first message.
fn agent(full_after: u32, compact_fails: bool) -> FakeAgentOptions {
    FakeAgentOptions {
        transcript: true,
        context_full_after: Some(full_after),
        compact_fails,
        answers_first_message: true,
        ..FakeAgentOptions::default()
    }
}

/// Starts a task whose first message the agent answers, and waits for that answer.
async fn started(app: &Arc<App>, data: &Path) -> Result<TaskId, Box<dyn std::error::Error>> {
    let (sandbox, user) = seed(app, data, &["a-only"]).await?;
    let id = new_task(app, &sandbox, &user, "Full").await?;
    wait_for("the first answer", Duration::from_secs(30), || {
        let app = app.clone();
        let id = id.clone();
        async move { screen(&app, &id).contains("ECHO: Full: answer with an echo") }
    })
    .await?;
    Ok(id)
}

/// CTX-002/003, the acceptance of #213: a prompt the full context refuses ends the turn (the
/// task is not left `working`), Yulius compacts the session once with a focus built from the
/// task and wakes it, and the next prompt reaches the agent.
#[test]
fn a_full_context_is_compacted_once_and_the_next_prompt_reaches_the_agent() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_with(
            &data,
            &fake,
            Arc::new(ScriptedUsage::default()),
            agent(1, false),
        )?;
        let id = started(&app, &data).await?;
        let mut frames = app.runtime(&id).hub.watch();
        app.send_input(&id, b"too much\r").await?;
        wait_for("the compaction", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("COMPACTED: Keep: task #") }
        })
        .await?;
        let shown = screen(&app, &id);
        assert!(shown.contains("PROMPT IS TOO LONG"), "{shown}");
        assert!(
            shown.contains("COMPACTED: Keep: task #1 \"Full\" and its goal: Full: answer with"),
            "the focus names the task: {shown}"
        );
        wait_for(
            "the wake-up after the compaction",
            Duration::from_secs(30),
            || {
                let app = app.clone();
                let id = id.clone();
                async move { screen(&app, &id).contains("ECHO: Yulius: your context was full") }
            },
        )
        .await?;
        app.send_input(&id, b"next\r").await?;
        wait_for("the next prompt", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { echoed(&app, &id).iter().any(|l| l == "next") }
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(attempts.len(), 1, "compacted, not restarted");
        assert_eq!(
            screen(&app, &id).matches("COMPACTED").count(),
            1,
            "compacted once"
        );
        let mut said = Vec::new();
        notices(&mut frames, &mut said);
        assert!(
            said.iter().any(|n| n.contains("context is full")),
            "the terminal says it: {said:?}"
        );
        assert!(
            said.iter()
                .any(|n| n.contains("compacted the conversation"))
        );
        assert_eq!(
            audited(&data, &id)?,
            ["context full: compacting", "compacted"],
            "the log says it"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// CTX-002: the refused prompt ends the turn: the task waits, it is not left `working`.
#[test]
fn a_refused_prompt_does_not_leave_the_task_working() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        // `/compact` fails and the fresh session is full from its first message: nothing the
        // agent does after the refusal makes it work again.
        let app = build_with(
            &data,
            &fake,
            Arc::new(ScriptedUsage::default()),
            agent(1, true),
        )?;
        let id = started(&app, &data).await?;
        app.send_input(&id, b"too much\r").await?;
        wait_for("the refusal", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("PROMPT IS TOO LONG") }
        })
        .await?;
        wait_for("the task not working", Duration::from_secs(10), || {
            let app = app.clone();
            let id = id.clone();
            async move {
                task(&app, &id)
                    .await
                    .is_ok_and(|t| t.activity != Some(Activity::Working))
            }
        })
        .await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// CTX-004: a compaction that fails continues the task in a fresh session on the same
/// subscription, opened with the handoff, which reaches the agent.
#[test]
fn a_failed_compaction_continues_the_task_in_a_fresh_session() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_with(
            &data,
            &fake,
            Arc::new(ScriptedUsage::default()),
            agent(1, true),
        )?;
        let id = started(&app, &data).await?;
        let mut frames = app.runtime(&id).hub.watch();
        app.send_input(&id, b"too much\r").await?;
        wait_for("the fresh session", Duration::from_secs(60), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2 && attempts[1].state == AttemptState::Live
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(attempts[1].launch_kind, LaunchKind::New, "a fresh session");
        assert_ne!(
            attempts[0].provider_session_id,
            attempts[1].provider_session_id
        );
        assert_eq!(attempts[0].account_id, attempts[1].account_id);
        wait_for("the handoff", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("ECHO: Yulius: you continue task #1") }
        })
        .await?;
        let mut said = Vec::new();
        notices(&mut frames, &mut said);
        assert!(
            said.iter()
                .any(|n| n.contains("the compaction failed") && n.contains("fresh session")),
            "the terminal says it: {said:?}"
        );
        assert!(
            said.iter()
                .any(|n| n.contains("continues in a fresh session"))
        );
        assert_eq!(
            audited(&data, &id)?,
            [
                "context full: compacting",
                "the compaction failed: fresh session"
            ]
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// CTX-005: a fresh session that is full too flags the task with the reason; no third run.
#[test]
fn a_full_fresh_session_needs_a_person() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        // Full from the first message: the task's own and the handoff are refused alike.
        let app = build_with(
            &data,
            &fake,
            Arc::new(ScriptedUsage::default()),
            agent(0, true),
        )?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        let id = new_task(&app, &sandbox, &user, "Full").await?;
        let mut frames = app.runtime(&id).hub.watch();
        wait_for("needs attention", Duration::from_secs(90), || {
            let app = app.clone();
            let id = id.clone();
            async move {
                task(&app, &id)
                    .await
                    .is_ok_and(|t| t.activity == Some(Activity::NeedsAttention))
            }
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(
            attempts.len(),
            2,
            "compacted, restarted once, then a person"
        );
        let mut said = Vec::new();
        notices(&mut frames, &mut said);
        assert!(
            said.iter()
                .any(|n| n.contains("needs you") && n.contains("still full")),
            "the reason in the terminal: {said:?}"
        );
        assert_eq!(
            audited(&data, &id)?.last().map(String::as_str),
            Some("needs attention: the context is still full after compacting and a fresh session")
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// CTX-007: a session whose context filled while Yulius was down (its transcript ends in the
/// refusal) is compacted once the restarted server reattaches it.
#[test]
fn a_context_that_filled_during_a_restart_is_compacted_after_it() -> TestResult {
    use yulius::wiring::harness::HarnessAdapter;
    use yulius_app::ports::harness::HarnessPort;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let options = FakeAgentOptions {
        transcript: true,
        answers_first_message: true,
        ..FakeAgentOptions::default()
    };
    // Phase 1: the task runs and answers its first message; then the server goes away.
    let first = runtime()?;
    let id = first.block_on(async {
        let app = build_with(
            &data,
            &fake,
            Arc::new(ScriptedUsage::default()),
            options.clone(),
        )?;
        started(&app, &data).await
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let rt = runtime()?;
    rt.block_on(async {
        let app = build_with(&data, &fake, Arc::new(ScriptedUsage::default()), options)?;
        // Meanwhile the API refused a prompt as too long (Claude Code 2.1.292's records).
        let record = task(&app, &id).await?;
        let attempt = app.deps.store.live_attempt(&id).await?.ok_or("attempt")?;
        let session = attempt.provider_session_id.ok_or("session")?;
        let worktree = std::path::PathBuf::from(record.worktree_path.ok_or("worktree")?);
        let home = data.join("sandboxes").join("s-pool").join("home");
        let adapter =
            HarnessAdapter::new(std::path::PathBuf::from(env!("CARGO_BIN_EXE_yulius")), "x");
        let transcript = adapter.claude_transcript(&home, &worktree, &session);
        let refused = serde_json::json!({"type": "assistant", "isSidechain": false,
            "isApiErrorMessage": true, "error": "invalid_request", "sessionId": session,
            "message": {"model": "<synthetic>", "role": "assistant",
                "content": [{"type": "text", "text": "Prompt is too long"}]}});
        let mut lines = std::fs::read_to_string(&transcript)?;
        lines.push_str(&format!("{refused}\n"));
        std::fs::write(&transcript, lines)?;

        // Phase 2: the new server reconciles, reattaches and replays the transcript.
        app.reconcile().await?;
        wait_for(
            "the compaction after the restart",
            Duration::from_secs(30),
            || {
                let app = app.clone();
                let id = id.clone();
                async move { screen(&app, &id).contains("COMPACTED: Keep: task #1") }
            },
        )
        .await?;
        assert_eq!(app.deps.store.attempts(&id).await?.len(), 1, "the same run");
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
