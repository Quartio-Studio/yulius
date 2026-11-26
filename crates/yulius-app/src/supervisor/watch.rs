//! Watching a live run: attaching to it, the transcript watch loop and what each observation
//! changes (activity, usage, limits, the run's goal).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use yulius_domain::harness::Harness;
use yulius_domain::ids::{AccountId, TaskId};
use yulius_domain::task::Activity;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{ModelFamily, ReportedLimit};

use super::antigravity::antigravity_run_home;
use super::codex::CodexInput;
use super::stop::request_shutdown;
use super::typing::CLAUDE_CREDENTIAL_DIGEST;
use super::{ActiveRun, EndIntent, ObservationSource, is_could_not_end};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::finish::{SHELL_PROBE_MS, TranscriptFacts};
use crate::model::{AccountStatus, AttemptRecord, LaunchKind, TaskMode, TaskRecord};
use crate::ports::harness::{Observation, UsageSnapshot, WatchFrom};
use crate::ports::session::RunEvent;

/// Transcript poll interval.
pub const WATCH_INTERVAL_MS: u64 = 1_000;
/// How often a live run's steps are synced from its spec's Plan (SPEC-341 LSS-001).
const SPEC_SYNC_INTERVAL: Duration = Duration::from_secs(5);
/// Reattach attempts after an unexpected host disconnect.
pub const REATTACH_ATTEMPTS: u32 = 3;

/// Longest gap between live metrics saves of a run in progress (#31).
const LIVE_METRICS_MS: i64 = 30_000;

/// Whether `observation` is a workflow agent's limit of another model family than `family`, the
/// one the session's run was launched with (SPEC-486 WFL-002): that agent's alone.
fn binds_another_model(observation: &Observation, family: ModelFamily) -> bool {
    matches!(
        observation,
        Observation::WorkflowLimitReached {
            scope: yulius_domain::usage::LimitScope::Model(Some(other)),
            ..
        } if *other != family
    )
}

/// The limit a workflow agent's limit observation reports.
fn reported_limit(observation: &Observation) -> Option<ReportedLimit> {
    match observation {
        Observation::WorkflowLimitReached {
            resets_at,
            window_minutes,
            scope,
            ..
        } => Some(ReportedLimit {
            scope: *scope,
            window_minutes: *window_minutes,
            resets_at: *resets_at,
        }),
        _ => None,
    }
}

/// Whether `observation` shows the model answered in the session: a record of its own work or of
/// its turn's end, not one Claude Code writes itself for a refusal, and not a sub-agent's turn
/// end (which carries no time).
fn model_answered(observation: &Observation) -> bool {
    matches!(
        observation,
        Observation::AgentWork { .. } | Observation::TurnComplete { at: Some(_) }
    )
}

