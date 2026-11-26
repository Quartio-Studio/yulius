//! A session's first message and a person's typing: only keystrokes count as
//! typing, so a continued goal session leaves a person's draft alone; a goal task's `/goal` waits
//! behind a folder-trust dialog; a first message Yulius types with images is never lost by a
//! server restart or carried without its images after a capacity hold.
//!
//! End to end: supervisor → detached `yulius ptyhost` → fake agent running Claude Code's hooks
//! through the real `yulius hook` client against the internal listener; real worktrees; a
//! scripted usage probe for the subscription pool.

use crate::support;
use crate::support::eventually;
use crate::support::live::Live;
use crate::support::live::first_message::{
    ShiftedClock, activity, data_dir, hooked, new_task, notices, runtime, screen, start,
    start_with, stop,
};
use crate::support::live::pool::{HOUR, ScriptedUsage, refresh, seed, usage};

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use support::TestResult;
use support::agents::FakeAgentOptions;
use yulius::wiring::clock::SystemClock;
use yulius_app::auth::AuthContext;
use yulius_app::context::App;
use yulius_app::model::{LaunchKind, PauseKind, TaskMode};
use yulius_app::ports::Clock as _;
use yulius_app::updates::UpdateRunRecord;
use yulius_contracts::api::tasks::ContinueTask;
use yulius_domain::ids::{AccountId, TaskId, UserId};
use yulius_domain::task::{Activity, AttemptState};
use yulius_domain::time::Timestamp;
use yulius_domain::update::RunState;
use yulius_ptyhost::test_support::locate_fake_agent;

type BoxError = Box<dyn std::error::Error>;

/// A send time stamped before a move of the clock reads with the offset of its sending.
#[test]
fn a_shifted_clock_reads_a_stamp_with_the_offset_of_its_time() {
    let clock = ShiftedClock::new(-13 * 60_000);
    let sent = SystemClock.now();
    // The next move takes effect in a later millisecond than `sent`: a stamp in the move's own
    // millisecond reads with the new offset.
    while SystemClock.now() == sent {
        std::hint::spin_loop();
    }
    clock.shift(-2 * 60_000);
    clock.advance(30_000);
    assert_eq!(clock.of_wall(sent).0, sent.0 - 13 * 60_000);
    let later = SystemClock.now();
    assert_eq!(clock.of_wall(later).0, later.0 - 90_000);
    assert!(clock.now().0 >= later.0 - 90_000);
}

/// A goal task that got its goal (its launch prompt), then moved by a person to
/// another subscription while the goal's turn still runs (`--goal-delay-ms` holds it open; a
/// task moved after its turn ended gets nothing typed): the new session opens
/// with a turn on "Continue the task …" of 12 s (`--opening-turn-ms`), after which its goal is
/// typed again. Returns the task, its creator's context and a writer's terminal on the
/// new session during that turn.
async fn goal_in_a_continued_turn(
    app: &Arc<App>,
    data: &Path,
) -> Result<(TaskId, AuthContext, yulius_app::terminal::TerminalAttach), BoxError> {
    let (sandbox, user) = seed(app, data, &["a-first", "b-second"]).await?;
    let id = new_task(app, &sandbox, &user, "Make the header blue", TaskMode::Goal).await?;
    eventually("the first goal's turn", Duration::from_secs(60), || async {
        app.goal_is_open(&id) && activity(app, &id).await == Some(Activity::Working)
    })
    .await?;
    let ctx = AuthContext {
        user: app.deps.store.user(&user).await?.ok_or("user")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    };
    app.switch_task(
        &ctx,
        id.as_str(),
        &ContinueTask {
            account_id: Some("b-second".into()),
            ..Default::default()
        },
    )
    .await?;
    // The new run's screen replaced the first one's: its opening turn runs, no goal yet.
    eventually("the continued turn", Duration::from_secs(60), || async {
        let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
        attempts.len() == 2
            && attempts[1].state == AttemptState::Live
            && screen(app, &id).contains("WORKING")
            && !screen(app, &id).contains("GOAL SET")
    })
    .await?;
    let attach = app
        .terminal_open(&ctx, id.as_str(), Some((120, 40)))
        .await?;
    Ok((id, ctx, attach))
}

fn opening_turn() -> FakeAgentOptions {
    FakeAgentOptions {
        // Long enough that a busy test machine still types into the running turn.
        opening_turn_ms: Some(12_000),
        // The goal's turn outlasts the move that cuts it, however fast its hooks are answered.
        goal_delay_ms: Some(60_000),
        ..hooked()
    }
}

