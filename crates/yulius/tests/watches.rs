//! Script watches (SPEC-630, ADR-0465): a project's shell script on a schedule, run by Yulius in
//! the sandbox without an agent, whose output becomes a notice keyed `watch:<name>` for its owner.
//! The scripts run for real through the detached process host, uncontained (as CI jobs do in
//! tests), on the test clock.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use axum::http::{Method, StatusCode};
use yulius::wiring::session::PtySessionHost;
use yulius_app::context::App;
use yulius_app::ports::store_ext::{WatchRecord, WatchStore as _};
use yulius_app::watches::NewWatch;
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::watch::RunBounds;

use crate::support::{
    self, Creds, Harness, HarnessOptions, call, harness_with, perms, seed_github_user, seed_sandbox,
};

type Error = Box<dyn std::error::Error>;
type TestResult = Result<(), Error>;

const MINUTE: i64 = 60_000;
/// 2026-11-22T08:00:00Z.
const EIGHT_AM: i64 = 1_795_334_400_000;

struct World {
    h: Harness,
    alice: Creds,
    /// The superadmin.
    admin: Creds,
}

/// Project s1 (`app`), which alice writes; the real process host; `configure` adjusts the
/// configuration (small run bounds, one run at a time) where a test asks for it.
async fn world_with(
    host: Option<Arc<dyn yulius_app::ports::session::SessionHost>>,
    configure: impl FnOnce(&mut yulius_app::context::AppConfig) + 'static,
    contained: bool,
) -> Result<World, Error> {
    // A short base: run sockets live at <data>/runs/watch-<id>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("ywt").tempdir_in("/tmp")?;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let _ = Command::new(&exe).arg("version").output();
    // A file store, so a test can refuse notices with a trigger (an outage).
    let store = yulius_store::SqliteStore::open(&dir.path().join("yulius.db"))?;
    let h = harness_with(HarnessOptions {
        session_host: Some(host.unwrap_or_else(|| Arc::new(PtySessionHost::new(exe)))),
        dir: Some(dir),
        store: Some(store),
        configure: Some(Box::new(move |config| {
            config.contained = contained;
            configure(config);
        })),
        ..HarnessOptions::default()
    })?;
    h.clock.0.store(EIGHT_AM, Ordering::SeqCst);
    let admin = support::setup_superadmin_creds(&h).await?;
    support::configure_github_app(&h).await?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    h.github.set_perm("alice", "acme/app", perms("write"));
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    Ok(World { h, alice, admin })
}

async fn world() -> Result<World, Error> {
    world_with(None, |_| {}, false).await
}

