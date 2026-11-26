//! Run-directory markers and typing into a new run: the opening message, the task's goal, and
//! the run's saved screen.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use yulius_domain::ids::{AttemptId, SandboxId, TaskId};
use yulius_domain::request::Paste;

use super::SessionInput;
use crate::attention::AttentionRecord;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, TaskMode, TaskRecord};
use crate::ports::harness::SessionMode;

/// File in a run directory holding the run's final screen (`<cols> <rows>\n` + redraw bytes).
pub(crate) const FINAL_SCREEN_FILE: &str = "final-screen";

/// File in a run directory while the run's first message, which Yulius types (SPEC-030), was not
/// taken by the session yet (SPEC-068 OPN-001). Empty while Yulius types it; `entered` once its
/// `UserPromptSubmit` reached Yulius (it may be held there). The session's first prompt let
/// through, or any Stop, removes it.
pub(crate) const OPENING_PENDING_FILE: &str = "opening-pending";
const OPENING_ENTERED: &[u8] = b"entered";

fn opening_marker(run_dir: &str) -> PathBuf {
    Path::new(run_dir).join(OPENING_PENDING_FILE)
}

/// Whether the run in `run_dir` has its typed first message pending (typed or held).
pub(crate) fn opening_pending(run_dir: &str) -> bool {
    opening_marker(run_dir).exists()
}

/// Whether the first message of the run in `run_dir` never reached Yulius's submit hook: a
/// restart cut its typing.
pub(crate) fn opening_unsubmitted(run_dir: &str) -> bool {
    std::fs::read(opening_marker(run_dir)).is_ok_and(|m| m != OPENING_ENTERED)
}

/// A `UserPromptSubmit` of the run in `run_dir` reached Yulius: its pending first message was
/// submitted (it may still be held for capacity or an update).
pub(crate) fn opening_entered(run_dir: &str) {
    use std::io::Write as _;
    // Never created here: a marker removed meanwhile stays removed.
    let written = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(opening_marker(run_dir))
        .and_then(|mut file| file.write_all(OPENING_ENTERED));
    match written {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            tracing::warn!(run_dir, error = %err, "could not record the submitted first message");
        }
        _ => {}
    }
}

/// The run in `run_dir` took a prompt: nothing of its first message is pending.
pub(crate) fn opening_sent(run_dir: &str) {
    match std::fs::remove_file(opening_marker(run_dir)) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            tracing::warn!(run_dir, error = %err, "could not clear the pending first message");
        }
        _ => {}
    }
}

/// File in a run directory while the run's launch `/goal` (SPEC-117) is not confirmed: written
/// before the spawn, removed when the goal watch ends with an outcome (the goal recorded, or the
/// task flagged). A restarted server watches the goal of a reattached run that still has it
/// (SPEC-146 GWR-001, GWR-002).
pub(super) const LAUNCH_GOAL_FILE: &str = "launch-goal-pending";

/// File in a run directory while Yulius still has to type the run's `/goal`: a resumed goal
/// session gets it typed once its opening turn ended (ONE-007). Written before the spawn, like
/// its siblings, and removed when the typist ends with an outcome (the goal recorded, the task
/// flagged, the prompt box left to a person). It holds [`TYPED_GOAL_OPENS`] when the session
/// opens with a turn. The typist lives in the server's memory, so a restarted server has none:
/// for a reattached run that still has the file it waits that turn out, watches whether Claude
/// Code recorded a goal meanwhile, and flags the task when it did not (SPEC-483 RST-002).
pub(super) const TYPED_GOAL_FILE: &str = "typed-goal-pending";

/// What [`TYPED_GOAL_FILE`] holds for a session that opens with a turn before its goal is typed.
pub(super) const TYPED_GOAL_OPENS: &str = "opening";

/// Whether the opening turn of the reattached run in `run_dir`, whose goal is still to be
/// typed, has not ended: the run opened with one and no Stop of it was answered yet.
pub(super) fn typed_goal_awaits_opening(run_dir: &Path) -> bool {
    std::fs::read_to_string(run_dir.join(TYPED_GOAL_FILE))
        .is_ok_and(|mark| mark.trim() == TYPED_GOAL_OPENS)
        && crate::outage::stop_answered_at(run_dir) == 0
}

/// A run's goal that is not confirmed yet, as its run directory records it.
#[derive(Debug, Clone, Copy)]
pub(super) enum PendingGoal {
    /// Given as the launch prompt ([`LAUNCH_GOAL_FILE`]).
    Launch,
    /// To be typed by Yulius ([`TYPED_GOAL_FILE`]).
    Typed,
}

impl PendingGoal {
    fn file(self) -> &'static str {
        match self {
            Self::Launch => LAUNCH_GOAL_FILE,
            Self::Typed => TYPED_GOAL_FILE,
        }
    }

    /// Whether the run in `run_dir` has this goal not confirmed yet.
    pub(super) fn pending(self, run_dir: &Path) -> bool {
        run_dir.join(self.file()).exists()
    }

    /// The goal of the run in `run_dir` has an outcome: no later server watches it again.
    pub(super) fn settled(self, run_dir: &Path) {
        match std::fs::remove_file(run_dir.join(self.file())) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                tracing::warn!(run_dir = %run_dir.display(), error = %err, "could not clear the pending goal");
            }
            _ => {}
        }
    }

    /// Why the task has no goal when the watch of this goal ends without one.
    fn not_taken(self) -> &'static str {
        match self {
            Self::Launch => "Claude Code did not take the task's goal",
            Self::Typed => "Yulius restarted before it set this task's goal",
        }
    }
}

/// The run was given a native goal whose submission its watcher has yet to confirm.
pub(crate) fn native_goal_pending(run_dir: &Path) -> bool {
    PendingGoal::Launch.pending(run_dir) || PendingGoal::Typed.pending(run_dir)
}

/// Why a task has no goal when a person typed before Yulius could type it.
const GOAL_LEFT_TO_DRAFT: &str = "Someone typed in the terminal before Yulius set the task's \
                                  goal, so their draft was left alone";

