//! A session whose context is full (SPEC-060, issue #213) with real processes: supervisor →
//! detached `yulius ptyhost` → fake agent that answers "Prompt is too long" like Claude Code
//! 2.1.292. Yulius compacts it once, continues the task in a fresh session when compacting
//! fails, and flags the task when the fresh session is full too.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::pool_supervision::{
    ScriptedUsage, build_tuned, build_with, echoed, new_task, runtime, screen, seed, stop, task,
    temp, wait_for,
};
use crate::support::TestResult;
use crate::support::agents::FakeAgentOptions;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::TryRecvError;
use yulius_app::context::App;
use yulius_app::hub::HubFrame;
use yulius_app::model::LaunchKind;
use yulius_domain::ids::TaskId;
use yulius_domain::task::{Activity, AttemptState};
use yulius_ptyhost::test_support::locate_fake_agent;

/// The notices said so far (frames the receiver lagged behind on are output, not notices: a
/// lag is skipped, not the end).
fn notices(frames: &mut Receiver<HubFrame>, said: &mut Vec<String>) {
    loop {
        match frames.try_recv() {
            Ok(HubFrame::Notice(text)) => said.push(text),
            Ok(_) | Err(TryRecvError::Lagged(_)) => {}
            Err(_) => return,
        }
    }
}

/// Collects notices until, for each group of `words`, one notice holds all of them (they may be
/// said a moment after what the test waited for), or 10 s passed.
async fn heard(frames: &mut Receiver<HubFrame>, said: &mut Vec<String>, words: &[&[&str]]) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        notices(frames, said);
        let all = words
            .iter()
            .all(|group| said.iter().any(|n| group.iter().all(|w| n.contains(w))));
        if all || tokio::time::Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
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

