//! Agent hook events (SPEC-002 §7.5, §7.6): briefing, the Stop discipline gate,
//! activity tracking, and the turn boundary where the subscription pool switches or pauses a
//! task (SPEC-002 ADR-17). A pause holds the hook ([`HookReply::Hold`]): the agent waits at its
//! boundary and is never told anything.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use yulius_contracts::hooks::{HookInput, HookOutput, events};
use yulius_domain::briefing::{
    BriefingInput, DEFAULT_BUDGET_BYTES, Foundations, MAX_RECENT_CHANGES, ParallelTask,
    RecentChange, TaskBrief, build_briefing,
};
use yulius_domain::harness::{Harness, SubmitOrigin};
use yulius_domain::ids::TaskId;
use yulius_domain::rotation::{self, Decision, Trigger};
use yulius_domain::spec_format::{DecisionFiles, NamedFile, decision_titles, project_layout};
use yulius_domain::spec_format::{DecisionsLocation, SpecPolicy, validate_spec_for};
use yulius_domain::task::{Activity, TaskStatus};
use yulius_domain::time::Timestamp;

use crate::capacity::{CONTINUATION_MAX, HOLD_RETRY_SECS};
use crate::context::App;
use crate::coordination::AgentContext;
use crate::delivery::{RepoReadError, read_spec};
use crate::error::{AppError, AppResult};
use crate::model::{PauseKind, TaskMode, TaskRecord};
use crate::ports::store::TaskFieldUpdate;
use crate::supervisor::{BoxFuture, RunStart};

/// Consecutive Stop-hook blocks per turn before the task needs attention (AC-DSC-3).
pub const MAX_GATE_BLOCKS: u32 = 3;

tokio::task_local! {
    /// Deadline for live browser observations made while answering one Stop hook.
    pub(crate) static STOP_BROWSER_DEADLINE: tokio::time::Instant;
}

struct TaskUpdates {
    task: yulius_domain::ids::TaskId,
    changed: std::cell::Cell<bool>,
    steps_changed: std::cell::Cell<bool>,
}

tokio::task_local! {
    /// Task and plan updates are published together after their writer releases the plan lock.
    static TASK_UPDATES: TaskUpdates;
}

pub(crate) fn defer_task_update(task: &yulius_domain::ids::TaskId, steps: bool) -> bool {
    TASK_UPDATES
        .try_with(|update| {
            if &update.task != task {
                return false;
            }
            update.changed.set(true);
            if steps {
                update.steps_changed.set(true);
            }
            true
        })
        .unwrap_or(false)
}

/// The agent-facing introduction to the goal's Stop instruction.
pub(crate) const GOAL_CONTINUATION: &str = "Continue the authorized goal. ";

/// A goal instruction must fit both the capacity record and a provider CLI argument.
/// The complete spec diagnostics and plan remain available through task tools.
pub(crate) fn retained_goal_instruction(prompt: Option<String>) -> String {
    prompt.filter(|text| !text.is_empty() && text.len() <= crate::capacity::CONTINUATION_MAX)
        .unwrap_or_else(|| format!("{GOAL_CONTINUATION}Run spec_check and correct any reported problems. Use steps_get to continue the next unfinished step and record its evidence."))
}

/// Largest repository document read for the briefing or the gate.
const MAX_DOC_BYTES: u64 = 256 * 1024;
/// Largest number of decision records read.
const MAX_DECISION_FILES: usize = 200;

/// The action owned by a turn boundary. A handoff suppresses the optional step nudge too.
enum StopGate {
    Pass,
    Handoff,
    /// Claude’s first native goal record has yet to reach the transcript watcher.
    NativeGoal,
    /// Observe the current plan and budget together after the live probes and before acting.
    Declared(StopPull),
    Continue(String),
}

/// What a Codex goal's own pull request leaves to do at Stop (SPEC-558 PKR-003): its Yulius
/// CI run at the head the agent is on failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StopPull {
    Settled,
    Failing(u64),
}

/// Joins a repository-relative path, refusing absolute paths and `..` components.
fn repo_path(root: &Path, relative: &str) -> Option<PathBuf> {
    let rel = Path::new(relative);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some(root.join(rel))
}

/// Reads `root/relative` as a document of the repository: a regular file (not a link) whose
/// resolved path stays under `root`. Directories linked out of the repository are not followed,
/// and on Linux the opened file's real path is checked again, so a link swapped in after the
/// check cannot lead outside either (#70). These files are agent-written and read by the server
/// outside containment for the briefing, `project_context`, the Stop gate and the repository's
/// `.mcp.json` (SPEC-095). The file is opened without blocking, so a FIFO swapped in after the
/// check cannot hang the server, and read only when it is still a regular file.
pub(crate) fn read_repo_doc(root: &Path, relative: &str) -> Option<String> {
    use std::io::Read;
    let path = repo_path(root, relative)?;
    let canonical_root = std::fs::canonicalize(root).ok()?;
    if !std::fs::symlink_metadata(&path).ok()?.is_file() {
        return None;
    }
    let resolved = std::fs::canonicalize(&path).ok()?;
    if !resolved.starts_with(&canonical_root) {
        return None;
    }
    let file = {
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NONBLOCK)
            .open(&resolved)
            .ok()?
    };
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let real = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).ok()?;
        if !real.starts_with(&canonical_root) {
            return None;
        }
    }
    let mut text = String::new();
    file.take(MAX_DOC_BYTES).read_to_string(&mut text).ok()?;
    Some(text)
}

/// `root/relative` resolved: the canonical root and the path's canonical form under it, when the
/// path itself is no link and `kind` holds for it (see [`read_repo_doc`]).
fn resolved_in_repo(
    root: &Path,
    relative: &str,
    kind: fn(&std::fs::Metadata) -> bool,
) -> Option<(PathBuf, PathBuf)> {
    let path = repo_path(root, relative)?;
    if !std::fs::symlink_metadata(&path).is_ok_and(|meta| kind(&meta)) {
        return None;
    }
    let root = std::fs::canonicalize(root).ok()?;
    let resolved = std::fs::canonicalize(&path).ok()?;
    resolved.starts_with(&root).then_some((root, resolved))
}

/// Whether `root/relative` is a regular file of the repository (see [`read_repo_doc`]).
fn repo_doc_exists(root: &Path, relative: &str) -> bool {
    resolved_in_repo(root, relative, std::fs::Metadata::is_file)
        .is_some_and(|(root, resolved)| doc_present(&root, &resolved))
}

/// Whether the `resolved` path (see [`resolved_in_repo`]) still is a regular file under `root`:
/// asked of the opened file, whose real path must still be under `root`, so a folder swapped for
/// a link after resolving is not followed (SPEC-262 VTR-004). A file the server cannot open is
/// absent, as [`read_repo_doc`] cannot read it either.
fn doc_present(root: &Path, resolved: &Path) -> bool {
    crate::files::open_inside(root, resolved, nix::fcntl::OFlag::O_RDONLY)
        .ok()
        .map(std::fs::File::from)
        .is_some_and(|file| file.metadata().is_ok_and(|meta| meta.is_file()))
}

/// What a hook call answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookReply {
    /// Print this output (empty = no decision) and let the CLI go on.
    Output(HookOutput),
    /// Keep the CLI at this boundary: the hook client asks again after `retry_after_secs`
    /// (`202 Accepted` with `Retry-After`). Used while no subscription has capacity.
    Hold {
        /// Seconds until the hook client asks again.
        retry_after_secs: u64,
    },
}

fn hold() -> HookReply {
    HookReply::Hold {
        retry_after_secs: HOLD_RETRY_SECS,
    }
}

/// 256-colour text for the status line (the colours the web mock shows, HRN-005), without the
/// control characters of `text` (C0, DEL, C1), so a name cannot print escapes of its own
/// (SPEC-262 VTR-003).
fn colour(code: u8, text: &str) -> String {
    let text: String = text.chars().filter(|c| !c.is_control()).collect();
    format!("\x1b[38;5;{code}m{text}\x1b[39m")
}

/// Claude Code's tools that start a subagent (the hook client sends their `PreToolUse` here).
fn agent_tool(tool: &str) -> bool {
    matches!(tool, "Agent" | "Task")
}

impl App {
    /// Handles one hook event for the run owning `token`, sent by its client at `sent_ms` (Unix
    /// ms; none from an older client: now). Unknown events answer empty.
    ///
    /// # Errors
    /// `Unauthenticated` for unknown or finished runs; store failures.
    pub async fn hook_event(
        self: &Arc<Self>,
        token: &str,
        harness: &str,
        event: &str,
        input: &HookInput,
        sent_ms: Option<i64>,
    ) -> AppResult<HookReply> {
        // Claude Code's and Codex's hooks (SPEC-503 CDX-007); each from a run of its own harness.
        let Ok(harness) = yulius_domain::harness::Harness::parse(harness) else {
            return Err(AppError::NotFound);
        };
        if let Some(session) = input.session_id.as_deref() {
            crate::race::reached(
                crate::race::Point::HookArrived,
                &Path::new(session).join(event),
            );
        }
        // Capture the ordering boundary before the token lookup yields to a viewer key.
        // The client stamps the host's wall clock (loopback only), read on this server's clock:
        // a hook that arrives late keeps its send time (SPEC-636). Never later than now: a time
        // from the future would order nothing. One not after the epoch is none (SPEC-346 LPH-001).
        let now = self.now().0;
        let sent = sent_ms.filter(|sent| *sent > 0).map_or(now, |sent| {
            let wall = yulius_domain::time::Timestamp(sent);
            self.deps.clock.of_wall(wall).0.min(now)
        });
        let sequence = self.viewer_input_sequence();
        let ctx = self.agent_context(token).await?;
        if ctx.attempt.harness != harness {
            return Err(AppError::Unauthenticated);
        }
        let runtime = self.runtime(&ctx.task.id);
        if event == events::STOP {
            crate::race::reached(
                crate::race::Point::StopTurnLock,
                std::path::Path::new(ctx.task.id.as_str()),
            );
        }
        // A timed-out hook may still be here while agy starts its next turn. Order its
        // Stop before every consequence (gates, rotation, idle, and the answer marker).
        let _native_order = if harness == Harness::Antigravity
            && matches!(
                event,
                events::STOP | events::PRE_INVOCATION | events::USER_PROMPT_SUBMIT
            ) {
            let mut latest = runtime.antigravity_hook_ms.lock().await;
            if sent < *latest && event != events::USER_PROMPT_SUBMIT {
                return Ok(HookReply::Output(HookOutput::default()));
            }
            *latest = (*latest).max(sent);
            Some(latest)
        } else {
            None
        };
        if event != events::STOP {
            return self.answer_hook(&ctx, event, input, sent, sequence).await;
        }
        // The agent sent this Stop after every prompt of its turn (SPEC-346 LPH-002).
        {
            let _order = runtime.turn_order.lock().await;
            runtime.turn_end_ms.fetch_max(sent, Ordering::SeqCst);
        }
        // A prompt that passed its check before that has set Working by now: the Stop settles the
        // task as it is, not as it read it before the wait (the copy if the store fails, so the
        // failed Stop's handling below still runs).
        let ctx = AgentContext {
            task: match self.deps.store.task(&ctx.task.id).await {
                Ok(Some(task)) => task,
                _ => ctx.task,
            },
            attempt: ctx.attempt,
        };
        let _in_flight = crate::finish::StopInFlight::enter(&runtime);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(750);
        // Keep one hook future across the nested task-local scopes. Moving the whole
        // turn through each scope exhausted the worker stack on a resumed goal.
        let answer = Box::pin(self.answer_hook(&ctx, event, input, sent, sequence));
        let reply = STOP_BROWSER_DEADLINE
            .scope(deadline, async {
                if ctx.task.mode == TaskMode::Goal {
                    self.publish_after_updates(&ctx.task, answer).await
                } else {
                    answer.await
                }
            })
            .await;
        // Only the live run's answers count: a late one of a run a switch ended is not this
        // session's (SPEC-290 GDW-001).
        let live = matches!(
            self.deps.store.live_attempt(&ctx.task.id).await,
            Ok(Some(attempt)) if attempt.id == ctx.attempt.id
        );
        let now = self.now().0;
        if live
            && ctx.task.mode == TaskMode::Goal
            && let Ok(HookReply::Output(output)) = &reply
            && output.decision.as_deref() == Some(yulius_contracts::hooks::DECISION_BLOCK)
            && let Some(reason) = &output.reason
        {
            self.note_issued_prompt(&ctx.task.id, reason.as_bytes());
        }
        if reply.is_ok() && live {
            // An answered Stop, whatever Yulius decided. Also kept in the run directory: after a
            // restart, a turn whose Stop was answered is not woken (SPEC-358 MSB-007).
            runtime.stop_answered_ms.store(now, Ordering::SeqCst);
            crate::outage::mark_stop_answered(std::path::Path::new(&ctx.attempt.run_dir), now);
        } else if reply.is_err()
            && crate::finish::failed_stop_settles(
                live,
                runtime.turn_record_at_ms.load(Ordering::SeqCst),
                runtime.turn_began_ms.load(Ordering::SeqCst),
            )
        {
            // It failed (the CLI went on, failing open) and its turn's record came meanwhile:
            // the turn settles as its clean Stop would have, the Stop standing at that record.
            let record_ms = runtime.turn_record_ms.load(Ordering::SeqCst);
            self.settle_unanswered_turn(&ctx.task.id, record_ms).await;
        }
        reply
    }

