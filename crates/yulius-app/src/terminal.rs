//! Terminal viewers: attach, input, resize, who sets the size, and permission re-checks
//! (SPEC-002 §7.7, AC-TSK-6, ACC-008; the size policy is [`crate::hub::control`]).

use std::sync::Arc;
use std::time::Duration;

use yulius_domain::ids::TaskId;
use yulius_domain::permission::Action;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::hub::{Change, Subscription};
use crate::supervisor::TaskRuntime;

/// Maximum concurrent viewers per task.
pub const MAX_VIEWERS: u64 = 16;
/// Longest wait for the agent's redraw after a viewer's size changed the terminal at attach.
pub const REDRAW_WAIT: Duration = Duration::from_millis(400);

/// An attached terminal viewer.
pub struct TerminalAttach {
    /// Task id.
    pub task_id: TaskId,
    /// Snapshot + live frames.
    pub subscription: Subscription,
    /// Whether this viewer may type.
    pub can_write: bool,
    /// This viewer's id within the task's hub.
    pub viewer_id: u64,
    runtime: Arc<TaskRuntime>,
}

impl TerminalAttach {
    /// Whether this viewer currently controls the PTY size (TSK-006).
    #[must_use]
    pub fn is_controller(&self) -> bool {
        self.runtime.hub.controller() == self.viewer_id
    }
}

impl Drop for TerminalAttach {
    fn drop(&mut self) {
        // Normally [`App::terminal_leave`] already handed control over (and resized the
        // terminal); this covers a viewer dropped without it.
        self.runtime.hub.leave(self.viewer_id);
        self.runtime.hub.unsubscribe();
    }
}

/// Facts sent in `state` frames.
#[derive(Debug, Clone)]
pub struct TerminalState {
    /// Attempt sequence.
    pub attempt_seq: Option<u32>,
    /// Harness wire name.
    pub harness: Option<String>,
    /// PTY size.
    pub size: (u16, u16),
    /// Task status wire name.
    pub status: String,
}

impl App {
    /// Attaches a viewer to a task terminal (read access suffices to watch). `size` (cols,
    /// rows) is the viewer's terminal: a writer gets it applied before its snapshot is taken
    /// when nobody controls the terminal or its controller is idle (it then controls it,
    /// TSK-006, F 2026-10-10), and when that changed the size, the snapshot waits for the
    /// agent's redraw ([`REDRAW_WAIT`] at most) — so the first state and screen it receives
    /// are already at its size and layout, and nothing reflows after the terminal appears
    /// (F 2026-10-09).
    ///
    /// # Errors
    /// Access errors; `Capacity` beyond [`MAX_VIEWERS`].
    pub async fn terminal_open(
        &self,
        ctx: &AuthContext,
        task_id: &str,
        size: Option<(u16, u16)>,
    ) -> AppResult<TerminalAttach> {
        let task = self.load_task(ctx, task_id, Action::View).await?;
        let (_, level) = self
            .require(&ctx.user, &task.sandbox_id, Action::View)
            .await?;
        let runtime = self.runtime(&task.id);
        if !runtime.hub.has_output()
            && self.deps.store.live_attempt(&task.id).await?.is_none()
            && let Some(last) = self.deps.store.attempts(&task.id).await?.pop()
            && let Some((cols, rows, screen)) =
                crate::supervisor::read_final_screen(std::path::Path::new(&last.run_dir))
        {
            runtime.hub.reset(1, &screen, cols, rows);
        }
        if runtime.hub.viewer_count() >= MAX_VIEWERS {
            return Err(AppError::Capacity("too many viewers on this task".into()));
        }
        // Only the owner types into the task (SPEC-070 R3).
        let can_write = level.allows(Action::Work)
            && ctx.user.disabled_at.is_none()
            && task.owner == ctx.user.id;
        let viewer_id = runtime.hub.new_viewer_id();
        if can_write {
            let before = runtime.hub.size();
            let since = runtime.hub.output_seq();
            let watch = runtime.hub.watch();
            let joined = runtime.hub.join_writer(viewer_id, size);
            if let Some((cols, rows)) = joined.resize {
                if task.status.is_live() {
                    if let Err(err) = self.resize_run(&task.id, cols, rows).await {
                        runtime.hub.leave(viewer_id);
                        return Err(err);
                    }
                    if runtime.hub.size() != before && runtime.has_run().await {
                        runtime.hub.settle(watch, since, REDRAW_WAIT).await;
                    }
                } else {
                    // No run yet (queued, about to start): the screen takes the size now, so the
                    // first state already says it and the run spawns at it (SPEC-050 TRM-101; a
                    // viewer of a task just created saw 120x40 first on the fast VM).
                    runtime
                        .hub
                        .resize(cols.clamp(10, 1000), rows.clamp(5, 1000));
                }
            }
        }
        let subscription = runtime.hub.subscribe();
        Ok(TerminalAttach {
            task_id: task.id,
            subscription,
            can_write,
            viewer_id,
            runtime,
        })
    }