/// SPEC-068 TYP-002, AC-TYP-2: clicking into and out of the terminal during a continued goal
/// session's opening turn (focus and mouse reports through `terminal_input`) is not typing: the
/// goal is still set, once, after that turn.
#[test]
fn focus_and_mouse_reports_do_not_keep_the_goal_out() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, opening_turn(), 0, probe).await?;
        let (id, ctx, attach) = goal_in_a_continued_turn(&app, &data).await?;
        for report in [
            b"\x1b[I".as_slice(),
            b"\x1b[<0;10;5M",
            b"\x1b[<0;10;5m",
            b"\x1b[M #%",
            b"\x1b[O",
        ] {
            app.terminal_input(&ctx, &attach, report, false).await?;
        }
        eventually("the goal", Duration::from_secs(90), || async {
            screen(&app, &id).contains("GOAL SET")
        })
        .await?;
        let shown = screen(&app, &id);
        assert_eq!(shown.matches("GOAL SET").count(), 1, "{shown}");
        assert_ne!(activity(&app, &id).await, Some(Activity::NeedsAttention));
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-068 TYP-003, AC-TYP-3: a person's draft typed during a continued goal session's
/// opening turn does not get `/goal` appended; the task is flagged and told why.
#[test]
fn a_draft_typed_during_the_opening_turn_is_left_alone() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, opening_turn(), 0, probe).await?;
        let (id, ctx, attach) = goal_in_a_continued_turn(&app, &data).await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.terminal_input(&ctx, &attach, b"my own draft", false)
            .await?;
        eventually("the flag", Duration::from_secs(60), || async {
            activity(&app, &id).await == Some(Activity::NeedsAttention)
        })
        .await?;
        let said = notices(&mut watch);
        assert!(said.iter().any(|n| n.contains("no goal yet")), "{said:?}");
        // The task says it to whoever opens it later.
        assert_eq!(
            support::attention(&app, id.as_str()).await?,
            serde_json::json!({
                "kind": "yulius",
                "problem": "Someone typed in the terminal before Yulius set the task's goal, so \
                            their draft was left alone. The task has no goal yet.",
                "action": "Type `/goal` and the request in the terminal, or stop and continue the task.",
                "link": null,
            })
        );
        let shown = screen(&app, &id);
        assert!(!shown.contains("/goal"), "the goal was typed:\n{shown}");
        assert!(!shown.contains("GOAL SET"), "{shown}");
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Claude Code opens on its folder-trust prompt and a person answers it
/// before the goal. Yulius types nothing into the prompt, the answer is not a draft, and the goal
/// is set once the prompt is gone, with no flag.
#[test]
fn a_trust_prompt_answered_before_the_goal_keeps_the_goal() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            trust_prompt: true,
            ..hooked()
        };
        let (app, _) = start(&data, &fake, agent, 0, probe).await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(
            &app,
            &sandbox,
            &user,
            "Make the header blue",
            TaskMode::Goal,
        )
        .await?;
        let mut watch = app.runtime(&id).hub.watch();
        eventually("the trust prompt", Duration::from_secs(60), || async {
            screen(&app, &id).contains("Yes, I trust this folder")
        })
        .await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let attach = app
            .terminal_open(&ctx, id.as_str(), Some((120, 40)))
            .await?;
        // Longer than the goal's quiet time: a Yulius that typed into the prompt would have now.
        tokio::time::sleep(Duration::from_secs(3)).await;
        app.terminal_input(&ctx, &attach, b"2\r", false).await?;
        eventually("the goal", Duration::from_secs(90), || async {
            screen(&app, &id).contains("GOAL SET")
        })
        .await?;
        let shown = screen(&app, &id);
        let answer = shown
            .lines()
            .find(|line| line.starts_with("TRUST ANSWERED:"))
            .map(str::trim_end);
        assert_eq!(answer, Some("TRUST ANSWERED: 2"), "{shown}");
        assert_eq!(shown.matches("GOAL SET").count(), 1, "{shown}");
        assert_ne!(activity(&app, &id).await, Some(Activity::NeedsAttention));
        let said = notices(&mut watch);
        assert!(!said.iter().any(|n| n.contains("no goal yet")), "{said:?}");
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A trust prompt nobody answers gets nothing typed into it; once the
/// time to answer it is over the task is flagged and says why.
#[test]
fn an_unanswered_trust_prompt_gets_nothing_typed_and_flags_the_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            trust_prompt: true,
            ..hooked()
        };
        let (app, _) = start_with(&data, &fake, agent, 0, probe, |config| {
            config.trust_answer_ms = 3_000;
            config.goal_ready_ms = 2_000;
        })
        .await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(
            &app,
            &sandbox,
            &user,
            "Make the header blue",
            TaskMode::Goal,
        )
        .await?;
        let mut watch = app.runtime(&id).hub.watch();
        eventually("the flag", Duration::from_secs(60), || async {
            activity(&app, &id).await == Some(Activity::NeedsAttention)
        })
        .await?;
        let shown = screen(&app, &id);
        assert!(shown.contains("Yes, I trust this folder"), "{shown}");
        assert!(
            !shown.contains("TRUST ANSWERED"),
            "something answered it:\n{shown}"
        );
        let said = notices(&mut watch);
        assert!(
            said.iter()
                .any(|n| n.contains("asks whether to trust this folder. Answer")),
            "{said:?}"
        );
        assert!(
            said.iter()
                .any(|n| n.contains("still asks whether to trust this folder")
                    && n.ends_with("the task starts by itself once it is answered.")),
            "{said:?}"
        );
        assert_eq!(
            support::attention(&app, id.as_str()).await?,
            serde_json::json!({
                "kind": "yulius",
                "problem": "Claude Code still asks whether to trust this folder, so Yulius typed \
                            nothing.",
                "action": "Answer it in the terminal; the task starts by itself once it is \
                           answered.",
                "link": null,
            })
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-483 slice 5 (review rounds 2 and 3): a terminal whose screen is withheld (an
/// agent secret no longer opens) shows nothing of what the program draws, so Yulius cannot tell
/// whether it reads input or asks something. It types nothing: no first message into a session
/// that started behind the withheld screen (the task is flagged with what to do), and no
/// message another agent sends the task.
#[test]
fn a_withheld_screen_gets_nothing_typed_and_flags_the_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            trust_prompt: true,
            ..hooked()
        };
        let (app, _) = start_with(&data, &fake, agent, 0, probe, |config| {
            // Longer than the test: the flag is the withheld screen's, never the trust dialog's.
            config.trust_answer_ms = 600_000;
            config.goal_ready_ms = 2_000;
        })
        .await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let add = |name: &'static str| {
            let (app, ctx, sandbox) = (Arc::clone(&app), ctx.clone(), sandbox.clone());
            async move {
                let input = serde_json::from_value(serde_json::json!({
                    "name": name,
                    "value": format!("{name}-value-0123456"),
                }))?;
                Ok::<_, BoxError>(
                    app.create_agent_secret(&ctx, sandbox.as_str(), &input)
                        .await?,
                )
            }
        };
        let key = add("DEPLOY_KEY").await?;
        // A first message Yulius types (it has an attachment), not one given at the launch.
        let id = new_task(
            &app,
            &sandbox,
            &user,
            "Make it blue\n\n@/srv/shot.png",
            TaskMode::Prompt,
        )
        .await?;
        let mut watch = app.runtime(&id).hub.watch();
        eventually("the trust prompt", Duration::from_secs(60), || async {
            screen(&app, &id).contains("Yes, I trust this folder")
        })
        .await?;

        // The secret no longer opens (a key-encryption key nobody has), and a change of a
        // secret makes the terminal read them anew.
        let seen = app.deps.store.vault_get(&key.id).await?.ok_or("secret")?;
        let mut broken = seen.clone();
        broken.envelope.kek_id = 9_999;
        assert!(app.deps.store.vault_update(&broken, &seen).await?);
        add("LATER_KEY").await?;
        // A person answers the dialog: the session starts behind a withheld screen.
        let attach = app
            .terminal_open(&ctx, id.as_str(), Some((120, 40)))
            .await?;
        app.terminal_input(&ctx, &attach, b"2\r", false).await?;

        eventually("the flag", Duration::from_secs(60), || async {
            activity(&app, &id).await == Some(Activity::NeedsAttention)
        })
        .await?;
        assert_eq!(
            support::attention(&app, id.as_str()).await?,
            serde_json::json!({
                "kind": "yulius",
                "problem": "Agent secrets could not be read, so this terminal's screen was \
                            withheld and Yulius typed nothing.",
                "action": "If the terminal still says its screen is withheld, replace or delete \
                           the agent secret that cannot be read (the server's log names the \
                           error). Then stop and continue the task.",
                "link": null,
            })
        );
        let said = notices(&mut watch);
        assert!(
            said.iter()
                .any(|n| n.contains("screen was withheld and Yulius typed nothing")),
            "{said:?}"
        );
        assert!(
            !said
                .iter()
                .any(|n| n.contains("still asks whether to trust")),
            "{said:?}"
        );
        // A message from the project's Superagent is not typed either, and says why.
        let caller = crate::support::task_message::superagent(&sandbox, &user, "admin");
        let message = serde_json::json!({"task_id": id.as_str(), "text": "Go on"});
        let refused = app
            .tool_task_message(&caller, &message)
            .await
            .err()
            .ok_or("a message was typed into a withheld screen")?;
        assert!(
            refused.to_string().contains("screen was withheld"),
            "{refused}"
        );
        app.terminal_leave(&attach).await;
        stop(&app, &id).await?;
        // Nothing reached the session: no transcript holds the first message or the
        // Superagent's.
        let mut pending = vec![data.clone()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir)? {
                let path = entry?.path();
                if path.is_dir() && !path.is_symlink() {
                    pending.push(path);
                } else if path.extension().is_some_and(|ext| ext == "jsonl") {
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    assert!(
                        !text.contains("Make it blue") && !text.contains("Go on"),
                        "{}: {text}",
                        path.display()
                    );
                }
            }
        }
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The trust dialog's words in a session that has started (here a person's
/// draft) are no trust prompt: the draft is left alone and the task flagged, as in #252.
#[test]
fn trust_words_after_the_session_started_are_no_trust_prompt() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, opening_turn(), 0, probe).await?;
        let (id, ctx, attach) = goal_in_a_continued_turn(&app, &data).await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.terminal_input(&ctx, &attach, b"Yes, I trust this folder", false)
            .await?;
        eventually("the flag", Duration::from_secs(60), || async {
            activity(&app, &id).await == Some(Activity::NeedsAttention)
        })
        .await?;
        let said = notices(&mut watch);
        assert!(said.iter().any(|n| n.contains("no goal yet")), "{said:?}");
        assert!(
            !said.iter().any(|n| n.contains("asks whether to trust")),
            "{said:?}"
        );
        assert!(!screen(&app, &id).contains("/goal"));
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A `/goal` launch prompt that Claude Code never takes (no goal
/// recorded within `launch_goal_ms` once the session reads input) flags the task and says so, as
/// a typed goal that was never taken does.
#[test]
fn a_launch_goal_never_taken_flags_the_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            ignores_goal_prompt: true,
            ..hooked()
        };
        let (app, _) = start_with(&data, &fake, agent, 0, probe, |config| {
            // 75 s in production (the typed goal's submit waits): shortened.
            config.launch_goal_ms = 3_000;
        })
        .await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(&app, &sandbox, &user, "Tidy the parser", TaskMode::Goal).await?;
        let mut watch = app.runtime(&id).hub.watch();
        eventually("the flag", Duration::from_secs(45), || async {
            activity(&app, &id).await == Some(Activity::NeedsAttention)
        })
        .await?;
        let said = notices(&mut watch);
        assert!(
            said.iter()
                .any(|n| n.contains("Claude Code did not take the task's goal")),
            "{said:?}"
        );
        assert_eq!(
            support::attention(&app, id.as_str()).await?,
            serde_json::json!({
                "kind": "yulius",
                "problem": "Claude Code did not take the task's goal. The task has no goal yet.",
                "action": "Type `/goal` and the request in the terminal, or stop and continue the task.",
                "link": null,
            })
        );
        assert!(
            !screen(&app, &id).contains("/goal"),
            "{}",
            screen(&app, &id)
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-117 ONE-006, AC-ONE-3: keys a person types while Yulius types a prompt task's first
/// message with an image are not written into it, and the person is told to type once it is sent;
/// after it, typing reaches the session again. Without a SessionStart hook the session reads input
/// 10 s after its start (half of `goal_ready_ms`), so the keys surely come before the message.
#[test]
fn keys_typed_while_yulius_types_the_first_message_wait() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            no_session_start: true,
            ..hooked()
        };
        let (app, _) = start_with(&data, &fake, agent, 0, probe, |config| {
            config.goal_ready_ms = 20_000;
        })
        .await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(
            &app,
            &sandbox,
            &user,
            "Make it blue\n\n@/srv/shot.png",
            TaskMode::Prompt,
        )
        .await?;
        eventually("the session", Duration::from_secs(60), || async {
            screen(&app, &id).contains("FAKE-AGENT READY")
        })
        .await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let attach = app
            .terminal_open(&ctx, id.as_str(), Some((120, 40)))
            .await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.terminal_input(&ctx, &attach, b"my draft", false)
            .await?;
        app.terminal_input(&ctx, &attach, b" more", false).await?;
        eventually("the first message", Duration::from_secs(60), || async {
            {
                let shown = screen(&app, &id);
                // The image is pasted where it sits: after the text.
                shown
                    .find("ECHO: Make it blue")
                    .is_some_and(|text| shown.find("[Image #1]").is_some_and(|image| image > text))
            }
        })
        .await?;
        let shown = screen(&app, &id);
        assert!(!shown.contains("my draft"), "{shown}");
        assert!(!shown.contains("more"), "{shown}");
        let said = notices(&mut watch);
        let told = said
            .iter()
            .filter(|n| n.contains("Yulius is typing this task's first message"))
            .count();
        assert_eq!(told, 1, "{said:?}");
        // Sent: the person types into the session again.
        app.terminal_input(&ctx, &attach, b"after\r", false).await?;
        eventually("the person's message", Duration::from_secs(30), || async {
            screen(&app, &id).contains("ECHO: after")
        })
        .await?;
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The folder-trust dialog in front of a prompt task's typed first message is
/// answered by a person: those keys pass (only a person answers it), and the message with its
/// image is typed once the dialog is gone.
#[test]
fn a_trust_prompt_before_a_typed_first_message_takes_the_answer() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            trust_prompt: true,
            ..hooked()
        };
        let (app, _) = start(&data, &fake, agent, 0, probe).await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(
            &app,
            &sandbox,
            &user,
            "Make it blue\n\n@/srv/shot.png",
            TaskMode::Prompt,
        )
        .await?;
        let mut watch = app.runtime(&id).hub.watch();
        eventually("the trust prompt", Duration::from_secs(60), || async {
            screen(&app, &id).contains("Yes, I trust this folder")
        })
        .await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let attach = app
            .terminal_open(&ctx, id.as_str(), Some((120, 40)))
            .await?;
        app.terminal_input(&ctx, &attach, b"2\r", false).await?;
        eventually("the first message", Duration::from_secs(60), || async {
            {
                let shown = screen(&app, &id);
                // The image is pasted where it sits: after the text.
                shown
                    .find("ECHO: Make it blue")
                    .is_some_and(|text| shown.find("[Image #1]").is_some_and(|image| image > text))
            }
        })
        .await?;
        let shown = screen(&app, &id);
        let answer = shown
            .lines()
            .find(|line| line.starts_with("TRUST ANSWERED:"))
            .map(str::trim_end);
        assert_eq!(answer, Some("TRUST ANSWERED: 2"), "{shown}");
        let said = notices(&mut watch);
        assert!(
            !said.iter().any(|n| n.contains("Yulius is typing")),
            "{said:?}"
        );
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A trust prompt nobody answers in front of a prompt task's
/// typed first message gets nothing typed into it; once the time to answer it is over the task
/// is flagged, and it says what its person does: answer, then type the first message.
#[test]
fn an_unanswered_trust_prompt_before_a_typed_first_message_flags_the_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            trust_prompt: true,
            ..hooked()
        };
        let (app, _) = start_with(&data, &fake, agent, 0, probe, |config| {
            config.trust_answer_ms = 3_000;
            config.goal_ready_ms = 2_000;
        })
        .await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(
            &app,
            &sandbox,
            &user,
            "Make it blue\n\n@/srv/shot.png",
            TaskMode::Prompt,
        )
        .await?;
        let mut watch = app.runtime(&id).hub.watch();
        eventually("the flag", Duration::from_secs(60), || async {
            activity(&app, &id).await == Some(Activity::NeedsAttention)
        })
        .await?;
        let shown = screen(&app, &id);
        assert!(shown.contains("Yes, I trust this folder"), "{shown}");
        assert!(
            !shown.contains("TRUST ANSWERED") && !shown.contains("/srv/shot.png"),
            "something was typed:\n{shown}"
        );
        let said = notices(&mut watch);
        assert!(
            said.iter().any(|n| n
                == "Claude Code still asks whether to trust this folder, so Yulius typed \
                    nothing. Answer it in the terminal, then type the task's first message."),
            "{said:?}"
        );
        assert_eq!(
            support::attention(&app, id.as_str()).await?,
            serde_json::json!({
                "kind": "yulius",
                "problem": "Claude Code still asks whether to trust this folder, so Yulius typed \
                            nothing.",
                "action": "Answer it in the terminal, then type the task's first message.",
                "link": null,
            })
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-068 OPN-002, AC-OPN-1: the server restarts while the first message of a task with
/// an image is being typed (the image pasted, no Enter yet). The new server reattaches the run
/// and flags the task with a notice; nothing is submitted for it.
#[test]
fn a_first_message_cut_by_a_restart_flags_the_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let first = runtime()?;
    let (port, id) = first.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, port) = start(&data, &fake, hooked(), 0, probe).await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(
            &app,
            &sandbox,
            &user,
            "Make it blue\n\n@/srv/shot.png",
            TaskMode::Prompt,
        )
        .await?;
        // The image's paste reached the terminal (its echo); the text and Enter come after the
        // paste settled, at least 500 ms later (`PASTE_QUIET`). A test that missed that window
        // would fail (the message echoed), never pass falsely.
        eventually("the image's paste", Duration::from_secs(60), || async {
            screen(&app, &id).contains("/srv/shot.png")
        })
        .await?;
        Ok::<_, BoxError>((port, id))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, hooked(), port, probe).await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.reconcile().await?;
        eventually("the flag", Duration::from_secs(20), || async {
            activity(&app, &id).await == Some(Activity::NeedsAttention)
        })
        .await?;
        let said = notices(&mut watch);
        assert!(
            said.iter()
                .any(|n| n.contains("first message was never sent")),
            "{said:?}"
        );
        // No viewer is connected when a server starts: the task itself says it.
        assert_eq!(
            support::attention(&app, id.as_str()).await?,
            serde_json::json!({
                "kind": "yulius",
                "problem": "Yulius restarted while it typed this task's first message, so the \
                            first message was never sent and may be half typed in the prompt box.",
                "action": "Complete it (paste the attached images again if they are missing; \
                           the task's request lists them) and press Enter.",
                "link": null,
            })
        );
        let shown = screen(&app, &id);
        assert!(
            !shown.contains("ECHO:"),
            "something was submitted:\n{shown}"
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Phase one of a restart during an instance update: a prompt task with an image whose first
/// message's submit reached Yulius, which held it for the update. Returns the data directory's
/// guard and path, the internal port and the task; the first server is gone.
fn first_message_held_by_an_update(
    fake: &Path,
) -> Result<(tempfile::TempDir, PathBuf, u16, TaskId), BoxError> {
    let (dir, data) = data_dir()?;
    let first = runtime()?;
    let (port, id) = first.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, port) = start(&data, fake, hooked(), 0, probe).await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(
            &app,
            &sandbox,
            &user,
            "Make it blue\n\n@/srv/shot.png",
            TaskMode::Prompt,
        )
        .await?;
        eventually("the run", Duration::from_secs(30), || async {
            matches!(app.deps.store.live_attempt(&id).await, Ok(Some(_)))
        })
        .await?;
        let attempt = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        // An update drains the instance before the first message is typed: every submit waits.
        let now = Timestamp(1);
        app.deps
            .store
            .insert_update_run(&UpdateRunRecord {
                id: "update-1".into(),
                target_commit: "b".repeat(40),
                from_commit: None,
                summary: "next".into(),
                state: RunState::Draining,
                outcome: None,
                error: None,
                requested_by: "admin".into(),
                created_at: now,
                state_since: now,
                finished_at: None,
            })
            .await?;
        app.restore_update_hold().await;
        let marker = Path::new(&attempt.run_dir).join("opening-pending");
        eventually("the held submit", Duration::from_secs(60), || async {
            std::fs::read(&marker).is_ok_and(|m| !m.is_empty())
        })
        .await?;
        Ok::<_, BoxError>((port, id))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));
    Ok((dir, data, port, id))
}

