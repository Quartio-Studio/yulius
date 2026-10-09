//! Terminal viewers: attach, input, resize and permission re-checks (SPEC-002 §7.7, AC-TSK-6,
//! ACC-008).

use std::sync::Arc;

use yulius_domain::ids::TaskId;
use yulius_domain::permission::Action;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::hub::Subscription;
use crate::supervisor::TaskRuntime;

/// Maximum concurrent viewers per task.
pub const MAX_VIEWERS: u64 = 16;

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
    /// Whether this viewer currently controls input focus and the PTY size (TSK-006).
    #[must_use]
    pub fn is_controller(&self) -> bool {
        self.runtime.hub.controller() == self.viewer_id
    }
}

impl Drop for TerminalAttach {
    fn drop(&mut self) {
        self.runtime.hub.release(self.viewer_id);
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
    /// Attaches a viewer to a task terminal (read access suffices to watch).
    ///
    /// # Errors
    /// Access errors; `Capacity` beyond [`MAX_VIEWERS`].
    pub async fn terminal_open(
        &self,
        ctx: &AuthContext,
        task_id: &str,
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
        let subscription = runtime.hub.subscribe();
        let viewer_id = runtime.hub.new_viewer_id();
        Ok(TerminalAttach {
            task_id: task.id,
            subscription,
            can_write: level.allows(Action::Work),
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
        // The last viewer to type controls the terminal (TSK-006). A forwarded wheel turn is not
        // typing: it must not move control, and with it the size, to that viewer (#78).
        if !scroll {
            attach.runtime.hub.take_control(attach.viewer_id);
        }
        self.send_input(&attach.task_id, data).await
    }

    /// Viewer resize (write access; the last resizing viewer controls the size).
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
        if !attach.runtime.hub.may_resize(attach.viewer_id) {
            // Another viewer controls the size; this viewer keeps watching at that size.
            return Ok(());
        }
        self.resize_run(&attach.task_id, cols, rows).await
    }

    /// Explicit "take control" by a writer: input focus and size follow this viewer.
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
        attach.runtime.hub.take_control(attach.viewer_id);
        Ok(())
    }

    /// Whether the caller still has write access to the task's sandbox.
    pub async fn can_work(&self, ctx: &AuthContext, task_id: &TaskId) -> bool {
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return false;
        };
        if ctx.user.disabled_at.is_some() {
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