    /// Gather same-task updates until the operation finishes. A spec watcher releases
    /// its plan lock before building the task view, including its live browser lookup.
    /// When Stop owns the outer operation it publishes the final continuation decision.
    pub(crate) fn publish_after_updates<'a, T: Send + 'a>(
        &'a self,
        task: &'a TaskRecord,
        work: BoxFuture<'a, T>,
    ) -> BoxFuture<'a, T> {
        // Spec reads are nested inside delivery, merge-queue and MCP futures. Keep
        // the work and its publication scope on the heap across those callers.
        Box::pin(async move {
            let (result, changed, steps_changed) = TASK_UPDATES
                .scope(
                    TaskUpdates {
                        task: task.id.clone(),
                        changed: Default::default(),
                        steps_changed: Default::default(),
                    },
                    async {
                        let result = work.await;
                        TASK_UPDATES.with(|updates| {
                            (result, updates.changed.get(), updates.steps_changed.get())
                        })
                    },
                )
                .await;
            if steps_changed {
                self.publish_steps(task).await;
            } else if changed {
                self.publish_task(&task.id).await;
            }
            result
        })
    }

    async fn submit_origin(
        &self,
        ctx: &AgentContext,
        input: &HookInput,
        sent: i64,
    ) -> SubmitOrigin {
        let (Some(session), Some(id), Some(prompt)) =
            (&input.session_id, &input.prompt_id, &input.prompt)
        else {
            return SubmitOrigin::Unmarked;
        };
        if ctx.attempt.harness != Harness::ClaudeCode
            || !yulius_domain::harness::valid_session_id(session)
            || ctx
                .attempt
                .provider_session_id
                .as_ref()
                .is_some_and(|current| current != session)
        {
            return SubmitOrigin::Unmarked;
        }
        // Use only the file derived from the authenticated task. The hook's path and cwd
        // are provider input, not paths the server may open.
        let anchor = self.sandbox_dir(&ctx.task.sandbox_id);
        let path = self.native_transcript(&ctx.task, Harness::ClaudeCode, session);
        let harness = Arc::clone(&self.deps.harness);
        let (session, id, prompt) = (session.clone(), id.clone(), prompt.clone());
        let run_to_submit = ctx.attempt.started_at..=yulius_domain::time::Timestamp(sent);
        // Claude 2.1.322 buffered its queue writes for 67–108 ms. Let the already-made
        // hand-over reach disk. Use the original submit time throughout, even if keys arrive.
        let age = self.now().0.saturating_sub(sent).clamp(0, 200);
        tokio::time::sleep(std::time::Duration::from_millis((200 - age).unsigned_abs())).await;
        tokio::task::spawn_blocking(move || {
            crate::transcripts::open(&anchor, &path).map_or(SubmitOrigin::Unmarked, |file| {
                harness.submit_origin(file, &session, &id, &prompt, run_to_submit)
            })
        })
        .await
        .unwrap_or(SubmitOrigin::Unmarked)
    }

    /// Answers the hook `event` of the run `ctx`, sent at `sent` ([`Self::hook_event`]).
    async fn answer_hook(
        self: &Arc<Self>,
        ctx: &AgentContext,
        event: &str,
        input: &HookInput,
        sent: i64,
        sequence: u64,
    ) -> AppResult<HookReply> {
        if event != events::USER_PROMPT_SUBMIT {
            return self
                .answer_event(ctx, event, input, sent, SubmitOrigin::Unmarked, false)
                .await;
        }
        // A prompt submitted from the session's prompt box empties it: what was typed before
        // it is no draft any more (SPEC-468 CCF-010). Claude Code runs this hook at the Enter,
        // also for a prompt entered while a turn runs (seen with 2.1.322), so the hook's time
        // is when the box was emptied. Once per submit, as of its first request: a prompt that
        // is held (an update, a pause, no subscription with room) is sent again every few
        // seconds, stamped anew, until it goes on, and what a person types meanwhile is a
        // draft.
        let runtime = self.runtime(&ctx.task.id);
        let box_text = crate::finish::from_prompt_box(input.prompt.as_deref());
        let origin = if box_text {
            self.submit_origin(ctx, input, sent).await
        } else {
            SubmitOrigin::Unmarked
        };
        let from_box = box_text && origin == SubmitOrigin::Unmarked;
        let sent_again = runtime.prompt_held.load(Ordering::SeqCst);
        let invocation = if ctx.task.mode == TaskMode::Goal {
            input.invocation
        } else {
            None
        };
        let submitted = if from_box {
            let mut mark = runtime.draft_mark.lock().await;
            if invocation.is_some() || !sent_again {
                let enter = self.viewer_submit_at(
                    &ctx.task.id,
                    sent,
                    sequence,
                    input.prompt.as_deref().unwrap_or_default().as_bytes(),
                    invocation,
                );
                // Enter empties the owner's draft at that key, even if more text is
                // already in the PTY when the provider sends or retries its callback.
                mark.submit = match enter {
                    Some((at, seq)) if ctx.task.mode == TaskMode::Goal => (at, seq, true),
                    _ => (sent, sequence, enter.is_some()),
                };
            }
            Some(mark.submit)
        } else {
            None
        };
        let viewer_input = submitted.is_some_and(|(_, _, owner)| owner);
        let reply = self
            .answer_event(ctx, event, input, sent, origin, viewer_input)
            .await;
        // A prompt that is held stays held through an answer that fails: the hook client, once
        // it holds, asks again after a 5xx or a lost connection (it fails open at a first
        // request). At a 4xx it ends its hold and the prompt goes on; that is read as held
        // here too, which errs toward a draft, as after a hold the CLI gave up.
        let held = matches!(reply, Ok(HookReply::Hold { .. })) || (sent_again && reply.is_err());
        if held && let (Some(id), Some((at, sequence, true))) = (invocation, submitted) {
            self.keep_held_owner(&ctx.task.id, id, (at, sequence));
        }
        // The box is taken as emptied once the prompt goes on, not while it is held: a prompt
        // Yulius blocks in the end (its scope check held it, TSK-013) never reaches the agent,
        // and Claude Code puts its text back into the box, so that submit emptied nothing.
        let blocked = matches!(
            &reply,
            Ok(HookReply::Output(output))
                if output.decision.as_deref() == Some(yulius_contracts::hooks::DECISION_BLOCK)
        );
        if from_box && blocked {
            if let Some((_, keys, true)) = submitted {
                runtime
                    .viewer_prompt
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .restored();
                self.retract_viewer_submit(&ctx.task.id, keys);
            }
            // What was submitted is back in the box: nothing was sent past a draft.
            runtime.sent_past_draft.store(false, Ordering::SeqCst);
        }
        if let Some((emptied, keys, owner)) = submitted
            && (ctx.task.mode == TaskMode::Prompt
                || owner
                || !self.persons_draft_holds(&ctx.task.id)
                || runtime.sent_past_draft.load(Ordering::SeqCst))
            && !held
            && !blocked
        {
            let run_dir = Path::new(&ctx.attempt.run_dir);
            // The owner's message typed behind their draft went on with it (SPEC-487
            // TCT-006): this prompt is known to follow the draft, whatever this server saw.
            let past_draft = runtime.sent_past_draft.swap(false, Ordering::SeqCst);
            // A draft this server knows from its mark alone (typed before it started, no key
            // seen since) may sit behind this very prompt: one held across the restart is
            // asked about again and reads as a new submit here. Such a draft stays one until
            // a prompt follows a key this server saw (SPEC-480 DRF-009). Read from the file,
            // not from what recovery kept: this hook may be served before the run is
            // reattached.
            let mut mark = runtime.draft_mark.lock().await;
            let inherited = if past_draft || runtime.typed_seq.load(Ordering::SeqCst) != 0 {
                0
            } else {
                crate::supervisor::person_typed_marked(run_dir).min(self.now().0)
            };
            if inherited > 0 {
                runtime.typed_at_ms.fetch_max(inherited, Ordering::SeqCst);
                mark.written = true;
            } else {
                if ctx.task.mode == TaskMode::Goal {
                    if owner {
                        self.retract_viewer_submit(&ctx.task.id, keys);
                    }
                } else {
                    self.consume_viewer_submits(&ctx.task.id, keys);
                }
                if emptied >= runtime.box_emptied_ms.fetch_max(emptied, Ordering::SeqCst) {
                    runtime.emptied_seq.fetch_max(keys, Ordering::SeqCst);
                }
                // With the box empty no person's draft holds a typed message any more
                // (DRF-007, DRF-009): its mark leaves the run's folder, whatever this server
                // remembers of having written it, and a flag that said a message of Yulius's
                // own waits for it ends, also after a prompt that starts no turn. A key typed
                // after this submit is a draft of its own, and both stay.
                if !self.persons_draft_holds(&ctx.task.id) {
                    crate::race::reached(crate::race::Point::DraftMarkCleanup, run_dir);
                    mark.written = false;
                    crate::supervisor::clear_person_typed(run_dir);
                    drop(mark);
                    self.settle_draft_flag(&ctx.task.id).await;
                }
            }
        }
        if box_text && !held && !blocked {
            // It goes on to Claude Code now, which writes where it went (a turn of its own,
            // or its queue) only after this answer: until that record is read, a turn's end
            // cannot tell (SPEC-471 CCQ-004).
            runtime
                .prompt_passed_ms
                .store(self.now().0, Ordering::SeqCst);
            runtime.prompts_passed.fetch_add(1, Ordering::SeqCst);
        }
        runtime.prompt_held.store(held, Ordering::SeqCst);
        reply
    }

    /// [`Self::answer_hook`], for every event alike.
    async fn answer_event(
        self: &Arc<Self>,
        ctx: &AgentContext,
        event: &str,
        input: &HookInput,
        sent: i64,
        origin: SubmitOrigin,
        viewer_input: bool,
    ) -> AppResult<HookReply> {
        // This run reports its turn boundaries through hooks: the Stop hook decides switches
        // and pauses, not the transcript.
        self.runtime(&ctx.task.id)
            .hooks_seen
            .store(true, Ordering::SeqCst);
        // The session's first message Yulius types (SPEC-068): its submit reached Yulius, even if
        // held below; any Stop means the session took a prompt, so nothing of it is pending.
        match event {
            events::USER_PROMPT_SUBMIT => crate::supervisor::opening_entered(&ctx.attempt.run_dir),
            events::STOP => crate::supervisor::opening_sent(&ctx.attempt.run_dir),
            _ => {}
        }
        // An instance update pauses every task at its turn boundary, as a capacity pause does:
        // the agent waits here and is never told (SPEC-002 ADR-18).
        if matches!(event, events::STOP | events::USER_PROMPT_SUBMIT)
            && let Some(newly) = self.hold_for_update(&ctx.task.id)
        {
            // Its pause took hold: the lists say so (SPEC-191 PAU-002).
            if newly {
                self.publish_task(&ctx.task.id).await;
            }
            return Ok(HookReply::Hold {
                retry_after_secs: crate::updates::HOLD_RETRY_SECS,
            });
        }
        // A person paused a run that could not be frozen: it waits here, after the update's
        // hold (the drain sees its pause point) and before any rotation (SPEC-191 PER-001).
        if matches!(event, events::STOP | events::USER_PROMPT_SUBMIT)
            && let Some(newly) = self.hold_for_person(&ctx.task, &ctx.attempt.id).await
        {
            if newly {
                self.publish_task(&ctx.task.id).await;
            }
            return Ok(HookReply::Hold {
                retry_after_secs: crate::pause::HOLD_RETRY_SECS,
            });
        }
        match event {
            events::SESSION_START => Ok(HookReply::Output(
                self.hook_session_start(ctx, input).await?,
            )),
            events::USER_PROMPT_SUBMIT => {
                self.hook_prompt_submit(ctx, input, sent, origin, viewer_input)
                    .await
            }
            events::PRE_INVOCATION if ctx.attempt.harness == Harness::Antigravity => {
                self.antigravity_work(&ctx.task.id, sent).await?;
                Ok(HookReply::Output(HookOutput::default()))
            }
            events::STOP => self.hook_stop(ctx, input).await,
            events::PRE_TOOL_USE => Ok(HookReply::Output(self.hook_pre_tool_use(ctx, input).await)),
            events::SUBAGENT_START => {
                self.review_subagent_started(ctx, input, Timestamp(sent))
                    .await;
                Ok(HookReply::Output(HookOutput::default()))
            }
            events::SUBAGENT_STOP => {
                self.review_subagent_stopped(ctx, input, Timestamp(sent))
                    .await;
                Ok(HookReply::Output(HookOutput::default()))
            }
            events::NOTIFICATION => {
                // Only a notice that asks a person to act needs one: Claude Code's reminder that
                // it waits for input ("idle", about a minute after a turn ended) keeps a finished
                // task waiting or done, so chains and onboarding see it as finished
                // (F 2026-10-11).
                if notification_needs_a_person(input) {
                    // The flag records the question's words and what can answer it (SPEC-472).
                    self.flag_attention(
                        &ctx.task.id,
                        crate::attention::AttentionRecord::terminal(input.message.as_deref())
                            .answered_by_work(notice_answered_by_work(input)),
                    )
                    .await?;
                }
                Ok(HookReply::Output(HookOutput::default()))
            }
            events::SESSION_END => {
                self.session_ending(ctx, input.reason.as_deref());
                Ok(HookReply::Output(HookOutput::default()))
            }
            _ => Ok(HookReply::Output(HookOutput::default())),
        }
    }

    /// A held hook answer for `task`, remembering when the hold was last asked about: a hold
    /// nobody asks about any more was given up by the CLI (ADR-17, [`Self::resume_round`]).
    fn hold_for(&self, task: &yulius_domain::ids::TaskId) -> HookReply {
        self.runtime(task)
            .hold_polled_ms
            .store(self.now().0, Ordering::SeqCst);
        hold()
    }

    /// `SessionEnd` (F 2026-10-10: the task follows its session): Claude Code says why the
    /// session ends before its process exits. Leaving it (`/exit`, Ctrl+C twice, `/logout`)
    /// makes the coming exit a normal close whatever its code; `/clear` keeps the process with
    /// a new conversation, so an achieved goal no longer counts (its `SessionStart` names the new
    /// session, which the supervisor adopts into the lineage).
    fn session_ending(&self, ctx: &AgentContext, reason: Option<&str>) {
        let runtime = self.runtime(&ctx.task.id);
        let reason = reason
            .filter(|r| !r.is_empty() && r.len() <= 64)
            .unwrap_or("other");
        tracing::info!(task = %ctx.task.id, %reason, "the agent session ends");
        if reason == yulius_contracts::hooks::session_end::CLEAR {
            runtime.goal_achieved.store(false, Ordering::SeqCst);
            return;
        }
        if let Ok(mut end) = runtime.session_end.lock() {
            *end = Some(reason.to_owned());
        }
    }

    /// Claude Code's status line of a run (`yulius hook claude StatusLine`): the repository and
    /// the task's branch, "→ studio git:(feat/12-checkout)" (HRN-005); and the subscription's
    /// plan usage it carries — read by Claude Code from its own API responses, the only usage a
    /// `claude setup-token` sign-in can see (ADR-17).
    ///
    /// # Errors
    /// `NotFound` for another harness, `Unauthenticated` for unknown or finished runs.
    pub async fn status_line(
        self: &Arc<Self>,
        token: &str,
        harness: &str,
        input: &yulius_contracts::hooks::StatusLineInput,
    ) -> AppResult<String> {
        if harness != "claude" {
            return Err(AppError::NotFound);
        }
        let ctx = self.agent_context(token).await?;
        if let (Some(limits), Some(account)) = (&input.rate_limits, &ctx.attempt.account_id) {
            self.session_usage(account, limits).await;
        }
        let sandbox = self.deps.store.sandbox(&ctx.task.sandbox_id).await?;
        let repository = sandbox.map_or_else(|| "repository".to_owned(), |s| s.repo_name);
        let branch = ctx.task.branch.clone().unwrap_or_else(|| "…".into());
        Ok(format!(
            "{} {} {}{}{}",
            colour(114, "→"),
            colour(80, &repository),
            colour(75, "git:("),
            colour(203, &branch),
            colour(75, ")")
        ))
    }

    /// Sets a running task's activity and publishes it. `NeedsAttention` is raised from the task
    /// as it is now, not as `task` says ([`Self::flag_needs_attention`]), and without a reason:
    /// for a flag whose own request says it (a goal pause, a host move: `needs_you`). Every
    /// other flag gives its reason ([`Self::flag_attention`], SPEC-472).
    pub(crate) async fn set_task_activity(
        &self,
        task: &TaskRecord,
        activity: Activity,
    ) -> AppResult<()> {
        self.set_task_activity_unless_held(task, activity)
            .await
            .map(|_| ())
    }

    /// [`Self::set_task_activity`]; whether the flag of a run that could not be ended held the
    /// task as it was ([`Self::write_session_activity`], SPEC-635).
    pub(crate) async fn set_task_activity_unless_held(
        &self,
        task: &TaskRecord,
        activity: Activity,
    ) -> AppResult<bool> {
        if activity == Activity::NeedsAttention {
            return self
                .flag_needs_attention(&task.id, None)
                .await
                .map(|()| false);
        }
        if task.status != TaskStatus::Running || task.activity == Some(activity) {
            return Ok(false);
        }
        self.runtime(&task.id)
            .notice_attention_ms
            .store(0, Ordering::SeqCst);
        if !self.write_session_activity(&task.id, activity).await? {
            return Ok(true);
        }
        // The turn settled: its screen is kept, so a run lost later (a reboot) still shows how
        // its last turn ended (SPEC-229 RBT-002).
        if activity.is_idle()
            && let Ok(Some(attempt)) = self.deps.store.live_attempt(&task.id).await
        {
            crate::supervisor::save_screen(
                &self.runtime(&task.id).hub,
                std::path::Path::new(&attempt.run_dir),
            );
        }
        self.publish_task(&task.id).await;
        Ok(false)
    }

    /// Flags a running task under its attention lock (SPEC-213, SPEC-472). The current
    /// matter can take over a secret request's flag, and a new terminal question can take
    /// over a flag answered by work. The replacement reason and its answer time survive
    /// a server restart.
    pub(crate) async fn flag_needs_attention(
        &self,
        task_id: &TaskId,
        reason: Option<&crate::attention::AttentionRecord>,
    ) -> AppResult<()> {
        let runtime = self.runtime(task_id);
        #[cfg(feature = "test-support")]
        if runtime.attention_lock.try_lock().is_err() {
            crate::race::reached(
                crate::race::Point::AttentionReplacementWaiting,
                std::path::Path::new(task_id.as_str()),
            );
        }
        let _held = runtime.attention_lock.lock().await;
        let store = &self.deps.store;
        let now = self.now();
        let notice = reason.filter(|r| r.kind == crate::attention::AttentionKind::Terminal);
        if !store.flag_attention(task_id, None, reason, now).await? {
            // A secret request gives up its flag to any new matter. An answerable question
            // gives its flag to a later question, which can require a worker's own answer.
            let secret_flag = self.secret_flag_at(&runtime, task_id).await?;
            runtime.secret_wait_at.store(0, Ordering::SeqCst);
            let flagged = if secret_flag != 0 {
                yulius_domain::time::Timestamp(secret_flag)
            } else if notice.is_some() {
                match store.task_attention(task_id).await? {
                    Some((previous, flagged)) if previous.work_answers => flagged,
                    None => {
                        // Goal pauses have an answerable flag with no stored terminal reason.
                        let Some(times) = store.task_times(task_id).await? else {
                            return Ok(());
                        };
                        if times.state_at.0 > runtime.notice_attention_ms.load(Ordering::SeqCst) {
                            return Ok(());
                        }
                        times.state_at
                    }
                    _ => return Ok(()),
                }
            } else {
                return Ok(());
            };
            let replacement = reason.map(|reason| {
                let mut reason = reason.clone();
                reason.answer_ends = false;
                reason.noticed_ms = notice.map(|_| now.0);
                reason
            });
            if !store
                .replace_attention(task_id, flagged, replacement.as_ref(), now)
                .await?
            {
                return Ok(());
            }
        }
        runtime.secret_wait_at.store(0, Ordering::SeqCst);
        if let Some(notice) = notice {
            let at = if notice.work_answers { now.0.max(1) } else { 0 };
            runtime.notice_attention_ms.store(at, Ordering::SeqCst);
        }
        self.publish_task(task_id).await;
        Ok(())
    }

    async fn hook_session_start(
        &self,
        ctx: &AgentContext,
        input: &HookInput,
    ) -> AppResult<HookOutput> {
        // The TUI's session, named by the TUI itself: at its start, and again after `/clear` or
        // `/resume` (`source: clear`, `resume`). Only this run's Claude Code runs this hook, so a
        // `claude` the agent starts in the worktree never becomes the task's session. It goes
        // into the lineage at once (a restart before the supervisor's next drift check must not
        // lose it); the supervisor follows the attempt (SPEC-281 SSN-001).
        if let Some(session) = input.session_id.as_deref()
            && yulius_domain::harness::valid_session_id(session)
            && ctx.attempt.provider_session_id.as_deref() != Some(session)
            && !self.another_tasks_session(ctx, session).await?
        {
            self.deps
                .store
                .set_attempt_session(&ctx.attempt.id, session)
                .await?;
            self.record_lineage(&ctx.task, ctx.attempt.harness, session, "native")
                .await?;
        }
        if let Err(err) = self.sync_steps_from_spec(&ctx.task).await {
            tracing::warn!(error = %err, task = %ctx.task.id, "step sync from the spec failed");
        }
        // The prompt box reads input from now on: the goal can be typed (TSK-010). A restarted
        // server reads it from the run's marker (SPEC-230).
        self.runtime(&ctx.task.id)
            .session_started
            .store(true, Ordering::SeqCst);
        crate::supervisor::mark_session_started(Path::new(&ctx.attempt.run_dir));
        // A session restarted on a newer Claude Code that got here runs on it (SPEC-461).
        self.claude_code_run_started(&ctx.task.id, &ctx.attempt.id);
        let briefing = self.briefing(&ctx.task, ctx.attempt.harness).await?;
        Ok(HookOutput::additional_context(
            events::SESSION_START,
            briefing,
        ))
    }

    /// Whether `session` is in the lineage of another task of the sandbox: the sandbox's tasks
    /// share one Claude directory, so an agent can name a session it did not run (HRN-004).
    async fn another_tasks_session(&self, ctx: &AgentContext, session: &str) -> AppResult<bool> {
        let store = &self.deps.store;
        let harness = ctx.attempt.harness.as_str();
        if !store
            .provider_session_exists(&ctx.task.sandbox_id, harness, session)
            .await?
        {
            return Ok(false);
        }
        let own = store.provider_sessions(&ctx.task.id).await?;
        Ok(!own.iter().any(|entry| entry.session_id == session))
    }

    /// The session briefing for a task (SPEC-002 §7.6, COM-004, DSC-009).
    ///
    /// # Errors
    /// Store failures.
    pub async fn briefing(
        &self,
        task: &TaskRecord,
        harness: yulius_domain::harness::Harness,
    ) -> AppResult<String> {
        let store = &self.deps.store;
        let sandbox = store
            .sandbox(&task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let root = task
            .worktree_path
            .as_ref()
            .map_or_else(|| self.sandbox_repo(&task.sandbox_id), PathBuf::from);
        let profile = crate::delivery::delivery_profile(&sandbox);
        let root_for_read = root.clone();
        let repo = tokio::task::spawn_blocking(move || RepoDocs::read(&root_for_read))
            .await
            .map_err(AppError::internal)?;

        let (_, steps) = self.step_states_of(&task.id).await?;
        let workflow = self.task_workflow(task).await.ok().flatten();
        let spec_policy = SpecPolicy::from_workflow(workflow.as_ref().map(|w| w.require_spec));
        let all = store.tasks(&task.sandbox_id, false).await?;
        let claims = store.active_claims(&task.sandbox_id).await?;
        let mut recent: Vec<&TaskRecord> = all
            .iter()
            .filter(|t| t.status == TaskStatus::Closed && t.id != task.id && t.closed_at.is_some())
            .collect();
        recent.sort_by_key(|t| std::cmp::Reverse(t.closed_at));
        let mut recent_changes = Vec::new();
        for t in recent.into_iter().take(MAX_RECENT_CHANGES) {
            // A personal task's request stays with tasks holding the same secrets (SPEC-625).
            let summary = if self.withholds_text(task, t).await? {
                String::new()
            } else {
                t.prompt
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(300)
                    .collect()
            };
            recent_changes.push(RecentChange {
                title: t.title.clone(),
                summary,
                at: t.closed_at.unwrap_or(t.updated_at),
            });
        }
        let parallel_tasks = all
            .iter()
            // A parked task's work goes on when it resumes (SPEC-512 PRK-001).
            .filter(|t| t.id != task.id && t.status.holds_claims())
            .map(|t| ParallelTask {
                number: t.number,
                title: t.title.clone(),
                harness: t.harness,
                status: t.status,
                branch: t.branch.clone().unwrap_or_default(),
                scopes: claims
                    .iter()
                    .filter(|c| c.task_id == t.id)
                    .map(|c| c.pattern.clone())
                    .collect(),
            })
            .collect();
        let decisions = repo.decision_files();
        // This sandbox's holder, not the setting: one without an engine is told so (SPEC-187).
        let docker = match self.isolation.get() {
            Some(isolation) => isolation.docker(&self.sandbox_ref(&sandbox)).await,
            None => false,
        };
        let input = BriefingInput {
            project_name: if sandbox.is_playground() {
                format!("{} (playground)", sandbox.repo_name)
            } else {
                format!("{}/{}", sandbox.repo_owner, sandbox.repo_name)
            },
            harness,
            task: TaskBrief {
                id: task.id.clone(),
                number: task.number,
                title: task.title.clone(),
                goal: task.prompt.clone(),
                spec_path: task.spec_path.clone().unwrap_or_default(),
                steps,
                branch: task.branch.clone().unwrap_or_default(),
                workflow: workflow.map(|w| w.name),
                spec_policy,
            },
            profile: &profile,
            layout: &repo.layout,
            product_head: repo.product.as_deref(),
            architecture_present: repo.architecture_present,
            decision_titles: decision_titles(&decisions),
            changes_head: repo.changes.as_ref().map(|(head, _)| head.as_str()),
            changes_newest_at_end: repo.changes.as_ref().is_some_and(|(_, at_end)| *at_end),
            recent_changes,
            parallel_tasks,
            port_base: sandbox.port_base,
            port_count: sandbox.port_count,
            repo_instructions: repo.instructions.clone(),
            foundations: repo.foundations.clone(),
            now: self.now(),
            playground: sandbox.is_playground(),
            isolated: self.isolation.get().is_some(),
            docker,
            vidoc: self.vidoc_briefing(&sandbox),
        };
        let workflow = self.workflow_instructions(task).await.map(|text| {
            let clipped: String = text.chars().take(3_000).collect();
            // The workflows describe GitHub delivery; a playground has only the local merge.
            let note = if sandbox.is_playground() {
                "\nIn this playground, delivery is `delivery_merge` alone, on green Yulius CI: no \
                 pull request, deploy or promotion.\n"
            } else {
                ""
            };
            format!("\n\n## Workflow instructions\n\n{clipped}\n{note}")
        });
        let chain = self.chain_briefing(task).await;
        let budget = DEFAULT_BUDGET_BYTES
            .saturating_sub(workflow.as_ref().map_or(0, String::len))
            .saturating_sub(chain.as_ref().map_or(0, String::len));
        let mut briefing = build_briefing(&input, budget);
        if let Some(workflow) = workflow {
            briefing.push_str(&workflow);
        }
        if let Some(chain) = chain {
            briefing.push_str(&chain);
        }
        Ok(briefing)
    }

    async fn hook_prompt_submit(
        self: &Arc<Self>,
        ctx: &AgentContext,
        input: &HookInput,
        sent: i64,
        origin: SubmitOrigin,
        viewer_input: bool,
    ) -> AppResult<HookReply> {
        let typed = origin == SubmitOrigin::Unmarked
            && crate::finish::is_typed_prompt(input.prompt.as_deref());
        let store = &self.deps.store;
        // A person wrote to the task (they typed it, even a prompt held below): what its agent
        // asked a person is answered (SPEC-134 CDT-004). Prompts Yulius types do not count.
        if typed
            && (ctx.task.mode == TaskMode::Prompt || viewer_input)
            && self.typed_recently(&ctx.task.id).is_some()
        {
            self.end_needs_you(&ctx.task.id).await;
        }
        // A prompt held for capacity stays held while its pause lasts (also across a restart).
        let refreshed;
        let ctx = match store.open_pause(&ctx.task.id).await? {
            Some(pause) if pause.kind == PauseKind::Prompt => {
                return Ok(self.hold_for(&ctx.task.id));
            }
            // A held turn the user interrupted to type something: the new prompt replaces it,
            // and the task runs again until this prompt is held or goes on.
            Some(_) => {
                self.hold_ended(&ctx.task.id, "superseded", Activity::Waiting)
                    .await;
                refreshed = AgentContext {
                    attempt: ctx.attempt.clone(),
                    task: store.task(&ctx.task.id).await?.ok_or(AppError::NotFound)?,
                };
                &refreshed
            }
            None => ctx,
        };
        // Scope discipline (TSK-013): a held prompt never reaches the agent, so nothing below
        // (capacity, activity) happens for it. A prompt typed into a task held
        // at its turn end (paused for capacity) is checked like any other.
        if origin != SubmitOrigin::Scheduled
            && let Some(held) = self.screen_prompt(ctx, input).await?
        {
            return Ok(HookReply::Output(held));
        }
        // The start of a turn: with no subscription below the threshold, the prompt waits here
        // until capacity returns (ROT-010). A prompt too long to carry to another account runs;
        // a limit the CLI reports pauses it then.
        let carried = input
            .prompt
            .as_deref()
            .filter(|p| !p.trim().is_empty() && p.len() <= CONTINUATION_MAX);
        if carried.is_some() || input.prompt.is_none() {
            let current = ctx.attempt.account_id.clone();
            let pool = self.rotation_input(&ctx.task, current.clone()).await?;
            if let (Decision::Wait { .. }, _) = rotation::decide(&pool, Trigger::Proactive) {
                // A replacement process carries the accepted prompt and has no viewer keys
                // of its own. Record new work before the pause becomes eligible to resume.
                if ctx.task.mode == TaskMode::Goal && viewer_input && typed {
                    let runtime = self.runtime(&ctx.task.id);
                    let _order = runtime.turn_order.lock().await;
                    if sent >= runtime.turn_end_ms.load(Ordering::SeqCst) {
                        self.reset_stop_budget(&ctx.task).await?;
                    }
                }
                self.pause_task(
                    &ctx.task.id,
                    PauseKind::Prompt,
                    current,
                    carried.map(str::to_owned),
                )
                .await?;
                return Ok(self.hold_for(&ctx.task.id));
            }
        }
        // The session took its first prompt: a first message Yulius typed is no longer pending
        // (SPEC-068).
        crate::supervisor::opening_sent(&ctx.attempt.run_dir);
        let runtime = self.runtime(&ctx.task.id);
        // Answered after a Stop or goal verdict that came after it (its client gave up waiting
        // and the agent went on, ADR-0115): its turn is over, and what that turn's end decided
        // stands. The transcript still shows the turn. Held until Working is set, so a turn end
        // arriving meanwhile settles after it (SPEC-346 LPH-002).
        let _order = runtime.turn_order.lock().await;
        if sent < runtime.turn_end_ms.load(Ordering::SeqCst) {
            tracing::info!(task = %ctx.task.id, "a prompt answered after its turn ended changes nothing");
            return Ok(HookReply::Output(HookOutput::default()));
        }
        crate::race::reached(
            crate::race::Point::PromptTurnChecked,
            std::path::Path::new(ctx.task.id.as_str()),
        );
        // A typed prompt starts new work: an achieved goal no longer makes the task done
        // (TSK-014). Claude Code's own wake-ups (a background shell ended) keep it.
        if typed {
            self.prompt_submitted(&ctx.task.id);
        }
        // `/compact` runs no turn and no Stop hook ends it: the task stays as it was (SPEC-085
        // STT-005).
        if origin != SubmitOrigin::Scheduled
            && !crate::finish::prompt_starts_work(input.prompt.as_deref())
        {
            return Ok(HookReply::Output(HookOutput::default()));
        }
        // As it is now: a Stop of the turn before may have settled it since it was read.
        let task = store.task(&ctx.task.id).await?.ok_or(AppError::NotFound)?;
        if task.mode == TaskMode::Prompt || viewer_input {
            self.reset_stop_budget(&task).await?;
        }
        runtime.resumed_turn.store(false, Ordering::SeqCst);
        runtime.background_agents.store(0, Ordering::SeqCst);
        // The retry's prompt (or a person's) starts the turn an armed retry waited for.
        runtime.goal_retry_armed.store(false, Ordering::SeqCst);
        runtime.turn_began_ms.store(self.now().0, Ordering::SeqCst);
        // Claude Code's own wake-up goes on with the stretch; a typed prompt starts a new one.
        let stretch = if typed {
            None
        } else {
            self.continue_working_stretch(&ctx.task.id).await
        };
        self.set_task_activity(&task, Activity::Working).await?;
        // Only now, with the task gone from idle to Working by a prompt sent after the pull
        // request watch typed: the turn of that news has begun (SPEC-487 CHN-002). Not for a
        // task that already worked: the prompt hook of news typed while a turn runs comes at
        // the Enter, and that news starts its own turn only after the running one.
        if task.activity != Some(Activity::Working) {
            self.pr_news_turn_began(&task.id, Some(sent));
        }
        self.keep_working_stretch(&task.id, stretch).await;
        // A workflow of the session is paused at a usage limit: at a prompt a person or Yulius
        // typed, its agent is told to resume it, not to start it again (SPEC-486 WFL-006).
        // Claude Code's own wake-ups say what ended; they get nothing added.
        let now = self.now();
        if typed
            && let Some(runs) =
                runtime.with_workflows(|workflows: &mut crate::dynamic_workflow::Unfinished| {
                    workflows.paused(now)
                })
        {
            return Ok(HookReply::Output(HookOutput::additional_context(
                events::USER_PROMPT_SUBMIT,
                crate::dynamic_workflow::paused_context(&runs),
            )));
        }
        Ok(HookReply::Output(HookOutput::default()))
    }

    async fn reset_stop_budget(&self, task: &TaskRecord) -> AppResult<()> {
        if task.gate_blocks != 0 {
            let update = TaskFieldUpdate {
                gate_blocks: Some(0),
                ..Default::default()
            };
            self.deps
                .store
                .update_task(&task.id, &update, self.now())
                .await?;
        }
        Ok(())
    }

    /// A clean goal turn continues declared work unless another actor already owns the next action.
    async fn stop_gate(&self, task: &TaskRecord, reported_background: bool) -> AppResult<StopGate> {
        if let Some(reason) = self.spec_gate(task).await? {
            return Ok(StopGate::Continue(if task.mode == TaskMode::Goal {
                retained_goal_instruction(Some(format!("{GOAL_CONTINUATION}{reason}")))
            } else {
                reason
            }));
        }
        if task.mode != TaskMode::Goal {
            return Ok(StopGate::Pass);
        }
        if reported_background {
            return Ok(StopGate::Handoff);
        }
        if self.turn_handoff_pending(task).await? {
            return Ok(StopGate::Handoff);
        }
        // These observations share a small part of the five-second hook deadline. Only
        // confirmed work owns a wake. An unavailable probe can safely continue this same
        // turn; the separate rotation decision still requires proof before moving a run.
        let budget = std::time::Duration::from_millis(750);
        let (background, delivery, browser, pull) = tokio::join!(
            tokio::time::timeout(budget, self.background_or_wait_now(&task.id)),
            tokio::time::timeout(budget, self.pr_news_due(task)),
            tokio::time::timeout(budget, self.browser_tab_requests(&task.sandbox_id)),
            // Local reads only; a slow store or Git ends this turn as on a settled PR.
            tokio::time::timeout(budget, self.stop_pull(task))
        );
        Ok(
            if matches!(background, Ok(Some(true)))
                || delivery.unwrap_or(false)
                || browser.is_ok_and(|tasks| tasks.contains(task.id.as_str()))
            {
                StopGate::Handoff
            } else {
                StopGate::Declared(pull.unwrap_or(StopPull::Settled))
            },
        )
    }

    /// A Codex goal has no native goal check: its own pull request failing Yulius CI at the
    /// head its worktree is on is work left (SPEC-558 PKR-003). The evidence is local: an
    /// action never rests on a display read of GitHub (ADR-0447), and a head the agent has
    /// moved past, pushed or not, is not charged to the retry budget.
    async fn stop_pull(&self, task: &TaskRecord) -> StopPull {
        if task.harness != Harness::Codex || task.mode != TaskMode::Goal {
            return StopPull::Settled;
        }
        let Ok(Some(record)) = self.deps.store.delivery(&task.id).await else {
            return StopPull::Settled;
        };
        let (yulius_domain::delivery::DeliveryStage::PrOpen, Some(pr), Some(worktree)) = (
            &record.stage,
            record.pr_number,
            task.worktree_path.as_deref(),
        ) else {
            return StopPull::Settled;
        };
        let Ok(Some(head)) = self
            .deps
            .git
            .resolve_commit(Path::new(worktree), "HEAD")
            .await
        else {
            return StopPull::Settled;
        };
        // The newest run of this pull request at this commit, however busy the sandbox is.
        let Ok(runs) = self
            .deps
            .store
            .ci_runs_for_sha(&task.sandbox_id, &head.to_ascii_lowercase())
            .await
        else {
            return StopPull::Settled;
        };
        let reference = yulius_domain::ci::CiRef::PullRequest(pr);
        let Some(run) = runs.into_iter().find(|run| run.reference == reference) else {
            return StopPull::Settled;
        };
        // Untold, the failure is the watch's to paste in first (SPEC-578). A newer run of the
        // pull request, or a pushed head past this commit, supersedes it as the watch says.
        if run.state != yulius_domain::ci::RunState::Failed
            || !self.run_failure_told(&task.id, pr, &run.id)
        {
            return StopPull::Settled;
        }
        // The watch's window of recent runs: a newer run of the pull request in it supersedes
        // this one; an unreadable window continues nothing.
        let Ok(recent) = self
            .deps
            .store
            .ci_runs(&task.sandbox_id, crate::pr_watch::RUNS_READ)
            .await
        else {
            return StopPull::Settled;
        };
        let newer = recent
            .iter()
            .find(|newest| newest.reference == reference)
            .is_some_and(|newest| newest.id != run.id);
        if newer || self.run_superseded_by_pushed_head(task, &run.sha).await {
            StopPull::Settled
        } else {
            StopPull::Failing(pr)
        }
    }

    async fn turn_handoff_pending(&self, task: &TaskRecord) -> AppResult<bool> {
        let store = &self.deps.store;
        Ok(task.activity == Some(Activity::NeedsAttention)
            || self.active_waits_of(&task.id) > 0
            || store.task_staging_wait(&task.id).await?.is_some()
            || store.task_needs_you(&task.id).await?.is_some()
            || store.pending_hold(&task.id).await?.is_some()
            || self.is_paused(task).await)
    }

    /// Local work is read after live observations, under the plan writers' lock at Stop.
    async fn declared_stop_work(
        &self,
        task: &TaskRecord,
        run_dir: Option<&Path>,
        pull: StopPull,
    ) -> AppResult<StopGate> {
        if self.turn_handoff_pending(task).await? {
            return Ok(StopGate::Handoff);
        }
        // A held opening turn still owns its instruction. Claude gets its native goal
        // after that turn ends, even if the goal marker already exists for recovery.
        let runtime = self.runtime(&task.id);
        if task.follows_goal()
            && !runtime.opening_turn.load(Ordering::SeqCst)
            && !runtime.goal_submitted.load(Ordering::SeqCst)
            && run_dir.is_some_and(crate::supervisor::native_goal_pending)
        {
            return Ok(StopGate::NativeGoal);
        }
        if self.goal_is_open(&task.id) {
            return Ok(StopGate::Pass);
        }
        let store = &self.deps.store;
        let steps = crate::discipline::step_states(&store.steps(&task.id).await?, &[]);
        let reason = if let Some(index) = yulius_domain::steps::current(&steps) {
            let next = &steps[index].title;
            format!(
                "{GOAL_CONTINUATION}The next unfinished step is \"{next}\". Record the evidence and update the step when its work is complete."
            )
        } else if steps.iter().any(|step| !step.status.is_resolved()) {
            return Ok(StopGate::Handoff);
        } else if self.read_spec_unfinished(&task.id).await? {
            format!(
                "{GOAL_CONTINUATION}The spec still declares unfinished work. Check its acceptance criteria, complete any remaining work and record the evidence."
            )
        } else {
            match pull {
                StopPull::Settled => return Ok(StopGate::Pass),
                StopPull::Failing(pr) => format!(
                    "{GOAL_CONTINUATION}Yulius CI failed on pull request #{pr} at this head. Read the failure (ci_status), repair it on this branch and commit the fix."
                ),
            }
        };

        Ok(StopGate::Continue(reason))
    }

    /// What a session resumed on another subscription should do after a held turn.
    pub(crate) async fn stop_continuation(&self, task: &TaskRecord) -> Option<String> {
        if task.mode == TaskMode::Prompt {
            return if task.gate_blocks < MAX_GATE_BLOCKS {
                self.spec_gate(task).await.ok().flatten()
            } else {
                None
            };
        }
        let gate = self.stop_gate(task, false).await.ok()?;
        let runtime = self.runtime(&task.id);
        let _work = runtime.steps_lock.lock().await;
        let task = self.deps.store.task(&task.id).await.ok()??;
        if task.gate_blocks >= MAX_GATE_BLOCKS {
            return None;
        }
        let gate = if let StopGate::Declared(pull) = gate {
            let run = if task.follows_goal() {
                self.deps.store.live_attempt(&task.id).await.ok()?
            } else {
                None
            };
            self.declared_stop_work(&task, run.as_ref().map(|run| Path::new(&run.run_dir)), pull)
                .await
                .ok()?
        } else {
            gate
        };
        match gate {
            StopGate::Continue(reason) => Some(reason),
            StopGate::Pass | StopGate::Handoff | StopGate::NativeGoal | StopGate::Declared(_) => {
                None
            }
        }
    }

    async fn hook_stop(
        self: &Arc<Self>,
        ctx: &AgentContext,
        input: &HookInput,
    ) -> AppResult<HookReply> {
        let store = &self.deps.store;
        let mut task = &ctx.task;
        // A turn its model API failed ends for good: its CLI goes on with nothing Yulius says,
        // so no gate, nudge or goal holds it (SPEC-505 AGY-006).
        let antigravity = ctx.attempt.harness == Harness::Antigravity;
        let failed = antigravity && input.error.is_some();
        let reported_background = antigravity && !failed && input.fully_idle == Some(false);
        // Antigravity sets `fullyIdle: false` for *any* live background task, including `yulius
        // wait`. Distinguish wait-only parking (idle for the merge queue and shown as waiting on
        // Yulius), observable non-wait PTY/child shells (which `probe_idle_shells` clears as soon
        // as the process exits), and in-process background work such as subagents or an
        // unreadable probe (which stays tracked in `background_agents` until `fullyIdle: true` or
        // the next prompt; SPEC-580 AWM-001, review F2).
        let (agy_process_shells, agy_inprocess_background, agy_probe_waits) = if reported_background
        {
            let active_waits = self.active_waits_of(&task.id);
            match self.background_probe_now(&task.id).await {
                Some(found) => {
                    let waits = found.waits.max(active_waits);
                    let inprocess = found.shells == 0 && waits == 0;
                    (found.shells, inprocess, waits)
                }
                None => (0, active_waits == 0, active_waits),
            }
        } else {
            (0, false, 0)
        };
        let background = agy_process_shells > 0 || agy_inprocess_background;
        // A turn held for capacity stays held while its pause lasts (also across a restart).
        let refreshed;
        let mut preceding_turn = false;
        if let Some(pause) = store.open_pause(&task.id).await? {
            if matches!(pause.kind, PauseKind::Turn | PauseKind::Limit) {
                return Ok(self.hold_for(&task.id));
            }
            // Claude runs the submit hook at Enter, even while the preceding turn runs.
            preceding_turn = pause.kind == PauseKind::Prompt && pause.during_turn;
            if !preceding_turn {
                // The prompt was held from idle: this Stop belongs to the turn that gave up its hold.
                self.hold_ended(&task.id, "superseded", Activity::Working)
                    .await;
                refreshed = store.task(&task.id).await?.ok_or(AppError::NotFound)?;
                task = &refreshed;
            }
        }
        // The opening turn of a resumed goal session (its continuation prompt) ended, not held:
        // it stops as it is, without gates or rotation, so the goal is typed next (SPEC-025,
        // SPEC-117).
        let opening = self
            .runtime(&task.id)
            .opening_turn
            .swap(false, Ordering::SeqCst);
        if opening && task.mode == TaskMode::Goal {
            return Ok(HookReply::Output(HookOutput::default()));
        }
        if let Err(err) = self.sync_steps_from_spec(task).await {
            tracing::warn!(error = %err, task = %task.id, "step sync from the spec failed");
        }
        if antigravity && input.fully_idle == Some(true) {
            // This Stop supersedes a previous in-process worker report, even if the
            // PreInvocation of the new turn was missed. Live launches are probed separately.
            let runtime = self.runtime(&task.id);
            runtime.background_agents.store(0, Ordering::SeqCst);
            runtime.background_shells.store(0, Ordering::SeqCst);
            runtime.shells_since_ms.store(0, Ordering::SeqCst);
        }
        if preceding_turn {
            // Preserve the later prompt's pause. Only the submit client keeps its hold alive.
            return Ok(
                if !failed && (task.follows_goal() || self.stop_continuation(task).await.is_some())
                {
                    hold()
                } else {
                    HookReply::Output(HookOutput::default())
                },
            );
        }

        // Prompt-mode spec discipline is known before the rotation and transcript
        // checks. Its capacity and licensing decisions retain their existing order.
        let prompt_gate = if task.mode == TaskMode::Prompt && !failed {
            Some(self.stop_gate(task, reported_background).await?)
        } else {
            None
        };
        // The turn boundary: stay, move to another subscription, or pause (ROT-003, ROT-010).
        self.runtime(&task.id)
            .proactive_switch
            .store(false, Ordering::SeqCst);
        let current = ctx.attempt.account_id.clone();
        let pool = self.rotation_input(task, current.clone()).await?;
        let decision = rotation::decide(&pool, Trigger::Proactive);
        // A Stop a hook blocks writes no closing record: what such Stops leave in Claude Code's
        // queue is told at each Stop, after the turn's lines were read when a move may be made
        // here (SPEC-493 DEF-001).
        let read = (matches!(decision.0, Decision::Switch { .. })
            && !self.switch_cooling(&task.id))
            || self.runtime(&task.id).reviewer_move.load(Ordering::SeqCst);
        if read {
            self.await_transcript_read(&task.id).await;
        }
        let now = self.now();
        self.runtime(&task.id)
            .with_queue(|queue| queue.stop_seen(now, read));
        // A reviewer a usage limit cut off moves the session to a subscription that can run
        // its model, at this turn's end, as a switch the pool decides would (SPEC-493 RLM-002).
        // While a round is open its cut-off may be in this turn's lines: they are read first.
        if matches!(decision.0, Decision::Stay) && self.review_round_open(&task.id).await {
            self.await_transcript_read(&task.id).await;
        }
        let mut move_reviewer = None;
        if matches!(decision.0, Decision::Stay)
            && self
                .runtime(&task.id)
                .reviewer_move
                .swap(false, Ordering::SeqCst)
        {
            let later = || {
                self.runtime(&task.id)
                    .reviewer_move
                    .store(true, Ordering::SeqCst);
            };
            match self.reviewer_move_target(task, current.clone()).await {
                crate::review::ReviewerMove::To(account) => {
                    let prompts_before = self.prompts_passed(&task.id);
                    self.await_transcript_read(&task.id).await;
                    if background
                        || self.background_work_now(&task.id).await != Some(false)
                        || self.prompt_waits_for_turns_end(&task.id, prompts_before)
                    {
                        later();
                    } else {
                        move_reviewer = Some(account);
                    }
                }
                crate::review::ReviewerMove::Later => later(),
                crate::review::ReviewerMove::Nothing => {}
            }
        }
        // A switch restarts the session and ends its background shells, agents and monitors: a
        // build, gate or review the agent left running would be lost (tasks #3 and #5, F
        // 2026-10-11/12, #271). An agent launched in this turn is known only from the lines
        // written before this Stop: they are read first (SPEC-079 BGW-004). It also ends what
        // Claude Code holds in its queue for the session, which a resumed session does not get
        // back: a prompt entered while this turn ran, by its person or by Yulius for one. While
        // one waits the session stays, is answered as on a subscription that stays, and moves
        // at a later turn's end (SPEC-479 SWQ-001). Asked after the same read. A look at the
        // background work that gives no answer counts as work running, as for everyone who
        // asks before a session is ended (SPEC-483 SWT-001).
        let (busy, prompt_waits) = if matches!(decision.0, Decision::Switch { .. })
            && !self.switch_cooling(&task.id)
        {
            let prompts_before = self.prompts_passed(&task.id);
            self.await_transcript_read(&task.id).await;
            let work = self.background_work_now(&task.id).await;
            if work.is_none() {
                tracing::info!(
                    task = %task.id,
                    "the session is not moved to another subscription at this turn's end: its background work could not be told"
                );
            }
            let busy = work != Some(false);
            crate::race::reached(
                crate::race::Point::SwitchProbed,
                std::path::Path::new(task.id.as_str()),
            );
            (
                busy,
                self.prompt_waits_for_turns_end(&task.id, prompts_before),
            )
        } else {
            (false, false)
        };
        let gate = if failed {
            StopGate::Handoff
        } else if let Some(gate) = prompt_gate {
            gate
        } else {
            self.stop_gate(task, reported_background).await?
        };
        // A session that works on after this boundary (a gate's block, its goal) and runs an
        // older Claude Code than runs get now restarts on the newer one here, as a move to
        // another subscription does (SPEC-461 CCM-013): the resumed session gets the gate's
        // reasons, and its goal again. An idle session moves once its turn has settled.
        // A goal that is achieved, or not set, is not typed again for it: such a session waits
        // for its next start. Nor while a goal its person set in a prompt-mode session is open,
        // which Yulius could not follow across the restart: that session stays, and the gate
        // blocks as usual (SPEC-468 CCF-001). A goal known by now saves the look at the session;
        // one set in this very turn may not be read yet, so it is asked again after the look,
        // which reads the turn's lines.
        let version = if matches!(decision.0, Decision::Stay)
            && move_reviewer.is_none()
            && !self.keeps_own_goal(task)
            && ((matches!(gate, StopGate::Continue(_) | StopGate::Declared(_))
                && (task.mode == TaskMode::Goal || task.gate_blocks < MAX_GATE_BLOCKS))
                || (task.mode == TaskMode::Goal && self.goal_is_open(&task.id)))
        {
            self.claude_code_move_due(&task.id, &ctx.attempt.id).await
        } else {
            None
        };

        // A worker may complete the selected step during any of the probes above.
        // Plan writers reset the budget under this lock. Keep it from the final local
        // read through issuing the continuation or recording exhaustion.
        let runtime = self.runtime(&task.id);
        let declared = matches!(gate, StopGate::Declared(_));
        let _work = if declared {
            Some(runtime.steps_lock.lock().await)
        } else {
            None
        };
        if !self.attempt_live(&task.id, &ctx.attempt.id).await {
            return Ok(HookReply::Output(HookOutput::default()));
        }
        let current_task = store.task(&task.id).await?.ok_or(AppError::NotFound)?;
        task = &current_task;
        let gate = if let StopGate::Declared(pull) = gate {
            self.declared_stop_work(task, Some(Path::new(&ctx.attempt.run_dir)), pull)
                .await?
        } else {
            gate
        };
        let clears_budget = task.mode == TaskMode::Prompt
            || (matches!(gate, StopGate::Pass) && !self.goal_is_open(&task.id));
        let can_nudge =
            matches!(gate, StopGate::Pass) || (declared && matches!(gate, StopGate::Continue(_)));
        let pending_goal = matches!(gate, StopGate::NativeGoal);
        let mut reason = match gate {
            StopGate::Continue(reason) => Some(reason),
            StopGate::Pass | StopGate::Handoff | StopGate::NativeGoal | StopGate::Declared(_) => {
                None
            }
        };
        if task.mode == TaskMode::Goal
            && let Some(reason) = &reason
            && task.gate_blocks >= MAX_GATE_BLOCKS
        {
            if move_reviewer.is_some() {
                runtime.reviewer_move.store(true, Ordering::SeqCst);
            }
            return self.exhausted_stop(task, reason).await;
        }

        let continuation = reason
            .as_ref()
            .filter(|_| task.gate_blocks < MAX_GATE_BLOCKS);
        let gate_continues = continuation.is_some();
        let continues = gate_continues || (task.follows_goal() && !failed);
        if let Some(account) = move_reviewer {
            let start = RunStart::Resume {
                prompt: continuation.cloned(),
            }
            .with_goal_continuation(task.mode == TaskMode::Goal);
            let app = Arc::clone(self);
            let task_id = task.id.clone();
            tokio::spawn(async move {
                if let Err(err) = app
                    .switch_with(&task_id, Some(account), crate::review::REVIEW_RESUME, start)
                    .await
                {
                    tracing::warn!(error = %err, task = %task_id, "the move for a cut-off reviewer failed");
                    // Due again at a later turn's end (review round 1 of fix 7).
                    app.runtime(&task_id)
                        .reviewer_move
                        .store(true, Ordering::SeqCst);
                }
            });
            return Ok(self.hold_for(&task.id));
        }
        if let Some(version) = version
            && !self.keeps_own_goal(task)
            && (gate_continues
                || (task.mode == TaskMode::Goal
                    && self.goal_is_open(&task.id)
                    && !runtime.goal_achieved.load(Ordering::SeqCst)))
        {
            let start = RunStart::Resume {
                prompt: continuation.cloned(),
            }
            .with_goal_continuation(task.mode == TaskMode::Goal);
            let app = Arc::clone(self);
            let task_id = task.id.clone();
            let run = (ctx.attempt.id.clone(), current.clone());
            tokio::spawn(async move {
                app.move_to_claude_code(&task_id, run, version, start).await;
            });
            return Ok(self.hold_for(&task.id));
        }
        let stays = match decision {
            (Decision::Stay, _) => true,
            (Decision::Switch { .. }, _) if background || busy => true,
            (Decision::Switch { .. }, _) if prompt_waits => {
                tracing::info!(
                    task = %task.id,
                    "the session is not moved to another subscription at this turn's end: a prompt waits in Claude Code's queue"
                );
                true
            }
            // A switch was refused moments ago (the new session could not start): the agent
            // keeps working on its current subscription instead of being held.
            (Decision::Switch { .. }, _) if self.switch_cooling(&task.id) => true,
            (Decision::Switch { account }, reason) => {
                // The agent waits here while it moves; the resumed session gets the gate's
                // reasons as its prompt when the gate would have kept it working.
                let start = RunStart::Resume {
                    prompt: continuation.cloned(),
                }
                .with_goal_continuation(task.mode == TaskMode::Goal);
                let app = Arc::clone(self);
                let task_id = task.id.clone();
                let reason = format!("{reason:?}");
                tokio::spawn(async move {
                    if let Err(err) = app
                        .switch_with(&task_id, Some(account), &reason, start)
                        .await
                    {
                        tracing::warn!(error = %err, task = %task_id, "switch at the turn boundary failed");
                    }
                });
                return Ok(self.hold_for(&task.id));
            }
            (Decision::Wait { .. }, _) if continues => {
                let carried = continuation
                    .filter(|_| task.mode == TaskMode::Goal)
                    .cloned();
                self.pause_task(&task.id, PauseKind::Turn, current, carried)
                    .await?;
                return Ok(self.hold_for(&task.id));
            }
            // The agent goes idle anyway; its next prompt is held if capacity is still gone.
            (Decision::Wait { .. }, _) => false,
        };

        // A clean turn end on the same subscription may carry one gentle step nudge (STP-008).
        // It is not a gate block: it is given once and never escalates to "needs attention".
        if can_nudge && stays && !reported_background {
            match self.take_turn_end_nudge(task).await {
                Ok(Some(text)) => match &mut reason {
                    Some(reason) => {
                        reason.push_str("\n\n");
                        reason.push_str(&text);
                    }
                    None => return Ok(HookReply::Output(HookOutput::block(text))),
                },
                Ok(None) => {}
                Err(err) => tracing::warn!(error = %err, task = %task.id, "step nudge unavailable"),
            }
        }
        let Some(reason) = reason else {
            drop(_work);
            if clears_budget && task.gate_blocks != 0 {
                let update = TaskFieldUpdate {
                    gate_blocks: Some(0),
                    ..TaskFieldUpdate::default()
                };
                store.update_task(&task.id, &update, self.now()).await?;
            }
            // Every clean Antigravity Stop ends the model turn (`Activity::Waiting`): non-wait
            // background work stays tracked in `background_agents`/`background_shells` (preserving
            // SPEC-528 without leaving `activity = Working`), while wait-only stops clear
            // `background_shells` and record `background_waits` so the merge queue proceeds
            // immediately (SPEC-580 AWM-001, AWM-002).
            if antigravity {
                let runtime = self.runtime(&task.id);
                let now_ms = self.now().0;
                if agy_process_shells > 0 {
                    // Process-table shells are tracked directly (not pinned in `background_agents`)
                    // so `probe_idle_shells` clears `background_shells` back to 0 as soon as the
                    // command exits, even without another Stop hook (SPEC-580 review F2).
                    runtime.background_agents.store(0, Ordering::SeqCst);
                    let previous = runtime
                        .background_shells
                        .swap(i64::from(agy_process_shells), Ordering::SeqCst);
                    if previous <= 0 {
                        runtime.shells_since_ms.store(now_ms, Ordering::SeqCst);
                    }
                    if agy_probe_waits > 0 {
                        runtime
                            .background_waits
                            .store(i64::from(agy_probe_waits), Ordering::SeqCst);
                    }
                } else if agy_inprocess_background {
                    // In-process background work (e.g. subagent, or probe unavailable): pin
                    // `background_agents = 1` so SPEC-528 never flickers Needs You until
                    // `fullyIdle: true` or the next user prompt arrives.
                    runtime.background_agents.store(1, Ordering::SeqCst);
                    if runtime.background_shells.load(Ordering::SeqCst) <= 0 {
                        runtime.background_shells.store(1, Ordering::SeqCst);
                        runtime.shells_since_ms.store(now_ms, Ordering::SeqCst);
                    }
                } else {
                    runtime.background_agents.store(0, Ordering::SeqCst);
                    runtime.background_shells.store(0, Ordering::SeqCst);
                    runtime.shells_since_ms.store(0, Ordering::SeqCst);
                    if reported_background && agy_probe_waits > 0 {
                        runtime
                            .background_waits
                            .store(i64::from(agy_probe_waits), Ordering::SeqCst);
                    }
                }
            }
            // Done when the goal is achieved, waiting otherwise (TSK-014), once the goal verdict
            // came while a goal is open (SPEC-079 BGW-005).
            self.clean_stop(task, &ctx.attempt.id, pending_goal).await?;
            return Ok(HookReply::Output(HookOutput::default()));
        };

        if task.gate_blocks >= MAX_GATE_BLOCKS {
            return self.exhausted_stop(task, &reason).await;
        }
        store
            .consume_stop_attempt(&task.id, MAX_GATE_BLOCKS, self.now())
            .await?;
        if task.mode == TaskMode::Prompt {
            self.publish_task(&task.id).await;
        }
        // A previously missed submit or an older capacity release can leave the row idle.
        // Only recover that same idle state; a permission raised meanwhile keeps its flag.
        if task.mode == TaskMode::Goal
            && !matches!(
                task.activity,
                Some(Activity::Working | Activity::NeedsAttention)
            )
            && let Some(times) = store.task_times(&task.id).await?
            && store
                .set_activity_from(
                    &task.id,
                    (task.activity, times.state_at),
                    Some(Activity::Working),
                    self.now(),
                )
                .await?
        {
            self.publish_task(&task.id).await;
        }
        Ok(HookReply::Output(HookOutput::block(reason)))
    }

    /// A repeated gate reason survives as the explanation shown to the task owner.
    async fn exhausted_stop(&self, task: &TaskRecord, reason: &str) -> AppResult<HookReply> {
        let (stopped, action) = if let Some(work) = reason.strip_prefix(GOAL_CONTINUATION) {
            (
                format!(
                    "could not finish after {MAX_GATE_BLOCKS} automatic attempts. {}",
                    work.chars().take(300).collect::<String>()
                ),
                "Review the last attempt in the task terminal to identify what blocked completion.",
            )
        } else {
            (
                format!(
                    "stopped {MAX_GATE_BLOCKS} times without fixing: {}",
                    reason.chars().take(300).collect::<String>()
                ),
                "Tell it how to go on, in the terminal.",
            )
        };
        // Said by the task itself, for whoever opens it later (SPEC-472), and in its
        // terminal for the viewers there now.
        self.flag_attention(
            &task.id,
            crate::attention::AttentionRecord::yulius(&format!("The agent {stopped}"), action),
        )
        .await?;
        self.runtime(&task.id)
            .hub
            .notice(format!("Yulius: the agent {stopped}"));
        Ok(HookReply::Output(HookOutput::default()))
    }

    /// A turn of a run without hooks ended (seen in its transcript): run a pending proactive
    /// rotation at this boundary (SPEC-002 §6.3).
    pub(crate) fn turn_ended(
        self: &Arc<Self>,
        task_id: &yulius_domain::ids::TaskId,
        account: Option<yulius_domain::ids::AccountId>,
    ) {
        let runtime = self.runtime(task_id);
        if runtime.proactive_switch.swap(false, Ordering::SeqCst) {
            let app = Arc::clone(self);
            let task_id = task_id.clone();
            tokio::spawn(async move {
                app.rotate(&task_id, account, Trigger::Proactive).await;
            });
        }
    }

    /// The discipline gate on the task spec: `Some(reason)` when the worktree has changes, the
    /// task needs a spec (its workflow, or a change beyond documentation, DSC-013) and the spec
    /// is missing or invalid.
    async fn spec_gate(&self, task: &TaskRecord) -> AppResult<Option<String>> {
        let Some(worktree) = task.worktree_path.as_ref() else {
            return Ok(None);
        };
        let worktree = PathBuf::from(worktree);
        if !self.branch_changed(task).await? || !self.spec_required(task).await? {
            return Ok(None);
        }
        // A required spec that does not exist yet blocks the turn here, not only the pull
        // request later (F 2026-10-11: features finished without one, and nothing opened); a
        // renamed spec is followed by its frontmatter.
        let spec_path = match self.task_spec_path(task, &worktree).await {
            Ok(path) => path,
            Err(dir) => {
                let place = task
                    .spec_path
                    .as_deref()
                    .map_or_else(|| format!("under `{dir}`"), |path| format!("at `{path}`"));
                return Ok(Some(format!(
                    "This task needs a spec (a feature, a fix, or a change beyond documentation) \
                     and none exists yet. Write it {place} in the Yulius format with \
                     `yulius-task: {}` in its frontmatter (spec_check validates it), keep its \
                     Plan in your steps, commit it and carry on. Decide its content yourself from \
                     the request and the code; do not ask anyone.",
                    task.id
                )));
            }
        };
        Ok(match read_spec(&worktree, &spec_path).await {
            Err(err) => Some(format!(
                "This task needs a readable spec. {err}. Check `{spec_path}` and run spec_check before finishing."
            )),
            Ok(text) => {
                let report = validate_spec_for(&text, &task.id);
                (!report.is_valid()).then(|| report.agent_message(&spec_path))
            }
        })
    }
}

