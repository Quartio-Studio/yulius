//! The instance's resource limits (SPEC-459): sized from the host by default, a unit value or a
//! setting on top, every pin recorded with the host it was configured on, flagged after a move
//! and adapted by the start that follows `yulius instance import`.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, Harness, HarnessOptions, TestResult, admit_and_sign_in, call, configure_github_app,
    harness, harness_with, setup_superadmin_creds, superadmin_bearer,
};
use yulius_app::limits::{ADAPT_MARK, AUDIT_TARGET, StartLimits, UnitLimits};
use yulius_app::ports::store::AuditStore as _;
use yulius_domain::limits::Limit;

const GIB: u64 = 1 << 30;
const GB: u64 = 1_000_000_000;
/// CPUs, GiB of memory and GB of data disk of the VM the instance ran on until 2026-11-11.
const OLD: (u32, u64, u64) = (12, 47, 387);
/// The host it moved to.
const NEW: (u32, u64, u64) = (64, 755, 3500);
/// The drop-ins of the old VM.
const OLD_PINS: [(Limit, u64); 3] = [
    (Limit::Sessions, 8),
    (Limit::SandboxMemory, 38),
    (Limit::SandboxDisk, 300),
];

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// Starts `h` on `host` with the unit's `pins`.
async fn start(h: &Harness, host: (u32, u64, u64), pins: &[(Limit, u64)]) -> StartLimits {
    h.system.set_host(host.0, host.1 * GIB);
    h.system.set_disk(host.2 * GB, 0);
    let unit: UnitLimits = pins.iter().copied().collect();
    h.app.load_limits(unit).await
}

/// A set-up instance started on `host` with the unit's `pins`.
async fn instance(host: (u32, u64, u64), pins: &[(Limit, u64)]) -> Fallible<(Harness, Creds)> {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    start(&h, host, pins).await;
    Ok((h, admin))
}

/// `before`'s instance (its store) started again on `host` with `pins`, in another data
/// directory; `moved`: `yulius instance import` put it there and left its mark.
async fn restarted(
    before: &Harness,
    host: (u32, u64, u64),
    pins: &[(Limit, u64)],
    moved: bool,
) -> Fallible<(Harness, Creds, StartLimits)> {
    let dir = tempfile::tempdir()?;
    if moved {
        std::fs::write(dir.path().join(ADAPT_MARK), b"")?;
    }
    let h = harness_with(HarnessOptions {
        store: Some(before.store.clone()),
        dir: Some(dir),
        ..HarnessOptions::default()
    })?;
    let started = start(&h, host, pins).await;
    let admin = superadmin_bearer(&h).await?;
    Ok((h, admin, started))
}

async fn view(h: &Harness, who: &Creds) -> Fallible<Value> {
    let r = call(&h.router, Method::GET, "/api/admin/limits", None, who).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    Ok(r.body)
}

/// The limit `key` of a limits view.
fn limit<'a>(view: &'a Value, key: &str) -> &'a Value {
    view["limits"]
        .as_array()
        .and_then(|limits| limits.iter().find(|l| l["key"] == key))
        .unwrap_or(&Value::Null)
}

/// `[value, source]` of every limit, in order.
fn in_force(view: &Value) -> Vec<(u64, String)> {
    view["limits"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|l| {
            (
                l["value"].as_u64().unwrap_or(u64::MAX),
                l["source"].as_str().unwrap_or("?").to_owned(),
            )
        })
        .collect()
}

fn defaults(values: [u64; 6]) -> Vec<(u64, String)> {
    values.map(|v| (v, "default".to_owned())).to_vec()
}

async fn put(h: &Harness, who: &Creds, key: &str, value: Value) -> Fallible<support::Resp> {
    let path = format!("/api/admin/limits/{key}");
    call(
        &h.router,
        Method::PUT,
        &path,
        Some(json!({ "value": value })),
        who,
    )
    .await
}

async fn post(h: &Harness, who: &Creds, action: &str) -> Fallible<Value> {
    let path = format!("/api/admin/limits/{action}");
    let r = call(&h.router, Method::POST, &path, None, who).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    Ok(r.body)
}