/// Waits until `check` holds, for at most 30 s.
async fn until<F, Fut>(what: &str, mut check: F) -> TestResult
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<bool, Error>>,
{
    let deadline = Instant::now() + Duration::from_secs(30);
    while !check().await? {
        if Instant::now() > deadline {
            return Err(format!("never: {what}").into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(())
}

fn new_watch(name: &str, script: &str) -> Result<NewWatch, Error> {
    Ok(NewWatch {
        sandbox: SandboxId::parse("s1")?,
        owner: UserId::parse("u-alice")?,
        name: name.into(),
        script: script.into(),
        every: Some("1h".into()),
        cron: None,
        utc_offset_minutes: 0,
        expiry: None,
        timeout_seconds: Some(20),
        secrets: Vec::new(),
        created_via: "ui",
    })
}

/// A script that counts its runs in its state directory and does `cases` on each count.
fn counting(cases: &str) -> String {
    format!(
        "n=$(cat \"$YULIUS_WATCH_STATE/n\" 2>/dev/null || echo 0); n=$((n+1)); \
         echo $n > \"$YULIUS_WATCH_STATE/n\"\ncase $n in\n{cases}\nesac\n"
    )
}

impl World {
    async fn watch(&self, id: &str) -> Result<WatchRecord, Error> {
        Ok(self.h.store.watch(id).await?.ok_or("the watch is gone")?)
    }

    /// Runs the watch once now, to its end.
    async fn run(&self, id: &str) -> Result<WatchRecord, Error> {
        assert!(self.h.app.watch_run(id, true).await, "a run started");
        self.watch(id).await
    }

    /// One scheduler tick on `app`, to the end of the runs it started; how many it started.
    async fn tick_on(app: &Arc<App>) -> Result<usize, Error> {
        let runs = app.watch_tick().await;
        let started = runs.len();
        for run in runs {
            run.await?;
        }
        Ok(started)
    }

    async fn tick(&self) -> Result<usize, Error> {
        Self::tick_on(&self.h.app).await
    }

    fn advance(&self, ms: i64) {
        self.h.clock.0.fetch_add(ms, Ordering::SeqCst);
    }

    /// Alice's live notices of the watch's keys: (key, title, id).
    async fn notices(&self) -> Result<Vec<(String, String, String)>, Error> {
        let listed = call(
            &self.h.router,
            Method::GET,
            "/api/me/notices",
            None,
            &self.alice,
        )
        .await?;
        assert_eq!(listed.status, StatusCode::OK, "{}", listed.body);
        let items = listed.body.as_array().cloned().unwrap_or_default();
        Ok(items
            .iter()
            .filter(|n| n["key"].as_str().is_some_and(|k| k.starts_with("watch:")))
            .map(|n| {
                let text = |field: &str| n[field].as_str().unwrap_or_default().to_owned();
                (text("key"), text("title"), text("id"))
            })
            .collect())
    }
}

fn keys(notices: &[(String, String, String)]) -> Vec<(&str, &str)> {
    notices
        .iter()
        .map(|(key, title, _)| (key.as_str(), title.as_str()))
        .collect()
}

/// AC-1 (WCH-009): an interval watch runs at the first tick, not again before its interval, and
/// again after it; a cron watch runs once its slot passed, once a day.
#[tokio::test]
async fn the_schedule_fires() -> TestResult {
    let w = world().await?;
    let mut every = new_watch("every", "echo tick")?;
    every.every = Some("5m".into());
    let every = w.h.app.watch_create(every).await?;
    let mut daily = new_watch("daily", "echo morning")?;
    daily.every = None;
    daily.cron = Some("0 9 * * *".into());
    let daily = w.h.app.watch_create(daily).await?;

    assert_eq!(
        w.tick().await?,
        1,
        "the interval watch at once, the cron one at 09:00"
    );
    assert_eq!(w.watch(&every.id).await?.state.runs, 1);
    w.advance(4 * MINUTE);
    assert_eq!(w.tick().await?, 0);
    w.advance(MINUTE);
    assert_eq!(w.tick().await?, 1);
    assert_eq!(w.watch(&every.id).await?.state.runs, 2);

    // 09:00:30: the cron watch's slot passed (the interval one is due again too).
    w.advance(55 * MINUTE + 30_000);
    assert_eq!(w.tick().await?, 2);
    assert_eq!(w.watch(&daily.id).await?.state.runs, 1);
    assert_eq!(w.watch(&every.id).await?.state.runs, 3);
    // 10:00: due only tomorrow at 09:00 (the interval watch runs once more meanwhile).
    w.advance(59 * MINUTE + 30_000);
    w.tick().await?;
    assert_eq!(w.watch(&daily.id).await?.state.runs, 1);
    w.advance(23 * 60 * MINUTE);
    w.tick().await?;
    assert_eq!(w.watch(&daily.id).await?.state.runs, 2);
    let titles = keys(&w.notices().await?)
        .into_iter()
        .map(|(k, t)| format!("{k}={t}"))
        .collect::<Vec<_>>();
    assert!(
        titles.contains(&"watch:daily=daily: morning".to_owned()),
        "{titles:?}"
    );
    Ok(())
}

/// AC-2 (WCH-005, WCH-006): exit 0 posts and replaces the previous post, no output posts nothing,
/// exit 3 posts and ends the watch.
#[tokio::test]
async fn exit_codes_post_replace_skip_and_end() -> TestResult {
    let w = world().await?;
    let script = counting(
        "1) echo 'first report'; echo 'more lines' ;;\n\
         2) echo 'second report' ;;\n\
         3) exit 0 ;;\n\
         *) echo 'due today'; exit 3 ;;",
    );
    let watch = w.h.app.watch_create(new_watch("release", &script)?).await?;

    let first = w.run(&watch.id).await?;
    assert_eq!(first.state.last_exit, Some(0));
    assert_eq!(first.state.last_failure, None);
    let posted = w.notices().await?;
    assert_eq!(keys(&posted), [("watch:release", "release: first report")]);

    w.advance(1);
    w.run(&watch.id).await?;
    let replaced = w.notices().await?;
    assert_eq!(
        keys(&replaced),
        [("watch:release", "release: second report")]
    );
    assert_ne!(
        replaced[0].2, posted[0].2,
        "a new notice replaced the first"
    );

    w.advance(1);
    let quiet = w.run(&watch.id).await?;
    assert_eq!(
        (quiet.state.last_exit, quiet.state.last_output),
        (Some(0), None)
    );
    assert_eq!(w.notices().await?, replaced, "no output posts nothing");

    w.advance(1);
    let done = w.run(&watch.id).await?;
    assert_eq!(done.state.last_exit, Some(3));
    assert!(
        done.ended_reason
            .as_deref()
            .is_some_and(|r| r.contains("exit 3"))
    );
    assert_eq!(
        keys(&w.notices().await?),
        [("watch:release", "release: due today")]
    );
    assert_eq!(done.state.posts, 3);

    // Ended: the schedule runs it no more.
    w.advance(30 * 24 * 60 * MINUTE);
    assert_eq!(w.tick().await?, 0);
    assert_eq!(w.watch(&watch.id).await?.state.runs, 4);
    Ok(())
}

/// AC-3 (WCH-007): three failures in a row post one warning, a fourth adds nothing, a success
/// removes it.
#[tokio::test]
async fn three_failures_make_one_warning() -> TestResult {
    let w = world().await?;
    let script =
        counting("[1-4]) echo 'cannot reach the API' >&2; exit 1 ;;\n*) echo recovered ;;");
    let watch = w.h.app.watch_create(new_watch("api", &script)?).await?;
    for _ in 0..2 {
        w.run(&watch.id).await?;
        w.advance(1);
    }
    assert!(w.notices().await?.is_empty(), "two failures say nothing");
    let third = w.run(&watch.id).await?;
    assert_eq!((third.state.failures, third.state.warned), (3, true));
    assert_eq!(third.state.last_failure.as_deref(), Some("exit 1"));
    let warned = w.notices().await?;
    assert_eq!(
        keys(&warned),
        [("watch:api:failing", "api: failing, 3 runs in a row")]
    );

    w.advance(1);
    let fourth = w.run(&watch.id).await?;
    assert_eq!(fourth.state.failures, 4);
    assert_eq!(w.notices().await?, warned, "the same one warning");

    w.advance(1);
    let ok = w.run(&watch.id).await?;
    assert_eq!((ok.state.failures, ok.state.warned), (0, false));
    assert_eq!(keys(&w.notices().await?), [("watch:api", "api: recovered")]);
    Ok(())
}

/// The warning carries the error output and the reason.
#[tokio::test]
async fn the_warning_says_why() -> TestResult {
    let w = world().await?;
    let watch =
        w.h.app
            .watch_create(new_watch(
                "down",
                "echo 'HTTP 503 from the API' >&2; exit 7",
            )?)
            .await?;
    for _ in 0..3 {
        w.run(&watch.id).await?;
        w.advance(1);
    }
    let listed = call(&w.h.router, Method::GET, "/api/me/notices", None, &w.alice).await?;
    let warning = listed
        .body
        .as_array()
        .and_then(|items| items.iter().find(|n| n["key"] == "watch:down:failing"))
        .cloned()
        .ok_or("the warning")?;
    let detail = warning["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("exit 7") && detail.contains("HTTP 503 from the API"),
        "{detail}"
    );
    assert_eq!(warning["tone"], "error");
    assert_eq!(warning["link"], "/s/app/watches");
    assert_eq!(warning["personal"], true);
    Ok(())
}

/// AC-4 (WCH-008): while the store refuses notices, a run's post stays pending; once it accepts
/// them again, the next tick posts it, once.
#[tokio::test]
async fn a_post_is_retried_after_an_outage() -> TestResult {
    let w = world().await?;
    let watch =
        w.h.app
            .watch_create(new_watch("ci-number", "echo 'CI is red'")?)
            .await?;
    let db = w.h.app.config.data_dir.join("yulius.db");
    rusqlite::Connection::open(&db)?.execute_batch(
        "CREATE TRIGGER notice_outage BEFORE INSERT ON notices \
         BEGIN SELECT RAISE(FAIL, 'injected outage'); END",
    )?;
    w.run(&watch.id).await?;
    assert!(w.notices().await?.is_empty());
    let pending = w.h.store.watch_pending().await?;
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0].key, "watch:ci-number");
    // A tick during the outage keeps it.
    w.advance(MINUTE);
    w.tick().await?;
    assert_eq!(w.h.store.watch_pending().await?.len(), 1);

    rusqlite::Connection::open(&db)?.execute_batch("DROP TRIGGER notice_outage")?;
    w.advance(MINUTE);
    assert_eq!(w.tick().await?, 0, "not due: only the retry");
    assert_eq!(
        keys(&w.notices().await?),
        [("watch:ci-number", "ci-number: CI is red")]
    );
    assert!(w.h.store.watch_pending().await?.is_empty());
    assert_eq!(w.watch(&watch.id).await?.state.posts, 1);
    w.tick().await?;
    assert_eq!(w.notices().await?.len(), 1, "posted once");
    Ok(())
}

/// AC-5 (WCH-010): a new server process on the same store lists the watch with its state,
/// records the run the old one left going as lost, and runs it again.
#[tokio::test]
async fn watches_survive_a_restart() -> TestResult {
    let w = world().await?;
    let script = counting(
        "2) echo $$ > \"$YULIUS_WATCH_STATE/pid\"; exec sleep 60 ;;\n*) echo \"run $n\" ;;",
    );
    let watch =
        w.h.app
            .watch_create(new_watch("survivor", &script)?)
            .await?;
    w.run(&watch.id).await?;
    w.advance(MINUTE);
    // The old process starts a run and dies while it goes: its future is dropped, its detached
    // process host and script keep running.
    let old = Arc::clone(&w.h.app);
    let id = watch.id.clone();
    let going = tokio::spawn(async move { old.watch_run(&id, true).await });
    let pid_file =
        w.h.app
            .config
            .data_dir
            .join("watches/s1/state")
            .join(&watch.id)
            .join("pid");
    until("the second run started", || {
        let pid_file = pid_file.clone();
        async move { Ok(pid_file.exists()) }
    })
    .await?;
    going.abort();
    let pid: u32 = std::fs::read_to_string(&pid_file)?.trim().parse()?;
    let alive = move || {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            !stat
                .rsplit(')')
                .next()
                .is_some_and(|rest| rest.trim_start().starts_with('Z'))
        })
    };
    assert!(alive(), "the script outlives the old process");
    let (run_id, _) = w.watch(&watch.id).await?.state.running.ok_or("running")?;

    let after = Arc::new(App::new(w.h.app.config.clone(), w.h.app.deps.clone()));
    after.watch_startup().await;
    until(
        "the lost run's process ended",
        || async move { Ok(!alive()) },
    )
    .await?;
    assert!(
        !after
            .config
            .data_dir
            .join("runs")
            .join(format!("watch-{run_id}"))
            .exists()
    );
    let recovered = w.watch(&watch.id).await?;
    assert_eq!(recovered.state.running, None);
    assert_eq!(recovered.state.runs, 2);
    assert_eq!(recovered.state.failures, 1);
    assert!(
        recovered
            .state
            .last_failure
            .as_deref()
            .is_some_and(|f| f.contains("lost")),
        "{:?}",
        recovered.state.last_failure
    );
    assert_eq!(World::tick_on(&after).await?, 1, "due again at once");
    let rerun = w.watch(&watch.id).await?;
    assert_eq!((rerun.state.runs, rerun.state.failures), (3, 0));
    assert_eq!(
        keys(&w.notices().await?),
        [("watch:survivor", "survivor: run 3")]
    );
    Ok(())
}