impl App {
    /// Attaches to a run's host, resets the hub and starts the output pump and transcript watcher.
    pub(super) async fn attach_run(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt: &AttemptRecord,
        host_token: &[u8; 32],
    ) -> AppResult<()> {
        let runtime = self.runtime(task_id);
        let sandbox = self
            .deps
            .store
            .task(task_id)
            .await?
            .ok_or(AppError::NotFound)?
            .sandbox_id;
        // The terminal people see is redacted of agent secrets (SPEC-002 ADR-20).
        let scrub = crate::vault::TerminalScrub::default();
        if let Ok(mut shown) = runtime.scrub.lock() {
            *shown = scrub.clone();
        }
        runtime.screen_flagged.store(false, Ordering::SeqCst);
        let attached = self
            .deps
            .session_host
            .attach(Path::new(&attempt.run_dir), host_token)
            .await?;
        let welcome = attached.welcome.clone();
        let screen = self
            .terminal_snapshot(
                &scrub,
                &sandbox,
                welcome.snapshot.as_deref().unwrap_or_default(),
            )
            .await;
        // The run takes input before anything of it shows: a viewer who sees the agent's screen
        // can type into it (F 2026-10-13: input right after the banner was refused as "not
        // running" on a fast machine).
        let codex = attempt.harness == Harness::Codex;
        let codex_input = codex.then(|| CodexInput::load(&attempt.run_dir));
        let repaint_size = codex_input.as_ref().and_then(|input| input.repaint_size);
        *runtime.run.lock().await = Some(ActiveRun {
            attempt_id: attempt.id.clone(),
            codex_input,
            writer: attached.writer,
        });
        runtime.retain_park_for_attached_run(&attempt.id);
        crate::race::reached(crate::race::Point::RunAttached, Path::new(task_id.as_str()));
        // What the sandbox's limits counted so far: a rise by the run's end says it was at one.
        let counters = self.limit_counters(&sandbox).await;
        runtime.set_start_counters(&attempt.id, counters);
        // A stop or close that came while the run was launching found no run to ask: it asks now,
        // so the run ends as asked instead of at the escalation (SPEC-140 STP-001, #464). One that
        // comes later finds the run itself; the host takes a second request as the first.
        let ending = runtime.intent.lock().is_ok_and(|intent| {
            matches!(
                *intent,
                Some(EndIntent::Cancel | EndIntent::Close | EndIntent::Park)
            )
        });
        if ending {
            // The stop may have come before this run existed, so it armed no escalation for it.
            self.arm_escalation(task_id, Some(attempt.id.clone()));
            self.end_person_pause(task_id).await;
            request_shutdown(&runtime, None).await;
        }
        // A viewer who took the size while the run was starting (the PTY spawned at the size before
        // it): the screen shows that viewer's size from the start, never the spawn size first,
        // and the PTY follows (TSK-006, F 2026-10-10).
        let (cols, rows) = runtime
            .hub
            .controller_size()
            .or(repaint_size)
            .unwrap_or((welcome.cols, welcome.rows));
        // The hub shows this run from now on (SPEC-301 LRS-001).
        runtime.show_screen_of(&attempt.id, |hub| {
            hub.restore(
                welcome.seq,
                &screen,
                cols,
                rows,
                crate::hub::ScreenUpdate::Host {
                    rendered: welcome.rendered_at((cols, rows)),
                },
            );
        });
        if ((cols, rows) != (welcome.cols, welcome.rows)
            || repaint_size.is_some_and(|size| size != (cols, rows)))
            && let Err(err) = self.resize_run(task_id, cols, rows).await
        {
            tracing::debug!(task = %task_id, error = %err, "viewer size after attach failed");
        }
        let app = self.clone();
        let tid = task_id.clone();
        let aid = attempt.id.clone();
        let token = *host_token;
        let mut reader = attached.reader;
        tokio::spawn(async move {
            let mut reattaches = 0;
            loop {
                match reader.next().await {
                    Ok(Some(RunEvent::Output { seq, bytes })) => {
                        app.terminal_output(&scrub, (&tid, &aid), &sandbox, seq, &bytes)
                            .await;
                    }
                    Ok(Some(RunEvent::Snapshot { seq, bytes })) => {
                        let (cols, rows) = app.runtime(&tid).hub.size();
                        let screen = app.terminal_snapshot(&scrub, &sandbox, &bytes).await;
                        app.runtime(&tid)
                            .on_screen_of(&aid, |hub| hub.reset(seq, &screen, cols, rows));
                    }
                    Ok(Some(RunEvent::Exit(exit))) => {
                        crate::race::reached(
                            crate::race::Point::HostExitReceived,
                            Path::new(aid.as_str()),
                        );
                        app.finalize_attempt(&tid, &aid, exit).await;
                        crate::race::reached(
                            crate::race::Point::HostExitFinalized,
                            Path::new(aid.as_str()),
                        );
                        return;
                    }
                    Ok(None) | Err(_) => {
                        // Connection lost without an exit: reattach (the host may have dropped a slow client).
                        if reattaches < REATTACH_ATTEMPTS {
                            reattaches += 1;
                            tokio::time::sleep(Duration::from_millis(200 * u64::from(reattaches)))
                                .await;
                            if let Ok(Some(run_dir)) = app
                                .deps
                                .store
                                .attempt(&aid)
                                .await
                                .map(|a| a.map(|a| a.run_dir))
                                && let Ok(mut again) = app
                                    .deps
                                    .session_host
                                    .attach(Path::new(&run_dir), &token)
                                    .await
                            {
                                {
                                    let screen = app
                                        .terminal_snapshot(
                                            &scrub,
                                            &sandbox,
                                            again.welcome.snapshot.as_deref().unwrap_or_default(),
                                        )
                                        .await;
                                    let rt = app.runtime(&tid);
                                    let mut run = rt.run.lock().await;
                                    // The host may have disconnected at the temporary
                                    // repaint width. The writer lock covers the whole probe,
                                    // so the hub now holds its restored (or latest viewer) size.
                                    let host_size = (again.welcome.cols, again.welcome.rows);
                                    let size = if codex { rt.hub.size() } else { host_size };
                                    if size != host_size
                                        && let Err(err) = again.writer.resize(size.0, size.1).await
                                    {
                                        tracing::warn!(attempt = %aid, error = %err, "could not restore PTY size on reconnect");
                                        continue;
                                    }
                                    let codex_input = run
                                        .as_mut()
                                        .filter(|run| run.attempt_id == aid)
                                        .and_then(|run| run.codex_input.take())
                                        .or_else(|| codex.then(|| CodexInput::load(&run_dir)));
                                    *run = Some(ActiveRun {
                                        attempt_id: aid.clone(),
                                        codex_input,
                                        writer: again.writer,
                                    });
                                    rt.on_screen_of(&aid, |hub| {
                                        hub.restore(
                                            again.welcome.seq,
                                            &screen,
                                            size.0,
                                            size.1,
                                            crate::hub::ScreenUpdate::Host {
                                                rendered: again.welcome.rendered_at(size),
                                            },
                                        );
                                    });
                                    reader = again.reader;
                                    continue;
                                }
                            }
                            continue;
                        }
                        app.resolve_lost_connection(&tid, &aid).await;
                        return;
                    }
                }
            }
        });
        self.start_watcher(task_id, attempt);
        Ok(())
    }

