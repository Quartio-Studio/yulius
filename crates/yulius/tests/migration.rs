//! A move asked from Settings → Instance (SPEC-470 MVB-020 to MVB-023), through the API with a
//! fake of the root helper's files: what the superadmin is told, what is written for root, what
//! is refused and what is audited. The real helper: `src/migrate_helper/tests.rs`; the real
//! files: `src/wiring/migrate_files.rs`.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use yulius_app::ports::migration::{MoveHelper, MoveStatus};
use yulius_app::ports::store_ext::AutomationStore as _;
use yulius_contracts::migrate::{
    MoveAction, MoveChecklist, MoveHelperState, MoveHelperStatus, MoveRequest, VERSION,
};
use yulius_contracts::update::{HostLine, HostOutcome};

use crate::support::updates::{FakeHelper, FakeSource, NEW, OLD, services};
use crate::support::{
    Creds, Harness, Resp, TestResult, call, harness, seed_github_user, setup_superadmin_creds,
};

const API: &str = "/api/admin/instance/migration";
const TARGET: &str = "root@new.example";

/// The root helper's files as the server sees them, and the helper as a test plays it.
#[derive(Default)]
struct FakeMoveHelper {
    installed: AtomicBool,
    pending: Mutex<Option<MoveRequest>>,
    written: Mutex<Vec<MoveRequest>>,
    cleared: AtomicUsize,
    unwritable: AtomicBool,
    /// A write takes a while, as on a slow disk: another ask has time to come.
    slow: AtomicBool,
    /// A write waits until a test lets it go.
    held: AtomicBool,
    helper: Mutex<Option<MoveHelperStatus>>,
    status: Mutex<Option<MoveStatus>>,
    arrived: Mutex<Option<MoveChecklist>>,
}

impl MoveHelper for FakeMoveHelper {
    fn installed(&self) -> bool {
        self.installed.load(Ordering::SeqCst)
    }

    fn request(&self, request: &MoveRequest) -> Result<(), String> {
        if self.unwritable.load(Ordering::SeqCst) {
            return Err("Read-only file system (os error 30)".into());
        }
        if self.slow.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let waited = std::time::Instant::now();
        while self.held.load(Ordering::SeqCst) && waited.elapsed().as_secs() < 10 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if let (Ok(mut pending), Ok(mut written)) = (self.pending.lock(), self.written.lock()) {
            *pending = Some(request.clone());
            written.push(request.clone());
        }
        Ok(())
    }

    fn request_again(&self, id: &str) {
        if let (Ok(pending), Ok(mut written)) = (self.pending.lock(), self.written.lock())
            && let Some(request) = pending.as_ref().filter(|request| request.id == id)
        {
            written.push(request.clone());
        }
    }

    fn pending(&self) -> Option<MoveRequest> {
        self.pending.lock().ok()?.clone()
    }