/// File in a run directory whose run was started with its owner's personal MCP servers: the
/// owner's id (SPEC-332 MSC-009). A live run keeps its MCP configuration, and which person's
/// servers it holds decides who may message it and whether a hand-over restarts it; a restarted
/// server reads it back for a reattached run (SPEC-483 RST-001).
pub(super) const PERSONAL_MCP_FILE: &str = "personal-mcp-of";

/// Digest of the sealed sign-in selected for a Claude run, kept for watcher reattachment.
pub(super) const CLAUDE_CREDENTIAL_DIGEST: &str = "claude-credential-sha256";

/// Whose personal MCP servers the run in `run_dir` was started with; `None` for none, and for
/// a mark that cannot be read (said in the log: the run then counts as holding nobody's).
pub(super) fn personal_mcp_marked(run_dir: &Path) -> Option<yulius_domain::ids::UserId> {
    let text = match std::fs::read_to_string(run_dir.join(PERSONAL_MCP_FILE)) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            tracing::warn!(run_dir = %run_dir.display(), error = %err, "the run's personal MCP mark cannot be read");
            return None;
        }
    };
    let marked = yulius_domain::ids::UserId::parse(text.trim()).ok();
    if marked.is_none() {
        tracing::warn!(run_dir = %run_dir.display(), "the run's personal MCP mark names no user");
    }
    marked
}

/// File in a run directory once its Claude Code session started (its SessionStart hook reached
/// Yulius): a restarted server knows the reattached run's prompt box reads input (SPEC-230).
const SESSION_STARTED_FILE: &str = "session-started";

/// Whether the session of the run in `run_dir` started.
pub(crate) fn session_started_marked(run_dir: &Path) -> bool {
    run_dir.join(SESSION_STARTED_FILE).exists()
}

/// Records that the session of the run in `run_dir` started.
pub(crate) fn mark_session_started(run_dir: &Path) {
    if let Err(err) = write_private_file(&run_dir.join(SESSION_STARTED_FILE), b"") {
        tracing::warn!(run_dir = %run_dir.display(), error = %err, "could not record the session start");
    }
}

/// File in a run directory while a person's draft may sit in the run's prompt box (SPEC-480
/// DRF-009): the time (ms) of the keystroke that began it. Written at a person's first keystroke
/// after the box was last empty and removed when a prompt submitted from the box empties it, so
/// a restarted server still knows of a draft typed before it.
const PERSON_TYPED_FILE: &str = "person-typed";

/// When the person's draft that may sit in the prompt box of the run in `run_dir` was begun
/// (ms); 0 when none is marked.
pub(crate) fn person_typed_marked(run_dir: &Path) -> i64 {
    std::fs::read_to_string(run_dir.join(PERSON_TYPED_FILE))
        .ok()
        .and_then(|text| text.trim().parse::<i64>().ok())
        .map_or(0, |at| at.max(0))
}

/// Records that a person began a draft in the run in `run_dir` at `at`; whether it was written.
/// Through a synced temporary file, so a crash leaves no mark or a whole one.
pub(crate) fn mark_person_typed(run_dir: &Path, at: i64) -> bool {
    let written = crate::outage::write_time(&run_dir.join(PERSON_TYPED_FILE), at);
    if let Err(err) = &written {
        tracing::warn!(run_dir = %run_dir.display(), error = %err, "could not record a person's draft");
    }
    written.is_ok()
}

/// The prompt box of the run in `run_dir` was emptied: no draft is marked any more.
pub(crate) fn clear_person_typed(run_dir: &Path) {
    match std::fs::remove_file(run_dir.join(PERSON_TYPED_FILE)) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            tracing::warn!(run_dir = %run_dir.display(), error = %err, "could not clear a person's draft mark");
        }
        _ => {}
    }
}

/// Writes `bytes` to `path` readable by the owner only.
pub(crate) fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

/// Saves the run's current screen as its final screen: at its end, at every settled turn and
/// after a reattach, so a run lost later (a reboot) still shows how its last turn ended (SPEC-229
/// RBT-002). A hub without output (a runtime made before its run was reattached) keeps the saved
/// screen. The file is replaced whole (a temporary file renamed over it): a reset or a concurrent
/// save never leaves a torn one.
pub(crate) fn save_screen(hub: &crate::hub::Hub, run_dir: &Path) {
    static SAVES: AtomicU64 = AtomicU64::new(0);
    if !hub.has_output() {
        return;
    }
    let (screen, (cols, rows)) = hub.formatted_screen();
    let mut record = format!("{cols} {rows}\n").into_bytes();
    record.extend_from_slice(&screen);
    let temporary = run_dir.join(format!(
        ".{FINAL_SCREEN_FILE}.{}.{}",
        std::process::id(),
        SAVES.fetch_add(1, Ordering::Relaxed)
    ));
    let saved = write_private_file(&temporary, &record)
        .and_then(|()| std::fs::rename(&temporary, run_dir.join(FINAL_SCREEN_FILE)));
    if let Err(err) = saved {
        let _ = std::fs::remove_file(&temporary);
        tracing::debug!(error = %err, "cannot save the final screen");
    }
}

/// Reads a saved final screen: (cols, rows, redraw bytes).
pub(crate) fn read_final_screen(run_dir: &Path) -> Option<(u16, u16, Vec<u8>)> {
    read_saved_screen(&run_dir.join(FINAL_SCREEN_FILE))
}

pub(crate) fn read_saved_screen(path: &Path) -> Option<(u16, u16, Vec<u8>)> {
    let bytes = std::fs::read(path).ok()?;
    let newline = bytes.iter().position(|b| *b == b'\n')?;
    let header = std::str::from_utf8(&bytes[..newline]).ok()?;
    let (cols, rows) = header.split_once(' ')?;
    Some((
        cols.parse().ok()?,
        rows.parse().ok()?,
        bytes[newline + 1..].to_vec(),
    ))
}