    /// Writes a running task's `activity`, as its session's work or turns decide it, unless the
    /// task reads the flag its live run raised when its host could not be terminated ("Yulius
    /// could not end this session"): no work of the session ends that cause, so the flag stays
    /// until a person stops the task or the run ends (SPEC-635 EFL-001). Once something else
    /// replaced that flag (a pause, another matter's), the session's activity rules again
    /// (EFL-002). Whether it wrote.
    ///
    /// # Errors
    /// The store's.
    pub(crate) async fn write_session_activity(
        &self,
        task_id: &TaskId,
        activity: Activity,
    ) -> AppResult<bool> {
        let runtime = self.runtime(task_id);
        let _gate = runtime.end_gate.lock().await;
        let failed = runtime
            .end_failed
            .lock()
            .ok()
            .and_then(|failed| failed.clone());
        if let Some((run, _)) = failed {
            let store = &self.deps.store;
            if store
                .live_attempt(task_id)
                .await?
                .is_none_or(|live| live.id != run)
            {
                // That run is over: its failure holds nothing any more.
                if let Ok(mut failed) = runtime.end_failed.lock()
                    && failed.as_ref().is_some_and(|(id, _)| *id == run)
                {
                    *failed = None;
                }
            } else {
                let held = runtime.end_flag_pending.load(Ordering::SeqCst) > 0
                    || store
                        .task_attention(task_id)
                        .await?
                        .is_some_and(|(reason, _)| is_could_not_end(&reason));
                crate::race::reached(
                    crate::race::Point::SessionActivityChecked,
                    Path::new(task_id.as_str()),
                );
                if held {
                    tracing::info!(task = %task_id, ?activity, "the run cannot be ended: its flag stays");
                    return Ok(false);
                }
            }
        }
        self.deps
            .store
            .set_activity(task_id, Some(activity), self.now())
            .await?;
        Ok(true)
    }

    /// Marks a refused sign-in while it is still current. The label is for the interrupted
    /// task's move notice, including when a newer sign-in is already available.
    async fn record_access_lost(
        &self,
        account: &AccountId,
        run_dir: &Path,
        message: &str,
    ) -> Option<String> {
        crate::race::reached(
            crate::race::Point::AccessLostRecord,
            Path::new(account.as_str()),
        );
        let launched =
            crate::leftovers::read_small_file(&run_dir.join(CLAUDE_CREDENTIAL_DIGEST), 64);
        // Claude sign-in takes this lock too. Keep it from the credential check through the
        // status write, which must also follow any usage write already under way (#1211).
        let _writing = self.capacity.usage_writes.lock().await;
        let record = match self.deps.store.account(account).await {
            Ok(account) => account?,
            Err(err) => {
                tracing::error!(error = %err, "could not read a refused subscription");
                return None;
            }
        };
        if let (Some(launched), Some(current)) = (launched, &record.credential)
            && launched == self.deps.crypto.sha256_hex(&current.0)
        {
            if let Err(err) = self
                .deps
                .store
                .set_account_state(
                    account,
                    AccountStatus::Relogin,
                    None,
                    Some(message),
                    self.now(),
                )
                .await
            {
                tracing::error!(error = %err, "could not mark a refused subscription");
            }
            self.capacity_changed();
        }
        Some(record.label)
    }

    /// Install the conversation's goal before a resumed process can send its first Stop.
    /// Earlier goal records describe the condition, never a verdict on the new turn.
    pub(super) async fn initialize_run_goal(
        &self,
        task: &TaskRecord,
        attempt: &AttemptRecord,
        unfinished: bool,
    ) {
        let mut goal = None;
        if task.mode == TaskMode::Prompt
            && attempt.harness == Harness::ClaudeCode
            && attempt.launch_kind == LaunchKind::Resume
            && let (Some(session), Some(worktree)) =
                (&attempt.provider_session_id, &task.worktree_path)
            && yulius_domain::harness::valid_session_id(session)
        {
            let harness = Arc::clone(&self.deps.harness);
            let path = harness.claude_transcript(
                &self.sandbox_home(&task.sandbox_id),
                Path::new(worktree),
                session,
            );
            let anchor = self.sandbox_dir(&task.sandbox_id);
            match tokio::task::spawn_blocking(move || harness.last_claude_goal(&anchor, &path))
                .await
            {
                Ok(Ok(outcome)) => goal = outcome,
                Ok(Err(err)) => {
                    tracing::warn!(task = %task.id, error = %err, "could not read the resumed goal")
                }
                Err(err) => {
                    tracing::warn!(task = %task.id, error = %err, "resumed goal read did not finish")
                }
            }
        }
        let runtime = self.runtime(&task.id);
        // A verdict either commits before the new run installs its state, or sees the new owner.
        let _order = runtime.turn_order.lock().await;
        runtime
            .replace_goal_unfinished(goal.map_or(unfinished, crate::finish::goal_unfinished))
            .await;
        runtime.goal_open.store(
            goal.is_some_and(crate::finish::goal_stays_open),
            Ordering::SeqCst,
        );
        runtime.goal_achieved.store(false, Ordering::SeqCst);
        runtime.goal_submitted.store(false, Ordering::SeqCst);
        runtime.verdict_stop_ms.clear();
    }

