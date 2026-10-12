//! Status panel (SPEC-001 UI-007, AC-STA-1): machine metrics, visible sandboxes with live
//! session counts, account usage and capacity.

use std::collections::HashMap;

use yulius_contracts::api::status::{
    SandboxStatus, SessionCounts, StatusAccount, StatusView, VmStatus,
};
use yulius_domain::task::TaskStatus;

use crate::accounts::{account_usage, in_use_count, usage_view};
use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::ports::system::SystemSnapshot;

fn vm_view(s: &SystemSnapshot) -> VmStatus {
    VmStatus {
        cpu_percent: s.cpu_percent.filter(|p| p.is_finite()),
        cpu_count: s.cpu_count,
        memory_used_bytes: s.memory_used,
        memory_total_bytes: s.memory_total,
        disk_used_bytes: s.disk_used,
        disk_total_bytes: s.disk_total,
        load: s.load.filter(|l| l.iter().all(|v| v.is_finite())),
        load_1m: s.load.map(|l| l[0]).filter(|v| v.is_finite()),
        uptime_s: s.uptime_s,
    }
}

impl App {
    /// Current machine metrics (read off the async executor).
    ///
    /// # Errors
    /// `Internal` when the blocking reader task fails.
    pub async fn system_snapshot(&self) -> AppResult<SystemSnapshot> {
        let probe = self.deps.system.clone();
        let dir = self.config.data_dir.clone();
        tokio::task::spawn_blocking(move || probe.snapshot(&dir))
            .await
            .map_err(AppError::internal)
    }

    /// The status panel for the caller: sandboxes are limited to those the caller can see;
    /// accounts show label, provider, status and usage to everyone and full detail to instance
    /// administrators.
    ///
    /// # Errors
    /// Store failures.
    pub async fn status(&self, ctx: &AuthContext) -> AppResult<StatusView> {
        let store = &self.deps.store;
        let snapshot = self.system_snapshot().await?;
        let live = store.live_attempts().await?;
        let mut live_by_sandbox: HashMap<String, u32> = HashMap::new();
        for attempt in &live {
            if let Some(task) = store.task(&attempt.task_id).await? {
                *live_by_sandbox
                    .entry(task.sandbox_id.to_string())
                    .or_default() += 1;
            }
        }
        let sandboxes = self
            .visible_sandboxes(&ctx.user)
            .await?
            .into_iter()
            .map(|(s, _)| SandboxStatus {
                live_sessions: live_by_sandbox.get(s.id.as_str()).copied().unwrap_or(0),
                id: s.id.to_string(),
                name: s.repo_name.clone(),
                slug: s.slug,
            })
            .collect();
        let admin = ctx.user.is_superadmin || ctx.user.is_instance_admin;
        let accounts = store
            .accounts()
            .await?
            .iter()
            .map(|a| StatusAccount {
                label: a.label.clone(),
                provider: a.provider.as_str().to_owned(),
                status: a.status.as_str().to_owned(),
                usage: usage_view(&account_usage(a)),
                id: admin.then(|| a.id.to_string()),
                last_error: if admin { a.last_error.clone() } else { None },
                in_use_by: admin.then(|| in_use_count(&a.id, &live)),
            })
            .collect();
        let queued = store.tasks_with_status(&[TaskStatus::Queued]).await?.len();
        Ok(StatusView {
            vm: vm_view(&snapshot),
            sessions: SessionCounts {
                // What the limit counts: Done tasks' idle sessions do not (SES-101).
                live: self.sessions_in_use().await?,
                limit: self.config.max_live_sessions,
                queued: u32::try_from(queued).unwrap_or(u32::MAX),
            },
            sandboxes,
            accounts,
            contained: self.config.contained,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_finite_metrics_become_unknown() {
        let view = vm_view(&SystemSnapshot {
            cpu_percent: Some(f32::NAN),
            load: Some([f64::INFINITY, 1.0, 1.0]),
            memory_total: Some(8),
            ..SystemSnapshot::default()
        });
        assert_eq!(view.cpu_percent, None);
        assert_eq!(view.load, None);
        assert_eq!(view.load_1m, None);
        assert_eq!(view.memory_total_bytes, Some(8));
        assert_eq!(view.disk_total_bytes, None);
    }
}