/// Quiet time after the TUI's first output before the goal is typed.
const GOAL_QUIET: Duration = Duration::from_millis(1500);
/// Upper bound for waiting until the TUI is ready for the goal (its SessionStart hook and quiet
/// output); a busy host starts Claude Code slowly. A TUI without hooks gets half of it. The
/// default of [`crate::context::AppConfig::goal_ready_ms`].
pub const GOAL_READY_TIMEOUT: Duration = Duration::from_secs(90);
/// How long a person gets to answer Claude Code's folder-trust prompt, from when it shows; the
/// default of [`crate::context::AppConfig::trust_answer_ms`] (SPEC-106 OTR-002).
pub const TRUST_ANSWER_TIMEOUT: Duration = Duration::from_secs(300);
/// Upper bound for waiting until the turn on a goal session's first message ended; after it the
/// goal is typed anyway (Claude Code queues it behind a running turn).
const GOAL_OPENING_TIMEOUT: Duration = Duration::from_secs(300);
/// How long Claude Code may take to record the goal after each submit before Yulius tries again:
/// Enter alone (the Enter was lost), the line again over a cleared prompt box (the text was
/// lost), then Enter alone, waiting longer each time.
const GOAL_SUBMIT_WAITS: [Duration; 5] = [
    Duration::from_secs(5),
    Duration::from_secs(8),
    Duration::from_secs(12),
    Duration::from_secs(20),
    Duration::from_secs(30),
];
/// How long a goal given as the launch prompt may take to be recorded once the session reads
/// input: as long as all the submit waits of a typed goal (SPEC-117 ONE-001).
pub(crate) fn launch_goal_bound_ms() -> u64 {
    let bound: Duration = GOAL_SUBMIT_WAITS.iter().sum();
    u64::try_from(bound.as_millis()).unwrap_or(75_000)
}

/// The attempt (index into [`GOAL_SUBMIT_WAITS`]) that types the whole line again.
const GOAL_RETYPE_ATTEMPT: usize = 2;
/// What Claude Code's folder-trust dialog shows: 2.1.x ("Quick safety check"), then older
/// releases. Yulius trusts every worktree and its main repository (SPEC-106), so the dialog means
/// Claude Code looks trust up somewhere new; a person answers it, never Yulius's typing. Looked
/// for only before the session's SessionStart hook: Claude Code asks before it starts the
/// session, and later the words can be anyone's output.
const TRUST_PROMPT_TEXTS: [&str; 2] = [
    "Yes, I trust this folder",
    "Do you trust the files in this folder?",
];

/// How [`App::wait_input_ready`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InputReady {
    /// The TUI reads input (or the wait for it was given up on): type.
    Ready,
    /// The attempt is no longer live: type nothing.
    Gone,
    /// Claude Code's folder-trust dialog was still shown after
    /// [`crate::context::AppConfig::trust_answer_ms`]: type nothing into it.
    TrustUnanswered,
    /// The terminal's screen is withheld, or misses output dropped while it was (agent secrets
    /// could not be read, ADR-0317): what the program shows is unknown, a dialog included, so
    /// type nothing.
    ScreenWithheld,
}

/// Said when the screen is not the program's: what is wrong, and what its person does.
pub(crate) const SCREEN_WITHHELD: &str = "Agent secrets could not be read, so this terminal's \
     screen was withheld and Yulius typed nothing";
const SCREEN_WITHHELD_ANSWER: &str = "If the terminal still says its screen is withheld, replace \
     or delete the agent secret that cannot be read (the server's log names the error). Then \
     stop and continue the task";

/// Said when the trust dialog is still shown at the end of the wait: what is wrong, and how its
/// person's part begins.
const TRUST_UNANSWERED: &str =
    "Claude Code still asks whether to trust this folder, so Yulius typed nothing";
const TRUST_ANSWER: &str = "Answer it in the terminal";

/// What a task whose goal was never set says: the notice for its terminal, and the reason kept
/// with its flag (SPEC-472). `why` is what went wrong; `first` is what its person does before a
/// goal can be typed at all (answer the trust question), when there is such a thing.
fn goal_missing(why: &str, first: Option<&str>) -> (String, AttentionRecord) {
    let notice = format!(
        "{why}{}. The task has no goal yet: type `/goal` and the request in the terminal, or stop \
         and continue the task.",
        first.map(|first| format!(". {first}")).unwrap_or_default()
    );
    let action = match first {
        Some(first) => {
            format!("{first}, then type `/goal` and the request, or stop and continue the task.")
        }
        None => "Type `/goal` and the request in the terminal, or stop and continue the task."
            .to_owned(),
    };
    let reason = AttentionRecord::yulius(&format!("{why}. The task has no goal yet."), &action);
    (notice, reason)
}

/// Whether `screen` shows Claude Code's folder-trust dialog.
pub(crate) fn trust_prompt_shown(screen: &str) -> bool {
    TRUST_PROMPT_TEXTS.iter().any(|text| screen.contains(text))
}

/// The Claude Code command that sets a session's goal, followed by its condition.
pub(super) const GOAL_COMMAND: &str = "/goal ";

/// A goal task's `/goal` conditions (SPEC-212).
pub(super) struct TaskGoal {
    /// The launch prompt's: the whole request, then the definition of done (WHL-001).
    pub(super) launch: String,
    /// The one typed into a resumed session: one short line (WHL-003).
    pub(super) typed: String,
}

/// Upper bound for waiting until the TUI took one paste of a typed first message (SPEC-030).
const PASTE_TIMEOUT: Duration = Duration::from_secs(5);
/// Quiet time after a paste changed the screen before the next one is written.
const PASTE_QUIET: Duration = Duration::from_millis(500);

/// Whether a launch's prompt sets the session goal itself: a fresh goal session's, or a held
/// `/goal` continued in another session. No `/goal` is typed after it (SPEC-117 ONE-003).
pub(super) fn sets_goal(mode: &SessionMode) -> bool {
    matches!(
        mode,
        SessionMode::New { prompt: Some(prompt), .. }
            | SessionMode::Resume { prompt: Some(prompt), .. }
            if prompt.starts_with(GOAL_COMMAND)
    )
}