/// The first message's submit reached Yulius but an instance update held it
/// when the server restarted. The new server does not flag the task: the held submit goes on and
/// the agent gets the message once.
#[test]
fn a_first_message_held_by_an_update_is_not_flagged_after_the_restart() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data, port, id) = first_message_held_by_an_update(&fake)?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, hooked(), port, probe).await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.reconcile().await?;
        eventually("the first message", Duration::from_secs(60), || async {
            {
                let shown = screen(&app, &id);
                // The image is pasted where it sits: after the text.
                shown
                    .find("ECHO: Make it blue")
                    .is_some_and(|text| shown.find("[Image #1]").is_some_and(|image| image > text))
            }
        })
        .await?;
        assert_ne!(activity(&app, &id).await, Some(Activity::NeedsAttention));
        let said = notices(&mut watch);
        assert!(!said.iter().any(|n| n.contains("never sent")), "{said:?}");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A hooked agent whose goal gets no verdict: the task stays on its run (not Done).
fn unjudged() -> FakeAgentOptions {
    FakeAgentOptions {
        goal_verdict: Some("none"),
        ..hooked()
    }
}

/// A goal run started by an older server typed a first message before its goal; that
/// message was submitted and held when the server restarted into this version, which types no
/// goal after it. The task is flagged and says why, instead of going on without a goal.
#[test]
fn a_goal_run_of_an_older_server_held_across_the_update_is_flagged() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let first = runtime()?;
    let (port, id) = first.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, port) = start(&data, &fake, unjudged(), 0, probe).await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(&app, &sandbox, &user, "Make it blue", TaskMode::Goal).await?;
        // The goal turn ended (its Stop clears a pending first message, so it comes first).
        eventually("the goal turn's end", Duration::from_secs(60), || async {
            screen(&app, &id).contains("GOAL SET")
                && !matches!(activity(&app, &id).await, Some(Activity::Working) | None)
        })
        .await?;
        // What the older server left in the run directory: a first message submitted, held.
        let attempt = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        std::fs::write(
            Path::new(&attempt.run_dir).join("opening-pending"),
            b"entered",
        )?;
        Ok::<_, BoxError>((port, id))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, unjudged(), port, probe).await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.reconcile().await?;
        eventually("the flag", Duration::from_secs(20), || async {
            activity(&app, &id).await == Some(Activity::NeedsAttention)
        })
        .await?;
        let said = notices(&mut watch);
        assert!(
            said.iter()
                .any(|n| n.contains("Yulius restarted before it set this task's goal")),
            "{said:?}"
        );
        assert_eq!(
            support::attention(&app, id.as_str()).await?,
            serde_json::json!({
                "kind": "yulius",
                "problem": "Yulius restarted before it set this task's goal. The task has no \
                            goal yet.",
                "action": "Type `/goal` and the request in the terminal, or stop and continue the task.",
                "link": null,
            })
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A person starts to type just before Yulius types a resumed session's
/// `/goal` (the opening turn is over and nobody had typed; Yulius may wait for the typing mark
/// there). Asked again with the mark held, their key keeps the goal out, and the task says that
/// it has no goal, as when they typed earlier.
#[test]
fn a_key_just_before_the_goal_keeps_it_out_and_is_said() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, opening_turn(), 0, probe).await?;
        let (id, ctx, _attach) = goal_in_a_continued_turn(&app, &data).await?;
        let mut watch = app.runtime(&id).hub.watch();
        {
            let (app, key, id, person) = (app.clone(), id.clone(), id.clone(), ctx.user.id.clone());
            yulius_app::race::on(
                yulius_app::race::Point::LineReady,
                Path::new(key.as_str()),
                move || app.record_typing(&id, &person),
            );
        }
        eventually("the flag", Duration::from_secs(60), || async {
            activity(&app, &id).await == Some(Activity::NeedsAttention)
        })
        .await?;
        let said = notices(&mut watch);
        assert!(
            said.iter()
                .any(|n| n
                    .contains("Someone typed in the terminal before Yulius set the task's goal")),
            "{said:?}"
        );
        assert!(!screen(&app, &id).contains("GOAL SET"), "no goal was typed");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// While Yulius types a resumed session's `/goal` (the line is in the prompt