/// `file` (absolute, or relative to `base`) with `.` and `..` resolved lexically.
fn lexical_path(base: &Path, file: &str) -> PathBuf {
    use std::path::Component;
    let mut normal = PathBuf::new();
    for part in base.join(file).components() {
        match part {
            Component::ParentDir => {
                normal.pop();
            }
            Component::CurDir => {}
            other => normal.push(other),
        }
    }
    normal
}

/// `path` relative to `root`; `None` outside it or for `root` itself.
fn relative_to(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?.to_str()?;
    (!rel.is_empty()).then(|| rel.to_owned())
}

/// `file` (absolute, or relative to `base`) as a path relative to `worktree`, with `.` and
/// `..` resolved lexically; `None` outside it or for the worktree itself.
fn worktree_relative(worktree: &Path, base: &Path, file: &str) -> Option<String> {
    relative_to(worktree, &lexical_path(base, file))
}

/// `path` with its longest existing ancestor resolved through links (the rest kept as written),
/// so a file not yet created is placed by its folder.
fn resolved_through_links(path: &Path) -> Option<PathBuf> {
    let mut rest = Vec::new();
    let mut probe = path;
    loop {
        if let Ok(real) = std::fs::canonicalize(probe) {
            return Some(rest.iter().rev().fold(real, |acc, part| acc.join(part)));
        }
        rest.push(probe.file_name()?.to_owned());
        probe = probe.parent()?;
    }
}