/// The pastes that type a first message with attached images (SPEC-030), each a bracketed paste
/// (xterm `ESC [200~ … ESC [201~`) with what the screen shows once Claude Code took it: in the
/// message's order (SPEC-124), each image's path (`[Image #N]`, at the cursor) and the text
/// around it. Text that ends the message ends with a newline, so the cursor is never on a
/// mention: Claude Code's file autocomplete would take the Enter (measured).
fn opening_pastes(pastes: Vec<Paste>) -> Vec<(Vec<u8>, Option<String>)> {
    let paste = |content: &str| format!("\x1b[200~{content}\x1b[201~").into_bytes();
    let last = pastes.len().saturating_sub(1);
    let mut images = 0;
    pastes
        .into_iter()
        .enumerate()
        .map(|(i, item)| match item {
            Paste::Image(path) => {
                images += 1;
                (paste(&path), Some(format!("[Image #{images}]")))
            }
            Paste::Text(text) if i == last => (paste(&format!("{text}\n")), None),
            Paste::Text(text) => (paste(&text), None),
        })
        .collect()
}

impl App {
    /// Writes the task's request file into the sandbox home (`.yulius/tasks/<number>.md`: the
    /// request, its attached files and the definition of done, SPEC-011) and returns the task's
    /// `/goal` conditions (SPEC-212).
    pub(super) async fn write_task_request(
        &self,
        sandbox: &SandboxRecord,
        task: &TaskRecord,
        home: &Path,
    ) -> AppResult<TaskGoal> {
        use yulius_domain::request::{
            DoneDefinition, Request, goal_condition, request_markdown, typed_goal_condition,
        };
        let profile = crate::delivery::delivery_profile(sandbox);
        let done = DoneDefinition::for_profile(&profile, sandbox.is_playground());
        let author = match self.deps.store.user(&task.created_by).await {
            Ok(Some(user)) => user.github_login.unwrap_or(user.display_name),
            _ => "a person".to_owned(),
        };
        let request = Request::parse(&task.prompt);
        let dir = home.join(".yulius").join("tasks");
        let path = dir.join(format!("{}.md", task.number));
        let body = request_markdown(task.number, &task.title, &author, &request, &done);
        let write_path = path.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&write_path, body)
        })
        .await
        .map_err(AppError::internal)?
        .map_err(AppError::internal)?;
        let path = path.to_string_lossy();
        Ok(TaskGoal {
            launch: goal_condition(&request, &path, &done),
            typed: typed_goal_condition(&request, &path, &done),
        })
    }

    /// The goal was never taken by the session: the task needs a person, said in its terminal
    /// and by the task itself.
    pub(super) async fn goal_not_taken(&self, task_id: &TaskId, why: &str) {
        self.goal_never_set(task_id, why, None).await;
    }

    /// [`Self::goal_not_taken`], with what the person does `first`, before a goal can be typed.
    async fn goal_never_set(&self, task_id: &TaskId, why: &str, first: Option<&str>) {
        let (notice, reason) = goal_missing(why, first);
        self.runtime(task_id).hub.notice(notice);
        self.flag_task(task_id, reason).await;
    }

    /// Claude Code still asks whether to trust the folder once the time to answer it is over
    /// (SPEC-106 OTR-002): the task needs a person, told what happens `then`.
    pub(super) async fn trust_unanswered(&self, task_id: &TaskId, then: &str) {
        self.runtime(task_id)
            .hub
            .notice(format!("{TRUST_UNANSWERED}. {TRUST_ANSWER}{then}"));
        self.flag_task(
            task_id,
            AttentionRecord::yulius(
                &format!("{TRUST_UNANSWERED}."),
                &format!("{TRUST_ANSWER}{then}"),
            ),
        )
        .await;
    }

    /// The terminal's screen is not what the program drew (agent secrets could not be read,
    /// ADR-0317): nothing was typed, and the task needs a person. The terminal is told once per
    /// attached run, however many typists are refused (a first message, messages from agents
    /// and Yulius). The flag is raised each time: one that stands stays as it is, and one the
    /// agent's own state replaced meanwhile (its turn ended) comes back with its reason.
    pub(crate) async fn screen_withheld(&self, task_id: &TaskId) {
        let runtime = self.runtime(task_id);
        if !runtime.screen_flagged.swap(true, Ordering::SeqCst) {
            runtime
                .hub
                .notice(format!("{SCREEN_WITHHELD}. {SCREEN_WITHHELD_ANSWER}."));
        }
        self.flag_task(
            task_id,
            AttentionRecord::yulius(
                &format!("{SCREEN_WITHHELD}."),
                &format!("{SCREEN_WITHHELD_ANSWER}."),
            ),
        )
        .await;
    }

    /// Marks the task as needing a person, for `reason`: what its terminal said to the viewers
    /// there, kept for whoever opens the task later (SPEC-472).
    pub(super) async fn flag_task(&self, task_id: &TaskId, reason: AttentionRecord) {
        if let Err(err) = self.flag_attention(task_id, reason).await {
            tracing::warn!(task = %task_id, error = %err, "could not flag the task");
        }
    }

    /// Whether the task's latest run still has its typed first message pending: a prompt held
    /// for capacity in that run is this first message (SPEC-068 OPN-003).
    pub(crate) async fn held_opening(&self, task_id: &TaskId) -> bool {
        self.deps
            .store
            .attempts(task_id)
            .await
            .is_ok_and(|attempts| attempts.last().is_some_and(|a| opening_pending(&a.run_dir)))
    }

    /// A restart cut the typing of the run's first message (SPEC-068 OPN-002): the task needs a
    /// person, said in its terminal.
    pub(super) async fn opening_never_sent(&self, task_id: &TaskId) {
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        let goal = if task.mode == TaskMode::Goal {
            " Then type `/goal` and the request: the task has no goal yet."
        } else {
            ""
        };
        let problem = "Yulius restarted while it typed this task's first message, so the first \
                       message was never sent and may be half typed in the prompt box.";
        let action = format!(
            "Complete it (paste the attached images again if they are missing; the task's \
             request lists them) and press Enter.{goal}"
        );
        // No viewer is connected when a server starts: the task itself has to say it.
        self.runtime(task_id)
            .hub
            .notice(format!("{problem} {action}"));
        self.flag_task(task_id, AttentionRecord::yulius(problem, &action))
            .await;
    }

    /// Waits until the attempt's TUI reads input: Claude Code's SessionStart hook arrived and
    /// [`GOAL_QUIET`] passed without new output (or at most the configured
    /// [`GOAL_READY_TIMEOUT`] after `started`, half of it without a SessionStart hook); with
    /// `after_opening`, also until the turn on a first message ended (its Stop hook, at most
    /// [`GOAL_OPENING_TIMEOUT`]). [`InputReady::Gone`] once the attempt is no longer live.
    ///
    /// Claude Code's folder-trust dialog (before the SessionStart hook) is never ready: Yulius
    /// waits for a person to answer it, at most [`crate::context::AppConfig::trust_answer_ms`]
    /// from when it showed ([`InputReady::TrustUnanswered`] when still shown then, so nothing is
    /// typed into it). The keys that answered it are no draft: `typed_before` moves past them.
    /// Once it is answered the wait starts over, as for a TUI that just started (SPEC-106
    /// OTR-002).
    pub(super) async fn wait_input_ready(
        &self,
        task_id: &TaskId,
        attempt_id: &AttemptId,
        started: tokio::time::Instant,
        after_opening: bool,
        typed_before: &mut i64,
    ) -> InputReady {
        let runtime = self.runtime(task_id);
        let ready_timeout = Duration::from_millis(self.config.goal_ready_ms);
        let mut last = runtime.hub.output_seq();
        let mut quiet_since = tokio::time::Instant::now();
        // When the trust dialog showed; `None` while it does not.
        let mut trust_since: Option<tokio::time::Instant> = None;
        let mut started = started;
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if !self.attempt_live(task_id, attempt_id).await {
                return InputReady::Gone;
            }
            // A screen that is withheld, or misses output dropped while it was, is not what the
            // program drew: whether it reads input, or asks something (the trust dialog), cannot
            // be told, also once output flows again (the dropped part is gone until the run is
            // attached anew). Nothing is typed; the task's person is told.
            if runtime.screen_withheld() {
                return InputReady::ScreenWithheld;
            }
            let trust = !runtime.session_started.load(Ordering::SeqCst)
                && trust_prompt_shown(&runtime.hub.screen_text());
            if trust || trust_since.is_some() {
                // Typed while the dialog shows, or in the moment it went: its answer.
                *typed_before = runtime.typed_at_ms.load(Ordering::SeqCst);
            }
            match (trust, trust_since) {
                (true, None) => {
                    runtime.hub.notice(
                        "Claude Code asks whether to trust this folder. Answer it in the \
                         terminal; Yulius sets the task going once it is answered."
                            .to_owned(),
                    );
                    trust_since = Some(tokio::time::Instant::now());
                    continue;
                }
                (true, Some(since)) => {
                    if since.elapsed() >= Duration::from_millis(self.config.trust_answer_ms) {
                        return InputReady::TrustUnanswered;
                    }
                    continue;
                }
                (false, Some(_)) => {
                    // Answered: the TUI starts its session now.
                    trust_since = None;
                    started = tokio::time::Instant::now();
                    quiet_since = started;
                    last = runtime.hub.output_seq();
                }
                (false, None) => {}
            }
            let seq = runtime.hub.output_seq();
            if seq != last {
                last = seq;
                quiet_since = tokio::time::Instant::now();
            }
            // Without hooks (an old CLI) the first output is the only sign of life.
            let hooked = runtime.session_started.load(Ordering::SeqCst)
                || started.elapsed() >= ready_timeout / 2;
            // A turn on the first message (the attached files) runs: quiet output is no sign
            // its end came, its Stop hook is (SPEC-025).
            let opening = after_opening && runtime.opening_turn.load(Ordering::SeqCst);
            let ready = hooked && !opening && last > 0 && quiet_since.elapsed() >= GOAL_QUIET;
            let limit = if opening {
                GOAL_OPENING_TIMEOUT
            } else {
                ready_timeout
            };
            if ready || started.elapsed() >= limit {
                return InputReady::Ready;
            }
        }
    }

    /// Steps a session's transcript back over a usage limit on a blocking thread (SPEC-072
    /// LRS-003; the file I/O is bounded by the transcript's size) and indexes it again only when
    /// it was rewritten. A failure is logged and the resume goes on: Claude Code skips the error
    /// row itself while it is recent (SPEC-034 LIM-002).
    pub(super) async fn step_back_transcript(
        &self,
        sandbox: &SandboxId,
        home: &Path,
        worktree: &Path,
        session_id: &str,
    ) {
        let harness = Arc::clone(&self.deps.harness);
        let (home, worktree, session) = (
            home.to_path_buf(),
            worktree.to_path_buf(),
            session_id.to_owned(),
        );
        let stepped = tokio::task::spawn_blocking(move || {
            harness.step_back_transcript(&home, &worktree, &session)
        })
        .await;
        match stepped {
            Ok(Ok(changed)) => {
                tracing::info!(session = %session_id, changed, "transcript stepped back over a usage limit");
                if changed {
                    self.reindex_session(sandbox, session_id).await;
                }
            }
            Ok(Err(err)) => {
                tracing::warn!(session = %session_id, error = %err, "could not step the transcript back");
            }
            Err(err) => {
                tracing::warn!(session = %session_id, error = %err, "the transcript step back did not finish");
            }
        }
    }

    /// Whether a person's keys are kept from the task's session now: Yulius types its first
    /// message, and Claude Code's folder-trust dialog, which only a person answers, is not
    /// shown (SPEC-117 ONE-006).
    pub(crate) fn keys_wait_for_opening(&self, task_id: &TaskId) -> bool {
        let runtime = self.runtime(task_id);
        runtime.typing_opening.load(Ordering::SeqCst)
            && (runtime.session_started.load(Ordering::SeqCst)
                || !trust_prompt_shown(&runtime.hub.screen_text()))
    }

    pub(crate) async fn attempt_live(&self, task_id: &TaskId, attempt_id: &AttemptId) -> bool {
        matches!(
            self.deps.store.live_attempt(task_id).await,
            Ok(Some(a)) if a.id == *attempt_id
        )
    }

    /// Types a session's first message that carries images (SPEC-030): in order, each image's
    /// path as a bracketed paste of its own where it sits, the text around them as pastes of their
    /// own (SPEC-124), then Enter. Each paste is taken
    /// before the next is written (pastes written in one burst lose the image): the screen shows
    /// its `[Image #N]`, or the output changed and settled. Typed once, never retried, so the
    /// request is never sent twice. Typing stops once the attempt is no longer live: input goes
    /// to the task's current run, which would then be another session.
    pub(super) async fn type_opening(
        &self,
        task_id: &TaskId,
        attempt_id: &AttemptId,
        pastes: Vec<Paste>,
    ) {
        let runtime = self.runtime(task_id);
        for (index, (paste, shown)) in opening_pastes(pastes).into_iter().enumerate() {
            if !self.attempt_live(task_id, attempt_id).await {
                return;
            }
            let before = runtime.hub.output_seq();
            let input = if index == 0 {
                SessionInput::Paste(&paste)
            } else {
                SessionInput::ContinuePaste(&paste)
            };
            if let Err(err) = self.send_session_input(task_id, input).await {
                tracing::warn!(task = %task_id, error = %err, "could not type the first message");
                return;
            }
            let started = tokio::time::Instant::now();
            let mut last = before;
            let mut quiet_since = started;
            while started.elapsed() < PASTE_TIMEOUT {
                tokio::time::sleep(Duration::from_millis(50)).await;
                if shown
                    .as_deref()
                    .is_some_and(|s| runtime.hub.screen_text().contains(s))
                {
                    break;
                }
                let seq = runtime.hub.output_seq();
                if seq != last {
                    last = seq;
                    quiet_since = tokio::time::Instant::now();
                }
                if last != before && quiet_since.elapsed() >= PASTE_QUIET {
                    break;
                }
            }
        }
        if !self.attempt_live(task_id, attempt_id).await {
            return;
        }
        if let Err(err) = self.send_session_input(task_id, SessionInput::Submit).await {
            tracing::warn!(task = %task_id, error = %err, "could not submit the first message");
        }
    }

    /// Watches a session that got its goal as its launch prompt (SPEC-117). A folder-trust dialog
    /// shown before it reads input is said in the terminal, and one still unanswered after
    /// [`crate::context::AppConfig::trust_answer_ms`] flags the task (ONE-005, SPEC-106 OTR-002);
    /// Claude Code runs the goal once it is answered. Once it reads input, Claude Code has to
    /// record the goal within [`crate::context::AppConfig::launch_goal_ms`] (time while a hold for
    /// capacity or an update keeps the prompt does not count), else the task is flagged as for a
    /// typed goal never taken (TSK-010). A watch that ends with an outcome clears the run's
    /// [`LAUNCH_GOAL_FILE`]; a restart watches a goal whose marker is left (SPEC-146 GWR-002).
    /// A restarted server watches a goal that was still to be typed ([`PendingGoal::Typed`])
    /// the same way: nothing types it any more, so only a goal Claude Code recorded before the
    /// restart ends the watch without a flag (SPEC-483 RST-002).
    pub(super) fn spawn_goal_watch(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt_id: &AttemptId,
        run_dir: &Path,
        goal: PendingGoal,
    ) {
        let app = Arc::clone(self);
        let (task_id, attempt_id) = (task_id.clone(), attempt_id.clone());
        let run_dir = run_dir.to_path_buf();
        tokio::spawn(async move {
            let mut typed_before = 0;
            let started = tokio::time::Instant::now();
            // A goal that was to be typed after the session's opening turn is watched from
            // that turn's end (the restarted server gave the run its opening turn back): a
            // flag raised inside the turn would be taken away by the turn's own end.
            let after_opening = matches!(goal, PendingGoal::Typed);
            match app
                .wait_input_ready(
                    &task_id,
                    &attempt_id,
                    started,
                    after_opening,
                    &mut typed_before,
                )
                .await
            {
                // This watch types nothing: whether the launch goal was taken is told by its
                // hook below, not by the screen, so a withheld screen changes nothing here.
                InputReady::Ready | InputReady::ScreenWithheld => {}
                InputReady::Gone => return,
                InputReady::TrustUnanswered => {
                    match goal {
                        PendingGoal::Launch => {
                            app.trust_unanswered(
                                &task_id,
                                "; the task starts by itself once it is answered.",
                            )
                            .await;
                        }
                        // Nothing types this goal once the dialog is answered: the person does,
                        // as the typist would have told them.
                        PendingGoal::Typed => {
                            app.goal_never_set(&task_id, TRUST_UNANSWERED, Some(TRUST_ANSWER))
                                .await;
                        }
                    }
                    goal.settled(&run_dir);
                    return;
                }
            }
            let runtime = app.runtime(&task_id);
            let bound = Duration::from_millis(app.config.launch_goal_ms);
            let step = Duration::from_millis(200);
            let mut waited = Duration::ZERO;
            while waited < bound {
                if runtime.goal_submitted.load(Ordering::SeqCst) {
                    goal.settled(&run_dir);
                    return;
                }
                if !app.attempt_live(&task_id, &attempt_id).await {
                    return;
                }
                tokio::time::sleep(step).await;
                if !app.launch_goal_held(&task_id).await {
                    waited += step;
                }
            }
            if !runtime.goal_submitted.load(Ordering::SeqCst)
                && app.attempt_live(&task_id, &attempt_id).await
            {
                tracing::error!(task = %task_id, "the task's launch goal was never taken by the agent");
                app.goal_not_taken(&task_id, goal.not_taken()).await;
                goal.settled(&run_dir);
            } else if runtime.goal_submitted.load(Ordering::SeqCst) {
                goal.settled(&run_dir);
            }
        });
    }

    /// Types `/goal <condition>` into a resumed session once its TUI reads input: after Claude
    /// Code's SessionStart hook, the turn on its first message ended and quiet output
    /// ([`Self::wait_input_ready`]). A fresh session gets its goal as its launch prompt instead
    /// (SPEC-117). The submit is confirmed
    /// by what Claude Code records about the goal and retried with growing waits; a goal never
    /// taken flags the task. A person who typed into the session since `typed_before` (the
    /// typing time when the run was attached) keeps their draft: no goal is typed and the task is
    /// flagged (SPEC-068).
    pub(super) fn spawn_goal(
        self: &Arc<Self>,
        task_id: &TaskId,
        (attempt_id, run_dir): (&AttemptId, &Path),
        condition: &str,
        typed_before: i64,
    ) {
        let app = Arc::clone(self);
        let (task_id, attempt_id) = (task_id.clone(), attempt_id.clone());
        let run_dir = run_dir.to_path_buf();
        let command = format!("{GOAL_COMMAND}{condition}");
        tokio::spawn(async move {
            if app
                .type_goal(&task_id, &attempt_id, &command, typed_before)
                .await
            {
                PendingGoal::Typed.settled(&run_dir);
            }
        });
    }

    /// The typist of [`Self::spawn_goal`]. `true` once the goal has an outcome (recorded, the
    /// task flagged, or the prompt box left to the person who typed into it); `false` when the
    /// run ended or could not be typed into, which says nothing about its goal.
    async fn type_goal(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt_id: &AttemptId,
        command: &str,
        typed_before: i64,
    ) -> bool {
        let app = self;
        let runtime = app.runtime(task_id);
        let live = || app.attempt_live(task_id, attempt_id);
        let started = tokio::time::Instant::now();
        let mut typed_before = typed_before;
        match app
            .wait_input_ready(task_id, attempt_id, started, true, &mut typed_before)
            .await
        {
            InputReady::Ready => {}
            InputReady::Gone => return false,
            InputReady::TrustUnanswered => {
                app.goal_never_set(task_id, TRUST_UNANSWERED, Some(TRUST_ANSWER))
                    .await;
                return true;
            }
            InputReady::ScreenWithheld => {
                app.goal_never_set(task_id, SCREEN_WITHHELD, Some(SCREEN_WITHHELD_ANSWER))
                    .await;
                return true;
            }
        }
        // The opening turn is over, or given up on: a later Stop is the goal's own.
        runtime.opening_turn.store(false, Ordering::SeqCst);
        runtime.goal_submitted.store(false, Ordering::SeqCst);
        // A person typed before the goal: `/goal` would be appended to their draft and sent
        // with it. The draft is theirs; the task waits for them instead (#252).
        if runtime.typed_at_ms.load(Ordering::SeqCst) != typed_before {
            app.goal_not_taken(task_id, GOAL_LEFT_TO_DRAFT).await;
            return true;
        }
        // One typist from the goal's line until Claude Code took it or the tries are over: a
        // submit the TUI dropped leaves the line in the prompt box, where a message typed
        // meanwhile would be sent with it by the next Enter, or erased by the retype (SPEC-483
        // TYP-002).
        let mut typing = None;
        for (attempt, wait) in GOAL_SUBMIT_WAITS.iter().enumerate() {
            // A person typing meanwhile owns the prompt box: nothing is cleared or retyped.
            if runtime.typed_at_ms.load(Ordering::SeqCst) != typed_before {
                return true;
            }
            let mut input = Vec::new();
            if attempt == 0 || attempt == GOAL_RETYPE_ATTEMPT {
                if attempt > 0 {
                    input.push(0x15); // Ctrl-U: clear what is in the prompt box
                }
                input.extend_from_slice(command.as_bytes());
            }
            if typing.is_none() {
                crate::race::reached(crate::race::Point::LineReady, Path::new(task_id.as_str()));
                typing = crate::task_message::Typing::wait(
                    &runtime.typing_message,
                    crate::task_message::TYPING_TURN_WAIT,
                )
                .await;
            }
            // Asked again with the mark held: the wait for it gives a person time to type.
            if runtime.typed_at_ms.load(Ordering::SeqCst) != typed_before {
                // Before anything was typed, the task has no goal: said, as above.
                if attempt == 0 {
                    app.goal_not_taken(task_id, GOAL_LEFT_TO_DRAFT).await;
                }
                return true;
            }
            if !input.is_empty()
                && let Err(err) = app
                    .send_session_input(task_id, SessionInput::Paste(&input))
                    .await
            {
                tracing::warn!(task = %task_id, error = %err, "could not set the task goal");
                return false;
            }
            crate::race::reached(crate::race::Point::LineTyped, Path::new(task_id.as_str()));
            // A separate Enter, so the TUI sees typed text first and then a submit.
            tokio::time::sleep(Duration::from_millis(300)).await;
            app.note_issued_prompt(task_id, command.as_bytes());
            if let Err(err) = app.send_session_input(task_id, SessionInput::Submit).await {
                tracing::warn!(task = %task_id, error = %err, "could not submit the task goal");
                return false;
            }
            crate::race::reached(crate::race::Point::LineEntered, Path::new(task_id.as_str()));
            let waited = tokio::time::Instant::now();
            while waited.elapsed() < *wait {
                if runtime.goal_submitted.load(Ordering::SeqCst) {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
                if !live().await {
                    return false;
                }
            }
            tracing::warn!(task = %task_id, attempt, "the task goal was not taken; submitting again");
        }
        if !live().await {
            return false;
        }
        if !runtime.goal_submitted.load(Ordering::SeqCst) {
            tracing::error!(task = %task_id, "the task goal was never taken by the agent");
            app.goal_not_taken(task_id, "Claude Code did not take the task's goal")
                .await;
        }
        true
    }

    /// Whether the launch goal's watch of `task` does not count now: a prompt held for capacity,
    /// an update or a person's pause is not refused, it waits (SPEC-191 PER-005).
    pub async fn launch_goal_held(&self, task: &TaskId) -> bool {
        self.held_for_update(task)
            || matches!(self.deps.store.open_pause(task).await, Ok(Some(_)))
            || self.is_paused_by_person(task).await
    }
}