#[tokio::test]
async fn a_host_without_pins_gets_limits_of_its_size() -> TestResult {
    let (h, admin) = instance(OLD, &[]).await?;
    let old = view(&h, &admin).await?;
    assert_eq!(in_force(&old), defaults([8, 4, 2, 41, 100, 20]));
    assert_eq!(old["host"]["cpus"], 12);
    assert_eq!(old["host"]["memory_bytes"], 47 * GIB);
    assert_eq!(old["host"]["disk_bytes"], 387 * GB);

    let (h, admin, started) = restarted(&h, NEW, &[], false).await?;
    let new = view(&h, &admin).await?;
    assert_eq!(in_force(&new), defaults([34, 12, 4, 660, 875, 175]));
    // The numbers are in force, not only shown.
    assert_eq!(h.app.max_live_sessions(), 34);
    assert_eq!(h.app.ci_max_jobs(), 12);
    assert_eq!(h.app.ci_lanes(), 4);
    assert_eq!(h.app.run_disk_max(), 175 * GB);
    assert_eq!(
        started,
        StartLimits {
            memory_gb: None,
            disk_gb: 875
        }
    );
    let sessions = limit(&new, "max_sessions");
    assert_eq!(sessions["env"], "YULIUS_MAX_SESSIONS");
    assert_eq!(sessions["host_sized"], 34);
    assert_eq!(sessions["stale"], false);
    assert_eq!(sessions["configured_on"], Value::Null);
    assert!(
        sessions["formula"]
            .as_str()
            .is_some_and(|f| f.contains("half the CPUs")),
        "{sessions}"
    );
    Ok(())
}

#[tokio::test]
async fn a_unit_value_is_in_force_and_remembers_its_host() -> TestResult {
    let (h, admin) = instance(OLD, &OLD_PINS).await?;
    let v = view(&h, &admin).await?;
    let sessions = limit(&v, "max_sessions");
    assert_eq!(sessions["value"], 8);
    assert_eq!(sessions["source"], "unit");
    assert_eq!(sessions["unit_value"], 8);
    assert_eq!(sessions["stale"], false);
    assert_eq!(sessions["configured_on"]["cpus"], 12);
    assert_eq!(limit(&v, "sandbox_memory_gb")["value"], 38);
    assert_eq!(limit(&v, "sandbox_disk_gb")["value"], 300);
    assert_eq!(limit(&v, "ci_max_jobs")["source"], "default");
    assert_eq!(h.app.max_live_sessions(), 8);
    Ok(())
}

#[tokio::test]
async fn pins_copied_to_a_bigger_host_are_flagged_and_adapted_on_request() -> TestResult {
    let (old, _) = instance(OLD, &OLD_PINS).await?;
    let (h, admin, started) = restarted(&old, NEW, &OLD_PINS, false).await?;
    // Nothing changes by itself without the import's mark: the pins are in force.
    assert_eq!(
        started,
        StartLimits {
            memory_gb: Some(38),
            disk_gb: 300
        }
    );
    let v = view(&h, &admin).await?;
    let sessions = limit(&v, "max_sessions");
    assert_eq!(sessions["value"], 8);
    assert_eq!(sessions["source"], "unit");
    assert_eq!(sessions["stale"], true);
    assert_eq!(sessions["host_sized"], 34);
    assert_eq!(sessions["configured_on"]["cpus"], 12);
    assert_eq!(sessions["configured_on"]["memory_bytes"], 47 * GIB);
    assert_eq!(limit(&v, "sandbox_memory_gb")["stale"], true);
    assert_eq!(limit(&v, "sandbox_disk_gb")["stale"], true);
    assert_eq!(limit(&v, "ci_max_jobs")["stale"], false);

    let v = post(&h, &admin, "adapt").await?;
    let sessions = limit(&v, "max_sessions");
    assert_eq!(sessions["value"], 34);
    assert_eq!(sessions["source"], "default");
    assert_eq!(sessions["unit_value"], 8, "the unit's number stays visible");
    assert_eq!(sessions["setting"]["value"], Value::Null);
    assert_eq!(sessions["stale"], false);
    assert_eq!(limit(&v, "sandbox_disk_gb")["value"], 875);
    assert_eq!(h.app.max_live_sessions(), 34);
    // The memory waits for a restart: sandboxes were sized with 38 GiB.
    let memory = limit(&v, "sandbox_memory_gb");
    assert_eq!(memory["value"], 660);
    assert_eq!(memory["restart_needed"], true);

    let audit = h
        .store
        .audit_details(AUDIT_TARGET, "limits.adapted", 5)
        .await?;
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert!(audit[0].contains("max_sessions 8 → 34"), "{audit:?}");
    assert!(
        audit[0].contains("sandbox_disk_gb 300 GB → 875 GB"),
        "{audit:?}"
    );

    // The answer holds across restarts with the same unit.
    let (h, admin, started) = restarted(&h, NEW, &OLD_PINS, false).await?;
    assert_eq!(
        started,
        StartLimits {
            memory_gb: None,
            disk_gb: 875
        }
    );
    let v = view(&h, &admin).await?;
    assert_eq!(in_force(&v), defaults([34, 12, 4, 660, 875, 175]));
    assert_eq!(limit(&v, "sandbox_memory_gb")["restart_needed"], false);
    Ok(())
}