impl App {
    /// `PreToolUse`: the edit gate's answer. It never gives up (SPEC-426 EDG-005): the spec is
    /// documentation, so the way through stays open, and an agent that loops meets the Stop
    /// gate. A failure to decide lets the edit go ahead (the Stop gate still checks).
    async fn hook_pre_tool_use(
        self: &Arc<Self>,
        ctx: &AgentContext,
        input: &HookInput,
    ) -> HookOutput {
        // A subagent's start: a review round's reviewer (SPEC-493 REV-002).
        if input.tool_name.as_deref().is_some_and(agent_tool) {
            return self.review_gate(ctx, input).await;
        }
        match self.edit_gate(&ctx.task, input).await {
            Ok(Some(reason)) => HookOutput::deny_tool(reason),
            Ok(None) => HookOutput::default(),
            Err(err) => {
                tracing::warn!(error = %err, task = %ctx.task.id, "edit gate unavailable");
                HookOutput::default()
            }
        }
    }

    /// The discipline gate before a code edit (SPEC-426 EDG-002): `Some(reason)` when an edit
    /// tool is about to change a file of the task's worktree that is not documentation, the task
    /// needs a spec for such a change, and its spec does not exist or has no Plan item yet.
    async fn edit_gate(&self, task: &TaskRecord, input: &HookInput) -> AppResult<Option<String>> {
        let Some(field) = input
            .tool_name
            .as_deref()
            .and_then(yulius_contracts::hooks::edited_file_field)
        else {
            return Ok(None);
        };
        let Some(file) = input
            .tool_input
            .as_ref()
            .and_then(|tool_input| tool_input.get(field))
            .and_then(serde_json::Value::as_str)
        else {
            return Ok(None);
        };
        let Some(worktree) = task.worktree_path.as_deref().map(PathBuf::from) else {
            return Ok(None);
        };
        // A relative path is the agent's folder's (`cwd`); a path through a link to the
        // worktree is placed by what it resolves to.
        let base = input
            .cwd
            .as_deref()
            .map(PathBuf::from)
            .filter(|cwd| cwd.is_absolute())
            .unwrap_or_else(|| worktree.clone());
        let rel = match worktree_relative(&worktree, &base, file) {
            Some(rel) => rel,
            None => {
                let (root, path) = (worktree.clone(), lexical_path(&base, file));
                let resolved = tokio::task::spawn_blocking(move || {
                    let root = std::fs::canonicalize(&root).ok()?;
                    relative_to(&root, &resolved_through_links(&path)?)
                })
                .await
                .map_err(AppError::internal)?;
                let Some(rel) = resolved else { return Ok(None) };
                rel
            }
        };
        if yulius_domain::spec_format::is_documentation_path(&rel) {
            return Ok(None);
        }
        // A workflow the store cannot read is a failure to decide, which passes, not the
        // spec-by-change default of the Stop gate.
        if !self
            .try_spec_policy(task)
            .await?
            .requires_spec([rel.as_str()], true)
        {
            return Ok(None);
        }
        let place = match self.task_spec_path(task, &worktree).await {
            Ok(spec_path) => {
                let planned = match read_spec(&worktree, &spec_path).await {
                    Ok(text) => validate_spec_for(&text, &task.id)
                        .plan
                        .iter()
                        .any(|item| item.depth == 0 && !item.text.trim().is_empty()),
                    Err(RepoReadError::Missing(_)) => false,
                    Err(_) => true,
                };
                if planned {
                    return Ok(None);
                }
                format!("at `{spec_path}`")
            }
            Err(dir) => task
                .spec_path
                .as_deref()
                .map_or_else(|| format!("under `{dir}`"), |path| format!("at `{path}`")),
        };
        Ok(Some(format!(
            "Yulius: `{rel}` is code, and this task needs its spec with a Plan before its code \
             changes. Write the spec {place} in the Yulius format first (`yulius-task: {}` in its \
             frontmatter, a Plan checklist of similar-effort steps; spec_check validates it), \
             then make this edit again. Documentation edits are not held. Decide the spec's \
             content yourself from the request and the code; do not ask anyone.",
            task.id
        )))
    }
}