/// box, its Enter not sent yet), a message for the task is refused as while another message is
/// being typed, and the goal is set as it was typed.
#[test]
fn nothing_is_typed_between_the_goal_and_its_enter() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, opening_turn(), 0, probe).await?;
        let (id, ctx, _attach) = goal_in_a_continued_turn(&app, &data).await?;
        let record = app.deps.store.task(&id).await?.ok_or("task")?;
        let refused = Arc::new(std::sync::Mutex::new(None));
        {
            let (app, key, id) = (app.clone(), id.clone(), id.clone());
            let refused = refused.clone();
            let caller = yulius_app::coordination::ProjectCaller {
                sandbox: record.sandbox_id.clone(),
                user: ctx.user.id.clone(),
                task: None,
                channel: yulius_app::coordination::Channel::Superagent,
                actor: "cockpit:admin".into(),
                detail: serde_json::json!({"via": "cockpit", "shell": "sh-test"}),
            };
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::LineTyped,
                Path::new(key.as_str()),
                move || {
                    let answer = tokio::task::block_in_place(|| {
                        handle.block_on(app.tool_task_message(
                            &caller,
                            &serde_json::json!({"task_id": id.as_str(), "text": "one more thing"}),
                        ))
                    });
                    if let Ok(mut slot) = refused.lock() {
                        *slot = Some(answer.map(|_| ()).map_err(|err| err.to_string()));
                    }
                },
            );
        }
        eventually("the typed goal", Duration::from_secs(60), || async {
            screen(&app, &id).contains("GOAL SET")
        })
        .await?;
        let answer = refused.lock().map_err(|_| "poisoned")?.clone();
        let answer = answer.ok_or("no message was tried while the goal waited for its Enter")?;
        assert!(
            matches!(&answer, Err(why) if why.contains("is being typed")),
            "{answer:?}"
        );
        assert!(!screen(&app, &id).contains("one more thing"));
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The run directory's marker of a `/goal` Yulius still has to type.
fn typed_goal_marker(run_dir: &str) -> PathBuf {
    Path::new(run_dir).join("typed-goal-pending")
}