#[cfg(test)]
mod tests {
    /// SPEC-106 OTR-002: the dialogs of Claude Code 2.1.292 (measured) and of older releases.
    #[test]
    fn the_folder_trust_dialog_is_recognised() {
        let current = "Quick safety check: Is this a project you created or one you trust?\n\
                       \u{276f} No, continue without these permissions\n  Yes, I trust this folder";
        assert!(super::trust_prompt_shown(current));
        assert!(super::trust_prompt_shown(
            "Do you trust the files in this folder?\n\u{276f} 1. Yes, proceed"
        ));
        assert!(!super::trust_prompt_shown(
            "\u{276f} Try \"refactor <filepath>\""
        ));
    }

    /// SPEC-472 ATN-004, ATN-006: a goal that was never set says in its terminal what it said
    /// before, and its flag keeps what is wrong apart from what to do. Behind an unanswered
    /// trust question the to-do starts there.
    #[test]
    fn a_goal_never_set_says_what_is_wrong_and_what_to_do() {
        let (notice, reason) =
            super::goal_missing("Claude Code did not take the task's goal", None);
        assert_eq!(
            notice,
            "Claude Code did not take the task's goal. The task has no goal yet: type `/goal` and \
             the request in the terminal, or stop and continue the task."
        );
        assert_eq!(
            reason.problem.as_deref(),
            Some("Claude Code did not take the task's goal. The task has no goal yet.")
        );
        assert_eq!(
            reason.action.as_deref(),
            Some("Type `/goal` and the request in the terminal, or stop and continue the task.")
        );
        assert!(reason.link.is_none() && !reason.work_answers);

        let (notice, reason) =
            super::goal_missing(super::TRUST_UNANSWERED, Some(super::TRUST_ANSWER));
        assert_eq!(
            notice,
            "Claude Code still asks whether to trust this folder, so Yulius typed nothing. Answer \
             it in the terminal. The task has no goal yet: type `/goal` and the request in the \
             terminal, or stop and continue the task."
        );
        assert_eq!(
            reason.problem.as_deref(),
            Some(
                "Claude Code still asks whether to trust this folder, so Yulius typed nothing. \
                 The task has no goal yet."
            )
        );
        assert_eq!(
            reason.action.as_deref(),
            Some(
                "Answer it in the terminal, then type `/goal` and the request, or stop and \
                 continue the task."
            )
        );
    }