impl App {
    /// Keeps the task's steps in sync with the Plan checklist of its spec (STP-004): the
    /// top-level plan items (at most [`crate::coordination::MAX_STEPS`]) become the steps in
    /// order; checked items are `done`; unchecked items keep the status and effort of the
    /// existing step with the same title (agents may mark them `in_progress` before ticking the
    /// box), so unticking never reopens a finished step; new items start `pending`. Writes only
    /// on change. Returns whether the steps changed. It also keeps the spec's `status` with the
    /// task (SPEC-491 DON-004). It runs at a session start, at a turn end and every few seconds
    /// of a live run (SPEC-341 LSS-001), under the task's steps lock.
    ///
    /// # Errors
    /// Store failures.
    pub async fn sync_steps_from_spec(&self, task: &TaskRecord) -> AppResult<bool> {
        self.publish_after_updates(task, Box::pin(self.sync_spec_plan(task)))
            .await
    }

    async fn sync_spec_plan(&self, task: &TaskRecord) -> AppResult<bool> {
        let (Some(worktree), Some(spec_path)) =
            (task.worktree_path.as_ref(), task.spec_path.as_ref())
        else {
            return Ok(false);
        };
        // Locked before the spec is read: a sync that read an older spec never writes it over a
        // newer one written meanwhile (SPEC-341 LSS-002).
        let runtime = self.runtime(&task.id);
        let _steps = runtime.steps_lock.lock().await;
        let root = PathBuf::from(worktree);
        // A spec moved or deleted since it was registered: found again where it is now, or, with
        // none left, its kept status goes (review of SPEC-491, m2). Only while a status is kept:
        // every task is given a path at its start, and a task that never wrote its spec has
        // nothing stale to clear, so it is not made to read the specs folder at every sync
        // (review round 2).
        let rel = if root.join(spec_path).is_file() {
            spec_path.clone()
        } else if root.is_dir()
            && matches!(self.deps.store.spec_status(&task.id).await, Ok(Some(_)))
        {
            match self.task_spec_path(task, &root).await {
                Ok(found) => found,
                Err(_) => {
                    self.note_spec_status(&task.id, None).await;
                    return Ok(false);
                }
            }
        } else {
            return Ok(false);
        };
        let report = match read_spec(&root, &rel).await {
            Ok(text) => validate_spec_for(&text, &task.id),
            Err(err @ RepoReadError::TooLarge { .. }) => {
                return Err(AppError::Invalid(err.to_string()));
            }
            Err(_) => return Ok(false),
        };
        // What the spec says of the task's work decides Done with the steps (SPEC-491 DON-004).
        self.note_spec_status(&task.id, report.status).await;
        let items: Vec<_> = report
            .plan
            .into_iter()
            .filter(|item| item.depth == 0 && !item.text.trim().is_empty())
            .take(crate::coordination::MAX_STEPS)
            .collect();
        if items.is_empty() {
            return Ok(false);
        }
        let existing = self.deps.store.steps(&task.id).await?;
        let now = self.now();
        let mut steps: Vec<crate::model::StepRecord> = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            let title: String = yulius_domain::steps::normalize_title(&item.text)
                .chars()
                .take(yulius_domain::steps::MAX_TITLE_CHARS)
                .collect();
            // A repeated item (the spec check reports it) becomes its own step.
            let previous = crate::discipline::step_by_title(&existing, &title)
                .filter(|p| !steps.iter().any(|s| s.id == p.id));
            let status = if item.checked {
                "done".to_owned()
            } else {
                previous.map_or_else(|| "pending".to_owned(), |p| p.status.clone())
            };
            let unchanged = previous.is_some_and(|p| p.status == status);
            steps.push(crate::model::StepRecord {
                id: match previous {
                    Some(p) => p.id.clone(),
                    None => yulius_domain::ids::StepId::parse(&self.deps.crypto.new_id())
                        .map_err(AppError::internal)?,
                },
                task_id: task.id.clone(),
                ordinal: u32::try_from(i + 1).unwrap_or(u32::MAX),
                title,
                status,
                note: previous.and_then(|p| p.note.clone()),
                updated_by: if unchanged {
                    previous.and_then(|p| p.updated_by.clone())
                } else {
                    Some("spec".into())
                },
                updated_at: if unchanged {
                    previous.and_then(|p| p.updated_at)
                } else {
                    Some(now)
                },
                effort: previous.map_or(1, |p| p.effort),
            });
        }
        let same = steps.len() == existing.len()
            && steps
                .iter()
                .zip(&existing)
                .all(|(a, b)| a.title == b.title && a.status == b.status);
        if same {
            return Ok(false);
        }
        self.deps
            .store
            .replace_steps(&task.id, &steps, "spec", now)
            .await?;
        self.publish_steps(task).await;
        Ok(true)
    }
}

