//! The instance for a person (SPEC-487 slice 7, INS-001 to INS-004; wall 1): Yulius's build,
//! the host's load, memory and disk, the sessions, and an update offered or under way, as
//! Settings shows them; and for a superadmin, the update and restart buttons.
//!
//! Contract:
//! - reading is everyone's who is signed in, as the Status panel and Settings show it to them;
//! - checking, updating, restarting and cancelling are a superadmin's: the buttons' own
//!   functions, with their refusals as they are. The person's ask is the confirmation the
//!   buttons ask for (their decision of 2026-11-13: these run at once for a superadmin's
//!   Superagent).

use serde_json::{Value, json};
use yulius_contracts::api::updates::{StartRestart, StartUpdate};

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};

/// What `instance_update` does.
pub const INSTANCE_ACTIONS: [&str; 4] = ["check", "update", "restart", "cancel"];

impl App {
    /// The instance as the person may see it (INS-001): the Status panel (`status`) and the
    /// update section of Settings (`update`: the build it runs, an update offered or under way).
    ///
    /// # Errors
    /// Store failures.
    pub async fn instance_for_person(&self, ctx: &AuthContext) -> AppResult<Value> {
        let status = self.status(ctx).await?;
        let update = self.update_view(ctx).await?;
        Ok(json!({
            "status": crate::person_tools::record(&status)?,
            "update": crate::person_tools::record(&update)?,
        }))
    }

    /// Checks for an update, installs the offered one (or `commit`), restarts the server with
    /// the build it runs, or cancels an update under way (INS-002, INS-003), for a superadmin:
    /// the buttons' functions, confirmed by the ask; their refusals pass as they are. Answers
    /// the update section as it is then.
    ///
    /// # Errors
    /// `Forbidden` for anyone but a superadmin; `Invalid` for another action; the functions'
    /// refusals (no helper on this server, nothing offered, an update or a move under way).
    pub async fn instance_update_for_person(
        &self,
        ctx: &AuthContext,
        action: &str,
        commit: Option<&str>,
    ) -> AppResult<Value> {
        if !INSTANCE_ACTIONS.contains(&action) {
            return Err(AppError::Invalid(
                "`do` is check, update, restart or cancel".into(),
            ));
        }
        if !ctx.user.is_superadmin {
            return Err(AppError::Forbidden(
                "only a superadmin checks for, installs or cancels an update of Yulius or \
                 restarts it; tell the person so"
                    .into(),
            ));
        }
        let view = match action {
            "check" => self.check_updates_now(ctx).await?,
            "update" => {
                let commit = match commit.map(str::trim).filter(|commit| !commit.is_empty()) {
                    Some(commit) => commit.to_owned(),
                    None => self
                        .update_view(ctx)
                        .await?
                        .available
                        .map(|offered| offered.commit)
                        .ok_or_else(|| {
                            AppError::Conflict(
                                "no update is offered now: check for one first (do: check)".into(),
                            )
                        })?,
                };
                self.start_update(
                    ctx,
                    &StartUpdate {
                        commit,
                        confirm: true,
                    },
                )
                .await?
            }
            "restart" => {
                self.start_restart(ctx, &StartRestart { confirm: true })
                    .await?
            }
            _ => self.cancel_update(ctx).await?,
        };
        crate::person_tools::record(&view)
    }
}