/// Yulius restarts while it still has to type a resumed session's `/goal`
/// (the session's opening turn runs). The new server has no typist for it; it waits that turn
/// out, watches whether Claude Code recorded a goal and, as it did not, flags the task and says
/// why, instead of leaving a goal task idle without a goal and without a word. The flag comes
/// after the opening turn's end, whose Stop would otherwise take it away (the bound here, 3 s,
/// is long over when the 12 s turn ends).
#[test]
fn a_goal_still_to_be_typed_at_a_restart_is_flagged() -> TestResult {
    let flagged = restart_before_goal_is_typed(FakeAgentOptions {
        forget_goal: true,
        ..opening_turn()
    })?;
    assert!(flagged, "a goal the session never took needs its person");
    Ok(())
}

/// A restored goal's opening-turn verdict satisfies the restarted server's goal watch.
#[test]
fn a_restored_goal_is_not_flagged_when_the_server_restarts_before_typing_it() -> TestResult {
    let flagged = restart_before_goal_is_typed(opening_turn())?;
    assert!(!flagged, "the restored goal received its verdict");
    Ok(())
}

fn restart_before_goal_is_typed(agent: FakeAgentOptions) -> Result<bool, BoxError> {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let first = runtime()?;
    let restored = !agent.forget_goal;
    let (port, id) = first.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, port) = start(&data, &fake, agent.clone(), 0, probe).await?;
        let (id, _ctx, _attach) = goal_in_a_continued_turn(&app, &data).await?;
        let attempt = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        assert!(typed_goal_marker(&attempt.run_dir).is_file());
        assert_eq!(screen(&app, &id).contains("GOAL RESTORED"), restored);
        Ok::<_, BoxError>((port, id))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let rt = runtime()?;
    let flagged = rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start_with(&data, &fake, agent, port, probe, |config| {
            config.launch_goal_ms = 3_000;
            config.goal_ready_ms = 4_000;
        })
        .await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.reconcile().await?;
        let attempt = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        // The opening turn ends under the new server: its Stop is answered.
        let stop_answered = Path::new(&attempt.run_dir).join("stop-answered");
        eventually(
            "the opening turn's end",
            Duration::from_secs(60),
            || async { stop_answered.exists() },
        )
        .await?;
        // Not flagged inside the turn: the watch's time starts at the turn's end.
        assert_ne!(activity(&app, &id).await, Some(Activity::NeedsAttention));
        assert!(notices(&mut watch).iter().all(|n| !n.contains("goal")));
        // The watch ends once, whether it observed a verdict or flagged the missing goal.
        let marker = typed_goal_marker(&attempt.run_dir);
        eventually("the marker's removal", Duration::from_secs(45), || async {
            !marker.exists()
        })
        .await?;
        let said = notices(&mut watch);
        let flagged = activity(&app, &id).await == Some(Activity::NeedsAttention);
        assert_eq!(
            said.iter()
                .any(|n| n.contains("Yulius restarted before it set this task's goal")),
            flagged,
            "{said:?}"
        );
        assert!(
            !screen(&app, &id).contains("GOAL SET"),
            "the typist ended with the first server"
        );
        stop(&app, &id).await?;
        Ok::<_, BoxError>(flagged)
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(flagged)
}