/// AC-6 (WCH-003): each bound stops a run that goes past it, with a reason that names it, and a
/// script cannot raise its own limits.
#[tokio::test]
async fn run_bounds_are_enforced() -> TestResult {
    let bounds = RunBounds {
        cpu_seconds: 1,
        memory_kib: 32 * 1024,
        output_kib: 64,
    };
    let w = world_with(None, move |c| c.watches.bounds = bounds, false).await?;
    let failure = |watch: &WatchRecord| watch.state.last_failure.clone().unwrap_or_default();

    let busy =
        w.h.app
            .watch_create(new_watch("busy", "while :; do :; done")?)
            .await?;
    let started = Instant::now();
    let ended = w.run(&busy.id).await?;
    assert_eq!(failure(&ended), "stopped: it used its CPU time of 1 s");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "before its 20 s timeout"
    );

    let hungry =
        w.h.app
            .watch_create(new_watch(
                "hungry",
                "x=$(head -c 100000000 /dev/zero | tr '\\0' a); echo \"got ${#x}\"",
            )?)
            .await?;
    let ended = w.run(&hungry.id).await?;
    assert!(ended.state.last_failure.is_some(), "{:?}", ended.state);
    assert_eq!(ended.state.posts, 0);

    let loud =
        w.h.app
            .watch_create(new_watch("loud", "head -c 1000000 /dev/zero | tr '\\0' a")?)
            .await?;
    let ended = w.run(&loud.id).await?;
    assert!(
        failure(&ended).contains("output went past 64 KiB"),
        "{}",
        failure(&ended)
    );

    // The bound holds for every file a run writes, not only its output (the disk).
    let disk =
        w.h.app
            .watch_create(new_watch(
                "disk",
                "head -c 1000000 /dev/zero > \"$YULIUS_WATCH_STATE/big\"; echo written",
            )?)
            .await?;
    let ended = w.run(&disk.id).await?;
    assert!(
        ended
            .state
            .last_errors
            .is_some_and(|e| e.contains("File too large"))
    );
    let big =
        w.h.app
            .config
            .data_dir
            .join("watches/s1/state")
            .join(&disk.id)
            .join("big");
    assert_eq!(std::fs::metadata(&big)?.len(), 64 * 1024);

    let mut slow = new_watch("slow", "sleep 30")?;
    slow.timeout_seconds = Some(1);
    let slow = w.h.app.watch_create(slow).await?;
    let started = Instant::now();
    let ended = w.run(&slow.id).await?;
    assert!(
        failure(&ended).contains("timeout of 1s"),
        "{}",
        failure(&ended)
    );
    assert!(started.elapsed() < Duration::from_secs(15));

    let sly =
        w.h.app
            .watch_create(new_watch("sly", "ulimit -t unlimited && echo raised")?)
            .await?;
    let ended = w.run(&sly.id).await?;
    assert!(ended.state.last_failure.is_some(), "{:?}", ended.state);
    let posted = w.notices().await?;
    assert!(
        !posted.iter().any(|(key, _, _)| key == "watch:sly"),
        "{posted:?}"
    );
    Ok(())
}