/// A Git credential issued to a task or a user process (`POST /internal/git-credential`).
#[derive(Debug)]
pub struct GitCredential {
    /// A user's GitHub token, or an installation access token.
    pub token: crate::model::Secret,
}

impl App {
    /// Issues the run's GitHub token (SPEC-002 §8, R-06): the App's agent token, restricted to
    /// the run's repository and to the agent permissions, never a person's token (ADR-0114), so
    /// GitHub refuses it on the protected production branch and it cannot loosen that protection.
    /// Commits stay authored by the task's owner; Yulius's own push, pull request and merge for
    /// the task use the owner's token on the server. `gh` in the run asks the same way (`yulius
    /// gh-token`, SPEC-004). Requests for any other host or repository are refused.
    ///
    /// The run gets it only while the task's owner is enabled and may work in the project
    /// (`git_credential_holder`, ADR-0481).
    ///
    /// # Errors
    /// `Unauthenticated` for unknown runs and a disabled owner, `NotFound` or `Forbidden` for an
    /// owner who may not work in the project, `Forbidden` for other repositories, `Conflict` when
    /// the App is not installed, GitHub failures.
    pub async fn agent_git_credential(
        &self,
        token: &str,
        host: &str,
        path: Option<&str>,
    ) -> AppResult<GitCredential> {
        let ctx = self.agent_context(token).await?;
        if host != "github.com" {
            return Err(AppError::Forbidden(
                "credentials are only issued for github.com".into(),
            ));
        }
        // Only while the task's owner is enabled and may still work in the project, as for a
        // person's own git (SPEC-675 L4Q3-001, ADR-0481). The terminal says why, once.
        let (_, sandbox) = match self
            .git_credential_holder(&ctx.task.owner, &ctx.task.sandbox_id)
            .await
        {
            Ok(holder) => holder,
            Err(err) => {
                let runtime = self.runtime(&ctx.task.id);
                if !runtime
                    .credential_refused_said
                    .swap(true, std::sync::atomic::Ordering::SeqCst)
                {
                    let owner = self.login_of(&ctx.task.owner).await;
                    runtime.hub.notice(format!(
                        "This task's agent gets no GitHub credential: its owner @{owner} is \
                         disabled, lost write access to the project, or their access cannot be \
                         checked now. Someone who may work here can take control of the task."
                    ));
                }
                return Err(err);
            }
        };
        // A playground has no GitHub repository: nothing is issued (SPEC-099 PLY-002).
        if sandbox.is_playground() {
            return Err(AppError::Forbidden(
                "this sandbox is a playground: its Git stays on this server and needs no GitHub \
                 credential"
                    .into(),
            ));
        }
        if let Some(path) = path {
            let wanted = path
                .trim_matches('/')
                .trim_end_matches(".git")
                .to_ascii_lowercase();
            let own = format!("{}/{}", sandbox.repo_owner, sandbox.repo_name).to_ascii_lowercase();
            if wanted != own {
                return Err(AppError::Forbidden(
                    "credentials are only issued for this project's repository".into(),
                ));
            }
        }
        Ok(GitCredential {
            token: self.agent_github_token(&sandbox).await?,
        })
    }