#[tokio::test]
async fn kept_pins_stay_and_are_not_flagged_again() -> TestResult {
    let (old, _) = instance(OLD, &OLD_PINS).await?;
    let (h, admin, _) = restarted(&old, NEW, &OLD_PINS, false).await?;
    let v = post(&h, &admin, "keep").await?;
    let sessions = limit(&v, "max_sessions");
    assert_eq!(sessions["value"], 8);
    assert_eq!(sessions["source"], "unit");
    assert_eq!(sessions["stale"], false);
    assert_eq!(sessions["configured_on"]["cpus"], 64);
    let kept = h
        .store
        .audit_details(AUDIT_TARGET, "limits.kept", 5)
        .await?;
    assert!(kept[0].contains("max_sessions 8"), "{kept:?}");

    let (h, admin, _) = restarted(&h, NEW, &OLD_PINS, false).await?;
    let v = view(&h, &admin).await?;
    assert_eq!(limit(&v, "max_sessions")["stale"], false);
    assert_eq!(limit(&v, "sandbox_disk_gb")["value"], 300);
    // A second answer has nothing to do and is not audited.
    post(&h, &admin, "keep").await?;
    post(&h, &admin, "adapt").await?;
    assert_eq!(
        h.store
            .audit_details(AUDIT_TARGET, "limits.kept", 5)
            .await?
            .len(),
        1
    );
    assert!(
        h.store
            .audit_details(AUDIT_TARGET, "limits.adapted", 5)
            .await?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn the_start_after_an_import_adapts_by_itself() -> TestResult {
    let (old, _) = instance(OLD, &OLD_PINS).await?;
    // The new unit holds one of the old drop-ins; the others are copied later.
    let (h, admin, started) = restarted(&old, NEW, &[(Limit::Sessions, 8)], true).await?;
    assert_eq!(
        started,
        StartLimits {
            memory_gb: None,
            disk_gb: 875
        }
    );
    let v = view(&h, &admin).await?;
    assert_eq!(in_force(&v), defaults([34, 12, 4, 660, 875, 175]));
    assert_eq!(limit(&v, "max_sessions")["setting"]["set_by"], "migration");
    assert_eq!(limit(&v, "sandbox_memory_gb")["restart_needed"], false);
    assert!(
        !h.app.config.data_dir.join(ADAPT_MARK).exists(),
        "the mark is used once"
    );
    let audit = h
        .store
        .audit_details(AUDIT_TARGET, "limits.adapted", 5)
        .await?;
    assert_eq!(audit.len(), 1, "{audit:?}");
    for part in [
        "max_sessions 8 → 34",
        "sandbox_memory_gb 38 GiB → 660 GiB",
        "sandbox_disk_gb 300 GB → 875 GB",
    ] {
        assert!(audit[0].contains(part), "{part} in {audit:?}");
    }

    // The rest of the old drop-ins arrives: already overruled.
    let (h, admin, started) = restarted(&h, NEW, &OLD_PINS, false).await?;
    assert_eq!(
        started,
        StartLimits {
            memory_gb: None,
            disk_gb: 875
        }
    );
    let v = view(&h, &admin).await?;
    assert_eq!(in_force(&v), defaults([34, 12, 4, 660, 875, 175]));
    let disk = limit(&v, "sandbox_disk_gb");
    assert_eq!(disk["unit_value"], 300);
    assert_eq!(disk["stale"], false);
    Ok(())
}

#[tokio::test]
async fn an_import_onto_a_host_of_the_same_size_changes_nothing() -> TestResult {
    let (old, _) = instance(OLD, &OLD_PINS).await?;
    // A few GiB less and a slightly smaller disk: the same size of host.
    let (h, admin, _) = restarted(&old, (12, 45, 360), &OLD_PINS, true).await?;
    let v = view(&h, &admin).await?;
    assert_eq!(limit(&v, "max_sessions")["source"], "unit");
    assert_eq!(limit(&v, "sandbox_memory_gb")["value"], 38);
    assert_eq!(limit(&v, "sandbox_disk_gb")["value"], 300);
    assert_eq!(limit(&v, "sandbox_disk_gb")["stale"], false);
    assert!(!h.app.config.data_dir.join(ADAPT_MARK).exists());
    assert!(
        h.store
            .audit_details(AUDIT_TARGET, "limits.adapted", 5)
            .await?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn a_pin_set_on_this_host_is_not_flagged_for_another_ones_sake() -> TestResult {
    let (old, _) = instance(OLD, &[(Limit::Sessions, 8)]).await?;
    // On the new host a person writes 32 into the unit: a new number, recorded for this host.
    let (h, admin, _) = restarted(&old, NEW, &[(Limit::Sessions, 32)], true).await?;
    let v = view(&h, &admin).await?;
    let sessions = limit(&v, "max_sessions");
    assert_eq!(sessions["value"], 32);
    assert_eq!(sessions["source"], "unit");
    assert_eq!(sessions["stale"], false);
    assert_eq!(sessions["configured_on"]["cpus"], 64);
    Ok(())
}

#[tokio::test]
async fn a_superadmin_pins_a_limit_makes_it_host_sized_or_forgets_the_setting() -> TestResult {
    let (h, admin) = instance(OLD, &[(Limit::CiJobs, 6)]).await?;
    assert_eq!(h.app.ci_max_jobs(), 6);

    let r = put(&h, &admin, "ci_max_jobs", json!(10)).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let jobs = limit(&r.body, "ci_max_jobs");
    assert_eq!(jobs["value"], 10);
    assert_eq!(jobs["source"], "settings");
    assert_eq!(jobs["unit_value"], 6);
    assert_eq!(jobs["setting"]["value"], 10);
    assert_eq!(jobs["setting"]["set_by"], "superadmin");
    assert_eq!(jobs["configured_on"]["cpus"], 12);
    assert_eq!(h.app.ci_max_jobs(), 10, "applied without a restart");

    let r = put(&h, &admin, "ci_max_jobs", Value::Null).await?;
    let jobs = limit(&r.body, "ci_max_jobs");
    assert_eq!(jobs["value"], 4);
    assert_eq!(jobs["source"], "default");
    assert_eq!(h.app.ci_max_jobs(), 4);

    let r = call(
        &h.router,
        Method::DELETE,
        "/api/admin/limits/ci_max_jobs",
        None,
        &admin,
    )
    .await?;
    let jobs = limit(&r.body, "ci_max_jobs");
    assert_eq!(jobs["value"], 6);
    assert_eq!(jobs["source"], "unit");
    assert_eq!(jobs["setting"], Value::Null);
    assert_eq!(h.app.ci_max_jobs(), 6);

    // The other live limits follow at once too.
    put(&h, &admin, "max_sessions", json!(3)).await?;
    put(&h, &admin, "ci_lanes", json!(5)).await?;
    put(&h, &admin, "cd_run_disk_gb", json!(50)).await?;
    assert_eq!(h.app.max_live_sessions(), 3);
    assert_eq!(h.app.ci_lanes(), 5);
    assert_eq!(h.app.run_disk_max(), 50 * GB);

    let set = h
        .store
        .audit_details("ci_max_jobs", "limits.set", 5)
        .await?;
    assert_eq!(set.len(), 2, "{set:?}");
    assert!(set.contains(&"10".to_owned()) && set.contains(&"host-sized".to_owned()));
    let removed = h
        .store
        .audit_details("ci_max_jobs", "limits.removed", 5)
        .await?;
    assert_eq!(removed, ["was host-sized"]);

    // The settings are there after a restart, whatever the unit says then.
    let (h, admin, _) = restarted(&h, OLD, &[(Limit::Sessions, 8)], false).await?;
    let v = view(&h, &admin).await?;
    assert_eq!(limit(&v, "max_sessions")["value"], 3);
    assert_eq!(limit(&v, "max_sessions")["source"], "settings");
    assert_eq!(limit(&v, "ci_lanes")["value"], 5);
    assert_eq!(
        limit(&v, "ci_max_jobs")["value"],
        4,
        "the unit dropped its 6"
    );
    assert_eq!(h.app.max_live_sessions(), 3);
    Ok(())
}

#[tokio::test]
async fn a_number_out_of_range_an_unknown_limit_and_other_people_are_refused() -> TestResult {
    let (h, admin) = instance(OLD, &[]).await?;
    for (key, value, words) in [
        (
            "max_sessions",
            0,
            "Live sessions must be between 1 and 1024",
        ),
        (
            "ci_max_jobs",
            65,
            "CI jobs at once must be between 1 and 64",
        ),
        ("ci_lanes", 9, "between 1 and 8"),
        (
            "sandbox_memory_gb",
            48,
            "Sandbox memory must be between 1 and 47 GiB",
        ),
        ("cd_run_disk_gb", 0, "between 1 and 10000 GB"),
    ] {
        let r = put(&h, &admin, key, json!(value)).await?;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{key}: {}",
            r.body
        );
        assert!(r.body.to_string().contains(words), "{key}: {}", r.body);
    }
    // No quota is a choice for a sandbox's disk.
    let r = put(&h, &admin, "sandbox_disk_gb", json!(0)).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let r = put(&h, &admin, "sandbox_pids", json!(9000)).await?;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let v = view(&h, &admin).await?;
    assert_eq!(
        limit(&v, "max_sessions")["value"],
        8,
        "a refusal changes nothing"
    );

    configure_github_app(&h).await?;
    let alice = admit_and_sign_in(&h, &admin, "alice", 11, true).await?;
    for (method, path, body) in [
        (Method::GET, "/api/admin/limits", None),
        (
            Method::PUT,
            "/api/admin/limits/max_sessions",
            Some(json!({ "value": 99 })),
        ),
        (Method::DELETE, "/api/admin/limits/sandbox_disk_gb", None),
        (Method::POST, "/api/admin/limits/adapt", None),
        (Method::POST, "/api/admin/limits/keep", None),
    ] {
        let r = call(&h.router, method.clone(), path, body, &alice).await?;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{method} {path}");
    }
    assert_eq!(h.app.max_live_sessions(), 8);
    assert_eq!(
        limit(&view(&h, &admin).await?, "sandbox_disk_gb")["value"],
        0
    );
    Ok(())
}

#[tokio::test]
async fn the_sandbox_memory_applies_at_the_next_restart() -> TestResult {
    let (h, admin) = instance(OLD, &[]).await?;
    let r = put(&h, &admin, "sandbox_memory_gb", json!(30)).await?;
    let memory = limit(&r.body, "sandbox_memory_gb");
    assert_eq!(memory["value"], 30);
    assert_eq!(memory["live"], false);
    assert_eq!(memory["restart_needed"], true);
    assert_eq!(limit(&r.body, "max_sessions")["live"], true);
    assert_eq!(limit(&r.body, "max_sessions")["restart_needed"], false);

    let (h, admin, started) = restarted(&h, OLD, &[], false).await?;
    assert_eq!(started.memory_gb, Some(30));
    let v = view(&h, &admin).await?;
    assert_eq!(limit(&v, "sandbox_memory_gb")["restart_needed"], false);
    Ok(())
}

#[tokio::test]
async fn an_instance_that_is_not_set_up_starts_with_host_sized_limits() -> TestResult {
    let h = harness()?;
    let started = start(&h, NEW, &[(Limit::Sessions, 8)]).await;
    assert_eq!(started.disk_gb, 875);
    assert_eq!(h.app.max_live_sessions(), 8);
    assert_eq!(h.app.ci_max_jobs(), 12);
    Ok(())
}

/// HSL-006: more live sessions apply while the server runs: the queued task is offered the new
/// slot at once, without waiting for another run to end.
#[tokio::test]
async fn a_queued_task_is_started_when_the_session_limit_is_raised() -> TestResult {
    use yulius_app::ports::store::TaskStore as _;
    use yulius_domain::task::TaskStatus;
    let (h, admin) = instance(OLD, &[(Limit::Sessions, 1)]).await?;
    support::add_sandbox(&h, "s1", "studio", 20000).await?;
    support::user_with_token(&h, "u-alice", "alice", false).await?;
    support::seed_task(&h, "t-run", "s1", "u-alice", TaskStatus::Running, None).await?;
    support::seed_live_attempt(&h, "t-run", "tok-run").await?;
    let waiting = support::seed_task(&h, "t-wait", "s1", "u-alice", TaskStatus::Queued, None)
        .await?
        .id;
    // The one slot is taken: a pass of the queue starts nothing.
    h.app.drain_queue_now().await;
    let status = call(&h.router, Method::GET, "/api/status", None, &admin).await?;
    assert_eq!(
        status.body["sessions"],
        json!({"live": 1, "limit": 1, "queued": 1})
    );
    assert_eq!(h.store.attempts(&waiting).await?.len(), 0);

    let r = put(&h, &admin, "max_sessions", json!(2)).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let task = h.store.task(&waiting).await?.ok_or("task")?;
        if task.status != TaskStatus::Queued {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "still queued under a limit of 2"
        );
        tokio::task::yield_now().await;
    }
    let status = call(&h.router, Method::GET, "/api/status", None, &admin).await?;
    assert_eq!(status.body["sessions"]["limit"], 2);
    Ok(())
}
