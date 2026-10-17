//! Settings → Instance, "Sandbox Docker" through the API (SPEC-203 HST-014, HST-015): the
//! verdict for superadmins only, a server without isolation, the base's rebuild and its
//! estimate, the sandboxes on their own Docker, the last update's re-check, and the smoke test
//! started from the page with each check as it ends.

use crate::support;

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use serde_json::Value;
use support::updates::{FakeHelper, FakeSource, OLD, services};
use support::{
    Creds, Harness, TestResult, admit_and_sign_in, call, configure_github_app, harness,
    seed_sandbox, setup_superadmin_creds,
};
use yulius_app::error::AppResult;
use yulius_app::ports::isolation::{
    DockerStatus, SandboxIsolation, SandboxJoin, SandboxRef, SmokeCheck,
};
use yulius_contracts::update::{HostCheckReport, HostLine, HostOutcome};
use yulius_domain::ids::SandboxId;

const PRODUCT: &str = "6b76ccc7-0000-4000-8000-000000000001";

/// An isolation adapter whose Docker status and smoke the test sets.
struct Docker {
    status: DockerStatus,
    /// The smoke's second check waits for this.
    go: tokio::sync::Notify,
    smokes: Mutex<usize>,
}

#[async_trait::async_trait]
impl SandboxIsolation for Docker {
    async fn prepare(&self, _: &SandboxRef) -> AppResult<Option<SandboxJoin>> {
        Ok(None)
    }
    fn release(&self, _: &SandboxId) {}
    async fn docker_status(&self) -> Option<DockerStatus> {
        Some(self.status.clone())
    }
    async fn docker_smoke(
        &self,
        report: &(dyn Fn(SmokeCheck) + Send + Sync),
    ) -> Result<bool, String> {
        if let Ok(mut n) = self.smokes.lock() {
            *n += 1;
        }
        report(SmokeCheck {
            name: "a run older than the engine gets the socket".into(),
            ok: true,
            detail: "/var/run/docker.sock".into(),
        });
        self.go.notified().await;
        report(SmokeCheck {
            name: "docker run --rm hello-world in the old run".into(),
            ok: false,
            detail: "toomanyrequests: You have reached your pull rate limit".into(),
        });
        Ok(false)
    }
}

struct Instance {
    h: Harness,
    admin: Creds,
    member: Creds,
    helper: Arc<FakeHelper>,
}

async fn instance() -> Result<Instance, Box<dyn std::error::Error>> {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let member = admit_and_sign_in(&h, &admin, "alice", 11, true).await?;
    let helper = FakeHelper::installed();
    assert!(
        h.app
            .set_updates(services(OLD, &FakeSource::behind(), &helper))
    );
    Ok(Instance {
        h,
        admin,
        member,
        helper,
    })
}