/// Claude Code recorded the typed goal, and the server stopped before it
/// removed the marker. The new server's watch finds the goal in the run's transcript: no flag,
/// and the marker goes.
#[test]
fn a_typed_goal_recorded_before_a_restart_is_not_flagged() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let first = runtime()?;
    let (port, id, marker) = first.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, port) = start(&data, &fake, opening_turn(), 0, probe).await?;
        let (id, _ctx, _attach) = goal_in_a_continued_turn(&app, &data).await?;
        let attempt = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        let marker = typed_goal_marker(&attempt.run_dir);
        eventually("the typed goal", Duration::from_secs(60), || async {
            screen(&app, &id).contains("GOAL SET")
        })
        .await?;
        eventually("the marker's removal", Duration::from_secs(20), || async {
            !marker.exists()
        })
        .await?;
        // What a server stopped between the goal's record and that removal leaves.
        std::fs::write(&marker, b"")?;
        Ok::<_, BoxError>((port, id, marker))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start_with(&data, &fake, opening_turn(), port, probe, |config| {
            config.launch_goal_ms = 3_000;
            config.goal_ready_ms = 4_000;
        })
        .await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.reconcile().await?;
        eventually("the marker's removal", Duration::from_secs(30), || async {
            !marker.exists()
        })
        .await?;
        assert_ne!(activity(&app, &id).await, Some(Activity::NeedsAttention));
        let said = notices(&mut watch);
        assert!(!said.iter().any(|n| n.contains("goal")), "{said:?}");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A goal Yulius typed and Claude Code took leaves no marker, so no later