    fn start_watcher(self: &Arc<Self>, task_id: &TaskId, attempt: &AttemptRecord) {
        let runtime = self.runtime(task_id);
        runtime.watcher_alive.store(true, Ordering::SeqCst);
        runtime.hooks_seen.store(false, Ordering::SeqCst);
        runtime.at_limit.store(false, Ordering::SeqCst);
        // Unmarked before it is emptied: a run is never taken as read with an empty snapshot.
        if let Ok(mut read) = runtime.usage_read.lock() {
            *read = None;
        }
        if let Ok(mut u) = runtime.usage.lock() {
            *u = UsageSnapshot::default();
        }
        runtime.turn_started_ms.store(0, Ordering::SeqCst);
        runtime.background_shells.store(-1, Ordering::SeqCst);
        runtime.background_waits.store(-1, Ordering::SeqCst);
        // `active_waits` is RAII-balanced by `ActiveWaitGuard` per live `/internal/agent/wait`
        // handler; resetting it here while an old attempt's long-poll is still dropping could
        // desynchronize or underflow the counter (SPEC-580 review F1).
        runtime.background_agents.store(0, Ordering::SeqCst);
        runtime.notice_attention_ms.store(0, Ordering::SeqCst);
        runtime.turn_record_ms.store(0, Ordering::SeqCst);
        runtime.resumed_turn.store(false, Ordering::SeqCst);
        // A retry armed in the old session is that process's: a new one does not make it.
        runtime.goal_retry_armed.store(false, Ordering::SeqCst);
        runtime.stop_answered_ms.store(0, Ordering::SeqCst);
        runtime.turn_began_ms.store(0, Ordering::SeqCst);
        runtime.turn_record_at_ms.store(0, Ordering::SeqCst);
        runtime.turn_closed_ms.store(0, Ordering::SeqCst);
        runtime.turn_moved_ms.store(0, Ordering::SeqCst);
        // A new session runs none of the old one's monitors or agents.
        if let Ok(mut launches) = runtime.launches.lock() {
            launches.clear();
        }
        if let Ok(mut end) = runtime.session_end.lock() {
            *end = None;
        }
        runtime.hold_polled_ms.store(0, Ordering::SeqCst);
        // A new session has its own bridge, heard of afresh (SPEC-432 review round 1).
        runtime.mcp_seen_ms.store(0, Ordering::SeqCst);
        runtime.mcp_pinged_ms.store(0, Ordering::SeqCst);
        if let Ok(mut reconnects) = runtime.mcp_reconnects.lock() {
            *reconnects = crate::mcp_watch::Reconnects::default();
        }
        let app = self.clone();
        let tid = task_id.clone();
        let attempt = attempt.clone();
        let alive = runtime.watcher_alive.clone();
        let generation = runtime.watcher_generation.fetch_add(1, Ordering::SeqCst) + 1;
        tokio::spawn(async move { app.watch_loop(tid, attempt, alive, generation).await });
    }

