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
}

/// Project s1 (`app`), which alice writes; the real process host; small run bounds where a test
/// asks for them.
async fn world_with(
    host: Option<Arc<dyn yulius_app::ports::session::SessionHost>>,
    bounds: Option<RunBounds>,
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
            if let Some(bounds) = bounds {
                config.watches.bounds = bounds;
            }
        })),
        ..HarnessOptions::default()
    })?;
    h.clock.0.store(EIGHT_AM, Ordering::SeqCst);
    support::setup_superadmin_creds(&h).await?;
    support::configure_github_app(&h).await?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    h.github.set_perm("alice", "acme/app", perms("write"));
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    Ok(World { h, alice })
}

async fn world() -> Result<World, Error> {
    world_with(None, None, false).await
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
        let watch = self.watch(id).await?;
        assert!(self.h.app.watch_run(watch).await, "a run started");
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
    let script = counting("*) echo \"run $n\" ;;");
    let watch =
        w.h.app
            .watch_create(new_watch("survivor", &script)?)
            .await?;
    w.run(&watch.id).await?;
    w.advance(MINUTE);
    // The old process claimed a run and died before it ended.
    assert!(
        w.h.store
            .watch_run_started(&watch.id, "wr_lost", w.h.app.now())
            .await?
    );

    let after = Arc::new(App::new(w.h.app.config.clone(), w.h.app.deps.clone()));
    after.watch_startup().await;
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
        [("watch:survivor", "survivor: run 2")]
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
    let w = world_with(None, Some(bounds), false).await?;
    let failure = |watch: &WatchRecord| watch.state.last_failure.clone().unwrap_or_default();

    let busy =
        w.h.app
            .watch_create(new_watch("busy", "while :; do :; done")?)
            .await?;
    let started = Instant::now();
    let ended = w.run(&busy.id).await?;
    assert!(
        failure(&ended).contains("CPU time of 1 s"),
        "{}",
        failure(&ended)
    );
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
    let disk = w
        .h
        .app
        .watch_create(new_watch(
            "disk",
            "head -c 1000000 /dev/zero > \"$YULIUS_WATCH_STATE/big\"; echo written",
        )?)
        .await?;
    let ended = w.run(&disk.id).await?;
    assert!(ended.state.last_errors.is_some_and(|e| e.contains("File too large")));
    let big = w.h.app.config.data_dir.join("watches/s1/state").join(&disk.id).join("big");
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
    assert!(!posted.iter().any(|(key, _, _)| key == "watch:sly"), "{posted:?}");
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
    let w = world_with(Some(recording.clone()), None, true).await?;
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