/// AC-7 (WCH-002, WCH-004): a contained run joins the sandbox's holder in CI's group on the
/// sandbox root, with only its watch directories writable; a named agent secret reaches it as a
/// placeholder, never its value.
#[tokio::test]
async fn a_contained_run_uses_the_sandbox_and_secret_placeholders() -> TestResult {
    use yulius_app::ports::session::Containment;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let recording = Arc::new(crate::cd::Recording {
        inner: PtySessionHost::new(exe),
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let w = world_with(Some(recording.clone()), |_| {}, true).await?;
    assert!(
        w.h.app
            .set_isolation(Arc::new(crate::cd::Holders::default()))
    );
    let value = "watch-secret-value-0123456789";
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(serde_json::json!({"name": "API_KEY", "description": "an API", "value": value})),
        &w.alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let mut new = new_watch("keyed", "echo \"key=$API_KEY\"; echo \"$YULIUS_WATCH\"")?;
    new.secrets = vec!["API_KEY".into(), "NOT_THERE".into()];
    let watch = w.h.app.watch_create(new).await?;
    let ran = w.run(&watch.id).await?;
    let output = ran.state.last_output.clone().unwrap_or_default();
    assert!(
        output.contains("key=") && output.contains("API_KEY"),
        "{output}"
    );
    assert!(!output.contains(value), "{output}");
    assert!(output.ends_with("keyed"), "{output}");
    let errors = ran.state.last_errors.clone().unwrap_or_default();
    assert!(
        errors.contains("agent secrets: API_KEY") && errors.contains("NOT_THERE"),
        "{errors}"
    );

    let data = w.h.app.config.data_dir.clone();
    let base = data.join("watches/s1");
    let seen = recording.seen.lock().map(|s| s.clone()).unwrap_or_default();
    let Some(Containment::Bubblewrap {
        sandbox_root,
        home,
        writable,
        join: Some(join),
        identity: None,
        ..
    }) = seen.first()
    else {
        return Err(format!("a joined containment: {seen:?}").into());
    };
    assert_eq!(sandbox_root, &data.join("sandboxes/s1"));
    assert_eq!(home, &base.join("home"));
    assert_eq!(writable.len(), 3, "{writable:?}");
    assert_eq!(writable[0], base.join("home"));
    assert_eq!(writable[1], base.join("state").join(&watch.id));
    assert!(writable[2].starts_with(base.join("runs")));
    assert_eq!(
        join.cgroup.as_deref(),
        Some(std::path::Path::new("/cg/s1/ci")),
        "CI's group"
    );
    assert!(!writable[2].exists(), "the run's files went with it");
    Ok(())
}

/// WCH-001: what a watch may be.
#[tokio::test]
async fn a_bad_watch_is_refused() -> TestResult {
    let w = world().await?;
    let refused = |new: NewWatch| {
        let app = Arc::clone(&w.h.app);
        async move { app.watch_create(new).await.err().map(|e| e.to_string()) }
    };
    let mut both = new_watch("both", "echo")?;
    both.cron = Some("0 9 * * *".into());
    assert!(refused(both).await.is_some_and(|e| e.contains("either")));
    assert!(refused(new_watch("Bad Name", "echo")?).await.is_some());
    assert!(refused(new_watch("empty", " ")?).await.is_some());
    let mut fast = new_watch("fast", "echo")?;
    fast.every = Some("30s".into());
    assert!(refused(fast).await.is_some());
    let mut at = new_watch("at", "echo")?;
    at.expiry = Some("2026-12-01T00:00:00Z".into());
    assert!(refused(at).await.is_some_and(|e| e.contains("not a time")));
    w.h.app.watch_create(new_watch("taken", "echo")?).await?;
    assert!(
        refused(new_watch("taken", "echo")?)
            .await
            .is_some_and(|e| e.contains("name"))
    );
    Ok(())
}

/// Review round 1: a removed project's watches neither run nor post, and its going run ends.
#[tokio::test]
async fn a_removed_projects_watches_stop() -> TestResult {
    let w = world().await?;
    let hanging =
        w.h.app
            .watch_create(new_watch(
                "hanging",
                "echo $$ > \"$YULIUS_WATCH_STATE/pid\"; sleep 60",
            )?)
            .await?;
    let reporting =
        w.h.app
            .watch_create(new_watch("reporting", "echo hi")?)
            .await?;
    let app = Arc::clone(&w.h.app);
    let id = hanging.id.clone();
    let going = tokio::spawn(async move { app.watch_run(&id, true).await });
    let pid =
        w.h.app
            .config
            .data_dir
            .join("watches/s1/state")
            .join(&hanging.id)
            .join("pid");
    until("the run started", || {
        let pid = pid.clone();
        async move { Ok(pid.exists()) }
    })
    .await?;

    let removed = call(
        &w.h.router,
        Method::DELETE,
        "/api/sandboxes/s1",
        None,
        &w.admin,
    )
    .await?;
    assert!(
        removed.status.is_success(),
        "{} {}",
        removed.status,
        removed.body
    );
    let ran = tokio::time::timeout(Duration::from_secs(20), going).await??;
    assert!(ran, "the going run was cancelled, not left running");
    let ended = w.watch(&hanging.id).await?;
    assert!(
        ended
            .state
            .last_failure
            .as_deref()
            .is_some_and(|f| f.contains("removed")),
        "{:?}",
        ended.state
    );

    w.advance(2 * 60 * MINUTE);
    assert_eq!(w.tick().await?, 0, "not scheduled");
    assert!(
        !w.h.app.watch_run(&reporting.id, true).await,
        "not even by Run now"
    );
    assert_eq!(w.watch(&reporting.id).await?.state.runs, 0);
    assert!(w.h.store.watch_pending().await?.is_empty());
    Ok(())
}

/// WCH-004 (review round 1): a run's named secret reaches it only through its own egress
/// credential, which intercepts that secret's host alone and ends with the run.
#[tokio::test]
async fn a_runs_secrets_reach_it_only_through_its_proxy_credential() -> TestResult {
    let w = world().await?;
    for name in ["API_KEY", "OTHER_KEY"] {
        let created = call(
            &w.h.router,
            Method::POST,
            "/api/sandboxes/s1/agent-secrets",
            Some(serde_json::json!({
                "name": name,
                "hosts": [format!("{}.test", name.to_ascii_lowercase().replace('_', "-"))],
                "injection": "bearer",
                "value": format!("{name}-value-0123456789"),
                "auto_allow": false,
            })),
            &w.alice,
        )
        .await?;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    }
    w.h.app.enable_egress(Some(
        "-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----\n".into(),
    ));
    let mut new = new_watch(
        "keyed",
        "printf %s \"$HTTPS_PROXY\" > \"$YULIUS_WATCH_STATE/proxy\"; \
         echo \"key=$API_KEY other=${OTHER_KEY:-none}\"; \
         while [ ! -e \"$YULIUS_WATCH_STATE/go\" ]; do sleep 0.05; done",
    )?;
    new.secrets = vec!["API_KEY".into()];
    let watch = w.h.app.watch_create(new).await?;
    let app = Arc::clone(&w.h.app);
    let id = watch.id.clone();
    let going = tokio::spawn(async move { app.watch_run(&id, true).await });
    let state =
        w.h.app
            .config
            .data_dir
            .join("watches/s1/state")
            .join(&watch.id);
    let proxy = state.join("proxy");
    until("the run wrote its proxy", || {
        let proxy = proxy.clone();
        async move { Ok(std::fs::read_to_string(&proxy).is_ok_and(|p| !p.is_empty())) }
    })
    .await?;
    let url = std::fs::read_to_string(&proxy)?;
    let token = url
        .strip_prefix("http://yulius-watch:")
        .and_then(|rest| rest.split_once('@'))
        .map(|(token, _)| token.to_owned())
        .ok_or_else(|| format!("unexpected proxy {url}"))?;
    let policy: Arc<dyn yulius_app::ports::egress::EgressPolicy> = w.h.app.clone();
    let client = policy
        .egress_client(&token)
        .await
        .ok_or("the run's credential names no client")?;
    assert_eq!(client.actor(), "ci:watch:keyed");
    assert!(policy.egress_intercepts(&client, "api-key.test", 443).await);
    assert!(
        !policy
            .egress_intercepts(&client, "other-key.test", 443)
            .await
    );
    std::fs::write(state.join("go"), "")?;
    assert!(going.await?);
    assert!(
        policy.egress_client(&token).await.is_none(),
        "the credential ends with the run"
    );
    let output = w
        .watch(&watch.id)
        .await?
        .state
        .last_output
        .unwrap_or_default();
    assert!(output.contains("other=none"), "{output}");
    assert!(!output.contains("API_KEY-value-0123456789"), "{output}");
    Ok(())
}

/// WCH-008 (review round 1): a run's end the store refused is kept in memory and written, with
/// its post, by the next tick.
#[tokio::test]
async fn a_run_end_the_store_refused_is_written_by_the_next_tick() -> TestResult {
    let w = world().await?;
    let watch =
        w.h.app
            .watch_create(new_watch("kept", "echo 'kept for later'")?)
            .await?;
    let db = w.h.app.config.data_dir.join("yulius.db");
    rusqlite::Connection::open(&db)?.execute_batch(
        "CREATE TRIGGER end_outage BEFORE UPDATE OF last_ended_at ON watches \
         BEGIN SELECT RAISE(FAIL, 'injected outage'); END",
    )?;
    assert!(w.h.app.watch_run(&watch.id, true).await);
    let unrecorded = w.watch(&watch.id).await?;
    assert!(
        unrecorded.state.running.is_some(),
        "its end is not written yet"
    );
    assert!(w.notices().await?.is_empty());
    w.tick().await?;
    assert!(
        w.watch(&watch.id).await?.state.running.is_some(),
        "kept while refused"
    );

    rusqlite::Connection::open(&db)?.execute_batch("DROP TRIGGER end_outage")?;
    w.advance(MINUTE);
    assert_eq!(w.tick().await?, 0, "not due: only the kept end");
    let recorded = w.watch(&watch.id).await?;
    assert_eq!((recorded.state.running, recorded.state.runs), (None, 1));
    assert_eq!(
        keys(&w.notices().await?),
        [("watch:kept", "kept: kept for later")]
    );
    w.tick().await?;
    assert_eq!(w.notices().await?.len(), 1, "posted once");
    Ok(())
}

/// WCH-003 (review round 1): the instance runs at most its bound of runs at once, the longest
/// overdue watch first.
#[tokio::test]
async fn runs_at_once_are_bounded_and_the_longest_overdue_goes_first() -> TestResult {
    let w = world_with(None, |c| c.watches.max_runs = 1, false).await?;
    let mut late = new_watch("z-late", "echo late")?;
    late.every = Some("5m".into());
    let late = w.h.app.watch_create(late).await?;
    w.advance(MINUTE);
    let early =
        w.h.app
            .watch_create(new_watch("a-early", "echo early")?)
            .await?;
    assert_eq!(w.tick().await?, 1, "one run at a time");
    assert_eq!(
        w.watch(&late.id).await?.state.runs,
        1,
        "the longest overdue first"
    );
    assert_eq!(w.watch(&early.id).await?.state.runs, 0);
    assert_eq!(w.tick().await?, 1);
    assert_eq!(w.watch(&early.id).await?.state.runs, 1);
    Ok(())
}

/// WCH-006 (review round 1): a report the title cannot hold keeps its whole text as detail; the
/// project's agent secrets are redacted from what is kept and posted; a script starting with
/// `-` is a script, not bash's options.
#[tokio::test]
async fn reports_keep_their_text_redacted_and_scripts_are_never_options() -> TestResult {
    let w = world().await?;
    let value = "watch-secret-value-0123456789";
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(serde_json::json!({"name": "API_KEY", "description": "an API", "value": value})),
        &w.alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let long =
        w.h.app
            .watch_create(new_watch(
                "long",
                &format!("printf 'x%.0s' $(seq 1 400); echo ' {value}'"),
            )?)
            .await?;
    let ran = w.run(&long.id).await?;
    let kept = ran.state.last_output.unwrap_or_default();
    assert!(
        kept.starts_with(&"x".repeat(400)) && !kept.contains(value),
        "{kept}"
    );
    let listed = call(&w.h.router, Method::GET, "/api/me/notices", None, &w.alice).await?;
    let report = listed
        .body
        .as_array()
        .and_then(|items| items.iter().find(|n| n["key"] == "watch:long"))
        .cloned()
        .ok_or("the report")?;
    let detail = report["detail"].as_str().unwrap_or_default();
    assert!(detail.starts_with(&"x".repeat(400)), "{detail}");
    assert!(
        !detail.contains(value) && !report["title"].as_str().unwrap_or_default().contains(value)
    );

    let dashed =
        w.h.app
            .watch_create(new_watch("dashed", "--version")?)
            .await?;
    let ran = w.run(&dashed.id).await?;
    assert_eq!(ran.state.last_exit, Some(127), "{:?}", ran.state);
    assert_eq!(ran.state.posts, 0);
    Ok(())
}

/// Review round 1: a project's watch files, outside the sandbox's quota, are bounded: a run that
/// grows them past the bound is stopped.
#[tokio::test]
async fn a_run_that_fills_the_watch_files_is_stopped() -> TestResult {
    let w = world_with(None, |c| c.watches.disk_max = 256 * 1024, false).await?;
    let mut new = new_watch(
        "filler",
        "for i in 1 2 3 4 5 6 7 8; do head -c 60000 /dev/zero > \"$YULIUS_WATCH_STATE/f$i\"; done; \
         sleep 25",
    )?;
    new.timeout_seconds = Some(30);
    let watch = w.h.app.watch_create(new).await?;
    let started = Instant::now();
    let ended = w.run(&watch.id).await?;
    let failure = ended.state.last_failure.unwrap_or_default();
    assert!(
        failure.contains("watch files") && failure.contains("disk bound"),
        "{failure}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "stopped before its timeout"
    );
    Ok(())
}

/// Review round 2: a run that was going when its project was removed is still recovered at the
/// next start, and runs write no core file.
#[tokio::test]
async fn a_removed_projects_lost_run_is_recovered_and_runs_write_no_core() -> TestResult {
    use yulius_app::ports::store::SandboxStore as _;
    let w = world().await?;
    let cores =
        w.h.app
            .watch_create(new_watch("cores", "ulimit -c; ulimit -Hc")?)
            .await?;
    let ran = w.run(&cores.id).await?;
    assert_eq!(ran.state.last_output.as_deref(), Some("0\n0"));

    let lost = w.h.app.watch_create(new_watch("lost", "echo hi")?).await?;
    assert!(
        w.h.store
            .watch_run_started(&lost.id, "wr_lost", w.h.app.now(), false)
            .await?
            .is_some()
    );
    w.h.store
        .remove_sandbox(&SandboxId::parse("s1")?, w.h.app.now())
        .await?;
    let after = Arc::new(App::new(w.h.app.config.clone(), w.h.app.deps.clone()));
    after.watch_startup().await;
    let recovered = w.watch(&lost.id).await?;
    assert_eq!(recovered.state.running, None);
    assert!(
        recovered
            .state
            .last_failure
            .as_deref()
            .is_some_and(|f| f.contains("lost")),
        "{:?}",
        recovered.state
    );
    Ok(())
}

async fn http(
    w: &World,
    method: Method,
    path: &str,
    body: Option<serde_json::Value>,
    who: &Creds,
) -> Result<support::Resp, Error> {
    call(&w.h.router, method, path, body, who).await
}

/// WCH-011: people list, create, edit, run, pause, resume and remove watches over HTTP, each by
/// their rights, and each change is audited.
#[tokio::test]
async fn people_manage_watches_over_http() -> TestResult {
    use yulius_app::ports::store::AuditStore as _;
    let w = world().await?;
    let bob = seed_github_user(&w.h, "u-bob", "bob").await?;
    let carol = seed_github_user(&w.h, "u-carol", "carol").await?;
    w.h.github.set_perm("bob", "acme/app", perms("read"));
    w.h.github.set_perm("carol", "acme/app", perms("admin"));
    // A writer who is not its owner (review round 1): none of its changes are theirs.
    let dave = seed_github_user(&w.h, "u-dave", "dave").await?;
    w.h.github.set_perm("dave", "acme/app", perms("write"));
    let body = serde_json::json!({
        "name": "release", "script": "echo v1", "every": "1h", "expiry": "2h", "secrets": ["API_KEY"]
    });

    let refused = http(
        &w,
        Method::POST,
        "/api/sandboxes/s1/watches",
        Some(body.clone()),
        &bob,
    )
    .await?;
    assert_eq!(
        refused.status,
        StatusCode::FORBIDDEN,
        "a reader creates none: {}",
        refused.body
    );
    let made = http(
        &w,
        Method::POST,
        "/api/sandboxes/s1/watches",
        Some(body.clone()),
        &w.alice,
    )
    .await?;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    let id = made.body["id"].as_str().ok_or("id")?.to_owned();
    assert_eq!(made.body["owner"], "alice");
    assert_eq!(made.body["every"], "1h");
    assert_eq!(made.body["state"], "active");
    assert_eq!(
        (
            made.body["can_edit"].as_bool(),
            made.body["can_remove"].as_bool()
        ),
        (Some(true), Some(true))
    );
    let again = http(
        &w,
        Method::POST,
        "/api/sandboxes/s1/watches",
        Some(body),
        &w.alice,
    )
    .await?;
    assert_eq!(again.status, StatusCode::CONFLICT, "{}", again.body);

    let listed = http(&w, Method::GET, "/api/sandboxes/s1/watches", None, &bob).await?;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.body);
    assert_eq!(listed.body[0]["name"], "release");
    assert_eq!(
        (
            listed.body[0]["can_edit"].as_bool(),
            listed.body[0]["can_remove"].as_bool()
        ),
        (Some(false), Some(false))
    );
    let by_admin = http(&w, Method::GET, &format!("/api/watches/{id}"), None, &carol).await?;
    assert_eq!(
        (
            by_admin.body["can_edit"].as_bool(),
            by_admin.body["can_remove"].as_bool()
        ),
        (Some(false), Some(true))
    );

    // Only the owner edits; a name never changes.
    let edit =
        serde_json::json!({"script": "echo v2", "cron": "0 9 * * *", "utc_offset_minutes": 60});
    let other = http(
        &w,
        Method::PUT,
        &format!("/api/watches/{id}"),
        Some(edit.clone()),
        &carol,
    )
    .await?;
    assert_eq!(other.status, StatusCode::FORBIDDEN, "{}", other.body);
    let renamed = http(
        &w,
        Method::PUT,
        &format!("/api/watches/{id}"),
        Some(serde_json::json!({"name": "other", "script": "echo", "every": "1h"})),
        &w.alice,
    )
    .await?;
    assert_eq!(
        renamed.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        renamed.body
    );
    let edited = http(
        &w,
        Method::PUT,
        &format!("/api/watches/{id}"),
        Some(edit),
        &w.alice,
    )
    .await?;
    assert_eq!(edited.status, StatusCode::OK, "{}", edited.body);
    assert_eq!(
        (edited.body["script"].as_str(), edited.body["cron"].as_str()),
        (Some("echo v2"), Some("0 9 * * *"))
    );
    assert_eq!(edited.body["every"], serde_json::Value::Null);

    // Another writer, and the administrator for Run now, are refused, and see why.
    let as_dave = http(&w, Method::GET, &format!("/api/watches/{id}"), None, &dave).await?;
    assert_eq!(
        (
            as_dave.body["can_edit"].as_bool(),
            as_dave.body["can_remove"].as_bool()
        ),
        (Some(false), Some(false))
    );
    let edit_body = serde_json::json!({"script": "echo dave", "every": "1h"});
    for (method, path, body) in [
        (Method::PUT, format!("/api/watches/{id}"), Some(edit_body)),
        (Method::POST, format!("/api/watches/{id}/run"), None),
        (Method::POST, format!("/api/watches/{id}/pause"), None),
        (Method::POST, format!("/api/watches/{id}/resume"), None),
        (Method::DELETE, format!("/api/watches/{id}"), None),
    ] {
        let refused = http(&w, method.clone(), &path, body, &dave).await?;
        assert_eq!(
            refused.status,
            StatusCode::FORBIDDEN,
            "dave {method} {path}: {}",
            refused.body
        );
    }
    let carol_runs = http(
        &w,
        Method::POST,
        &format!("/api/watches/{id}/run"),
        None,
        &carol,
    )
    .await?;
    assert_eq!(
        carol_runs.status,
        StatusCode::FORBIDDEN,
        "{}",
        carol_runs.body
    );

    // Run now: its owner only; it posts as a scheduled run does.
    let bob_runs = http(
        &w,
        Method::POST,
        &format!("/api/watches/{id}/run"),
        None,
        &bob,
    )
    .await?;
    assert_eq!(bob_runs.status, StatusCode::FORBIDDEN, "{}", bob_runs.body);
    let ran = http(
        &w,
        Method::POST,
        &format!("/api/watches/{id}/run"),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(ran.status, StatusCode::ACCEPTED, "{}", ran.body);
    until("the run ended", || async {
        Ok(w.watch(&id).await?.state.runs == 1)
    })
    .await?;
    assert_eq!(
        keys(&w.notices().await?),
        [("watch:release", "release: v2")]
    );

    // An administrator pauses; only the owner resumes.
    let paused = http(
        &w,
        Method::POST,
        &format!("/api/watches/{id}/pause"),
        None,
        &carol,
    )
    .await?;
    assert_eq!(
        (paused.status, paused.body["state"].as_str()),
        (StatusCode::OK, Some("paused"))
    );
    w.advance(48 * 60 * MINUTE);
    assert_eq!(w.tick().await?, 0, "paused");
    let not_carol = http(
        &w,
        Method::POST,
        &format!("/api/watches/{id}/resume"),
        None,
        &carol,
    )
    .await?;
    assert_eq!(not_carol.status, StatusCode::FORBIDDEN);
    let resumed = http(
        &w,
        Method::POST,
        &format!("/api/watches/{id}/resume"),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(resumed.body["state"], "active");
    assert!(resumed.body["next_run_at"].is_string());

    // Removal: its notices go with it.
    let bob_removes = http(
        &w,
        Method::DELETE,
        &format!("/api/watches/{id}"),
        None,
        &bob,
    )
    .await?;
    assert_eq!(
        bob_removes.status,
        StatusCode::FORBIDDEN,
        "{}",
        bob_removes.body
    );
    let removed = http(
        &w,
        Method::DELETE,
        &format!("/api/watches/{id}"),
        None,
        &carol,
    )
    .await?;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{}", removed.body);
    assert!(w.notices().await?.is_empty(), "its notices are removed");
    let gone = http(
        &w,
        Method::GET,
        &format!("/api/watches/{id}"),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);

    for action in [
        "watch.create",
        "watch.update",
        "watch.run",
        "watch.pause",
        "watch.resume",
        "watch.remove",
    ] {
        let entries = w.h.store.audit_details("s1", action, 5).await?;
        assert!(
            entries.iter().any(|e| e.contains("release")),
            "{action}: {entries:?}"
        );
    }
    Ok(())
}

/// A remote tool's `(is_error, text, record)` for the person of `creds`.
async fn tool(
    w: &World,
    creds: &Creds,
    name: &str,
    args: serde_json::Value,
) -> Result<(bool, String, serde_json::Value), Error> {
    let r = http(
        w,
        Method::POST,
        "/mcp",
        Some(
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": name, "arguments": args, "_meta": {"yulius/structured": true}}}),
        ),
        creds,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{name}: {}", r.body);
    let result = &r.body["result"];
    Ok((
        result["isError"] == true,
        result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        result["structuredContent"].clone(),
    ))
}

async fn token(w: &World, scope: &str) -> Result<Creds, Error> {
    use yulius_app::ports::store::SessionStore as _;
    // Tokens are made in the browser: alice's cookie session, as the web UI has it.
    let (cookie, csrf) = (format!("browser-{scope}"), format!("csrf-{scope}"));
    let now = w.h.app.now();
    w.h.store
        .insert_web_session(&yulius_app::model::WebSessionRecord {
            id_hash: w.h.app.deps.crypto.sha256_hex(cookie.as_bytes()),
            user_id: UserId::parse("u-alice")?,
            csrf_token: csrf.clone(),
            created_at: now,
            expires_at: now.plus_millis(86_400_000),
            revoked_at: None,
        })
        .await?;
    let browser = Creds {
        cookie: Some(cookie),
        csrf: Some(csrf),
        bearer: None,
        oauth: None,
    };
    let made = http(
        w,
        Method::POST,
        "/api/me/tokens",
        Some(serde_json::json!({"label": scope, "scope": scope})),
        &browser,
    )
    .await?;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    Ok(Creds {
        bearer: made.body["token"].as_str().map(str::to_owned),
        ..Creds::default()
    })
}

/// WCH-012: the person tools (the Superagent's, and a person's own agent's over the remote MCP)
/// create, list, update, run, pause, resume and remove watches; a read-only token only lists.
#[tokio::test]
async fn the_person_tools_manage_watches() -> TestResult {
    let w = world().await?;
    let agent = token(&w, "operate").await?;
    let reader = token(&w, "read").await?;
    let project = "acme/app";
    let (error, text, made) = tool(
        &w,
        &agent,
        "watch_create",
        serde_json::json!({"project": project, "name": "due-date", "every": "1d",
            "script": "[ \"$(date -u +%F)\" \\< 2026-11-24 ] && exit 0; echo 'due'; exit 3"}),
    )
    .await?;
    assert!(!error, "{text}");
    assert_eq!(made["created_via"], "agent");
    assert_eq!(made["owner"], "alice");
    let (error, text, _) = tool(
        &w,
        &reader,
        "watch_create",
        serde_json::json!({"project": project, "name": "nope", "every": "1d", "script": "echo"}),
    )
    .await?;
    assert!(error, "a read token creates none: {text}");

    let (_, _, listed) = tool(
        &w,
        &reader,
        "watches_list",
        serde_json::json!({"project": project}),
    )
    .await?;
    assert_eq!(listed["watches"][0]["name"], "due-date", "{listed}");

    let (error, text, updated) = tool(
        &w,
        &agent,
        "watch_update",
        serde_json::json!({"project": project, "watch": "due-date", "every": "12h", "script": "echo 'merge-queue targets are due'; exit 3"}),
    )
    .await?;
    assert!(!error, "{text}");
    assert_eq!(updated["every"], "12h");

    let (error, text, _) = tool(
        &w,
        &agent,
        "watch_run",
        serde_json::json!({"project": project, "watch": "due-date"}),
    )
    .await?;
    assert!(!error, "{text}");
    let id = made["id"].as_str().ok_or("id")?.to_owned();
    until("the run ended", || async {
        Ok(w.watch(&id).await?.state.runs == 1)
    })
    .await?;
    let ended = w.watch(&id).await?;
    assert!(ended.ended_reason.is_some(), "exit 3 ends it");
    assert_eq!(
        keys(&w.notices().await?),
        [("watch:due-date", "due-date: merge-queue targets are due")]
    );

    let (_, _, resumed) = tool(
        &w,
        &agent,
        "watch_pause",
        serde_json::json!({"project": project, "watch": "due-date", "paused": false}),
    )
    .await?;
    assert_eq!(resumed["state"], "active");
    let (_, _, paused) = tool(
        &w,
        &agent,
        "watch_pause",
        serde_json::json!({"project": project, "watch": "due-date"}),
    )
    .await?;
    assert_eq!(paused["state"], "paused");
    let (error, text, _) = tool(
        &w,
        &agent,
        "watch_remove",
        serde_json::json!({"project": project, "watch": id}),
    )
    .await?;
    assert!(!error, "{text}");
    assert!(w.h.store.watch(&id).await?.is_none());
    let (error, text, _) = tool(
        &w,
        &agent,
        "watch_run",
        serde_json::json!({"project": project, "watch": "due-date"}),
    )
    .await?;
    assert!(error && text.contains("no watch"), "{text}");
    Ok(())
}

/// Review round 1 of PR #1722: a removal frees the watch's state; Run now is refused while the
/// watch runs or while the instance's runs are all taken; an edit's new schedule counts from now
/// and an unchanged one keeps its anchor; resuming an active watch changes nothing.
#[tokio::test]
async fn removal_frees_state_run_now_is_bounded_and_edits_keep_the_schedule() -> TestResult {
    let w = world_with(None, |c| c.watches.max_runs = 1, false).await?;
    let body = |script: &str| serde_json::json!({"script": script, "every": "1h"});

    // Removal frees its state directory.
    let mut stateful = new_watch("stateful", "echo kept > \"$YULIUS_WATCH_STATE/file\"")?;
    stateful.every = Some("1h".into());
    let stateful = w.h.app.watch_create(stateful).await?;
    w.run(&stateful.id).await?;
    let state =
        w.h.app
            .config
            .data_dir
            .join("watches/s1/state")
            .join(&stateful.id);
    assert!(state.join("file").exists());
    let removed = http(
        &w,
        Method::DELETE,
        &format!("/api/watches/{}", stateful.id),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{}", removed.body);
    assert!(!state.exists(), "its state goes with it");

    // Run now: one run of a watch, and none past the instance's bound.
    let hold = w.h.app.watch_create(new_watch("hold", "echo $$ > \"$YULIUS_WATCH_STATE/pid\"; while [ ! -e \"$YULIUS_WATCH_STATE/go\" ]; do sleep 0.05; done")?).await?;
    let other =
        w.h.app
            .watch_create(new_watch("other", "echo other")?)
            .await?;
    let first = http(
        &w,
        Method::POST,
        &format!("/api/watches/{}/run", hold.id),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(first.status, StatusCode::ACCEPTED, "{}", first.body);
    let hold_state =
        w.h.app
            .config
            .data_dir
            .join("watches/s1/state")
            .join(&hold.id);
    until("the held run started", || {
        let pid = hold_state.join("pid");
        async move { Ok(pid.exists()) }
    })
    .await?;
    let again = http(
        &w,
        Method::POST,
        &format!("/api/watches/{}/run", hold.id),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(again.status, StatusCode::CONFLICT, "{}", again.body);
    // Refused because a run of it goes, before the instance's bound (review round 2).
    assert!(
        again
            .body
            .to_string()
            .contains("a run of this watch is going"),
        "{}",
        again.body
    );
    let full = http(
        &w,
        Method::POST,
        &format!("/api/watches/{}/run", other.id),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(
        full.status,
        StatusCode::CONFLICT,
        "the instance's one run is taken: {}",
        full.body
    );
    std::fs::write(hold_state.join("go"), "")?;
    until("the held run ended", || async {
        Ok(w.watch(&hold.id).await?.state.runs == 1)
    })
    .await?;
    until("its permit is free", || async {
        let r = http(
            &w,
            Method::POST,
            &format!("/api/watches/{}/run", other.id),
            None,
            &w.alice,
        )
        .await?;
        Ok(r.status == StatusCode::ACCEPTED)
    })
    .await?;
    until("the other run ended", || async {
        Ok(w.watch(&other.id).await?.state.runs == 1)
    })
    .await?;

    for done in [&hold.id, &other.id] {
        let removed = http(
            &w,
            Method::DELETE,
            &format!("/api/watches/{done}"),
            None,
            &w.alice,
        )
        .await?;
        assert_eq!(removed.status, StatusCode::NO_CONTENT, "{}", removed.body);
    }

    // An edit to a cron whose slot already passed today waits for tomorrow's slot.
    let edited = w.h.app.watch_create(new_watch("edited", "echo e")?).await?;
    assert_eq!(w.tick().await?, 1, "the new interval watch runs at once");
    w.advance(90 * MINUTE); // 09:30
    let unchanged = http(
        &w,
        Method::PUT,
        &format!("/api/watches/{}", edited.id),
        Some(body("echo e2")),
        &w.alice,
    )
    .await?;
    assert_eq!(unchanged.status, StatusCode::OK, "{}", unchanged.body);
    assert_eq!(
        w.watch(&edited.id).await?.since,
        edited.since,
        "an unchanged schedule keeps its anchor"
    );
    let to_cron = serde_json::json!({"script": "echo e3", "cron": "0 9 * * *"});
    let changed = http(
        &w,
        Method::PUT,
        &format!("/api/watches/{}", edited.id),
        Some(to_cron),
        &w.alice,
    )
    .await?;
    assert_eq!(changed.status, StatusCode::OK, "{}", changed.body);
    assert_eq!(
        w.tick().await?,
        0,
        "09:00 passed before the change: tomorrow"
    );

    // Resuming an active watch changes nothing.
    let before = w.watch(&edited.id).await?;
    let resumed = http(
        &w,
        Method::POST,
        &format!("/api/watches/{}/resume", edited.id),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(resumed.status, StatusCode::OK, "{}", resumed.body);
    let after = w.watch(&edited.id).await?;
    assert_eq!(
        (after.since, after.state.last_started_at),
        (before.since, before.state.last_started_at)
    );
    Ok(())
}

/// Review round 1 of PR #1722: `watch_update` keeps what the call leaves out, and `paused` must be
/// a boolean.
#[tokio::test]
async fn watch_update_keeps_what_it_is_not_given() -> TestResult {
    let w = world().await?;
    let agent = token(&w, "operate").await?;
    let project = "acme/app";
    let (error, text, _) = tool(
        &w,
        &agent,
        "watch_create",
        serde_json::json!({"project": project, "name": "kept", "cron": "0 9 * * *", "utc_offset_minutes": 60,
            "expiry": "never", "timeout_seconds": 60, "secrets": ["API_KEY"], "script": "echo one"}),
    )
    .await?;
    assert!(!error, "{text}");
    let (error, text, updated) = tool(
        &w,
        &agent,
        "watch_update",
        serde_json::json!({"project": project, "watch": "kept", "script": "echo two"}),
    )
    .await?;
    assert!(!error, "{text}");
    assert_eq!(updated["script"], "echo two");
    assert_eq!(
        (
            &updated["cron"],
            &updated["utc_offset_minutes"],
            &updated["expiry"],
            &updated["timeout_seconds"],
            &updated["secrets"]
        ),
        (
            &serde_json::json!("0 9 * * *"),
            &serde_json::json!(60),
            &serde_json::json!("never"),
            &serde_json::json!(60),
            &serde_json::json!(["API_KEY"])
        )
    );
    let (_, _, switched) = tool(
        &w,
        &agent,
        "watch_update",
        serde_json::json!({"project": project, "watch": "kept", "every": "2h"}),
    )
    .await?;
    assert_eq!(
        (&switched["every"], &switched["cron"], &switched["script"]),
        (
            &serde_json::json!("2h"),
            &serde_json::Value::Null,
            &serde_json::json!("echo two")
        )
    );
    let (error, text, _) = tool(
        &w,
        &agent,
        "watch_pause",
        serde_json::json!({"project": project, "watch": "kept", "paused": "false"}),
    )
    .await?;
    assert!(error && text.contains("paused"), "{text}");
    Ok(())
}