    /// Current terminal facts.
    ///
    /// # Errors
    /// Store failures.
    pub async fn terminal_state(&self, task_id: &TaskId) -> AppResult<TerminalState> {
        let task = self
            .deps
            .store
            .task(task_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let attempt = self.deps.store.live_attempt(task_id).await?;
        Ok(TerminalState {
            attempt_seq: attempt.as_ref().map(|a| a.seq),
            harness: Some(
                attempt
                    .map_or(task.harness, |a| a.harness)
                    .as_str()
                    .to_owned(),
            ),
            size: self.runtime(task_id).hub.size(),
            status: task.status.as_str().to_owned(),
        })
    }

    /// Viewer input (requires write access, re-checked against the permission cache).
    ///
    /// # Errors
    /// `Forbidden` for read-only viewers, `Conflict` when nothing is running.
    pub async fn terminal_input(
        &self,
        ctx: &AuthContext,
        attach: &TerminalAttach,
        data: &[u8],
        scroll: bool,
    ) -> AppResult<()> {
        if !attach.can_write || !self.can_work(ctx, &attach.task_id).await {
            return Err(AppError::Forbidden("read-only access".into()));
        }
        if data.len() > 64 * 1024 {
            return Err(AppError::Invalid("input frame too large".into()));
        }
        // The viewer who types controls the terminal and its size, at once (TSK-006). A
        // forwarded wheel turn is not typing: it must not move control, and with it the size, to
        // that viewer (#78).
        if !scroll {
            let change = attach.runtime.hub.claim(attach.viewer_id);
            // The keystroke matters more than the size: a failed resize never drops it.
            if let Err(err) = self.apply_control(&attach.task_id, change).await {
                tracing::debug!(task = %attach.task_id, error = %err, "typist's size not applied");
            }
            // A prompt submitted soon after is this person's (TSK-013).
            self.record_typing(&attach.task_id, &ctx.user.id);
        }
        self.send_input(&attach.task_id, data).await
    }

    /// The size the viewer's window fits now (write access). The terminal takes it when this
    /// viewer controls the size; otherwise it is kept for when it does.
    ///
    /// # Errors
    /// `Forbidden` for read-only viewers.
    pub async fn terminal_resize(
        &self,
        ctx: &AuthContext,
        attach: &TerminalAttach,
        cols: u16,
        rows: u16,
    ) -> AppResult<()> {
        if !attach.can_write || !self.can_work(ctx, &attach.task_id).await {
            return Err(AppError::Forbidden("read-only access".into()));
        }
        let change = attach
            .runtime
            .hub
            .report_size(attach.viewer_id, (cols, rows));
        self.apply_control(&attach.task_id, change).await
    }

    /// A writer clicked into the terminal (`take_control`): it controls the size now.
    ///
    /// # Errors
    /// `Forbidden` for read-only viewers.
    pub async fn terminal_take_control(
        &self,
        ctx: &AuthContext,
        attach: &TerminalAttach,
    ) -> AppResult<()> {
        if !attach.can_write || !self.can_work(ctx, &attach.task_id).await {
            return Err(AppError::Forbidden("read-only access".into()));
        }
        let change = attach.runtime.hub.claim(attach.viewer_id);
        self.apply_control(&attach.task_id, change).await
    }

    /// The viewer is active (`active`) or idle (`idle`): an active writer takes the size from
    /// an idle controller; an idle controller hands it to the most recently active viewer.
    /// Read-only viewers never control, so their presence changes nothing.
    ///
    /// # Errors
    /// Store or PTY failures while applying a new size.
    pub async fn terminal_presence(
        &self,
        ctx: &AuthContext,
        attach: &TerminalAttach,
        active: bool,
    ) -> AppResult<()> {
        if !attach.can_write {
            return Ok(());
        }
        let change = if active {
            if !self.can_work(ctx, &attach.task_id).await {
                return Ok(());
            }
            attach.runtime.hub.viewer_active(attach.viewer_id)
        } else {
            attach.runtime.hub.viewer_idle(attach.viewer_id)
        };
        self.apply_control(&attach.task_id, change).await
    }

    /// The viewer left, or lost write access: control (and the size) passes on.
    pub async fn terminal_leave(&self, attach: &TerminalAttach) {
        let change = attach.runtime.hub.leave(attach.viewer_id);
        if let Err(err) = self.apply_control(&attach.task_id, change).await {
            tracing::debug!(task = %attach.task_id, error = %err, "terminal size hand-over failed");
        }
    }

    /// Applies a size-control change: the new controller's size, while the task is live (an
    /// ended task's final screen keeps its size).
    async fn apply_control(&self, task_id: &TaskId, change: Change) -> AppResult<()> {
        let Some((cols, rows)) = change.resize else {
            return Ok(());
        };
        let live = self
            .deps
            .store
            .task(task_id)
            .await?
            .is_some_and(|task| task.status.is_live());
        if live {
            self.resize_run(task_id, cols, rows).await?;
        }
        Ok(())
    }

    /// Whether the caller still owns the task and has write access to its sandbox (SPEC-070).
    pub async fn can_work(&self, ctx: &AuthContext, task_id: &TaskId) -> bool {
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return false;
        };
        if ctx.user.disabled_at.is_some() || task.owner != ctx.user.id {
            return false;
        }
        self.require(&ctx.user, &task.sandbox_id, Action::Work)
            .await
            .is_ok()
    }

    /// Whether the caller can still view the task.
    pub async fn can_view_task(&self, ctx: &AuthContext, task_id: &TaskId) -> bool {
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return false;
        };
        self.require(&ctx.user, &task.sandbox_id, Action::View)
            .await
            .is_ok()
    }
}