    /// SPEC-117 ONE-003: a launch prompt that is a `/goal` sets the goal; any other prompt, or
    /// none, leaves it to be typed.
    #[test]
    fn only_a_goal_launch_prompt_sets_the_goal() {
        use super::{SessionMode, sets_goal};
        let new = |prompt: Option<&str>| SessionMode::New {
            session_id: None,
            prompt: prompt.map(str::to_owned),
        };
        let resume = |prompt: Option<&str>| SessionMode::Resume {
            session_id: "s".into(),
            prompt: prompt.map(str::to_owned),
            continue_interrupted: false,
            fork: None,
        };
        assert!(sets_goal(&new(Some(
            "/goal Everything requested in /t/1.md is done."
        ))));
        assert!(sets_goal(&resume(Some("/goal Make it blue"))));
        for mode in [
            new(None),
            new(Some("Make it blue")),
            new(Some("/goalkeeper")),
            resume(None),
            resume(Some("Continue the task exactly where you left off.")),
        ] {
            assert!(!sets_goal(&mode), "{mode:?}");
        }
    }

    #[test]
    fn a_typed_first_message_pastes_each_image_where_it_sits_the_last_text_ending_in_a_newline() {
        use yulius_domain::request::Paste;
        let pastes = super::opening_pastes(vec![
            Paste::Text("Look at ".into()),
            Paste::Image("/s/a.png".into()),
            Paste::Text(" and ".into()),
            Paste::Image("/s/my b.png".into()),
            Paste::Text(" @\"/s/c.txt\"".into()),
        ]);
        let shown: Vec<(String, Option<String>)> = pastes
            .into_iter()
            .map(|(bytes, shown)| (String::from_utf8_lossy(&bytes).into_owned(), shown))
            .collect();
        assert_eq!(
            shown,
            [
                ("\x1b[200~Look at \x1b[201~".to_owned(), None),
                (
                    "\x1b[200~/s/a.png\x1b[201~".to_owned(),
                    Some("[Image #1]".to_owned())
                ),
                ("\x1b[200~ and \x1b[201~".to_owned(), None),
                (
                    "\x1b[200~/s/my b.png\x1b[201~".to_owned(),
                    Some("[Image #2]".to_owned())
                ),
                ("\x1b[200~ @\"/s/c.txt\"\n\x1b[201~".to_owned(), None),
            ]
        );
        // A message that ends with an image ends with its paste.
        let only = super::opening_pastes(vec![Paste::Image("/s/a.png".into())]);
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].1.as_deref(), Some("[Image #1]"));
    }
    #[test]
    fn final_screen_round_trips() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(super::FINAL_SCREEN_FILE);
        super::write_private_file(&path, b"120 40\n\x1b[1mbye")?;
        let (cols, rows, bytes) = super::read_final_screen(dir.path()).ok_or("unreadable")?;
        assert_eq!(
            (cols, rows, bytes.as_slice()),
            (120, 40, b"\x1b[1mbye".as_slice())
        );
        assert!(super::read_final_screen(&dir.path().join("missing")).is_none());
        Ok(())
    }

    /// SPEC-229 RBT-002 (review): a hub with no output yet (a runtime made fresh by a hook that
    /// came before reattach) never replaces a saved screen with a blank one; one with output
    /// replaces it whole, leaving no temporary file behind.
    #[test]
    fn a_screen_is_saved_only_from_a_hub_with_output() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(super::FINAL_SCREEN_FILE);
        super::write_private_file(&path, b"80 24\nthe last turn")?;
        let hub = crate::hub::Hub::new(120, 40);
        super::save_screen(&hub, dir.path());
        assert_eq!(std::fs::read(&path)?, b"80 24\nthe last turn");

        hub.output(1, b"a newer turn");
        super::save_screen(&hub, dir.path());
        let (cols, rows, bytes) = super::read_final_screen(dir.path()).ok_or("unreadable")?;
        assert_eq!((cols, rows), (120, 40));
        assert!(String::from_utf8_lossy(&bytes).contains("a newer turn"));
        let names: Vec<_> = std::fs::read_dir(dir.path())?
            .map(|e| e.map(|e| e.file_name()))
            .collect::<Result<_, _>>()?;
        assert_eq!(names, [std::ffi::OsString::from(super::FINAL_SCREEN_FILE)]);
        Ok(())
    }
}