/// server watches it or flags the task.
#[test]
fn a_typed_goal_that_was_taken_leaves_no_marker() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, opening_turn(), 0, probe).await?;
        let (id, _ctx, _attach) = goal_in_a_continued_turn(&app, &data).await?;
        let attempt = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        let marker = typed_goal_marker(&attempt.run_dir);
        assert!(marker.is_file());
        eventually("the typed goal", Duration::from_secs(60), || async {
            screen(&app, &id).contains("GOAL SET")
        })
        .await?;
        eventually("the marker's removal", Duration::from_secs(20), || async {
            !marker.exists()
        })
        .await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The run directory's marker of a launch `/goal` not confirmed yet.
fn launch_goal_marker(run_dir: &str) -> PathBuf {
    Path::new(run_dir).join("launch-goal-pending")
}

/// The server restarts while a goal task's launch `/goal` is not
/// recorded (the agent ignores it). The new server watches it again: once the session reads input
/// and `launch_goal_ms` passed, the task is flagged with the notice, as without a restart.
#[test]
fn a_launch_goal_unrecorded_across_a_restart_is_watched_again() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let agent = || FakeAgentOptions {
        ignores_goal_prompt: true,
        ..hooked()
    };
    let first = runtime()?;
    let (port, id) = first.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, port) = start_with(&data, &fake, agent(), 0, probe, |config| {
            // The first server never gets to flag it.
            config.launch_goal_ms = 600_000;
        })
        .await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(&app, &sandbox, &user, "Tidy the parser", TaskMode::Goal).await?;
        // The run is up, not only recorded: a server stopped while it spawns leaves no run the
        // next server can reattach, so that server ends it as lost instead of watching it.
        eventually("the run", Duration::from_secs(30), || async {
            matches!(
                app.deps.store.live_attempt(&id).await,
                Ok(Some(attempt)) if attempt.state == AttemptState::Live
            )
        })
        .await?;
        let attempt = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        assert!(launch_goal_marker(&attempt.run_dir).is_file());
        Ok::<_, BoxError>((port, id))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start_with(&data, &fake, agent(), port, probe, |config| {
            config.launch_goal_ms = 3_000;
            config.goal_ready_ms = 4_000;
        })
        .await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.reconcile().await?;
        eventually("the flag", Duration::from_secs(45), || async {
            activity(&app, &id).await == Some(Activity::NeedsAttention)
        })
        .await?;
        let said = notices(&mut watch);
        assert!(
            said.iter()
                .any(|n| n.contains("Claude Code did not take the task's goal")),
            "{said:?}"
        );
        // Flagged once: no later restart flags it again.
        let attempt = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        let marker = launch_goal_marker(&attempt.run_dir);
        eventually("the marker's removal", Duration::from_secs(10), || async {
            !marker.exists()
        })
        .await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A launch goal Claude Code recorded is never flagged after a
/// restart. Recorded while the server runs, its marker goes; recorded while the server was down
/// (the marker left), the reattached run's transcript replay gives it to the new watch, which
/// ends without a flag (its marker goes then).
#[test]
fn a_launch_goal_recorded_before_a_restart_is_not_watched_again() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let first = runtime()?;
    let (port, id, marker) = first.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, port) = start(&data, &fake, unjudged(), 0, probe).await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(&app, &sandbox, &user, "Make it blue", TaskMode::Goal).await?;
        eventually("the goal", Duration::from_secs(60), || async {
            screen(&app, &id).contains("GOAL SET")
        })
        .await?;
        let attempt = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        let marker = launch_goal_marker(&attempt.run_dir);
        eventually("the marker's removal", Duration::from_secs(60), || async {
            !marker.exists()
        })
        .await?;
        // As if the goal had been recorded while the server was down.
        std::fs::write(&marker, b"")?;
        Ok::<_, BoxError>((port, id, marker))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start_with(&data, &fake, unjudged(), port, probe, |config| {
            // Room for the replay under load; the watch ends as soon as the replay gives it.
            config.launch_goal_ms = 10_000;
            config.goal_ready_ms = 2_000;
        })
        .await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.reconcile().await?;
        // The watch ended: its outcome is decided.
        eventually("the watch's end", Duration::from_secs(45), || async {
            !marker.exists()
        })
        .await?;
        assert_ne!(activity(&app, &id).await, Some(Activity::NeedsAttention));
        let said = notices(&mut watch);
        assert!(!said.iter().any(|n| n.contains("did not take")), "{said:?}");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A task with an image whose first message is held for capacity: a-first runs out once the run