    fn clear_request(&self, id: &str) {
        if let Ok(mut pending) = self.pending.lock()
            && pending.as_ref().is_some_and(|request| request.id == id)
        {
            *pending = None;
            self.cleared.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn helper_status(&self) -> Option<MoveHelperStatus> {
        self.helper.lock().ok()?.clone()
    }

    fn status(&self) -> Option<MoveStatus> {
        self.status.lock().ok()?.clone()
    }

    fn arrived(&self) -> Option<MoveChecklist> {
        self.arrived.lock().ok()?.clone()
    }
}

impl FakeMoveHelper {
    fn written(&self) -> Vec<MoveRequest> {
        self.written.lock().map(|w| w.clone()).unwrap_or_default()
    }

    fn says(&self, change: impl FnOnce(&mut MoveHelperStatus)) {
        if let Ok(mut helper) = self.helper.lock()
            && let Some(status) = helper.as_mut()
        {
            change(status);
        }
    }

    /// The root helper takes the pending request and runs its command.
    fn takes(&self, now: i64) -> Result<MoveRequest, Box<dyn std::error::Error>> {
        let request = self.pending().ok_or("no request waits")?;
        if let Ok(mut helper) = self.helper.lock() {
            *helper = Some(MoveHelperStatus {
                version: VERSION,
                id: request.id.clone(),
                action: request.action,
                values: request.values(),
                state: MoveHelperState::Running,
                exit_code: None,
                error: None,
                started_at: now,
                updated_at: now,
                ended_at: None,
            });
        }
        // The checklist of an earlier run stays where it is: the helper does not touch it, and
        // the command replaces it once it has accepted the run.
        Ok(request)
    }

    /// The command ended.
    fn ends(&self, exit_code: Option<i32>, now: i64) {
        self.says(|status| {
            status.state = MoveHelperState::Ended;
            status.exit_code = exit_code;
            status.updated_at = now;
            status.ended_at = Some(now);
        });
    }

    /// The command's checklist.
    fn checklist(&self, phase: &str, lines: &[(&str, HostOutcome)], key: Option<&str>, at_ms: i64) {
        if let Ok(mut status) = self.status.lock() {
            *status = Some(MoveStatus {
                id: "ab12cd34".into(),
                target: TARGET.into(),
                phase: phase.into(),
                lines: lines
                    .iter()
                    .map(|(name, outcome)| line(name, *outcome))
                    .collect(),
                updated_at: u64::try_from(at_ms / 1000).unwrap_or(0),
                public_key: key.map(str::to_owned),
                new_url: Some("https://new.example".into()),
            });
        }
    }
}

fn line(name: &str, outcome: HostOutcome) -> HostLine {
    HostLine {
        name: name.into(),
        outcome,
        detail: format!("what holds of {name}"),
    }
}

struct Instance {
    h: Harness,
    admin: Creds,
    helper: Arc<FakeMoveHelper>,
}

async fn instance() -> Result<Instance, Box<dyn std::error::Error>> {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    let helper = Arc::new(FakeMoveHelper::default());
    helper.installed.store(true, Ordering::SeqCst);
    assert!(h.app.set_move_helper(helper.clone()));
    Ok(Instance { h, admin, helper })
}

impl Instance {
    fn now(&self) -> i64 {
        self.h.clock.0.load(Ordering::SeqCst)
    }

    fn later(&self, ms: i64) {
        self.h.clock.0.fetch_add(ms, Ordering::SeqCst);
    }

    async fn view(&self) -> Result<Value, Box<dyn std::error::Error>> {
        let view = call(&self.h.router, Method::GET, API, None, &self.admin).await?;
        assert_eq!(view.status, StatusCode::OK, "{}", view.body);
        Ok(view.body)
    }

    async fn ask(&self, body: Value) -> Result<Resp, Box<dyn std::error::Error>> {
        call(&self.h.router, Method::POST, API, Some(body), &self.admin).await
    }

    /// Asks and expects the refusal `status` whose message holds `why`.
    async fn refused(&self, body: Value, status: StatusCode, why: &str) -> TestResult {
        let before = self.helper.written().len();
        let answer = self.ask(body.clone()).await?;
        assert_eq!(answer.status, status, "{body}: {}", answer.body);
        let message = answer.body["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains(why), "{body}: {message}");
        assert_eq!(
            self.helper.written().len(),
            before,
            "{body}: something was written"
        );
        Ok(())
    }

    /// A plan of [`plan`] that the helper ran to its end, with this checklist.
    async fn planned(&self, lines: &[(&str, HostOutcome)], key: Option<&str>) -> TestResult {
        let asked = self.ask(plan()).await?;
        assert_eq!(asked.status, StatusCode::ACCEPTED, "{}", asked.body);
        self.helper.takes(self.now())?;
        self.helper.checklist("Plan", lines, key, self.now());
        self.helper.ends(Some(0), self.now());
        Ok(())
    }

    async fn audited(
        &self,
        target: &str,
        action: &str,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        Ok(self
            .h
            .store
            .automation_audit(target, 20)
            .await?
            .into_iter()
            .filter(|entry| entry.action == action)
            .map(|entry| format!("{}: {}", entry.actor, entry.detail.unwrap_or_default()))
            .collect())
    }
}

fn plan() -> Value {
    json!({
        "action": "plan", "target": TARGET, "ssh_port": 2222,
        "public_url": "https://new.example", "data_disks": ["/dev/vdc", "/dev/vdd"],
    })
}

fn moving() -> Value {
    let mut body = plan();
    body["action"] = "move".into();
    body["confirm"] = "new.example".into();
    body
}

const GOOD: [(&str, HostOutcome); 3] = [
    ("old host", HostOutcome::Ok),
    ("public URL", HostOutcome::Manual),
    ("downtime", HostOutcome::Ok),
];

/// MVB-020, MVB-021: only the superadmin sees a move or asks for one, and a write needs the
/// session's CSRF token.
#[tokio::test]
async fn only_the_superadmin_sees_and_asks_for_a_move() -> TestResult {
    let i = instance().await?;
    let member = seed_github_user(&i.h, "u-alice", "alice").await?;
    for (who, expected) in [
        (&member, StatusCode::FORBIDDEN),
        (&Creds::default(), StatusCode::UNAUTHORIZED),
    ] {
        let seen = call(&i.h.router, Method::GET, API, None, who).await?;
        assert_eq!(seen.status, expected);
        assert_eq!(seen.body["move"], Value::Null);
        for body in [plan(), moving(), json!({"action": "abort"})] {
            let asked = call(&i.h.router, Method::POST, API, Some(body), who).await?;
            assert_eq!(asked.status, expected, "{}", asked.body);
        }
    }
    // The superadmin's own session without the token: another site cannot ask.
    let forged = Creds {
        csrf: None,
        ..i.admin.clone()
    };
    let asked = call(&i.h.router, Method::POST, API, Some(plan()), &forged).await?;
    assert_eq!(asked.status, StatusCode::FORBIDDEN);
    assert_eq!(asked.body["error"]["code"], "csrf_invalid");
    assert!(i.helper.written().is_empty());
    assert_eq!(
        i.audited(TARGET, "migration.planned").await?,
        Vec::<String>::new()
    );
    Ok(())
}

/// MVB-020 to MVB-022: a plan is written once for root with the values the person gave, is
/// audited, and its state is told from the helper's files.
#[tokio::test]
async fn a_plan_is_written_for_root_audited_and_followed() -> TestResult {
    let i = instance().await?;
    let idle = i.view().await?;
    assert_eq!(idle["public_url"], "https://yulius.test");
    assert_eq!(idle["helper_installed"], true);
    assert_eq!(idle["update_running"], false);
    for nothing in ["request", "run", "move", "arrived"] {
        assert_eq!(idle[nothing], Value::Null, "{nothing}");
    }
    assert_eq!(
        idle["can"],
        json!({"plan": true, "move": false, "abort": false})
    );

    // As typed: spaces around the host, a slash after the URL, no port (22).
    let asked = i
        .ask(json!({
            "action": "plan", "target": "  root@new.example ", "public_url": "https://new.example/",
            "data_disks": [" /dev/vdc ", ""],
        }))
        .await?;
    assert_eq!(asked.status, StatusCode::ACCEPTED, "{}", asked.body);
    let written = i.helper.written();
    assert_eq!(written.len(), 1);
    let request = &written[0];
    assert_eq!(request.version, VERSION);
    assert_eq!(request.action, MoveAction::Plan);
    assert_eq!(request.target.as_deref(), Some(TARGET));
    assert_eq!(request.ssh_port, Some(22));
    assert_eq!(request.public_url.as_deref(), Some("https://new.example"));
    assert_eq!(request.data_disks, ["/dev/vdc"]);
    assert_eq!(request.requested_at, i.now());
    assert!(
        yulius_app::migration::rules::valid_id(&request.id),
        "{}",
        request.id
    );
    assert_eq!(
        asked.body["request"],
        json!({"id": request.id, "action": "plan", "state": "requested",
               "requested_at": asked.body["request"]["requested_at"]})
    );
    assert_eq!(
        asked.body["can"],
        json!({"plan": false, "move": false, "abort": false})
    );
    let audit = i.audited(TARGET, "migration.planned").await?;
    assert_eq!(audit.len(), 1);
    assert!(audit[0].starts_with("superadmin: "), "{audit:?}");
    for said in [
        request.id.as_str(),
        "port 22",
        "https://new.example",
        "/dev/vdc",
    ] {
        assert!(audit[0].contains(said), "{said}: {audit:?}");
    }

    // One at a time while it waits.
    i.refused(
        plan(),
        StatusCode::CONFLICT,
        "is waiting for the move helper",
    )
    .await?;

    // The helper took it: the request file goes, the run is told, and it can be stopped.
    let taken = i.helper.takes(i.now())?;
    let running = i.view().await?;
    assert_eq!(running["request"], Value::Null);
    assert_eq!(i.helper.cleared.load(Ordering::SeqCst), 1);
    assert!(i.helper.pending().is_none());
    assert_eq!(running["run"]["id"], taken.id.as_str());
    assert_eq!(running["run"]["state"], "running");
    assert_eq!(running["run"]["action"], "plan");
    assert_eq!(running["run"]["target"], TARGET);
    assert_eq!(running["run"]["ssh_port"], 22);
    assert_eq!(running["run"]["data_disks"], json!(["/dev/vdc"]));
    assert_eq!(
        running["can"],
        json!({"plan": false, "move": false, "abort": true})
    );
    i.refused(plan(), StatusCode::CONFLICT, "is running")
        .await?;

    // Its checklist as it grows, then how it ended.
    i.helper.checklist("Plan", &GOOD[..2], None, i.now());
    let growing = i.view().await?;
    assert_eq!(growing["move"]["phase"], "Plan");
    assert_eq!(growing["move"]["target"], TARGET);
    assert_eq!(growing["move"]["by_command"], false);
    assert_eq!(growing["move"]["new_url"], "https://new.example");
    assert_eq!(
        growing["move"]["lines"],
        json!([
            {"name": "old host", "outcome": "ok", "detail": "what holds of old host"},
            {"name": "public URL", "outcome": "manual", "detail": "what holds of public URL"},
        ])
    );
    i.later(4_000);
    i.helper.checklist("Plan", &GOOD, None, i.now());
    i.helper.ends(Some(0), i.now());
    let ended = i.view().await?;
    assert_eq!(ended["run"]["state"], "ended");
    assert_eq!(ended["run"]["exit_code"], 0);
    assert_eq!(ended["move"]["lines"].as_array().map(Vec::len), Some(3));
    assert_eq!(
        ended["can"],
        json!({"plan": true, "move": true, "abort": true})
    );
    Ok(())
}

/// MVB-021: a move is the plan the person saw: the same values, shown to its end without a
/// failure and without a key still to add, at most an hour ago, and the host typed.
#[tokio::test]
async fn a_move_needs_the_plan_that_was_shown_and_the_typed_host() -> TestResult {
    const PLAN_FIRST: &str = "show the plan";
    let i = instance().await?;
    i.refused(moving(), StatusCode::CONFLICT, PLAN_FIRST)
        .await?;

    // A plan that stopped at the key: the key first.
    i.planned(
        &[("one-time key", HostOutcome::Manual)],
        Some("ssh-ed25519 AAAA yulius-migrate-ab12cd34"),
    )
    .await?;
    let view = i.view().await?;
    assert_eq!(
        view["move"]["public_key"],
        "ssh-ed25519 AAAA yulius-migrate-ab12cd34"
    );
    assert_eq!(view["can"]["move"], false);
    i.refused(moving(), StatusCode::CONFLICT, "add the key")
        .await?;

    // A plan with a failed line.
    i.later(1_000);
    i.planned(
        &[
            ("old host", HostOutcome::Ok),
            ("disk space", HostOutcome::Failed),
        ],
        None,
    )
    .await?;
    i.helper.says(|status| status.exit_code = Some(1));
    assert_eq!(i.view().await?["can"]["move"], false);
    i.refused(moving(), StatusCode::CONFLICT, "found problems")
        .await?;
    // A helper that said 0 over a failed line is not believed either.
    i.helper.says(|status| status.exit_code = Some(0));
    i.refused(moving(), StatusCode::CONFLICT, "found problems")
        .await?;

    // A plan that was stopped before its end: its lines are fine as far as they go.
    i.later(1_000);
    i.planned(&GOOD[..2], None).await?;
    for stopped in [None, Some(2)] {
        i.helper.says(|status| status.exit_code = stopped);
        assert_eq!(i.view().await?["can"]["move"], false, "{stopped:?}");
        i.refused(moving(), StatusCode::CONFLICT, PLAN_FIRST)
            .await?;
    }

    // A good plan.
    i.later(1_000);
    i.planned(&GOOD, None).await?;
    assert_eq!(i.view().await?["can"]["move"], true);
    // Other values than the plan's: each one.
    let changes: [(&str, Value, &str); 4] = [
        ("target", "root@other.example".into(), "other.example"),
        ("ssh_port", 22.into(), "new.example"),
        ("public_url", Value::Null, "new.example"),
        ("data_disks", json!(["/dev/vdc"]), "new.example"),
    ];
    for (field, value, typed) in changes {
        let mut other = moving();
        other[field] = value;
        other["confirm"] = typed.into();
        i.refused(other, StatusCode::CONFLICT, PLAN_FIRST).await?;
    }
    // The host is typed, exactly.
    for typed in [
        Value::Null,
        "".into(),
        "root@new.example".into(),
        "NEW.EXAMPLE".into(),
        "new.exampl".into(),
    ] {
        let mut untyped = moving();
        untyped["confirm"] = typed;
        i.refused(
            untyped,
            StatusCode::UNPROCESSABLE_ENTITY,
            "type the new host's name",
        )
        .await?;
    }
    // A plan from more than an hour ago is shown again first.
    i.later(3_600_001);
    assert_eq!(i.view().await?["can"]["move"], false);
    i.refused(moving(), StatusCode::CONFLICT, "more than an hour ago")
        .await?;
    i.planned(&GOOD, None).await?;
    i.later(3_600_000);

    let asked = i.ask(moving()).await?;
    assert_eq!(asked.status, StatusCode::ACCEPTED, "{}", asked.body);
    let request = i.helper.written().pop().ok_or("nothing written")?;
    assert_eq!(request.action, MoveAction::Move);
    assert_eq!(request.target.as_deref(), Some(TARGET));
    assert_eq!(request.ssh_port, Some(2222));
    assert_eq!(request.data_disks, ["/dev/vdc", "/dev/vdd"]);
    let audit = i.audited(TARGET, "migration.started").await?;
    assert_eq!(audit.len(), 1);
    assert!(audit[0].starts_with("superadmin: ") && audit[0].contains(&request.id));

    // A move that stopped is continued with the same values, without another plan; a move that
    // ended well is not asked for again.
    i.helper.takes(i.now())?;
    i.helper.checklist(
        "Prepare",
        &[("packages", HostOutcome::Failed)],
        None,
        i.now(),
    );
    i.helper.ends(Some(1), i.now());
    i.later(7_200_000);
    assert_eq!(i.view().await?["can"]["move"], true);
    let mut other = moving();
    other["ssh_port"] = 22.into();
    i.refused(other, StatusCode::CONFLICT, PLAN_FIRST).await?;
    assert_eq!(i.ask(moving()).await?.status, StatusCode::ACCEPTED);
    i.helper.takes(i.now())?;
    i.helper.checklist("done", &GOOD, None, i.now());
    i.helper.ends(Some(0), i.now());
    assert_eq!(
        i.view().await?["can"],
        json!({"plan": true, "move": false, "abort": false})
    );
    i.refused(moving(), StatusCode::CONFLICT, PLAN_FIRST)
        .await?;
    Ok(())
}

/// MVB-021 with MVB-002: what a request may name is checked before anything is written.
#[tokio::test]
async fn a_field_outside_the_rules_is_refused_before_anything_is_written() -> TestResult {
    let i = instance().await?;
    let cases: [(&str, Value, &str); 9] = [
        ("target", Value::Null, "the new host is missing"),
        ("target", "".into(), "the new host is missing"),
        (
            "target",
            "-oProxyCommand=id".into(),
            "`root@host` or `host`",
        ),
        ("target", "admin@new.example".into(), "reached as root"),
        ("ssh_port", 0.into(), "the SSH port"),
        ("public_url", "http://new.example".into(), "the public URL"),
        (
            "public_url",
            "https://new.example/x".into(),
            "the public URL",
        ),
        ("data_disks", json!(["/etc/passwd"]), "a data disk"),
        (
            "data_disks",
            json!(["/dev/a", "/dev/b", "/dev/c"]),
            "one, or two for a mirror",
        ),
    ];
    for (field, value, why) in cases {
        let mut body = plan();
        body[field] = value;
        i.refused(body, StatusCode::UNPROCESSABLE_ENTITY, why)
            .await?;
    }
    // An abort names nothing: it is of the recorded move.
    i.refused(
        json!({"action": "abort", "target": TARGET}),
        StatusCode::UNPROCESSABLE_ENTITY,
        "an abort names no host",
    )
    .await?;
    // No such action, a port that is no port: not a request at all.
    for body in [
        json!({"action": "install", "target": TARGET}),
        json!({"action": "plan", "target": TARGET, "ssh_port": 70000}),
    ] {
        let answer = i.ask(body).await?;
        assert!(answer.status.is_client_error(), "{}", answer.status);
    }
    assert!(i.helper.written().is_empty());
    Ok(())
}

/// MVB-021, MVB-022: an abort needs a recorded move; it may arrive while a run is in flight,
/// which it stops, and it is audited with the move it is of.
#[tokio::test]
async fn an_abort_is_of_the_recorded_move_and_may_stop_a_run() -> TestResult {
    const NOTHING: &str = "nothing to abort";
    let abort = || json!({"action": "abort"});
    let i = instance().await?;
    i.refused(abort(), StatusCode::CONFLICT, NOTHING).await?;

    // A plan in flight: the abort is written beside it.
    assert_eq!(i.ask(plan()).await?.status, StatusCode::ACCEPTED);
    i.refused(
        abort(),
        StatusCode::CONFLICT,
        "is waiting for the move helper",
    )
    .await?;
    let running = i.helper.takes(i.now())?;
    let asked = i.ask(abort()).await?;
    assert_eq!(asked.status, StatusCode::ACCEPTED, "{}", asked.body);
    let request = i.helper.written().pop().ok_or("nothing written")?;
    assert_eq!(request.action, MoveAction::Abort);
    assert_eq!(
        request.values(),
        Default::default(),
        "an abort names nothing"
    );
    assert_eq!(asked.body["request"]["action"], "abort");
    assert_eq!(asked.body["request"]["state"], "requested");
    // The run it stops is still the helper's, and its request stays for the helper to find.
    assert_eq!(asked.body["run"]["id"], running.id.as_str());
    assert_eq!(asked.body["run"]["state"], "running");
    assert!(i.helper.pending().is_some());
    assert_eq!(
        asked.body["can"],
        json!({"plan": false, "move": false, "abort": false})
    );
    let audit = i.audited(TARGET, "migration.aborted").await?;
    assert_eq!(audit.len(), 1, "audited with the host of the run it stops");
    assert!(audit[0].starts_with("superadmin: ") && audit[0].contains(&request.id));
    i.refused(
        abort(),
        StatusCode::CONFLICT,
        "is waiting for the move helper",
    )
    .await?;

    // The helper stopped the plan and runs the abort: nothing else beside it.
    i.helper.takes(i.now())?;
    i.helper
        .checklist("Abort", &[("old URL", HostOutcome::Changed)], None, i.now());
    let aborting = i.view().await?;
    assert_eq!(aborting["run"]["action"], "abort");
    assert_eq!(aborting["run"]["state"], "running");
    assert_eq!(
        aborting["can"],
        json!({"plan": false, "move": false, "abort": false})
    );
    i.refused(abort(), StatusCode::CONFLICT, "an abort is running")
        .await?;
    i.refused(plan(), StatusCode::CONFLICT, "an abort is running")
        .await?;
    i.helper.checklist(
        "aborted",
        &[("old instance", HostOutcome::Changed)],
        None,
        i.now(),
    );
    i.helper.ends(Some(0), i.now());
    let done = i.view().await?;
    assert_eq!(done["run"]["action"], "abort");
    assert_eq!(done["move"]["phase"], "aborted");
    assert_eq!(
        done["can"],
        json!({"plan": true, "move": false, "abort": false})
    );
    i.refused(abort(), StatusCode::CONFLICT, NOTHING).await?;

    // A recorded move the command refused to go on with (another host is recorded) wrote no
    // checklist: the one that was there stays, as an earlier run's, and the abort is what the
    // command names, so it is offered.
    i.later(2_000);
    assert_eq!(i.ask(plan()).await?.status, StatusCode::ACCEPTED);
    i.helper.takes(i.now())?;
    i.helper.ends(Some(2), i.now());
    i.helper
        .says(|status| status.error = Some("a move to root@old.example is recorded here".into()));
    let refused = i.view().await?;
    assert_eq!(refused["move"]["phase"], "aborted");
    assert_eq!(refused["move"]["earlier"], true);
    assert_eq!(
        refused["run"]["error"],
        "a move to root@old.example is recorded here"
    );
    assert_eq!(refused["can"]["abort"], true);
    assert_eq!(i.ask(abort()).await?.status, StatusCode::ACCEPTED);
    Ok(())
}

/// MVB-021, MVB-023: nothing is asked of a move while an update or restart runs, and neither
/// starts while a move's request waits or its run is in flight.
#[tokio::test]
async fn updates_and_moves_refuse_each_other() -> TestResult {
    let i = instance().await?;
    crate::support::configure_github_app(&i.h).await?;
    let source = FakeSource::behind();
    let updates = FakeHelper::installed();
    assert!(i.h.app.set_updates(services(OLD, &source, &updates)));
    let post = |path: &'static str, body: Value| {
        call(&i.h.router, Method::POST, path, Some(body), &i.admin)
    };
    assert_eq!(
        post("/api/admin/update/check", json!({})).await?.status,
        StatusCode::OK
    );

    // A move's request waits, then its run is in flight: no update, no restart.
    assert_eq!(i.ask(plan()).await?.status, StatusCode::ACCEPTED);
    for _ in 0..2 {
        for (path, body) in [
            ("/api/admin/update", json!({"commit": NEW, "confirm": true})),
            ("/api/admin/restart", json!({"confirm": true})),
        ] {
            let refused = post(path, body).await?;
            assert_eq!(
                refused.status,
                StatusCode::CONFLICT,
                "{path}: {}",
                refused.body
            );
            let message = refused.body["error"]["message"]
                .as_str()
                .unwrap_or_default();
            assert!(
                message.contains("is being moved to another host"),
                "{message}"
            );
        }
        if i.helper.pending().is_some() {
            i.helper.takes(i.now())?;
        }
    }
    assert!(updates.last().is_none(), "the update helper was asked");
    i.helper.checklist("Plan", &GOOD, None, i.now());
    i.helper.ends(Some(0), i.now());

    // The move's run ended: an update starts, and now the move waits.
    let started = post("/api/admin/update", json!({"commit": NEW, "confirm": true})).await?;
    assert_eq!(started.status, StatusCode::ACCEPTED, "{}", started.body);
    let view = i.view().await?;
    assert_eq!(view["update_running"], true);
    assert_eq!(
        view["can"],
        json!({"plan": false, "move": false, "abort": false})
    );
    for body in [plan(), moving(), json!({"action": "abort"})] {
        i.refused(
            body,
            StatusCode::CONFLICT,
            "an update or restart is running",
        )
        .await?;
    }
    Ok(())
}

/// MVB-020: a request nobody takes is said after 20 s and can be asked again; a helper that
/// stopped reporting is lost after 60 s; a request too old to be taken is removed.
#[tokio::test]
async fn an_unanswered_request_and_a_silent_helper_are_said() -> TestResult {
    let i = instance().await?;
    assert_eq!(i.ask(plan()).await?.status, StatusCode::ACCEPTED);
    // The path unit fires on a change of the file, and a change that comes while the helper is
    // ending another run is lost (seen on a real host): after 5 s the same request is written
    // again whenever it is read, until its answer is no longer awaited.
    i.later(5_000);
    assert_eq!(i.view().await?["request"]["state"], "requested");
    assert_eq!(i.helper.written().len(), 1, "not before 5 s");
    i.later(1);
    assert_eq!(i.view().await?["request"]["state"], "requested");
    let written = i.helper.written();
    assert_eq!(written.len(), 2);
    assert_eq!(
        written[0], written[1],
        "the same request: its id and its time"
    );
    i.later(14_999);
    assert_eq!(i.view().await?["request"]["state"], "requested");
    assert_eq!(i.helper.written().len(), 3);
    i.later(1);
    let waiting = i.view().await?;
    assert_eq!(waiting["request"]["state"], "unanswered");
    assert_eq!(
        waiting["can"],
        json!({"plan": true, "move": false, "abort": false})
    );
    assert_eq!(
        i.helper.written().len(),
        3,
        "not once it is said unanswered"
    );
    // Asked again: a new request replaces the old one.
    assert_eq!(i.ask(plan()).await?.status, StatusCode::ACCEPTED);
    let written = i.helper.written();
    assert_eq!(written.len(), 4);
    assert_ne!(written[2].id, written[3].id);
    assert_eq!(i.view().await?["request"]["id"], written[3].id.as_str());
    // A request the helper took is not written again, however long its run takes.
    i.helper.takes(i.now())?;
    i.later(6_000);
    assert_eq!(i.view().await?["run"]["state"], "running");
    assert_eq!(i.helper.written().len(), 4);
    i.helper.ends(Some(0), i.now());
    assert_eq!(i.ask(plan()).await?.status, StatusCode::ACCEPTED);

    // Never taken: after 15 minutes the helper would refuse it as stale, so it goes.
    let cleared = i.helper.cleared.load(Ordering::SeqCst);
    i.later(15 * 60_000);
    assert_eq!(i.view().await?["request"]["state"], "unanswered");
    assert_eq!(i.helper.cleared.load(Ordering::SeqCst), cleared);
    i.later(1);
    assert_eq!(i.view().await?["request"], Value::Null);
    assert_eq!(i.helper.cleared.load(Ordering::SeqCst), cleared + 1);

    // A run whose helper stops reporting (killed, the host restarted): before its first line
    // there is no checklist, and what it began can still be aborted.
    assert_eq!(i.ask(plan()).await?.status, StatusCode::ACCEPTED);
    i.helper.takes(i.now())?;
    i.later(60_001);
    let silent = i.view().await?;
    assert_eq!(silent["move"], Value::Null);
    assert_eq!(silent["run"]["state"], "lost");
    assert_eq!(
        silent["can"],
        json!({"plan": true, "move": false, "abort": true})
    );
    i.helper.says(|status| status.updated_at += 60_001);
    i.helper.checklist("Plan", &GOOD[..1], None, i.now());
    i.later(60_000);
    assert_eq!(i.view().await?["run"]["state"], "running");
    i.later(1);
    let lost = i.view().await?;
    assert_eq!(lost["run"]["state"], "lost");
    assert_eq!(
        lost["can"],
        json!({"plan": true, "move": false, "abort": true})
    );
    // A heartbeat brings it back.
    i.helper.says(|status| status.updated_at += 30_000);
    assert_eq!(i.view().await?["run"]["state"], "running");
    Ok(())
}

/// MVB-020: a run the command refuses changes nothing, the recorded move's checklist
/// included. It stays, said to be an earlier run's and never read as this run's, and what is
/// recorded can still be aborted.
#[tokio::test]
async fn a_refused_run_keeps_the_recorded_moves_checklist() -> TestResult {
    let i = instance().await?;
    i.planned(&GOOD, None).await?;
    i.later(1_000);
    assert_eq!(i.ask(moving()).await?.status, StatusCode::ACCEPTED);
    i.helper.takes(i.now())?;
    // Before its first line, the plan's checklist is not the move's.
    let begun = i.view().await?;
    assert_eq!(begun["run"]["action"], "move");
    assert_eq!(begun["move"], Value::Null);
    i.helper.checklist(
        "Copy",
        &[("old host", HostOutcome::Ok), ("copy", HostOutcome::Failed)],
        None,
        i.now(),
    );
    i.helper.ends(Some(1), i.now());
    let stopped = i.view().await?;
    assert_eq!(stopped["move"]["earlier"], false);
    assert_eq!(
        stopped["can"],
        json!({"plan": true, "move": true, "abort": true})
    );

    // A plan for another host: the command refuses it, since a move is recorded here.
    i.later(5_000);
    let mut other = plan();
    other["target"] = "root@other.example".into();
    assert_eq!(i.ask(other.clone()).await?.status, StatusCode::ACCEPTED);
    i.helper.takes(i.now())?;
    let running = i.view().await?;
    assert_eq!(running["run"]["state"], "running");
    assert_eq!(
        running["move"],
        Value::Null,
        "the failed move's lines as the plan's"
    );
    i.helper.says(|status| {
        status.error = Some(
            "a move to root@new.example is recorded here: finish it with the same command".into(),
        );
    });
    i.helper.ends(Some(2), i.now());
    let refused = i.view().await?;
    assert_eq!(refused["run"]["exit_code"], 2);
    assert_eq!(refused["move"]["phase"], "Copy");
    assert_eq!(refused["move"]["earlier"], true);
    assert_eq!(refused["move"]["by_command"], false);
    assert_eq!(refused["move"]["lines"].as_array().map(Vec::len), Some(2));
    // The lines are not this plan's: no move of the other host, and the recorded one can go.
    assert_eq!(
        refused["can"],
        json!({"plan": true, "move": false, "abort": true})
    );
    other["action"] = "move".into();
    other["confirm"] = "other.example".into();
    i.refused(
        other,
        StatusCode::CONFLICT,
        "show the plan for these values first",
    )
    .await?;

    // A helper that stopped without a word before its first line shows no earlier checklist.
    i.later(5_000);
    assert_eq!(i.ask(plan()).await?.status, StatusCode::ACCEPTED);
    i.helper.takes(i.now())?;
    i.later(60_001);
    let silent = i.view().await?;
    assert_eq!(silent["run"]["state"], "lost");
    assert_eq!(silent["move"], Value::Null);
    Ok(())
}

/// MVB-021: an abort the command refused because nothing is recorded leaves nothing to abort;
/// one that did not complete can be asked again.
#[tokio::test]
async fn an_abort_that_found_nothing_is_not_offered_again() -> TestResult {
    let i = instance().await?;
    // A plan that a signal ended before its first line: what it began may be recorded.
    assert_eq!(i.ask(plan()).await?.status, StatusCode::ACCEPTED);
    i.helper.takes(i.now())?;
    i.helper.ends(None, i.now());
    assert_eq!(i.view().await?["can"]["abort"], true);
    assert_eq!(
        i.ask(json!({"action": "abort"})).await?.status,
        StatusCode::ACCEPTED
    );
    i.helper.takes(i.now())?;
    i.helper
        .says(|status| status.error = Some("no move is recorded on this host".into()));
    i.helper.ends(Some(2), i.now());
    let nothing = i.view().await?;
    assert_eq!(nothing["run"]["action"], "abort");
    assert_eq!(
        nothing["can"],
        json!({"plan": true, "move": false, "abort": false})
    );
    i.refused(
        json!({"action": "abort"}),
        StatusCode::CONFLICT,
        "nothing to abort",
    )
    .await?;

    // An abort that did not complete says so in its checklist, and is asked again.
    assert_eq!(i.ask(plan()).await?.status, StatusCode::ACCEPTED);
    i.helper.takes(i.now())?;
    i.helper.checklist("Plan", &GOOD, None, i.now());
    i.helper.ends(Some(0), i.now());
    assert_eq!(
        i.ask(json!({"action": "abort"})).await?.status,
        StatusCode::ACCEPTED
    );
    i.helper.takes(i.now())?;
    i.helper.checklist(
        "Abort",
        &[("new instance", HostOutcome::Failed)],
        None,
        i.now(),
    );
    i.helper.ends(Some(1), i.now());
    assert_eq!(i.view().await?["can"]["abort"], true);
    Ok(())
}

/// MVB-021: of two asks at once (two tabs, two superadmins) one is written; the other is told
/// that a request waits, however long the write takes.
#[tokio::test]
async fn of_two_asks_at_once_one_is_written() -> TestResult {
    let i = instance().await?;
    i.helper.slow.store(true, Ordering::SeqCst);
    let (first, second) = tokio::join!(i.ask(plan()), i.ask(plan()));
    let (first, second) = (first?, second?);
    let mut answers = [first.status, second.status];
    answers.sort();
    assert_eq!(
        answers,
        [StatusCode::ACCEPTED, StatusCode::CONFLICT],
        "{} / {}",
        first.body,
        second.body
    );
    assert_eq!(i.helper.written().len(), 1);
    assert_eq!(i.audited(TARGET, "migration.planned").await?.len(), 1);
    Ok(())
}

/// MVB-020: on the host a move arrived at, its checklist; and a checklist written after the
/// helper's last run is the command line's.
#[tokio::test]
async fn the_view_tells_what_arrived_and_what_was_run_on_the_host() -> TestResult {
    let i = instance().await?;
    if let Ok(mut arrived) = i.helper.arrived.lock() {
        *arrived = Some(MoveChecklist {
            version: VERSION,
            id: "ab12cd34".into(),
            target: "root@185.189.47.160".into(),
            from: Some("https://old.example".into()),
            to: Some("https://yulius.test".into()),
            lines: vec![
                line("copy proven", HostOutcome::Ok),
                line("old host", HostOutcome::Manual),
            ],
            finished_at: 1_794_000_000,
        });
    }
    let view = i.view().await?;
    assert_eq!(
        view["arrived"],
        json!({
            "id": "ab12cd34", "target": "root@185.189.47.160", "from": "https://old.example",
            "to": "https://yulius.test", "finished_at": "2026-11-06T21:20:00Z",
            "lines": [
                {"name": "copy proven", "outcome": "ok", "detail": "what holds of copy proven"},
                {"name": "old host", "outcome": "manual", "detail": "what holds of old host"},
            ],
        })
    );
    // Nothing moves from here yet.
    assert_eq!(
        view["can"],
        json!({"plan": true, "move": false, "abort": false})
    );

    // A move run on the host, which the helper knows nothing of.
    i.helper.checklist("Prepare", &GOOD, None, i.now());
    let view = i.view().await?;
    assert_eq!(view["run"], Value::Null);
    assert_eq!(view["move"]["by_command"], true);
    assert_eq!(
        view["can"]["abort"], true,
        "a recorded move can be aborted from the page"
    );
    assert_eq!(view["can"]["move"], false);

    // After a plan from the page the checklist is the page's; one written later is not.
    i.planned(&GOOD, None).await?;
    assert_eq!(i.view().await?["move"]["by_command"], false);
    i.later(5_000);
    i.helper.checklist("Cutover", &GOOD, None, i.now());
    let view = i.view().await?;
    assert_eq!(view["move"]["by_command"], true);
    assert_eq!(
        view["can"]["move"], false,
        "the plan shown is no longer the last word"
    );
    i.refused(moving(), StatusCode::CONFLICT, "show the plan")
        .await?;
    Ok(())
}

/// MVB-020, MVB-021: without the helper nothing can be asked, and a request that cannot be
/// written is said.
#[tokio::test]
async fn without_the_helper_nothing_is_asked() -> TestResult {
    // A server that was given no helper at all (a development build).
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    let view = call(&h.router, Method::GET, API, None, &admin).await?;
    assert_eq!(view.status, StatusCode::OK);
    assert_eq!(view.body["helper_installed"], false);
    assert_eq!(
        view.body["can"],
        json!({"plan": false, "move": false, "abort": false})
    );
    let asked = call(&h.router, Method::POST, API, Some(plan()), &admin).await?;
    assert_eq!(asked.status, StatusCode::CONFLICT);

    // Its units are not enabled on this host.
    let i = instance().await?;
    i.helper.installed.store(false, Ordering::SeqCst);
    let view = i.view().await?;
    assert_eq!(view["helper_installed"], false);
    assert_eq!(
        view["can"],
        json!({"plan": false, "move": false, "abort": false})
    );
    i.refused(
        plan(),
        StatusCode::CONFLICT,
        "the move helper is not enabled on this server",
    )
    .await?;

    // Enabled, and the request cannot be written: the ask is in the log (it is written
    // first), and so is that nothing came of it.
    i.helper.installed.store(true, Ordering::SeqCst);
    i.helper.unwritable.store(true, Ordering::SeqCst);
    i.refused(plan(), StatusCode::CONFLICT, "could not be written")
        .await?;
    let asked = i.audited(TARGET, "migration.planned").await?;
    assert_eq!(asked.len(), 1, "{asked:?}");
    let unwritten = i.audited(TARGET, "migration.unwritten").await?;
    assert_eq!(unwritten.len(), 1, "{unwritten:?}");
    assert!(
        unwritten[0].contains("could not be written") && unwritten[0].contains("request "),
        "{unwritten:?}"
    );
    Ok(())
}

/// MVB-022: nothing is asked of root that the audit log does not hold: the entry is written
/// before the request.
#[tokio::test]
async fn an_ask_is_audited_before_its_request_is_written() -> TestResult {
    let i = instance().await?;
    i.planned(&GOOD, None).await?;
    i.helper.held.store(true, Ordering::SeqCst);
    let (answer, audited) = tokio::join!(i.ask(moving()), async {
        let waited = std::time::Instant::now();
        let seen = loop {
            let entries = i.audited(TARGET, "migration.started").await;
            if !matches!(&entries, Ok(found) if found.is_empty()) {
                break entries;
            }
            if waited.elapsed().as_secs() >= 3 {
                break Err("the ask was not audited while its request was held".into());
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        // The move's request is still on its way: only the plan's was written.
        let written = i.helper.written().len();
        i.helper.held.store(false, Ordering::SeqCst);
        seen.map(|entries| (entries, written))
    });
    let (entries, written) = audited?;
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(written, 1, "the request was written before its audit entry");
    assert_eq!(answer?.status, StatusCode::ACCEPTED);
    assert_eq!(i.helper.written().len(), 2);
    Ok(())
}