    /// The App's agent token for `sandbox`'s repository (ADR-0114).
    ///
    /// # Errors
    /// `Conflict` when the App is not installed; GitHub failures.
    pub(crate) async fn agent_github_token(
        &self,
        sandbox: &crate::model::SandboxRecord,
    ) -> AppResult<crate::model::Secret> {
        crate::playground::refuse_in_playground(sandbox, "A GitHub token")?;
        let installation = self.installation_id().await?;
        let app = self.github_app().await?;
        self.deps
            .github
            .agent_token(&app, installation, &sandbox.repo_name)
            .await
    }

    /// The App's installation on this instance.
    ///
    /// # Errors
    /// `Conflict` when the App is not installed; store failures.
    async fn installation_id(&self) -> AppResult<u64> {
        self.deps
            .store
            .instance()
            .await?
            .and_then(|i| i.installation_id)
            .ok_or_else(|| AppError::Conflict("the GitHub App is not installed".into()))
    }

    /// An installation token restricted to `sandbox`'s repository (agents' Git, and the
    /// superadmin's shells, ACC-012).
    ///
    /// # Errors
    /// `Conflict` when the App is not installed; GitHub failures.
    pub(crate) async fn repository_installation_token(
        &self,
        sandbox: &crate::model::SandboxRecord,
    ) -> AppResult<crate::model::Secret> {
        crate::playground::refuse_in_playground(sandbox, "A GitHub token")?;
        let installation = self.installation_id().await?;
        let app = self.github_app().await?;
        self.deps
            .github
            .repository_token(&app, installation, &sandbox.repo_name)
            .await
    }
}

/// Repository documents read for the briefing.
struct RepoDocs {
    layout: yulius_domain::spec_format::DocsLayout,
    product: Option<String>,
    architecture_present: bool,
    /// The newest part of the changes log, and whether it is the log's end; of a changes
    /// folder, its newest notes (SPEC-363).
    changes: Option<(String, bool)>,
    decision_file: Option<String>,
    decision_dir: Vec<(String, String)>,
    instructions: Vec<String>,
    foundations: Foundations,
}

/// Most entries of `.github/workflows` examined.
const MAX_WORKFLOW_ENTRIES: usize = 200;

/// GitHub Actions workflow files of the repository (a real directory inside it, not a link).
fn actions_workflows(root: &Path) -> usize {
    resolved_in_repo(root, ".github/workflows", std::fs::Metadata::is_dir)
        .map_or(0, |(root, dir)| workflow_count(&root, &dir))
}

/// The workflow files in the `resolved` folder (see [`resolved_in_repo`]) under `root`.
fn workflow_count(root: &Path, dir: &Path) -> usize {
    folder_names(root, dir, MAX_WORKFLOW_ENTRIES)
        .iter()
        .filter(|name| name.ends_with(".yml") || name.ends_with(".yaml"))
        .count()
}

/// The names in the `resolved` folder (see [`resolved_in_repo`]) under `root`, at most about
/// `max` entries, listed through the opened folder, whose real path must still be under `root`
/// (SPEC-262 VTR-004).
fn folder_names(root: &Path, dir: &Path, max: usize) -> Vec<String> {
    use nix::fcntl::OFlag;
    let Some(mut listing) =
        crate::files::open_inside(root, dir, OFlag::O_RDONLY | OFlag::O_DIRECTORY)
            .ok()
            .and_then(|fd| nix::dir::Dir::from_fd(fd).ok())
    else {
        return Vec::new();
    };
    // Bounded before errors are dropped: a folder whose reads keep failing ends the listing.
    listing
        .iter()
        .take(max + 2)
        .filter_map(Result::ok)
        .filter(|entry| !matches!(entry.file_name().to_bytes(), b"." | b".."))
        .filter_map(|entry| entry.file_name().to_str().ok().map(str::to_owned))
        .collect()
}

/// Most entries of a changes folder listed (SPEC-363), in directory order before the newest are
/// picked: a folder of more notes may lose some of its newest from the briefing.
const MAX_CHANGE_NOTE_ENTRIES: usize = 10_000;

/// The newest notes of the changes `folder` as the briefing quotes them (SPEC-363 CHN-003), or
/// `None` without any: a real directory inside the repository, each note read like a document.
fn change_notes(root: &Path, folder: &str) -> Option<(String, bool)> {
    use yulius_domain::briefing::{
        MAX_CHANGE_NOTES, change_notes_excerpt, change_notes_newest_first,
    };
    let (canonical, dir) = resolved_in_repo(root, folder, std::fs::Metadata::is_dir)?;
    let names = folder_names(&canonical, &dir, MAX_CHANGE_NOTE_ENTRIES);
    let notes: Vec<(String, String)> = change_notes_newest_first(names)
        .into_iter()
        .take(MAX_CHANGE_NOTES)
        .filter_map(|name| {
            let text = read_repo_doc(root, &format!("{folder}/{name}"))?;
            Some((name, text))
        })
        .collect();
    let excerpt = change_notes_excerpt(&notes, 4_000);
    (!excerpt.is_empty()).then_some((excerpt, false))
}

/// The project's foundations as the briefing reports them (DSC-014).
fn foundations(root: &Path, toml: Option<&str>) -> Foundations {
    let commands = toml.and_then(|text| {
        yulius_domain::spec_format::parse_yulius_toml(text)
            .ok()
            .map(|config| config.commands)
    });
    let set = |value: Option<&String>| value.is_some_and(|v| !v.trim().is_empty());
    Foundations {
        config: toml.is_some(),
        test_command: commands.as_ref().is_some_and(|c| set(c.test.as_ref())),
        lint_command: commands.as_ref().is_some_and(|c| set(c.lint.as_ref())),
        actions_workflows: actions_workflows(root),
        cd_defined: toml.is_some_and(|text| {
            yulius_domain::spec_format::parse_yulius_toml_cd(text).is_ok_and(|cd| cd.is_defined())
        }),
        cd_without_rollback: toml
            .and_then(|text| yulius_domain::spec_format::parse_yulius_toml_cd(text).ok())
            .map(|cd| {
                cd.environments
                    .into_iter()
                    .filter(|target| target.deploy.is_some() && target.rollback.is_none())
                    .map(|target| target.name)
                    .collect()
            })
            .unwrap_or_default(),
        merge_first: toml
            .and_then(|text| yulius_domain::spec_format::parse_yulius_toml_cd(text).ok())
            .and_then(|cd| cd.merge_first().map(|target| target.name.clone())),
        local: toml
            .and_then(|text| yulius_domain::spec_format::parse_yulius_toml_local(text).ok())
            .flatten(),
    }
}

impl RepoDocs {
    fn read(root: &Path) -> Self {
        let toml = read_repo_doc(root, "yulius.toml");
        let mut layout = project_layout(toml.as_deref());
        layout.resolve_changes_folder(|rel| {
            resolved_in_repo(root, rel, std::fs::Metadata::is_dir).is_some()
        });
        let doc = |rel: &str| read_repo_doc(root, rel);
        let product = doc(&layout.product).map(|t| t.chars().take(4_000).collect());
        let architecture_present = repo_doc_exists(root, &layout.architecture);
        let changes = match layout.changes_folder() {
            Some(folder) => change_notes(root, folder),
            None => {
                doc(&layout.changes).map(|t| yulius_domain::briefing::changes_excerpt(&t, 4_000))
            }
        };
        let (mut decision_file, mut decision_dir) = (None, Vec::new());
        match &layout.decisions {
            DecisionsLocation::File(rel) => decision_file = doc(rel.as_str()),
            DecisionsLocation::Directory(rel) => {
                // The directory itself must be a real one inside the repository (#70).
                if let Some(dir) = repo_path(root, rel.as_str())
                    && std::fs::symlink_metadata(&dir).is_ok_and(|meta| meta.is_dir())
                    && std::fs::canonicalize(root)
                        .ok()
                        .zip(std::fs::canonicalize(&dir).ok())
                        .is_some_and(|(root, dir)| dir.starts_with(root))
                    && let Ok(entries) = std::fs::read_dir(&dir)
                {
                    let mut names: Vec<String> = entries
                        .filter_map(Result::ok)
                        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
                        .filter(|name| name.ends_with(".md"))
                        .collect();
                    names.sort();
                    for name in names.into_iter().take(MAX_DECISION_FILES) {
                        if let Some(text) = read_repo_doc(root, &format!("{rel}/{name}")) {
                            decision_dir.push((name, text));
                        }
                    }
                }
            }
        }
        let instructions = [layout.agents.as_str(), "CLAUDE.md"]
            .iter()
            .filter(|rel| repo_doc_exists(root, rel))
            .map(|rel| (*rel).to_owned())
            .collect();
        let foundations = foundations(root, toml.as_deref());
        Self {
            layout,
            product,
            architecture_present,
            changes,
            decision_file,
            decision_dir,
            instructions,
            foundations,
        }
    }

    fn decision_files(&self) -> DecisionFiles<'_> {
        if let Some(text) = &self.decision_file {
            return DecisionFiles::File(text);
        }
        if self.decision_dir.is_empty() {
            return DecisionFiles::Missing;
        }
        DecisionFiles::Directory(
            self.decision_dir
                .iter()
                .map(|(name, contents)| NamedFile { name, contents })
                .collect(),
        )
    }
}

/// The `notification_type`s of Claude Code 2.1.292 that ask a person to act: a permission prompt
/// (also of a worker), a question for the user (also a URL to open) and an agent that needs
/// input. The others report something (the idle reminder, a sign-in, a background agent that
/// finished, a push notification, a quota resume, a refusal fallback) and ask nobody: they are
/// frequent and arrive while the agent works, so a type this list does not know is not one
/// either (STA-002, SPEC-032).
const NOTICES_ASKING_A_PERSON: [&str; 5] = [
    "permission_prompt",
    "worker_permission_prompt",
    "elicitation_dialog",
    "elicitation_url_dialog",
    "agent_needs_input",
];

/// Whether the main session's next work shows the person answered the notice: a permission
/// prompt or a question of the session itself. A worker's prompt or an agent that needs input
/// is not answered by the session going on with something else.
fn notice_answered_by_work(input: &HookInput) -> bool {
    !matches!(
        input.notification_type.as_deref(),
        Some("worker_permission_prompt" | "agent_needs_input")
    )
}