/// Waits until the task's recovery audit satisfies `done`, then returns it. A row is written a
/// moment after the flag or the notice it goes with, so a test reading the audit right after
/// those can see the row before (#430).
async fn audit_when(
    data: &Path,
    id: &TaskId,
    done: impl Fn(&[String]) -> bool,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let rows = || audited(data, id).unwrap_or_default();
    // A timeout is reported by the caller's assertion on what the audit then holds.
    let _ = wait_for("the audit", Duration::from_secs(30), || {
        let ready = done(&rows());
        async move { ready }
    })
    .await;
    audited(data, id)
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

/// CTX-002/003, the acceptance of #213: a prompt the full context refuses ends the turn, Yulius
/// compacts the session once with a focus built from the task and wakes it, and the next prompt
/// reaches the agent.
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
        heard(
            &mut frames,
            &mut said,
            &[&["context is full"], &["compacted the conversation"]],
        )
        .await;
        assert!(
            said.iter().any(|n| n.contains("context is full")),
            "the terminal says it: {said:?}"
        );
        assert!(
            said.iter()
                .any(|n| n.contains("compacted the conversation"))
        );
        assert_eq!(
            audit_when(&data, &id, |rows| rows.len() >= 2).await?,
            ["context full: compacting", "compacted"],
            "the log says it"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The activities the UI is told about `id` from now on, as they come (a repeat is kept once).
fn published(
    app: &Arc<App>,
    id: &TaskId,
) -> (
    Arc<std::sync::Mutex<Vec<String>>>,
    tokio::task::JoinHandle<()>,
) {
    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let mut events = app.events.subscribe();
    let collector = tokio::spawn({
        let (seen, id) = (Arc::clone(&seen), id.as_str().to_owned());
        async move {
            loop {
                match events.recv().await {
                    Ok(envelope) => {
                        if let yulius_contracts::events::UiEvent::TaskUpdated { task } =
                            envelope.event
                            && task.id == id
                        {
                            let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
                            if seen.last() != task.activity.as_ref() {
                                seen.push(task.activity.unwrap_or_default());
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    });
    (seen, collector)
}

/// CTX-002 (SPEC-085 STT-006): the refused prompt ends the turn, and while Yulius compacts the
/// session by itself the task reads Working, not Your turn: nobody has to act. The compaction here
/// never finishes, so nothing else moves the task; it never read idle since the prompt.
#[test]
fn a_full_session_reads_working_while_yulius_compacts_it() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let options = FakeAgentOptions {
            compact_hangs: true,
            echo_turn_ms: Some(1_500),
            ..agent(1, false)
        };
        let app = build_with(&data, &fake, Arc::new(ScriptedUsage::default()), options)?;
        let id = started(&app, &data).await?;
        let (seen, collector) = published(&app, &id);
        app.send_input(&id, b"too much\r").await?;
        wait_for("the prompt worked on", Duration::from_secs(30), || {
            activity_is(&app, &id, Activity::Working)
        })
        .await?;
        wait_for("the compaction asked for", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("COMPACTING") }
        })
        .await?;
        assert!(screen(&app, &id).contains("PROMPT IS TOO LONG"));
        assert_eq!(audited(&data, &id)?, ["context full: compacting"]);
        // Two more watcher rounds: everything the session wrote by now was applied.
        let runtime = app.runtime(&id);
        let rounds = runtime.transcript_rounds();
        wait_for("the transcript read", Duration::from_secs(20), || async {
            runtime.transcript_rounds() >= rounds + 2
        })
        .await?;
        assert!(
            activity_is(&app, &id, Activity::Working).await,
            "Yulius compacts"
        );
        collector.abort();
        let seen = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert!(
            seen.iter().all(|a| a == "working"),
            "never Your turn while Yulius recovers: {seen:?}"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// CTX-002, CTX-006 (SPEC-085 STT-006): a person who types into the full session takes it over;
/// Yulius types nothing, says so, and the task waits for them (Your turn), not Working.
#[test]
fn a_full_session_a_person_types_into_waits_for_them() -> TestResult {
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
        let person = task(&app, &id).await?.created_by;
        let mut frames = app.runtime(&id).hub.watch();
        // The person types all along: whenever Yulius looks before typing, they typed since the
        // refusal.
        let typing = tokio::spawn({
            let (app, id) = (app.clone(), id.clone());
            async move {
                loop {
                    app.record_typing(&id, &person);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        });
        app.send_input(&id, b"too much\r").await?;
        let mut said = Vec::new();
        heard(
            &mut frames,
            &mut said,
            &[&["leaves the full session to you"]],
        )
        .await;
        typing.abort();
        assert!(
            said.iter()
                .any(|n| n.contains("leaves the full session to you")),
            "the terminal says it: {said:?}"
        );
        wait_for("Your turn", Duration::from_secs(10), || {
            activity_is(&app, &id, Activity::Waiting)
        })
        .await?;
        assert!(
            !screen(&app, &id).contains("COMPACT"),
            "Yulius typed nothing"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A hooked agent (its prompts empty the prompt box, as Claude Code's do) whose second message
/// meets a full context after a turn of `turn_ms`.
fn hooked(turn_ms: u64) -> FakeAgentOptions {
    FakeAgentOptions {
        hooks: true,
        echo_turn_ms: Some(turn_ms),
        ..agent(1, false)
    }
}

/// An app whose hooks reach it (a listener of its own), with `tune` applied to its settings.
async fn served(
    data: &Path,
    fake: &Path,
    agent: FakeAgentOptions,
    tune: impl FnOnce(&mut yulius_app::context::AppConfig),
) -> Result<(Arc<App>, u16), Box<dyn std::error::Error>> {
    let usage = Arc::new(ScriptedUsage::default());
    crate::first_message::start_with(data, fake, agent, 0, usage, tune).await
}

/// A person at the task's terminal: their context and a writer's attachment.
async fn at_the_terminal(
    app: &Arc<App>,
    id: &TaskId,
) -> Result<
    (
        yulius_app::auth::AuthContext,
        yulius_app::terminal::TerminalAttach,
    ),
    Box<dyn std::error::Error>,
> {
    let person = task(app, id).await?.created_by;
    let ctx = yulius_app::auth::AuthContext {
        user: app.deps.store.user(&person).await?.ok_or("user")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    };
    let attach = app
        .terminal_open(&ctx, id.as_str(), Some((120, 40)))
        .await?;
    Ok((ctx, attach))
}

/// A task of a hooked agent whose person typed a prompt and sent it; it is being answered (the
/// prompt emptied the box) and the full context has not refused it yet.
async fn refused_soon(
    app: &Arc<App>,
    data: &Path,
) -> Result<
    (
        TaskId,
        yulius_app::auth::AuthContext,
        yulius_app::terminal::TerminalAttach,
    ),
    Box<dyn std::error::Error>,
> {
    let id = started(app, data).await?;
    let (ctx, attach) = at_the_terminal(app, &id).await?;
    app.terminal_input(&ctx, &attach, b"too much", false)
        .await?;
    assert!(app.has_draft(&id), "typed, not sent yet");
    app.terminal_input(&ctx, &attach, b"\r", false).await?;
    wait_for("the prompt's submit", Duration::from_secs(30), || async {
        !app.has_draft(&id)
    })
    .await?;
    Ok((id, ctx, attach))
}

/// SPEC-483 TYP-001: the way a context usually fills. A person types a prompt and sends it, and
/// the full context refuses it: the prompt emptied the box, their keys are no draft, and Yulius
/// compacts the session.
#[test]
fn a_persons_own_refused_prompt_is_compacted() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, _) = served(&data, &fake, hooked(0), |_| {}).await?;
        let (id, _ctx, _attach) = refused_soon(&app, &data).await?;
        wait_for("the compaction", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("COMPACTED: Keep: task #") }
        })
        .await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-483 TYP-001: a person types while the turn runs and sends nothing; then the context is
/// full. Their text may sit in the prompt box, so `/compact` typed now would be sent with it:
/// Yulius holds the compaction, the task reads Needs your action with why and what to do, and
/// nothing is typed. When they type on, the session is theirs (the flag ends, Your turn); when
/// they send what they typed, the full context refuses it and that refusal is compacted.
#[test]
fn a_possible_draft_holds_the_compaction_and_its_person_is_told() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, _) = served(&data, &fake, hooked(1_500), |_| {}).await?;
        let (id, ctx, attach) = refused_soon(&app, &data).await?;
        let mut frames = app.runtime(&id).hub.watch();
        // The next instruction, begun while the turn runs.
        app.terminal_input(&ctx, &attach, b"and then", false)
            .await?;
        assert!(app.has_draft(&id));
        wait_for("the flag", Duration::from_secs(30), || {
            activity_is(&app, &id, Activity::NeedsAttention)
        })
        .await?;
        let why = crate::support::attention(&app, id.as_str()).await?;
        assert_eq!(
            why["problem"],
            "The session's context is full, and Yulius does not compact it yet: someone typed in \
             the terminal and sent no prompt since, so their text may still be in the prompt box."
        );
        assert_eq!(
            why["action"],
            "Send what you typed in the terminal, or clear it and send any prompt: Yulius then \
             compacts the session. In ten minutes it compacts it all the same, and that text is \
             sent with the command."
        );
        let mut said = Vec::new();
        notices(&mut frames, &mut said);
        assert!(
            said.iter().any(|n| n.contains("holds the compaction")),
            "{said:?}"
        );
        assert!(
            !screen(&app, &id).contains("COMPACT"),
            "nothing typed behind the draft"
        );
        // They type on: the session is theirs now, the flag ends and the task waits for them.
        app.terminal_input(&ctx, &attach, b" this", false).await?;
        heard(
            &mut frames,
            &mut said,
            &[&["leaves the full session to you"]],
        )
        .await;
        wait_for("Your turn", Duration::from_secs(30), || {
            activity_is(&app, &id, Activity::Waiting)
        })
        .await?;
        assert!(!screen(&app, &id).contains("COMPACT"));
        // They send it: the box is empty, the full context refuses the prompt, and that
        // refusal is compacted.
        app.terminal_input(&ctx, &attach, b"\r", false).await?;
        wait_for("the compaction", Duration::from_secs(60), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("COMPACTED: Keep: task #") }
        })
        .await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-483 TYP-001: nobody answers the flag (a goal task whose person pressed a key hours ago
/// and left). The hold is bounded: after it Yulius compacts the session all the same, and the
/// task works on instead of waiting for someone who is not there.
#[test]
fn a_held_compaction_is_typed_once_its_hold_is_over() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, _) = served(&data, &fake, hooked(1_500), |config| {
            config.compact_hold_ms = 3_000;
        })
        .await?;
        let (id, _ctx, _attach) = refused_soon(&app, &data).await?;
        // A key that leaves nothing in the box (an arrow, Esc, a dialog's answer): Yulius sees
        // keys, not the box, so it counts as a possible draft all the same.
        let person = task(&app, &id).await?.created_by;
        app.record_typing(&id, &person);
        assert!(app.has_draft(&id));
        wait_for("the flag", Duration::from_secs(30), || {
            activity_is(&app, &id, Activity::NeedsAttention)
        })
        .await?;
        assert!(!screen(&app, &id).contains("COMPACT"), "held first");
        wait_for("the compaction", Duration::from_secs(60), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("COMPACTED: Keep: task #") }
        })
        .await?;
        assert!(
            !activity_is(&app, &id, Activity::NeedsAttention).await,
            "the flag ended with the hold"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Records a key of the task's person when Yulius reaches `point` for the task.
fn a_key_at(
    app: &Arc<App>,
    id: &TaskId,
    person: &yulius_domain::ids::UserId,
    point: yulius_app::race::Point,
) {
    let (app, key, id, person) = (app.clone(), id.clone(), id.clone(), person.clone());
    yulius_app::race::on(point, Path::new(key.as_str()), move || {
        app.record_typing(&id, &person);
    });
}

/// SPEC-483 TYP-002: a person starts to type just before Yulius types `/compact` (the terminal
/// was quiet and nobody had typed; Yulius may wait for the typing mark there). Asked again
/// with the mark held, their key keeps the line out: the session is left to them.
#[test]
fn a_key_just_before_the_compaction_keeps_it_out() -> TestResult {
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
        let person = task(&app, &id).await?.created_by;
        let mut frames = app.runtime(&id).hub.watch();
        a_key_at(&app, &id, &person, yulius_app::race::Point::LineReady);
        app.send_input(&id, b"too much\r").await?;
        let mut said = Vec::new();
        heard(
            &mut frames,
            &mut said,
            &[&["leaves the full session to you"]],
        )
        .await;
        assert!(
            said.iter()
                .any(|n| n.contains("leaves the full session to you")),
            "{said:?}"
        );
        wait_for("Your turn", Duration::from_secs(10), || {
            activity_is(&app, &id, Activity::Waiting)
        })
        .await?;
        assert!(
            !screen(&app, &id).contains("COMPACT"),
            "Yulius typed nothing"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-483 TYP-001: a person types while Yulius compacts the session (here at the moment the
/// command is in the box; this agent runs no hooks, so nothing empties the box after it). The
/// line that wakes the compacted session is not typed behind what they typed: the session is
/// compacted, the task waits for them, and the agent is not told to carry on.
#[test]
fn a_draft_typed_during_the_compaction_keeps_the_wake_up_out() -> TestResult {
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
        let person = task(&app, &id).await?.created_by;
        a_key_at(&app, &id, &person, yulius_app::race::Point::LineTyped);
        app.send_input(&id, b"too much\r").await?;
        wait_for("the compaction", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("COMPACTED: Keep: task #") }
        })
        .await?;
        assert!(app.has_draft(&id));
        wait_for("Your turn", Duration::from_secs(30), || {
            activity_is(&app, &id, Activity::Waiting)
        })
        .await?;
        assert!(
            !screen(&app, &id).contains("Yulius: your context was full"),
            "no wake-up behind the draft"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-483 TYP-002: while Yulius types `/compact` (the line is in the prompt box, its Enter not
/// sent yet), nothing else is typed into the session: a message for the task is refused, as
/// while another message is being typed, and the command is submitted as it was typed.
#[test]
fn nothing_is_typed_between_a_line_of_yulius_and_its_enter() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        // A session without hooks counts as started half this time after its spawn: a message
        // for it is not refused for a session that still starts.
        let app = build_tuned(&data, &fake, agent(1, false), |config| {
            config.goal_ready_ms = 200;
        })?;
        let id = started(&app, &data).await?;
        let record = task(&app, &id).await?;
        let refused = Arc::new(std::sync::Mutex::new(None));
        {
            let (app, key, id) = (app.clone(), id.clone(), id.clone());
            let refused = refused.clone();
            let caller = yulius_app::coordination::ProjectCaller {
                sandbox: record.sandbox_id.clone(),
                user: record.created_by.clone(),
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
        app.send_input(&id, b"too much\r").await?;
        wait_for("the compaction", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("COMPACTED: Keep: task #") }
        })
        .await?;
        let answer = refused.lock().map_err(|_| "poisoned")?.clone();
        let answer = answer.ok_or("no message was tried while the line waited for its Enter")?;
        assert!(
            matches!(&answer, Err(why) if why.contains("is being typed")),
            "{answer:?}"
        );
        assert!(
            !screen(&app, &id).contains("one more thing"),
            "nothing of the message reached the session"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

async fn activity_is(app: &Arc<App>, id: &TaskId, activity: Activity) -> bool {
    task(app, id)
        .await
        .is_ok_and(|t| t.activity == Some(activity))
}

/// CTX-006: a compaction cut short (the person stopped the task and continued it) does not keep
/// the episode waiting: the continued session's full context is compacted in its turn.
#[test]
fn a_continued_session_is_compacted_after_an_unfinished_compaction() -> TestResult {
    use yulius_app::auth::AuthContext;
    use yulius_contracts::api::tasks::ContinueTask;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        // Full from the first message, so no answer ever ends the episode; each refusal takes a
        // while, like the API's, so the continued run's transcript watcher (it reads from where
        // the file ended when it opened) is attached by then.
        let options = FakeAgentOptions {
            compact_hangs: true,
            echo_turn_ms: Some(2_000),
            ..agent(0, false)
        };
        let app = build_with(&data, &fake, Arc::new(ScriptedUsage::default()), options)?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        let id = new_task(&app, &sandbox, &user, "Full").await?;
        wait_for("the compaction asked for", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("COMPACTING") }
        })
        .await?;
        stop(&app, &id).await?;
        let record = task(&app, &id).await?;
        let ctx = AuthContext {
            user: app
                .deps
                .store
                .user(&record.created_by)
                .await?
                .ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        app.continue_task(&ctx, id.as_str(), &ContinueTask { account_id: None })
            .await?;
        wait_for("the continued run", Duration::from_secs(30), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2 && attempts[1].state == AttemptState::Live
        })
        .await?;
        wait_for("the continued agent", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        // The continued session is just as full.
        app.send_input(&id, b"one\r").await?;
        wait_for("the second compaction", Duration::from_secs(30), || async {
            audited(&data, &id).is_ok_and(|a| a.len() == 2)
        })
        .await?;
        wait_for("the second /compact typed", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("COMPACTING") }
        })
        .await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// CTX-004/005: a fresh session that cannot start (here the repository is gone) leaves the full
/// session as it is, so the task needs a person, told why.
#[test]
fn a_fresh_session_that_cannot_start_needs_a_person() -> TestResult {
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
        let record = task(&app, &id).await?;
        let worktree = std::path::PathBuf::from(record.worktree_path.ok_or("worktree")?);
        let repo = data.join("sandboxes").join("s-pool").join("repo");
        std::fs::rename(worktree.join(".git"), worktree.join("git.away"))?;
        std::fs::rename(repo.join(".git"), repo.join("git.away"))?;
        let mut frames = app.runtime(&id).hub.watch();
        app.send_input(&id, b"too much\r").await?;
        wait_for("needs attention", Duration::from_secs(60), || {
            activity_is(&app, &id, Activity::NeedsAttention)
        })
        .await?;
        assert_eq!(app.deps.store.attempts(&id).await?.len(), 1, "no fresh run");
        let mut said = Vec::new();
        heard(
            &mut frames,
            &mut said,
            &[&["needs you", "fresh session could not start"]],
        )
        .await;
        assert!(
            said.iter()
                .any(|n| n.contains("needs you") && n.contains("fresh session could not start")),
            "the reason in the terminal: {said:?}"
        );
        assert!(
            !said.iter().any(|n| n.contains("Switching subscriptions")),
            "no word of subscriptions: {said:?}"
        );
        std::fs::rename(worktree.join("git.away"), worktree.join(".git"))?;
        std::fs::rename(repo.join("git.away"), repo.join(".git"))?;
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
        heard(
            &mut frames,
            &mut said,
            &[
                &["the compaction failed", "fresh session"],
                &["continues in a fresh session"],
            ],
        )
        .await;
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
            audit_when(&data, &id, |rows| rows.len() >= 2).await?,
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
        heard(&mut frames, &mut said, &[&["needs you", "still full"]]).await;
        assert!(
            said.iter()
                .any(|n| n.contains("needs you") && n.contains("still full")),
            "the reason in the terminal: {said:?}"
        );
        assert_eq!(
            audit_when(&data, &id, |rows| rows
                .last()
                .is_some_and(|r| r.starts_with("needs attention")))
            .await?
            .last()
            .map(String::as_str),
            Some("needs attention: the context is still full after compacting and a fresh session")
        );
        // The task says it to whoever opens it later (SPEC-472 ATN-004).
        assert_eq!(
            crate::support::attention(&app, id.as_str()).await?,
            serde_json::json!({
                "kind": "yulius",
                "problem": "The context is still full after compacting and a fresh session.",
                "action": "Find what fills it (a huge file, tool output or instructions), then \
                           type `/compact` or `/clear` in the terminal, or stop and continue the \
                           task.",
                "link": null,
            })
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