    async fn watch_loop(
        self: Arc<Self>,
        task_id: TaskId,
        attempt: AttemptRecord,
        alive: Arc<AtomicBool>,
        generation: u64,
    ) {
        let Ok(Some(task)) = self.deps.store.task(&task_id).await else {
            return;
        };
        let Ok(Some(sandbox)) = self.deps.store.sandbox(&task.sandbox_id).await else {
            return;
        };
        let Some(worktree) = task.worktree_path.clone().map(PathBuf::from) else {
            return;
        };
        // A request for a goal pause that is still open (made before this server started) is
        // answered by this run's work, like one made while it is watched (SPEC-469 GPW-004).
        self.watch_goal_pause_request(&task_id).await;
        // So is a flag that the session's work answers (a notice's), raised before this server
        // started: its reason says so (SPEC-472 ATN-010).
        self.watch_answerable_flag(&task_id).await;
        // The launched model stays the same for this attempt. For older run records, the
        // task already read for this watch supplies the model. Polling needs no store read.
        let family = crate::capacity::run_family(
            attempt.model.as_deref(),
            crate::capacity::task_model(&task),
        );
        let mut session_id = attempt.provider_session_id.clone();
        let mut watch = None;
        let mut last_drift_check = Timestamp(0);
        let mut spec_synced = std::time::Instant::now();
        let mut saved: Option<UsageSnapshot> = None;
        let mut saved_at = Timestamp(0);
        // The Claude Code version the run's lines name, as last recorded (SPEC-461 CCM-011).
        let mut cli_version: Option<String> = None;
        // After a restart, the transcript the reattached run already wrote is history until the
        // first poll reaches its end: it is reduced to its final state, not replayed event by
        // event (TSK-014; a replayed prompt used to put a finished task back to Working after
        // every restart). A run started by this server is watched live from its first line.
        let mut replay = self
            .runtime(&task_id)
            .replay_transcript
            .swap(false, Ordering::SeqCst);
        let mut catching_up = false;
        let mut facts = TranscriptFacts::default();
        // The replay's last usage limit, acted on once caught up if no turn came after it.
        let mut replayed_limit = None;
        let mut probed_at = Timestamp(0);
        // `existing_runtime`: a deleted task's runtime is not brought back.
        let current = || {
            alive.load(Ordering::SeqCst)
                && self.existing_runtime(&task_id).is_some_and(|runtime| {
                    runtime.watcher_generation.load(Ordering::SeqCst) == generation
                })
        };
        while current() {
            tokio::time::sleep(Duration::from_millis(WATCH_INTERVAL_MS)).await;
            if !current() {
                break;
            }
            // A round begins: once the next one begins, every line written before this one was
            // read and applied (a Stop hook deciding a switch waits for that, BGW-004).
            self.runtime(&task_id)
                .watch_polls
                .fetch_add(1, Ordering::SeqCst);
            let now = self.now();
            // Claude session-id drift (/clear, /resume inside the TUI): the TUI's SessionStart
            // hook names its new session on the attempt; adopt it into the lineage. Never the
            // newest transcript of the worktree: a `claude` the agent runs there writes one too
            // (SPEC-281 SSN-002).
            // Codex names its session itself: its SessionStart hook names it the same way, and
            // without hooks the rollout started for the worktree since the run began does
            // (SPEC-503 CDX-003).
            if now.millis_since(last_drift_check) > 5_000 {
                last_drift_check = now;
                let mut named = self
                    .deps
                    .store
                    .attempt(&attempt.id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|named| named.provider_session_id);
                // The rollouts are read only while no hook has named the session yet.
                if named.is_none()
                    && session_id.is_none()
                    && attempt.harness == Harness::Codex
                    && let Ok(Some((id, _))) = self.deps.harness.discover_codex_session(
                        &self.codex_sessions(&sandbox.id),
                        &worktree,
                        attempt.started_at,
                    )
                {
                    let _ = self.deps.store.set_attempt_session(&attempt.id, &id).await;
                    named = Some(id);
                }
                // An Antigravity run whose SessionStart named nothing: its CLI's last
                // conversation of the worktree, written since the run began (SPEC-505 AGY-005).
                if named.is_none()
                    && session_id.is_none()
                    && attempt.harness == Harness::Antigravity
                    && let Some(id) = self.deps.harness.discover_antigravity_session(
                        &antigravity_run_home(Path::new(&attempt.run_dir)).join(".gemini"),
                        &self.antigravity_brain(&sandbox.id),
                        &worktree,
                        attempt.started_at,
                    )
                {
                    let _ = self.deps.store.set_attempt_session(&attempt.id, &id).await;
                    named = Some(id);
                }
                if let Some(id) = named
                    && session_id.as_deref() != Some(id.as_str())
                {
                    let _ = self
                        .record_lineage(&task, attempt.harness, &id, "native")
                        .await;
                    // `/clear` or `/resume` in the TUI: a person took the session in hand.
                    if session_id.is_some() {
                        self.context_session_changed(&task_id);
                    }
                    session_id = Some(id);
                    watch = None;
                }
            }
            if watch.is_none()
                && let Some(id) = &session_id
                && let path = self.transcript_path(&sandbox, &worktree, attempt.harness, id)
                && path.exists()
            {
                // A resumed session's transcript holds its earlier runs' lines: the run reads its
                // own, from its start, whether it is watched from its launch or reattached after a
                // restart (SPEC-087 MET-002). Reading from the end when the watch opened lost the
                // lines written before it, and after a restart all of the run's figures.
                let from = match attempt.launch_kind {
                    LaunchKind::New => WatchFrom::Start,
                    LaunchKind::Resume => WatchFrom::Since(attempt.started_at),
                };
                watch = self.deps.harness.watch(attempt.harness, &path, from).ok();
                catching_up = replay && watch.is_some();
                replay &= watch.is_none();
                facts = TranscriptFacts::default();
                replayed_limit = None;
            }
            // The steps follow the spec's Plan while a turn runs, not only at its end (SPEC-341
            // LSS-001): a long goal turn that grew its Plan showed the old list's progress.
            if spec_synced.elapsed() >= SPEC_SYNC_INTERVAL {
                spec_synced = std::time::Instant::now();
                if let Ok(Some(fresh)) = self.deps.store.task(&task_id).await
                    && let Err(err) = self.sync_steps_from_spec(&fresh).await
                {
                    tracing::debug!(error = %err, task = %task_id, "step sync from the spec failed");
                }
            }
            // Background shells of an idle task, at most every SHELL_PROBE_MS (TSK-014).
            if now.millis_since(probed_at) >= SHELL_PROBE_MS {
                probed_at = now;
                self.probe_idle_shells(&task_id).await;
            }
            let Some(w) = watch.as_mut() else { continue };
            let observations = match w.poll() {
                Ok(o) => o,
                Err(err) => {
                    tracing::debug!(error = %err, "transcript poll failed");
                    continue;
                }
            };
            // A line's time is stamped on the host's wall clock: read on Yulius's clock, so a
            // test clock that runs shifted orders it with the hooks (SPEC-636).
            let observations: Vec<Observation> = observations
                .into_iter()
                .map(|observation| observation.with_record_times(|at| self.deps.clock.of_wall(at)))
                .collect();
            // A workflow agent's limit of another model than the session runs is not the
            // session's (SPEC-486 WFL-002): it goes into the subscription's reading when it is
            // read live, and never into what is known of the session.
            let (other_models, observations): (Vec<_>, Vec<_>) = observations
                .into_iter()
                .partition(|observation| binds_another_model(observation, family));
            for limit in other_models {
                if let (false, Some(account), Some(limit)) =
                    (catching_up, &attempt.account_id, reported_limit(&limit))
                {
                    tracing::info!(task = %task_id, ?limit, "a workflow agent's model is at its limit");
                    self.record_limit(account, limit, family).await;
                }
            }
            for observation in &observations {
                facts.fold(observation);
            }
            let usage = w.usage();
            let runtime = self.runtime(&task_id);
            if let Ok(mut u) = runtime.usage.lock() {
                *u = usage.clone();
            }
            // Which Claude Code runs this session is kept as soon as a line names it: the task's
            // info shows it, and the turn's end moves a session on an older one (SPEC-461).
            if usage.cli_version.is_some() && usage.cli_version != cli_version {
                cli_version.clone_from(&usage.cli_version);
                if let Some(version) = &cli_version
                    && let Err(err) = self
                        .deps
                        .store
                        .set_run_cli_version(&attempt.id, &task_id, version, self.now())
                        .await
                {
                    tracing::debug!(error = %err, task = %task_id, "the run's Claude Code version was not recorded");
                }
            }
            if w.caught_up()
                && let Ok(mut read) = runtime.usage_read.lock()
            {
                *read = Some(attempt.id.clone());
            }
            let turn_ended = observations
                .iter()
                .any(|o| matches!(o, Observation::TurnComplete { .. }));
            if catching_up {
                // What a review round's reviewer ran on, whether a limit cut it off and its report
                // are facts to note however late (SPEC-493 REV-004): noting one again changes
                // nothing. A report written while Yulius was down is only here (SPEC-633 RRE-002).
                for observation in &observations {
                    if let Observation::SubagentAnswered { .. }
                    | Observation::SubagentCutOff { .. }
                    | Observation::SubagentReported { .. } = observation
                    {
                        self.on_observation(
                            &task_id,
                            &attempt,
                            observation.clone(),
                            ObservationSource::Replay,
                        )
                        .await;
                    }
                }
                // Turns, the goal and a usage limit are applied once, as the replay's final
                // state: a limit that a later turn overtook (Claude Code continued by itself) is
                // history (SPEC-072 LRS-001).
                if let Some(limit) = observations.into_iter().rfind(|o| {
                    matches!(
                        o,
                        Observation::LimitReached { .. }
                            | Observation::WorkflowLimitReached { .. }
                            | Observation::AccessLost { .. }
                    )
                }) {
                    replayed_limit = Some(limit);
                }
                if w.caught_up() {
                    catching_up = false;
                    if let Some(limit) = replayed_limit.take()
                        && facts.limit_pending
                    {
                        self.on_observation(&task_id, &attempt, limit, ObservationSource::Replay)
                            .await;
                        // The goal pause Claude Code wrote at that limit is the limit's too.
                        if let Some(notice) = &facts.limit_goal_pause {
                            self.goal_retry_observed(&task_id, false, notice).await;
                        }
                    } else if !facts.limit_pending {
                        // Claude Code continued past its limit while Yulius was down.
                        self.cut_turn_continued(&task_id, facts.last_turn_at).await;
                    }
                    self.apply_transcript_facts(&task_id, facts.clone()).await;
                    // A session left full while Yulius was down is recovered now (CTX-007).
                    if facts.context_full {
                        self.context_full_replayed(&task_id, &attempt).await;
                    }
                }
            } else {
                // A limit that cut off a subagent of the session's thread is the session's news
                // as a workflow agent's is (SPEC-493 RLM-001), read live only, and after the
                // thread's own lines of this read: a turn they start is under way by then, so a
                // move for a reviewer waits for its end (review round 1 of fix 7).
                let cut_offs: Vec<Observation> = observations
                    .iter()
                    .filter(|o| matches!(o, Observation::SubagentCutOff { .. }))
                    .cloned()
                    .collect();
                for observation in observations {
                    self.on_observation(&task_id, &attempt, observation, ObservationSource::Live)
                        .await;
                }
                for observation in &cut_offs {
                    self.subagent_limit(&task_id, &attempt, family, observation)
                        .await;
                }
            }
            // Live metrics (#31): the dashboard counts a run while it goes, saved after each turn
            // and at most every 30 s while something changes (a turn in progress grows its
            // active time with each line).
            let now = self.now();
            if saved.as_ref() != Some(&usage)
                && (turn_ended || now.millis_since(saved_at) >= LIVE_METRICS_MS)
            {
                self.save_metrics(&task_id, &attempt.id, &runtime).await;
                saved = Some(usage);
                saved_at = now;
            }
        }
    }