/// Whether a `Notification` hook asks a person to act ([`NOTICES_ASKING_A_PERSON`]).
/// Without `notification_type` (older Claude Code) the idle reminder is known by its text.
fn notification_needs_a_person(input: &HookInput) -> bool {
    match input.notification_type.as_deref() {
        Some(kind) => NOTICES_ASKING_A_PERSON.contains(&kind),
        None => !input
            .message
            .as_deref()
            .is_some_and(|m| m.to_ascii_lowercase().contains("waiting for your input")),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RepoDocs, colour, doc_present, foundations, notice_answered_by_work,
        notification_needs_a_person, read_repo_doc, repo_doc_exists, repo_path, resolved_in_repo,
        resolved_through_links, workflow_count, worktree_relative,
    };
    use std::path::Path;
    use yulius_contracts::hooks::HookInput;

    #[test]
    fn retained_goal_instruction_preserves_the_byte_limit_and_recovers_missing_text() {
        let limit = crate::capacity::CONTINUATION_MAX;
        let exact = "é".repeat(limit / 2);
        assert_eq!(super::retained_goal_instruction(Some(exact.clone())), exact);
        for prompt in [None, Some(String::new()), Some(format!("{exact}x"))] {
            let instruction = super::retained_goal_instruction(prompt);
            assert!(instruction.len() <= limit);
            assert!(instruction.starts_with(super::GOAL_CONTINUATION));
            assert!(instruction.contains("spec_check") && instruction.contains("steps_get"));
        }
    }

    fn notice(kind: Option<&str>, message: Option<&str>) -> HookInput {
        HookInput {
            notification_type: kind.map(str::to_owned),
            message: message.map(str::to_owned),
            ..HookInput::default()
        }
    }

    /// STA-002, STA-003 (SPEC-032): the allow-list, and which notices the session's own work
    /// answers.
    #[test]
    fn only_notices_that_ask_a_person_flag_the_task() {
        for kind in [
            "permission_prompt",
            "worker_permission_prompt",
            "elicitation_dialog",
            "elicitation_url_dialog",
            "agent_needs_input",
        ] {
            assert!(
                notification_needs_a_person(&notice(Some(kind), None)),
                "{kind}"
            );
        }
        for kind in [
            "idle_prompt",
            "auth_success",
            "agent_completed",
            "push_notification",
            "x",
        ] {
            assert!(
                !notification_needs_a_person(&notice(Some(kind), None)),
                "{kind}"
            );
        }
        // Older CLIs: the idle text is the reminder, anything else asks.
        assert!(!notification_needs_a_person(&notice(
            None,
            Some("Claude is waiting for your input")
        )));
        assert!(notification_needs_a_person(&notice(
            None,
            Some("Claude needs your permission")
        )));
        // The session's own next work answers its prompts, not a worker's or an agent's.
        assert!(notice_answered_by_work(&notice(
            Some("permission_prompt"),
            None
        )));
        assert!(notice_answered_by_work(&notice(
            None,
            Some("Claude needs your permission")
        )));
        assert!(!notice_answered_by_work(&notice(
            Some("worker_permission_prompt"),
            None
        )));
        assert!(!notice_answered_by_work(&notice(
            Some("agent_needs_input"),
            None
        )));
    }

    /// SPEC-363 CHN-001/003: a changes folder, named with a trailing `/` or existing as a
    /// directory, is read as its newest notes; one through a link out of the repository is not.
    #[test]
    fn repo_docs_read_a_changes_folder_as_its_newest_notes()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path().join("repo");
        let notes = root.join("docs/changes");
        std::fs::create_dir_all(&notes)?;
        std::fs::write(notes.join("README.md"), "# How to write a note")?;
        std::fs::write(notes.join("2026-10-30-1-search.md"), "# Search")?;
        std::fs::write(notes.join("2026-10-31-2-folder.md"), "# Notes as files")?;
        for changes in ["docs/changes/", "docs/changes"] {
            std::fs::write(
                root.join("yulius.toml"),
                format!("[docs]\nchanges = \"{changes}\"\n"),
            )?;
            let docs = RepoDocs::read(&root);
            assert_eq!(docs.layout.changes, "docs/changes/", "{changes}");
            let (excerpt, at_end) = docs.changes.ok_or("the notes are read")?;
            assert!(!at_end);
            assert!(
                excerpt.starts_with("`2026-10-31-2-folder.md`:\n# Notes as files\n\n`2026-10-30-1-search.md`:\n# Search"),
                "{excerpt}"
            );
            assert!(!excerpt.contains("How to write"), "{excerpt}");
        }
        // No notes yet: nothing to quote.
        for name in ["2026-10-30-1-search.md", "2026-10-31-2-folder.md"] {
            std::fs::remove_file(notes.join(name))?;
        }
        assert_eq!(RepoDocs::read(&root).changes, None);
        // The single file stays a file.
        std::fs::write(root.join("docs/changes.md"), "# Changes\n\n- One.\n")?;
        std::fs::write(
            root.join("yulius.toml"),
            "[docs]\nchanges = \"docs/changes.md\"\n",
        )?;
        let docs = RepoDocs::read(&root);
        assert_eq!(docs.layout.changes, "docs/changes.md");
        assert_eq!(
            docs.changes,
            Some(("# Changes\n\n- One.\n".to_owned(), false))
        );
        // A folder through a link out of the repository is neither listed nor a folder.
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside)?;
        std::fs::write(outside.join("2026-10-31-9-secret.md"), "TOP SECRET")?;
        std::os::unix::fs::symlink(&outside, root.join("x"))?;
        for changes in ["x/", "x"] {
            std::fs::write(
                root.join("yulius.toml"),
                format!("[docs]\nchanges = \"{changes}\"\n"),
            )?;
            assert_eq!(RepoDocs::read(&root).changes, None, "{changes}");
        }
        Ok(())
    }

    /// #70: a document path through a linked directory never leaves the repository (the
    /// briefing and `project_context` would hand the agent a host file otherwise).
    #[test]
    fn repo_docs_never_follow_linked_directories_out_of_the_repository()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path().join("repo");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&root)?;
        std::fs::create_dir_all(outside.join("decisions"))?;
        std::fs::write(outside.join("secret.md"), "TOP SECRET")?;
        std::fs::write(
            outside.join("decisions/0001-secret.md"),
            "# Secret decision",
        )?;
        std::os::unix::fs::symlink(&outside, root.join("x"))?;
        std::fs::write(
            root.join("yulius.toml"),
            "[docs]\nproduct = \"x/secret.md\"\nchanges = \"x/secret.md\"\ndecisions = \"x/decisions\"\n",
        )?;
        let docs = RepoDocs::read(&root);
        assert_eq!(
            docs.product, None,
            "product read through a linked directory"
        );
        assert_eq!(
            docs.changes, None,
            "changes read through a linked directory"
        );
        assert!(
            docs.decision_dir.is_empty(),
            "decisions listed through a link"
        );
        // A real document inside the repository is still read.
        std::fs::create_dir_all(root.join("docs"))?;
        std::fs::write(root.join("docs/product.md"), "# Product")?;
        std::fs::write(
            root.join("yulius.toml"),
            "[docs]\nproduct = \"docs/product.md\"\n",
        )?;
        assert_eq!(RepoDocs::read(&root).product.as_deref(), Some("# Product"));
        Ok(())
    }

    /// DSC-014: the briefing knows which foundations a project lacks, without following links
    /// out of the repository.
    #[test]
    fn foundations_are_read_from_the_repository() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(root.join(".github/workflows"))?;
        std::fs::write(root.join(".github/workflows/ci.yml"), "on: push\n")?;
        std::fs::write(root.join(".github/workflows/README.md"), "notes\n")?;
        let bare = foundations(&root, None);
        assert!(!bare.config && !bare.test_command && !bare.lint_command);
        assert_eq!(bare.actions_workflows, 1);
        let configured = foundations(
            &root,
            Some("[commands]\ntest = \"cargo test\"\nlint = \"cargo clippy\"\n"),
        );
        assert!(configured.config && configured.test_command && configured.lint_command);
        std::fs::remove_dir_all(root.join(".github"))?;
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside)?;
        std::fs::write(outside.join("deploy.yml"), "on: push\n")?;
        std::fs::create_dir_all(root.join(".github"))?;
        std::os::unix::fs::symlink(&outside, root.join(".github/workflows"))?;
        assert_eq!(
            foundations(&root, None).actions_workflows,
            0,
            "a linked directory is not read"
        );
        Ok(())
    }

    /// #70: a document that is itself a link (a file in the repository root linked to a host
    /// file, or even to a file of the repository) is never read, nor counted as present; a
    /// regular file is.
    #[test]
    fn linked_documents_are_not_read() -> Result<(), Box<dyn std::error::Error>> {
        let host = tempfile::tempdir()?;
        let secret = host.path().join("secret.txt");
        std::fs::write(&secret, "host secret")?;
        let repo = tempfile::tempdir()?;
        let root = repo.path();
        std::fs::write(root.join("real.md"), "# real")?;
        std::os::unix::fs::symlink(&secret, root.join("p.md"))?;
        std::os::unix::fs::symlink(root.join("real.md"), root.join("inside.md"))?;
        std::fs::create_dir(root.join("docs"))?;
        std::os::unix::fs::symlink(&secret, root.join("docs/product.md"))?;
        for linked in ["p.md", "inside.md", "docs/product.md"] {
            assert_eq!(read_repo_doc(root, linked), None, "{linked}");
            assert!(!repo_doc_exists(root, linked), "{linked}");
        }
        assert_eq!(read_repo_doc(root, "real.md").as_deref(), Some("# real"));
        assert!(repo_doc_exists(root, "real.md"));
        Ok(())
    }

    /// SPEC-262 VTR-003 (#688): a repository or branch name with control characters (an
    /// imported branch, say) cannot end the colour and print escapes of its own.
    #[test]
    fn status_line_text_carries_no_control_characters() {
        assert_eq!(
            colour(203, "feat/1\x1b]0;owned\x07-x\u{9b}2J\x7f\ny"),
            "\x1b[38;5;203mfeat/1]0;owned-x2Jy\x1b[39m"
        );
        assert_eq!(colour(80, "studio"), "\x1b[38;5;80mstudio\x1b[39m");
    }

    /// SPEC-262 VTR-004 (#697): a folder on the way swapped for a link to a host folder after
    /// the path was resolved is not followed: the host file is not reported as a document, nor
    /// the host folder's workflow files counted.
    #[test]
    fn a_folder_swapped_for_a_link_after_resolving_is_not_followed()
    -> Result<(), Box<dyn std::error::Error>> {
        let host = tempfile::tempdir()?;
        std::fs::create_dir_all(host.path().join("workflows"))?;
        std::fs::write(host.path().join("architecture.md"), "host")?;
        std::fs::write(host.path().join("workflows/deploy.yml"), "on: push\n")?;
        let repo = tempfile::tempdir()?;
        let root = repo.path();
        std::fs::create_dir_all(root.join("docs"))?;
        std::fs::write(root.join("docs/architecture.md"), "# arch")?;
        std::fs::create_dir_all(root.join(".github/workflows"))?;
        std::fs::write(root.join(".github/workflows/ci.yml"), "on: push\n")?;
        let (canonical, doc) =
            resolved_in_repo(root, "docs/architecture.md", std::fs::Metadata::is_file)
                .ok_or("resolved")?;
        let (_, workflows) = resolved_in_repo(root, ".github/workflows", std::fs::Metadata::is_dir)
            .ok_or("resolved")?;
        assert!(doc_present(&canonical, &doc));
        assert_eq!(workflow_count(&canonical, &workflows), 1);

        std::fs::rename(root.join("docs"), root.join("docs.moved"))?;
        std::os::unix::fs::symlink(host.path(), root.join("docs"))?;
        std::fs::rename(root.join(".github"), root.join(".github.moved"))?;
        std::os::unix::fs::symlink(host.path(), root.join(".github"))?;
        assert!(!doc_present(&canonical, &doc), "a host file is no document");
        assert_eq!(
            workflow_count(&canonical, &workflows),
            0,
            "a host folder is not counted"
        );
        Ok(())
    }

    #[test]
    fn repo_paths_stay_inside_the_root() {
        let root = Path::new("/r");
        assert_eq!(repo_path(root, "docs/a.md"), Some(root.join("docs/a.md")));
        assert_eq!(repo_path(root, "../x"), None);
        assert_eq!(repo_path(root, "/etc/passwd"), None);
        assert_eq!(repo_path(root, "a/../../x"), None);
    }

    #[test]
    fn edited_files_are_placed_in_the_worktree_lexically() {
        let wt = Path::new("/w/t1");
        let rel = |file: &str| worktree_relative(wt, wt, file);
        assert_eq!(rel("/w/t1/src/a.rs").as_deref(), Some("src/a.rs"));
        assert_eq!(rel("src/./a.rs").as_deref(), Some("src/a.rs"));
        assert_eq!(rel("/w/t1/docs/../a.rs").as_deref(), Some("a.rs"));
        assert_eq!(rel("/w/t10/a.rs"), None, "a sibling worktree is outside");
        assert_eq!(rel("/w/t1/../t2/a.rs"), None);
        assert_eq!(rel("../../etc/passwd"), None);
        assert_eq!(rel("/w/t1"), None);
        let docs = Path::new("/w/t1/docs");
        assert_eq!(
            worktree_relative(wt, docs, "x.svg").as_deref(),
            Some("docs/x.svg"),
            "a relative path is the agent's folder's"
        );
        assert_eq!(worktree_relative(wt, docs, "../../t2/a.rs"), None);
    }

    #[test]
    fn links_are_followed_by_the_longest_existing_folder() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let root = std::fs::canonicalize(dir.path())?;
        std::fs::create_dir_all(root.join("wt/src"))?;
        std::os::unix::fs::symlink(root.join("wt"), root.join("link"))?;
        std::os::unix::fs::symlink(root.join("loop"), root.join("loop"))?;
        assert_eq!(
            resolved_through_links(&root.join("link/src/new/deep/a.rs")),
            Some(root.join("wt/src/new/deep/a.rs")),
            "a file not created yet is placed by its folder"
        );
        assert_eq!(
            resolved_through_links(&root.join("loop/a.rs")),
            Some(root.join("loop/a.rs")),
            "a link loop ends at its folder, never spins"
        );
        Ok(())
    }
}
