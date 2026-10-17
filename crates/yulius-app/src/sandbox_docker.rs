//! Settings → Instance, "Sandbox Docker" (SPEC-203 HST-014, HST-015): whether isolated sandboxes
//! get Docker on this server, why not and the fix (the doctor's words, from the isolation
//! adapter), the base's rebuild and when it will be ready, the sandboxes on their own Docker,
//! the last update's re-check, and the Docker smoke test run on demand.

use std::sync::{Arc, Mutex};

use yulius_contracts::api::updates::{
    BaseRebuildView, DockerSmokeCheck, DockerSmokeView, OwnEngineView, SandboxBaseView,
    SandboxDockerView,
};
use yulius_domain::ids::SandboxId;
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::ports::isolation::SmokeCheck;

/// Why sandboxes get no Docker on a server that does not isolate them.
pub const NOT_ISOLATED: &str = "sandboxes are not isolated on this server, so they get no \
    Docker: run Yulius contained (YULIUS_CONTAINED=true in yulius.service) with the isolation \
    prerequisites, which `sudo yulius host setup` installs; `sudo -u yulius yulius doctor \
    --contained` says what is missing";

/// How a sandbox on its own Docker goes back to Docker's packages (the doctor's words).
pub const OWN_ENGINE_FIX: &str = "to use Docker's packages, as the base does, run in that \
    sandbox `sudo apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin \
    docker-compose-plugin` (apt replaces docker.io; the engine switches when it next starts, \
    after it was idle); never `sudo apt-get remove docker.io`, which hides the base's Docker too";

/// The smoke test started from the page: one at a time, its checks as they end.
#[derive(Default)]
pub struct SmokeState {
    run: Arc<Mutex<Option<DockerSmokeView>>>,
}

impl SmokeState {
    fn view(&self) -> Option<DockerSmokeView> {
        self.run.lock().ok()?.clone()
    }
}

fn ms_to_rfc3339(ms: i64) -> String {
    rfc3339(Timestamp(ms))
}

impl App {
    /// `GET /api/admin/sandbox-docker`.
    ///
    /// # Errors
    /// Not the superadmin.
    pub async fn sandbox_docker_view(&self, ctx: &AuthContext) -> AppResult<SandboxDockerView> {
        self.require_superadmin(ctx)?;
        let host_check = self.updates_helper().and_then(|helper| helper.host_check());
        let smoke = self.docker_smoke.view();
        let status = match self.isolation.get() {
            Some(isolation) => isolation.docker_status().await,
            None => None,
        };
        let Some(status) = status else {
            return Ok(SandboxDockerView {
                ok: false,
                detail: NOT_ISOLATED.into(),
                isolated: false,
                base: None,
                host_dockerd: false,
                rebuild: None,
                own_engines: Vec::new(),
                own_engine_fix: OWN_ENGINE_FIX.into(),
                host_check,
                smoke,
            });
        };
        let mut own_engines = Vec::new();
        for (sandbox, dockerd) in status.own_engines {
            let name = match SandboxId::parse(&sandbox) {
                Ok(id) => self
                    .deps
                    .store
                    .sandbox(&id)
                    .await
                    .ok()
                    .flatten()
                    .map(|s| format!("{}/{}", s.repo_owner, s.repo_name)),
                Err(_) => None,
            };
            own_engines.push(OwnEngineView {
                sandbox,
                name,
                dockerd,
            });
        }
        let last_ms = status
            .last_build_secs
            .and_then(|s| i64::try_from(s.saturating_mul(1000)).ok());
        let ready_by = status
            .building_since_ms
            .or(status.starts_at_ms)
            .zip(last_ms)
            .map(|(from, took)| ms_to_rfc3339(from.saturating_add(took)));
        let rebuild = (status.building_since_ms.is_some()
            || status.pending_since_ms.is_some()
            || status.build_error.is_some())
        .then(|| BaseRebuildView {
            building_since: status.building_since_ms.map(ms_to_rfc3339),
            pending_since: status.pending_since_ms.map(ms_to_rfc3339),
            starts_at: status.starts_at_ms.map(ms_to_rfc3339),
            ready_by,
            last_build_secs: status.last_build_secs,
            error: status.build_error,
        });
        Ok(SandboxDockerView {
            ok: status.ok,
            detail: status.detail,
            isolated: true,
            base: status
                .base
                .map(|(id, dockerd)| SandboxBaseView { id, dockerd }),
            host_dockerd: status.host_dockerd,
            rebuild,
            own_engines,
            own_engine_fix: OWN_ENGINE_FIX.into(),
            host_check,
            smoke,
        })
    }

    /// `POST /api/admin/sandbox-docker/smoke`: starts the smoke test in the background (one at
    /// a time); the view shows each check as it ends.
    ///
    /// # Errors
    /// Not the superadmin; one runs already; sandboxes are not isolated here.
    pub async fn start_docker_smoke(&self, ctx: &AuthContext) -> AppResult<SandboxDockerView> {
        self.require_superadmin(ctx)?;
        let Some(isolation) = self.isolation.get().cloned() else {
            return Err(AppError::Conflict(NOT_ISOLATED.into()));
        };
        let login = self.login_of(&ctx.user.id).await;
        {
            let mut run = self
                .docker_smoke
                .run
                .lock()
                .map_err(|_| AppError::Internal("the smoke state is poisoned".into()))?;
            if run.as_ref().is_some_and(|r| r.state == "running") {
                return Err(AppError::Conflict(
                    "a Docker smoke test is running already".into(),
                ));
            }
            *run = Some(DockerSmokeView {
                state: "running".into(),
                started_by: login,
                started_at: rfc3339(self.now()),
                finished_at: None,
                checks: Vec::new(),
                error: None,
            });
        }
        let state = Arc::clone(&self.docker_smoke.run);
        let clock = Arc::clone(&self.deps.clock);
        tokio::spawn(async move {
            let checks = Arc::clone(&state);
            let report = move |check: SmokeCheck| {
                if let Ok(mut run) = checks.lock()
                    && let Some(run) = run.as_mut()
                {
                    run.checks.push(DockerSmokeCheck {
                        name: check.name,
                        ok: check.ok,
                        detail: check.detail,
                    });
                }
            };
            let result = isolation.docker_smoke(&report).await;
            if let Ok(mut run) = state.lock()
                && let Some(run) = run.as_mut()
            {
                run.finished_at = Some(rfc3339(clock.now()));
                match result {
                    Ok(true) => run.state = "passed".into(),
                    Ok(false) => run.state = "failed".into(),
                    Err(why) => {
                        run.state = "failed".into();
                        run.error = Some(why);
                    }
                }
            }
        });
        self.sandbox_docker_view(ctx).await
    }
}