    async fn on_observation(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt: &AttemptRecord,
        observation: Observation,
        source: ObservationSource,
    ) {
        let runtime = self.runtime(task_id);
        // The model answered: a later limit nothing says the end of is no repeat of an earlier
        // one, and has its own retry (SPEC-464 UCL-010).
        if model_answered(&observation)
            && let Ok(mut blind) = runtime.blind_limits.lock()
        {
            blind.answered();
        }
        // A line of the model written after a hand-over is of the turn the hand-over started,
        // which a prompt Claude Code scheduled for itself begins with no prompt record read
        // here (SPEC-471 CCQ-003). Said to the queue once the line was applied below, as a
        // prompt record's turn is: such a prompt has had its hook by then, so the task reads
        // Working either way.
        let worked = match &observation {
            Observation::AgentWork { at } | Observation::TurnComplete { at } => *at,
            _ => None,
        };
        // A workflow launched, ended, or met by a limit while unfinished (SPEC-486 WFL-003).
        runtime.with_workflows(|workflows| workflows.observe(&observation));
        match observation {
            Observation::LimitReached {
                resets_at,
                message,
                window_minutes,
                scope,
                at,
            } => {
                tracing::info!(task = %task_id, %message, ?scope, "usage limit reached");
                let limit = ReportedLimit {
                    scope,
                    window_minutes,
                    resets_at,
                };
                self.limit_met(task_id, attempt, limit, at, source).await;
            }
            Observation::WorkflowLimitReached {
                resets_at,
                message,
                window_minutes,
                scope,
                at,
            } => {
                // One that binds the model the session runs (the watcher keeps the others to
                // the subscription's reading): the session cannot go on here either, whatever
                // its own thread is doing (its next request would be refused).
                tracing::info!(task = %task_id, %message, ?scope, "usage limit reached by a workflow's agent");
                let limit = ReportedLimit {
                    scope,
                    window_minutes,
                    resets_at,
                };
                self.limit_met(task_id, attempt, limit, at, source).await;
            }
            Observation::AccessLost { message, at } => {
                // The refused turn moves on like a limit's, including after another sign-in.
                tracing::warn!(task = %task_id, %message, "subscription access refused");
                runtime.end_turn();
                runtime.limit_cut_the_turn(&attempt.id, at, None);
                let refused = match &attempt.account_id {
                    Some(account) => {
                        self.record_access_lost(account, Path::new(&attempt.run_dir), &message)
                            .await
                    }
                    None => None,
                };
                let app = self.clone();
                let tid = task_id.clone();
                let account = attempt.account_id.clone();
                let provider = attempt.harness.provider();
                let attempt_id = attempt.id.clone();
                tokio::spawn(async move {
                    app.rotate_limited_attempt(&tid, &attempt_id, account, false, None, source)
                        .await;
                    // Said in one line, with a Needs you when nothing is left (SPEC-447 MOV-006).
                    if let Some(label) = refused {
                        let left = crate::moves::Left::Refused {
                            provider,
                            label,
                            message,
                        };
                        app.tell_left(&tid, &left).await;
                    }
                });
            }
            Observation::UserTurn { at, wake } => {
                crate::race::reached(
                    crate::race::Point::UserTurnRead,
                    Path::new(task_id.as_str()),
                );
                if let Some(at) = at {
                    runtime.turn_moved_ms.fetch_max(at.0, Ordering::SeqCst);
                }
                // The retry's prompt (or a person's) starts the turn an armed retry waited for.
                runtime.goal_retry_armed.store(false, Ordering::SeqCst);
                // A new prompt: an achieved goal no longer makes the task done (TSK-014). Claude
                // Code's own wake-ups (background work ended) keep it, as the prompt hook does.
                if !wake {
                    runtime.goal_achieved.store(false, Ordering::SeqCst);
                }
                // A prompt of a turn whose clean Stop already came is not a new turn. Held until
                // Working is set, so a clean Stop arriving meanwhile opens its verdict wait, or
                // settles, after this prompt started its turn (SPEC-453 TPO-001).
                let _order = runtime.turn_order.lock().await;
                if at.is_some_and(|at| at.0 <= runtime.last_stop_ms.load(Ordering::SeqCst)) {
                    // Its turn came and went: what Claude Code handed on had it (SPEC-471).
                    runtime.with_queue(yulius_domain::agent_cli::Queue::turn_seen);
                    return;
                }
                crate::race::reached(
                    crate::race::Point::UserTurnChecked,
                    Path::new(task_id.as_str()),
                );
                // A prompt reached the agent while its turn or prompt was held for capacity:
                // the CLI gave the hold up (an interrupt, its own hook timeout), so the task
                // runs again (F 2026-10-10: the task follows its session).
                self.hold_overtaken(task_id, at, Activity::Working).await;
                runtime.at_limit.store(false, Ordering::SeqCst);
                runtime.background_agents.store(0, Ordering::SeqCst);
                runtime.notice_attention_ms.store(0, Ordering::SeqCst);
                runtime.resumed_turn.store(false, Ordering::SeqCst);
                // A new turn: the one that waited for its verdict goes on (BGW-005).
                runtime.verdict_stop_ms.clear();
                let now = self.now();
                runtime
                    .turn_began_ms
                    .store(at.map_or(now.0, |at| at.0), Ordering::SeqCst);
                runtime.start_turn(now);
                // Claude Code's own wake-up goes on with the stretch (SPEC-309 WSW-001), also when
                // its prompt hook never reached Yulius.
                let stretch = if wake {
                    self.continue_working_stretch(task_id).await
                } else {
                    None
                };
                let _ = self
                    .write_session_activity(task_id, Activity::Working)
                    .await;
                // When this turn began after the pull request watch typed, it is that news'
                // turn (SPEC-487 CHN-002); a prompt read late is another turn's.
                self.pr_news_turn_began(task_id, at.map(|at| at.0));
                self.publish_task(task_id).await;
                self.keep_working_stretch(task_id, stretch).await;
                // Only now, with the task reading Working: the turn of what Claude Code handed
                // on from its queue is known, so the hand-over is over (SPEC-471 CCQ-003).
                runtime.with_queue(yulius_domain::agent_cli::Queue::turn_seen);
            }
            Observation::TurnComplete { at } => {
                if let Some(at) = at {
                    runtime.turn_closed_ms.fetch_max(at.0, Ordering::SeqCst);
                    runtime.turn_moved_ms.fetch_max(at.0, Ordering::SeqCst);
                }
                self.context_replied(task_id, &attempt.id);
                // A continuation that is only the answer ending the turn has no agent work.
                self.cut_turn_continued(task_id, at).await;
                runtime.end_turn();
                // A sub-agent's turn end (no time) leaves the session at its limit.
                if at.is_some() {
                    runtime.at_limit.store(false, Ordering::SeqCst);
                    // The session answered: a goal pause before it asks nobody any more.
                    self.goal_pause_overtaken(task_id).await;
                }
                // With hooks the Stop hook is the boundary that switches or pauses; the
                // transcript decides only for a run without them.
                if !runtime.hooks_seen.load(Ordering::SeqCst) {
                    self.turn_ended(task_id, attempt.account_id.clone());
                }
            }
            Observation::TurnInterrupted { at } => {
                // Esc or Ctrl+C stopped the turn: no Stop hook runs, the agent waits for input.
                runtime.goal_retry_armed.store(false, Ordering::SeqCst);
                runtime.end_turn();
                self.turn_interrupted(task_id, at).await;
            }
            Observation::Goal(outcome, at) => {
                self.goal_observed(task_id, &attempt.id, outcome, at).await;
                crate::race::reached(
                    crate::race::Point::NativeVerdictChecked,
                    Path::new(attempt.id.as_str()),
                );
            }
            Observation::TurnEnded {
                background_agents,
                at,
            } => {
                if let Some(at) = at {
                    runtime.turn_closed_ms.fetch_max(at.0, Ordering::SeqCst);
                }
                // What still waits in Claude Code's queue was left by this turn's end: it is
                // not handed on at a later one (SPEC-479 SWQ-002).
                runtime.with_queue(yulius_domain::agent_cli::Queue::turn_closed);
                self.background_agents_observed(task_id, background_agents.unwrap_or(0))
                    .await;
                self.turn_record_seen(task_id, &attempt.id, at).await;
            }
            Observation::AgentWork { at } => {
                runtime.goal_retry_armed.store(false, Ordering::SeqCst);
                self.context_replied(task_id, &attempt.id);
                // A usage limit ends the turn, so a `limit` pause is open only between turns (or
                // after a limit replayed at a restart, which may find a turn open).
                if runtime.at_limit.load(Ordering::SeqCst) || !runtime.turn_in_progress() {
                    self.cut_turn_continued(task_id, at).await;
                }
                runtime.at_limit.store(false, Ordering::SeqCst);
                // The agent works: a goal pause before it asks nobody any more (SPEC-469).
                self.goal_pause_overtaken(task_id).await;
                self.agent_worked(task_id, &attempt.id, at).await;
            }
            Observation::GoalRetry { armed, notice } => {
                self.goal_retry_observed(task_id, armed, &notice).await;
            }
            Observation::ContextFull => self.context_full_observed(task_id, attempt).await,
            Observation::Compacted => self.compacted_observed(task_id).await,
            Observation::CompactFailed => self.compact_failed_observed(task_id, attempt).await,
            Observation::BackgroundStarted {
                id,
                agent,
                expires_in_ms,
                at,
            } => {
                self.background_started(task_id, id, agent, expires_in_ms, at)
                    .await;
            }
            Observation::BackgroundEnded { id } => self.background_ended(task_id, &id).await,
            Observation::BackgroundResumable { id } => {
                self.background_resumable(task_id, id).await;
            }
            // Kept above; its background work is the `BackgroundStarted` beside it.
            Observation::WorkflowLaunched { .. } => {}
            // What a review round's reviewer really ran on, and whether a limit cut it off
            // (SPEC-493 REV-004): nothing else depends on it.
            Observation::SubagentAnswered {
                agent_id,
                model,
                at,
            } => {
                self.review_subagent_noted(
                    task_id,
                    &agent_id,
                    Some(&model),
                    false,
                    source.review_time(at, self.now()),
                )
                .await;
            }
            // A reviewer's report in the transcript ends its round when its `SubagentStop`
            // never reached Yulius (SPEC-633 RRE-002).
            Observation::SubagentReported {
                agent_id,
                report,
                at,
            } => {
                self.review_agent_reported(
                    task_id,
                    &agent_id,
                    &report,
                    source.review_time(at, self.now()),
                )
                .await;
            }
            Observation::SubagentCutOff { agent_id, at, .. } => {
                self.review_subagent_noted(
                    task_id,
                    &agent_id,
                    None,
                    true,
                    source.review_time(at, self.now()),
                )
                .await;
            }
            // In the order of the lines: the count is ahead of nothing the watcher applied.
            Observation::Queue(change, at) => runtime.with_queue(|queue| queue.record(change, at)),
            Observation::Session { .. } => {}
            // The account's windows as Codex saw them on its last response (SPEC-503 CDX-004).
            Observation::RateLimits(usage) => {
                if let Some(account) = &attempt.account_id {
                    self.rollout_usage(account, &usage).await;
                }
            }
        }
        runtime.with_queue(|queue| queue.work_seen(worked));
    }
}

#[cfg(test)]
mod tests {
    /// SPEC-464 UCL-010: only the model's own records say it answered. A turn's work (a tool
    /// call) and its end both do; a refusal Claude Code writes itself, a prompt and a
    /// sub-agent's turn end (no time) do not, so a retry refused the same way stays a retry.
    #[test]
    fn the_models_own_records_say_it_answered() {
        use super::{Observation, Timestamp, model_answered};
        use yulius_domain::usage::LimitScope;
        let at = Some(Timestamp(1_000));
        assert!(model_answered(&Observation::AgentWork { at }));
        assert!(model_answered(&Observation::AgentWork { at: None }));
        assert!(model_answered(&Observation::TurnComplete { at }));
        assert!(!model_answered(&Observation::TurnComplete { at: None }));
        assert!(!model_answered(&Observation::UserTurn { at, wake: false }));
        assert!(!model_answered(&Observation::LimitReached {
            resets_at: None,
            message: "You're out of usage credits.".into(),
            window_minutes: None,
            scope: LimitScope::Model(None),
            at,
        }));
    }
}