async fn view(i: &Instance) -> Result<Value, Box<dyn std::error::Error>> {
    let r = call(
        &i.h.router,
        Method::GET,
        "/api/admin/sandbox-docker",
        None,
        &i.admin,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    Ok(r.body)
}

/// Waits (bounded) until the view satisfies `done`.
async fn until(
    i: &Instance,
    done: impl Fn(&Value) -> bool,
) -> Result<Value, Box<dyn std::error::Error>> {
    for _ in 0..500 {
        let v = view(i).await?;
        if done(&v) {
            return Ok(v);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err("the view never reached the expected state".into())
}

/// HST-014: superadmins only; without isolation sandboxes get no Docker, and the view says how
/// to change that.
#[tokio::test]
async fn only_superadmins_see_it_and_no_isolation_says_why() -> TestResult {
    let i = instance().await?;
    for (method, path) in [
        (Method::GET, "/api/admin/sandbox-docker"),
        (Method::POST, "/api/admin/sandbox-docker/smoke"),
    ] {
        let r = call(&i.h.router, method, path, None, &i.member).await?;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{path}: {}", r.body);
    }
    let v = view(&i).await?;
    assert_eq!(v["isolated"], false);
    assert_eq!(v["ok"], false);
    let detail = v["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("YULIUS_CONTAINED=true"), "{detail}");
    assert!(detail.contains("sudo yulius host setup"), "{detail}");
    let r = call(
        &i.h.router,
        Method::POST,
        "/api/admin/sandbox-docker/smoke",
        None,
        &i.admin,
    )
    .await?;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.body);
    Ok(())
}

/// HST-014/HST-017/HST-012: the verdict's words, the rebuild with its estimate (started + the
/// last build's duration), a sandbox on its own Docker by name with the way back, and the last
/// update's re-check.
#[tokio::test]
async fn the_view_has_the_verdict_rebuild_own_engines_and_re_check() -> TestResult {
    let i = instance().await?;
    seed_sandbox(&i.h, PRODUCT, "product", "alice").await?;
    let started = 1_790_000_000_000_i64;
    assert!(
        i.h.app.set_isolation(Arc::new(Docker {
            status: DockerStatus {
                ok: false,
                detail:
                    "the current sandbox base (base-3) has no `dockerd` though the host has one: a \
                     new base is being built"
                        .into(),
                base: Some(("base-3".into(), false)),
                host_dockerd: true,
                building_since_ms: Some(started),
                last_build_secs: Some(70),
                own_engines: vec![(PRODUCT.into(), "usr/sbin/dockerd".into())],
                ..DockerStatus::default()
            },
            go: tokio::sync::Notify::new(),
            smokes: Mutex::new(0),
        }))
    );
    if let Ok(mut kept) = i.helper.host_check.lock() {
        *kept = Some(HostCheckReport {
            ok: false,
            lines: vec![HostLine {
                name: "host Docker units".into(),
                outcome: HostOutcome::Failed,
                detail: "`systemctl disable --now docker.service` failed".into(),
            }],
            error: None,
            commit: Some("3333333333333333333333333333333333333333".into()),
            checked_at: 1,
        });
    }
    let v = view(&i).await?;
    assert_eq!(v["isolated"], true);
    assert_eq!(v["ok"], false);
    assert!(v["detail"].as_str().unwrap_or_default().contains("base-3"));
    assert_eq!(v["base"]["id"], "base-3");
    assert_eq!(v["base"]["dockerd"], false);
    assert_eq!(v["host_dockerd"], true);
    assert_eq!(v["rebuild"]["building_since"], "2026-09-21T14:13:20Z");
    assert_eq!(v["rebuild"]["ready_by"], "2026-09-21T14:14:30Z");
    assert_eq!(v["own_engines"][0]["name"], "acme/product");
    assert_eq!(v["own_engines"][0]["dockerd"], "usr/sbin/dockerd");
    assert!(
        v["own_engine_fix"]
            .as_str()
            .unwrap_or_default()
            .contains("apt-get install -y docker-ce")
    );
    assert_eq!(v["host_check"]["ok"], false);
    assert_eq!(v["host_check"]["lines"][0]["name"], "host Docker units");
    Ok(())
}

/// HST-015: the smoke runs in the background, one at a time; each check is in the view as it
/// ends, then the outcome.
#[tokio::test]
async fn the_smoke_test_reports_each_check_as_it_ends() -> TestResult {
    let i = instance().await?;
    let docker = Arc::new(Docker {
        status: DockerStatus {
            ok: true,
            detail: "each isolated sandbox gets its own engine".into(),
            ..DockerStatus::default()
        },
        go: tokio::sync::Notify::new(),
        smokes: Mutex::new(0),
    });
    assert!(i.h.app.set_isolation(docker.clone()));
    let r = call(
        &i.h.router,
        Method::POST,
        "/api/admin/sandbox-docker/smoke",
        None,
        &i.admin,
    )
    .await?;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{}", r.body);
    let v = until(&i, |v| {
        v["smoke"]["checks"]
            .as_array()
            .is_some_and(|c| c.len() == 1)
    })
    .await?;
    assert_eq!(v["smoke"]["state"], "running");
    assert_eq!(v["smoke"]["started_by"], "superadmin");
    let again = call(
        &i.h.router,
        Method::POST,
        "/api/admin/sandbox-docker/smoke",
        None,
        &i.admin,
    )
    .await?;
    assert_eq!(
        again.status,
        StatusCode::CONFLICT,
        "one at a time: {}",
        again.body
    );
    docker.go.notify_one();
    let v = until(&i, |v| v["smoke"]["state"] != "running").await?;
    assert_eq!(v["smoke"]["state"], "failed");
    assert_eq!(v["smoke"]["checks"][1]["ok"], false);
    assert!(
        v["smoke"]["checks"][1]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("pull rate limit")
    );
    assert!(v["smoke"]["finished_at"].is_string());
    assert_eq!(docker.smokes.lock().map(|n| *n).unwrap_or(0), 1);
    Ok(())
}
