//! Input to a live run: the viewer's keys and submits, the prompts Yulius issues, and writes
//! and resizes of the session.

use std::path::Path;
use std::sync::atomic::Ordering;

use yulius_domain::ids::TaskId;

use super::{IssuedPrompt, SessionInput, SubmitInvocation, ViewerSubmission};
use crate::context::App;
use crate::error::{AppError, AppResult};

impl App {
    /// A delivery records its actual bytes before the provider can acknowledge it.
    /// This disambiguates its callback from an owner editing or recalling a prompt.
    pub(crate) fn note_issued_prompt(&self, task: &TaskId, text: &[u8]) {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(text).into();
        let runtime = self.runtime(task);
        let mut receipts = runtime
            .prompt_receipts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let issued = &mut receipts.issued;
        const LIMIT: usize = 16;
        let pending = issued
            .iter()
            .position(|p| p.digest == digest)
            .and_then(|index| issued.remove(index))
            .map_or(1, |p| p.pending.saturating_add(1));
        if issued.len() == LIMIT {
            issued.pop_front();
        }
        issued.push_back(IssuedPrompt { digest, pending });
    }

    pub(crate) fn record_viewer_key(
        &self,
        task: &TaskId,
        at: i64,
        enter: crate::terminal::ViewerEnter,
    ) -> u64 {
        let seq = self.supervisor.input_seq.fetch_add(1, Ordering::SeqCst) + 1;
        if enter.may_submit() {
            let mut submits = self
                .supervisor
                .viewer_submits
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let pending = submits.entry(task.clone()).or_default();
            // Keep the first unacknowledged Enter and the most recent submissions.
            const MAX_PENDING_SUBMITS: usize = 256;
            if pending.len() == MAX_PENDING_SUBMITS {
                pending.remove(1);
            }
            pending.push_back(ViewerSubmission {
                time: at,
                sequence: seq,
                enter,
            });
        }
        seq
    }

    pub(crate) fn viewer_input_sequence(&self) -> u64 {
        self.supervisor.input_seq.load(Ordering::SeqCst)
    }

    pub(crate) fn retract_viewer_submit(&self, task: &TaskId, seq: u64) {
        let mut submits = self
            .supervisor
            .viewer_submits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(pending) = submits.get_mut(task) {
            pending.retain(|key| key.sequence != seq);
            if pending.is_empty() {
                submits.remove(task);
            }
        }
    }

    pub(crate) fn viewer_submit_at(
        &self,
        task: &TaskId,
        sent: i64,
        sequence: u64,
        prompt: &[u8],
        invocation: Option<u128>,
    ) -> Option<(i64, u64)> {
        use crate::terminal::ViewerEnter;
        use sha2::Digest;
        let digest: [u8; 32] = sha2::Sha256::digest(prompt).into();
        let runtime = self.runtime(task);
        let mut submits = self
            .supervisor
            .viewer_submits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut eligible = submits.get(task).into_iter().flatten().filter(|key| {
            key.sequence <= sequence
                && key.time <= sent
                && sent.saturating_sub(key.time) <= yulius_domain::intake::TYPED_WINDOW_MS
        });
        let mut receipts = runtime
            .prompt_receipts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(id) = invocation {
            // A replay cannot acknowledge another owner's Enter, even if the words match.
            if let Some(previous) = receipts
                .invocations
                .iter_mut()
                .find(|request| request.id == id)
            {
                return previous.held_owner.take();
            }
            const LIMIT: usize = 128;
            if receipts.invocations.len() == LIMIT {
                receipts.invocations.pop_front();
            }
            receipts.invocations.push_back(SubmitInvocation {
                id,
                held_owner: None,
            });
        }
        let direct = eligible
            .clone()
            .find(|key| key.enter == ViewerEnter::Submitted(digest));
        // An invocation with different identity may submit an edited copy of an
        // acknowledged instruction. Older clients need direct input evidence for a copy.
        let owner =
            if direct.is_some() {
                direct
            } else if let Some(issued) = receipts.issued.iter_mut().find(|issued| {
                issued.digest == digest && (invocation.is_none() || issued.pending > 0)
            }) {
                issued.pending = issued.pending.saturating_sub(1);
                None
            } else {
                eligible.next()
            };
        let owner = owner.map(|key| (key.time, key.sequence));
        if let (Some(_), Some((_, seq))) = (invocation, owner)
            && let Some(pending) = submits.get_mut(task)
        {
            // Transfer the Enter to its invocation before another callback can claim it.
            // A held invocation retains the selected key for its own release.
            pending.retain(|key| key.sequence != seq);
            if pending.is_empty() {
                submits.remove(task);
            }
        }
        owner
    }

    /// The release of a held submit acknowledges the original Enter, even if
    /// another invocation was processed while the subscription was unavailable.
    pub(crate) fn keep_held_owner(&self, task: &TaskId, id: u128, owner: (i64, u64)) {
        let runtime = self.runtime(task);
        let mut receipts = runtime
            .prompt_receipts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(request) = receipts
            .invocations
            .iter_mut()
            .find(|request| request.id == id)
        {
            request.held_owner = Some(owner);
        }
    }

    pub(crate) fn consume_viewer_submits(&self, task: &TaskId, through: u64) {
        let mut keys = self
            .supervisor
            .viewer_submits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(pending) = keys.get_mut(task) {
            pending.retain(|key| key.sequence > through);
            if pending.is_empty() {
                keys.remove(task);
            }
        }
    }

    /// Writes viewer keyboard input to the run.
    ///
    /// # Errors
    /// `Conflict` when no run is attached.
    pub async fn send_input(&self, task_id: &TaskId, bytes: &[u8]) -> AppResult<()> {
        self.send_session_input(task_id, SessionInput::Viewer(bytes))
            .await
    }

    /// Writes input through the attempt's writer, with a screen guard for automation.
    pub(crate) async fn send_session_input(
        &self,
        task_id: &TaskId,
        input: SessionInput<'_>,
    ) -> AppResult<()> {
        let runtime = self
            .existing_runtime(task_id)
            .ok_or_else(|| AppError::Conflict("the task is not running".into()))?;
        let now = self.now();
        if matches!(input, SessionInput::Viewer(_)) {
            runtime.last_input_ms.store(now.0, Ordering::SeqCst);
        }
        let mut run = runtime.run.lock().await;
        let run = run
            .as_mut()
            .ok_or_else(|| AppError::Conflict("the task is not running".into()))?;
        runtime.write_session_input(run, now, input).await
    }

    /// Resizes the run's PTY.
    ///
    /// # Errors
    /// `Conflict` when no run is attached.
    pub async fn resize_run(&self, task_id: &TaskId, cols: u16, rows: u16) -> AppResult<()> {
        let (cols, rows) = (cols.clamp(10, 1000), rows.clamp(5, 1000));
        let runtime = self.runtime(task_id);
        let mut run = runtime.run.lock().await;
        if let Some(input) = run.as_mut().and_then(|run| run.codex_input.as_mut()) {
            input.follow_viewer((cols, rows));
        }
        runtime.hub.resize(cols, rows);
        crate::race::reached(
            crate::race::Point::TerminalResizeOwnsWriter,
            Path::new(task_id.as_str()),
        );
        match run.as_mut() {
            Some(run) => run.writer.resize(cols, rows).await,
            None => Ok(()),
        }
    }
}
