//! A run's metrics with real processes (SPEC-087 MET-001, MET-002, MET-004): supervisor →
//! detached `yulius ptyhost` → fake agent writing a Claude Code shaped transcript. Tokens, turns
//! and active time come from the run's own transcript lines and its sub-agents' transcripts, so
//! a Yulius restart reads them again instead of starting the run's figures from nothing.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::pool_supervision::{
    ScriptedUsage, build_with, new_task, runtime, screen, seed, stop, temp, wait_for,
};
use crate::support::TestResult;
use crate::support::agents::FakeAgentOptions;
use std::sync::Arc;
use yulius_app::auth::AuthContext;
use yulius_app::context::App;
use yulius_app::model::LaunchKind;
use yulius_domain::ids::{AttemptId, TaskId};
use yulius_domain::task::AttemptState;
use yulius_ptyhost::test_support::locate_fake_agent;

/// One run's stored metrics: output tokens, turns, active ms and when they were saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Saved {
    output: Option<i64>,
    turns: Option<i64>,
    active_ms: Option<i64>,
    at: i64,
}

fn saved(data: &Path, attempt: &AttemptId) -> Option<Saved> {
    let db = rusqlite::Connection::open(data.join("yulius.db")).ok()?;
    db.query_row(
        "SELECT output_tokens, turns, active_ms, updated_at FROM metrics WHERE attempt_id = ?1",
        [attempt.as_str()],
        |row| {
            Ok(Saved {
                output: row.get(0)?,
                turns: row.get(1)?,
                active_ms: row.get(2)?,
                at: row.get(3)?,
            })
        },
    )
    .ok()
}

/// The session transcripts under `dir` (bounded walk; the fake agent writes one per session).
fn transcripts(dir: &Path, found: &mut Vec<PathBuf>, budget: &mut usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            transcripts(&path, found, budget);
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            found.push(path);
        }
    }
}

fn now_rfc3339() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0));
    yulius_app::auth::rfc3339(yulius_domain::time::Timestamp(ms))
}

async fn attempts(app: &App, id: &TaskId) -> Vec<yulius_app::model::AttemptRecord> {
    app.deps.store.attempts(id).await.unwrap_or_default()
}

/// SPEC-087 MET-001, MET-002, MET-004: a resumed run's tokens, turns and active time — its
/// sub-agent's tokens included — survive a Yulius restart. The reattached run reads its own
/// transcript lines again; it used to read from the end and save nothing over them, which left
/// dozens of runs on the instance with no tokens and no active time.
#[test]
fn a_restart_keeps_a_resumed_run_s_metrics() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let probe = Arc::new(ScriptedUsage::default());
    let agent = FakeAgentOptions {
        transcript: true,
        ..FakeAgentOptions::default()
    };
    let first = runtime()?;
    let (id, before_first, before_second) = first.block_on(async {
        let app = build_with(&data, &fake, probe.clone(), agent.clone())?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        let id = new_task(&app, &sandbox, &user, "Metrics").await?;
        wait_for("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        app.send_input(&id, b"/work 1500 first\r").await?;
        let first_run = attempts(&app, &id).await.first().ok_or("run")?.id.clone();
        wait_for("the first run's metrics", Duration::from_secs(30), || {
            let found = saved(&data, &first_run);
            async move { found.is_some_and(|s| s.output.is_some() && s.turns.is_some()) }
        })
        .await?;
        // A person continues the session on another subscription: a native resume.
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
        };
        app.switch_task(
            &ctx,
            id.as_str(),
            &yulius_contracts::api::tasks::ContinueTask {
                account_id: Some("b-second".into()),
            },
        )
        .await?;
        wait_for("the resumed run", Duration::from_secs(60), || async {
            let runs = attempts(&app, &id).await;
            runs.len() == 2 && runs[1].state == AttemptState::Live
        })
        .await?;
        let runs = attempts(&app, &id).await;
        assert_eq!(runs[1].launch_kind, LaunchKind::Resume);
        let second_run = runs[1].id.clone();
        wait_for("the resumed agent", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        app.send_input(&id, b"/work 1500 second\r").await?;
        wait_for("the second turn", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("DONE second") }
        })
        .await?;
        // A sub-agent of the resumed run: its transcript sits next to the session's.
        let mut found = Vec::new();
        transcripts(&data.join("sandboxes"), &mut found, &mut 100_000);
        let session = found
            .iter()
            .find(|p| p.components().any(|c| c.as_os_str() == ".claude"))
            .ok_or("the session transcript")?;
        let sub = session.with_extension("").join("subagents");
        std::fs::create_dir_all(&sub)?;
        std::fs::write(
            sub.join("agent-a1.jsonl"),
            format!(
                "{}\n",
                serde_json::json!({"type": "assistant", "isSidechain": true,
                    "timestamp": now_rfc3339(), "message": {"id": "msg_sub_1",
                    "model": "fake-model", "content": [], "stop_reason": "end_turn",
                    "usage": {"input_tokens": 3, "output_tokens": 1000}}})
            ),
        )?;
        wait_for("the second run's metrics", Duration::from_secs(60), || {
            let found = saved(&data, &second_run);
            async move {
                found.is_some_and(|s| {
                    s.output.is_some_and(|o| o > 1000)
                        && s.turns == Some(1)
                        && s.active_ms.is_some_and(|ms| ms >= 1_000)
                })
            }
        })
        .await?;
        let before_first = saved(&data, &first_run).ok_or("first")?;
        let before_second = saved(&data, &second_run).ok_or("second")?;
        Ok::<_, Box<dyn std::error::Error>>((id, before_first, before_second))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let second = runtime()?;
    second.block_on(async {
        let app = build_with(&data, &fake, probe.clone(), agent)?;
        app.reconcile().await?;
        let runs = attempts(&app, &id).await;
        let second_run = runs.get(1).ok_or("second run")?.id.clone();
        // The reattached run's first save after the restart.
        wait_for("a save after the restart", Duration::from_secs(30), || {
            let found = saved(&data, &second_run);
            async move { found.is_some_and(|s| s.at > before_second.at) }
        })
        .await?;
        let after = saved(&data, &second_run).ok_or("second")?;
        assert_eq!(
            (after.output, after.turns),
            (before_second.output, before_second.turns),
            "the restart keeps the resumed run's tokens and turns"
        );
        assert!(
            after.active_ms >= before_second.active_ms,
            "active time is not lost: {after:?} after {before_second:?}"
        );
        assert_eq!(
            saved(&data, &runs[0].id),
            Some(before_first),
            "the first run's figures stay its own"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    second.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