/// started; b-second, the only other subscription, is full too. Returns the task and its creator.
async fn first_message_held_for_capacity(
    app: &Arc<App>,
    data: &Path,
    probe: &ScriptedUsage,
) -> Result<(TaskId, UserId), BoxError> {
    let (sandbox, user) = seed(app, data, &["a-first", "b-second"]).await?;
    probe.set("a-first", usage(10.0, 4 * HOUR, 30.0, 100));
    probe.set("b-second", usage(99.0, 4 * HOUR, 30.0, 100));
    refresh(app, &user, "a-first").await?;
    refresh(app, &user, "b-second").await?;
    let id = new_task(
        app,
        &sandbox,
        &user,
        "Make it blue\n\n@/srv/shot.png",
        TaskMode::Prompt,
    )
    .await?;
    eventually("the run on a-first", Duration::from_secs(30), || async {
        matches!(app.deps.store.live_attempt(&id).await, Ok(Some(_)))
    })
    .await?;
    // a-first runs out before the first message is typed: it is held at its submit.
    probe.set("a-first", usage(99.0, 4 * HOUR, 30.0, 100));
    refresh(app, &user, "a-first").await?;
    eventually(
        "the held first message",
        Duration::from_secs(60),
        || async {
            matches!(
                app.deps.store.open_pause(&id).await,
                Ok(Some(pause)) if pause.kind == PauseKind::Prompt
            )
        },
    )
    .await?;
    // b-second's window resets.
    probe.set("b-second", usage(0.0, 5 * HOUR, 30.0, 100));
    refresh(app, &user, "b-second").await?;
    Ok((id, user))
}

/// The task moved to b-second as a fresh session that typed the task's own opening there, the
/// image pasted, not the held prompt's text.
async fn typed_again_on_b_second(app: &Arc<App>, id: &TaskId) -> TestResult {
    eventually("the run on b-second", Duration::from_secs(60), || async {
        let attempts = app.deps.store.attempts(id).await.unwrap_or_default();
        attempts.len() == 2 && attempts[1].state == AttemptState::Live
    })
    .await?;
    eventually("the first message", Duration::from_secs(60), || async {
        {
            let shown = screen(app, id);
            // The image is pasted where it sits: after the text.
            shown
                .find("ECHO: Make it blue")
                .is_some_and(|text| shown.find("[Image #1]").is_some_and(|image| image > text))
        }
    })
    .await?;
    let attempts = app.deps.store.attempts(id).await?;
    let second = &attempts[1];
    assert_eq!(
        second.account_id.as_ref().map(AccountId::as_str),
        Some("b-second")
    );
    assert_eq!(second.launch_kind, LaunchKind::New, "a fresh session");
    // The new session got the image pasted (its echo), not the held prompt's text.
    assert!(
        screen(app, id).contains("/srv/shot.png"),
        "{}",
        screen(app, id)
    );
    let launch = std::fs::read_to_string(Path::new(&second.run_dir).join("fake-launch.txt"))?;
    assert!(launch.contains("prompt: None"), "{launch}");
    Ok(())
}

/// SPEC-068 OPN-003, AC-OPN-2: the first message of a task with an image is held for
/// capacity and resumed on another subscription: a fresh session there gets the task's own
/// opening, the image pasted, not the held prompt's text.
#[test]
fn a_held_first_message_resumes_elsewhere_with_its_image() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, hooked(), 0, probe.clone()).await?;
        let (id, _) = first_message_held_for_capacity(&app, &data, &probe).await?;
        let _ = app.resume_round().await;
        typed_again_on_b_second(&app, &id).await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A person switches the account of a task whose first message is held: the
/// same fresh session on the chosen account, not "continue where you left off" in an empty one.
#[test]
fn a_manual_switch_of_a_held_first_message_keeps_its_image() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, hooked(), 0, probe.clone()).await?;
        let (id, user) = first_message_held_for_capacity(&app, &data, &probe).await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        app.switch_task(
            &ctx,
            id.as_str(),
            &ContinueTask {
                account_id: Some("b-second".into()),
                ..Default::default()
            },
        )
        .await?;
        typed_again_on_b_second(&app, &id).await?;
        assert!(
            !screen(&app, &id).contains("Continue the task"),
            "{}",
            screen(&app, &id)
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The turn Claude redoes past its limit can reach Stop while the new PTY attach is held.
#[test]
fn a_restored_prompt_goal_waits_at_stop_before_attach() -> TestResult {
    let (_dir, data) = data_dir()?;
    let fake = locate_fake_agent().ok_or("fake-agent")?;
    let rt = runtime()?;
    rt.block_on(async {
        let host = Arc::new(crate::support::gated::GatedHost::new(
            env!("CARGO_BIN_EXE_yulius").into(),
        ));
        let options = FakeAgentOptions {
            opening_turn_ms: Some(1),
            goal_verdict: Some("none"),
            limit_after: Some(3),
            ..hooked()
        };
        let (app, _) = Live {
            usage: Arc::new(ScriptedUsage::default()),
            host: Some(host.clone()),
            ..Live::new(Some(&fake), options)
        }
        .start(&data, 0, |_| {})
        .await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(&app, &sandbox, &user, "initial turn", TaskMode::Prompt).await?;
        eventually("the initial Stop", Duration::from_secs(30), || async {
            activity(&app, &id).await == Some(Activity::Waiting)
        })
        .await?;
        crate::support::live::pool::add_account(&app, "b-second").await?;
        app.send_input(&id, b"/goal keep the header blue\r").await?;
        eventually("the idle open goal", Duration::from_secs(30), || async {
            app.goal_is_open(&id) && activity(&app, &id).await == Some(Activity::Waiting)
        })
        .await?;
        assert!(
            !crate::support::staging::red_gate_needs_people(&app, &sandbox, &user).await?,
            "the prompt-mode task has an outstanding native goal"
        );
        host.arm();
        app.send_input(&id, b"reach the limit\r").await?;
        tokio::time::timeout(Duration::from_secs(60), host.reached()).await?;
        let resumed = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("live resume")?;
        assert_eq!(resumed.launch_kind, LaunchKind::Resume);
        eventually(
            "the resumed turn's Stop answer before attach",
            Duration::from_secs(30),
            || async { Path::new(&resumed.run_dir).join("stop-answered").exists() },
        )
        .await?;
        let after_stop = activity(&app, &id).await;
        host.release();
        assert_eq!(
            after_stop,
            Some(Activity::Working),
            "Stop waits through attach"
        );
        eventually(
            "the same Stop's deadline after attach",
            Duration::from_secs(30),
            || async { activity(&app, &id).await == Some(Activity::Waiting) },
        )
        .await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
