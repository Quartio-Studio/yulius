//! Runs recorded before SPEC-087 get their figures from their transcripts, once (SPEC-091).
//!
//! Migration 23 queued every run recorded before it (`metrics_recounts`). After start, once
//! reconciliation has ended the runs that did not survive the restart, each queued run is counted
//! again from its own window of its session transcript, by the live watch's rules, and its outcome
//! is recorded with the figures it replaced. A recorded run is never counted again.

use std::path::PathBuf;

use yulius_domain::harness::Harness;
use yulius_domain::ids::AttemptId;
use yulius_domain::time::Timestamp;

use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{AttemptRecord, LaunchKind, MetricsRecord, RecountOutcome};
use crate::ports::harness::WatchFrom;
use crate::supervisor::metrics_of;

/// What one recount round did (SPEC-091 RCT-004).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecountTally {
    /// Runs whose figures were replaced by their transcripts'.
    pub recomputed: u32,
    /// Runs without a transcript: their figures stay.
    pub no_transcript: u32,
    /// Runs whose transcript holds none of their lines: their figures stay.
    pub no_lines: u32,
    /// Runs still running (their watch reads their transcript).
    pub live: u32,
    /// Runs deleted since they were queued.
    pub gone: u32,
    /// Runs left queued after a failure, for the next start.
    pub failed: u32,
}

impl RecountTally {
    fn add(&mut self, outcome: RecountOutcome) {
        let count = match outcome {
            RecountOutcome::Recomputed => &mut self.recomputed,
            RecountOutcome::NoTranscript => &mut self.no_transcript,
            RecountOutcome::NoLines => &mut self.no_lines,
            RecountOutcome::Live => &mut self.live,
            RecountOutcome::Gone => &mut self.gone,
        };
        *count += 1;
    }

    /// Whether the round did anything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Where a run's lines are: its session transcript and its window of it.
struct RunWindow {
    harness: Harness,
    path: PathBuf,
    from: WatchFrom,
    until: Option<Timestamp>,
}

impl App {
    /// Counts every queued run again from its transcript and records the outcome (SPEC-091
    /// RCT-004, RCT-005). Call it after reconciliation, in the background: files are read off the
    /// async workers, one run at a time. A run that fails stays queued for the next start.
    ///
    /// # Errors
    /// Store failures listing the queue.
    pub async fn recount_old_metrics(&self) -> AppResult<RecountTally> {
        let mut tally = RecountTally::default();
        for id in self.deps.store.queued_metrics_recounts().await? {
            match self.recount_run(&id).await {
                Ok(Some(outcome)) => tally.add(outcome),
                Ok(None) => {}
                Err(err) => {
                    tally.failed += 1;
                    tracing::warn!(attempt = %id, error = %err, "a run's metrics could not be recounted");
                }
            }
        }
        if !tally.is_empty() {
            let detail = format!(
                "recomputed {}, no transcript {}, no lines {}, live {}, gone {}, failed {}",
                tally.recomputed,
                tally.no_transcript,
                tally.no_lines,
                tally.live,
                tally.gone,
                tally.failed
            );
            tracing::info!(%detail, "metrics of runs recorded before SPEC-087 recounted");
            if let Err(err) = self
                .deps
                .store
                .audit(
                    self.now(),
                    "yulius",
                    "metrics.recounted",
                    None,
                    Some(&detail),
                )
                .await
            {
                tracing::warn!(error = %err, "audit of the metrics recount failed");
            }
        }
        Ok(tally)
    }

    /// Recounts one queued run; `None` when another round recorded it first.
    async fn recount_run(&self, id: &AttemptId) -> AppResult<Option<RecountOutcome>> {
        let (outcome, metrics) = match self.deps.store.attempt(id).await? {
            None => (RecountOutcome::Gone, None),
            Some(attempt) if attempt.ended_at.is_none() => (RecountOutcome::Live, None),
            Some(attempt) => self.count_ended_run(&attempt).await?,
        };
        let recorded = self
            .deps
            .store
            .finish_metrics_recount(id, outcome, metrics.as_ref(), self.now())
            .await?;
        Ok(recorded.then_some(outcome))
    }

    async fn count_ended_run(
        &self,
        attempt: &AttemptRecord,
    ) -> AppResult<(RecountOutcome, Option<MetricsRecord>)> {
        let Some(window) = self.run_window(attempt).await? else {
            return Ok((RecountOutcome::NoTranscript, None));
        };
        let harness = self.deps.harness.clone();
        let counted = tokio::task::spawn_blocking(move || {
            harness.recount(window.harness, &window.path, window.from, window.until)
        })
        .await
        .map_err(AppError::internal)??;
        let Some(snapshot) = counted else {
            return Ok((RecountOutcome::NoTranscript, None));
        };
        let metrics = metrics_of(snapshot);
        let counted_nothing = metrics.input_tokens.is_none()
            && metrics.output_tokens.is_none()
            && metrics.cache_read.is_none()
            && metrics.cache_write.is_none()
            && metrics.turns.is_none()
            && metrics.active_ms.is_none();
        Ok(if counted_nothing {
            (RecountOutcome::NoLines, None)
        } else {
            (RecountOutcome::Recomputed, Some(metrics))
        })
    }

    /// A run's lines (RCT-001): its session transcript, from its start as its live watch read
    /// it, to the earlier of its end and the next start of a run of its task on that session.
    /// `None` without a session, a task or a worktree to find the transcript by.
    async fn run_window(&self, attempt: &AttemptRecord) -> AppResult<Option<RunWindow>> {
        let Some(session) = attempt.provider_session_id.as_deref() else {
            return Ok(None);
        };
        let Some(task) = self.deps.store.task(&attempt.task_id).await? else {
            return Ok(None);
        };
        let Some(worktree) = task.worktree_path.as_deref() else {
            return Ok(None);
        };
        let path = match attempt.harness {
            Harness::ClaudeCode => self.deps.harness.claude_transcript(
                &self.sandbox_home(&task.sandbox_id),
                worktree.as_ref(),
                session,
            ),
        };
        let from = match attempt.launch_kind {
            LaunchKind::New => WatchFrom::Start,
            LaunchKind::Resume => WatchFrom::Since(attempt.started_at),
        };
        let next = self
            .deps
            .store
            .attempts(&attempt.task_id)
            .await?
            .into_iter()
            .filter(|other| {
                other.id != attempt.id
                    && other.provider_session_id.as_deref() == Some(session)
                    && other.started_at > attempt.started_at
            })
            .map(|other| other.started_at)
            .min();
        let until = match (attempt.ended_at, next) {
            (Some(end), Some(next)) => Some(end.min(next)),
            (end, next) => end.or(next),
        };
        Ok(Some(RunWindow {
            harness: attempt.harness,
            path,
            from,
            until,
        }))
    }
}
