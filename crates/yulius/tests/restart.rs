//! SPEC-358: a running task session keeps Yulius across a server restart. Its MCP bridge
//! (`yulius mcp`), git credential helper, `gh` token and egress proxy work afterwards with no
//! manual step. The first server runs in a runtime of its own, dropped as a dying process drops
//! its connections; the second is a new `App` on the same store, listening on the same port.

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use axum::extract::connect_info::MockConnectInfo;
use serde_json::{Value, json};
use yulius_app::context::App;
use yulius_app::ports::egress::EgressPolicy as _;
use yulius_app::ports::store::InstanceStore as _;
use yulius_domain::task::TaskStatus;

use crate::support::{
    configure_github_app, harness, perms, seed_github_user, seed_live_attempt, seed_sandbox,
    seed_task,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
/// An error that crosses `spawn_blocking`.
type SendError = Box<dyn std::error::Error + Send + Sync>;

/// The run's task token.
const TOKEN: &str = "tt";
const YULIUS: &str = env!("CARGO_BIN_EXE_yulius");

/// Serves `app`'s internal listener on `port` from `runtime`.
fn serve_on(runtime: &tokio::runtime::Handle, app: Arc<App>, port: u16) {
    runtime.spawn(async move {
        // The port of the server before may still be closing.
        let deadline = Instant::now() + Duration::from_secs(10);
        let listener = loop {
            match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                Ok(listener) => break listener,
                Err(_) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(err) => panic!("port {port} stays taken: {err}"),
            }
        };
        let router = yulius_server::internal_router(app)
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))));
        let _ = axum::serve(listener, router).await;
    });
}

/// The environment a task run's processes get.
fn session_env(command: &mut Command, port: u16) -> &mut Command {
    command
        .env_clear()
        .env("YULIUS_URL", format!("http://127.0.0.1:{port}"))
        .env("YULIUS_TASK_TOKEN", TOKEN)
}

/// `yulius mcp` as Claude Code runs it, with its answers as lines.
struct Bridge {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl Bridge {
    fn start(port: u16) -> Result<Self, Box<dyn std::error::Error>> {
        let mut child = session_env(&mut Command::new(YULIUS), port)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = child.stdin.take().ok_or("stdin")?;
        let stdout = child.stdout.take().ok_or("stdout")?;
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            lines,
        })
    }

    fn send(&mut self, message: &Value) -> TestResult {
        writeln!(self.stdin, "{message}")?;
        Ok(self.stdin.flush()?)
    }

    fn answer(&self, within: Duration) -> Result<Value, SendError> {
        Ok(serde_json::from_str(&self.lines.recv_timeout(within)?)?)
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn call(id: u64, tool: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
           "params": {"name": tool, "arguments": {}}})
}

/// Runs a credential command of the session (`git-credential get`, `gh-token`); its output.
fn credential(port: u16, args: &[&str], stdin: &str) -> Result<String, SendError> {
    let mut child = session_env(&mut Command::new(YULIUS), port)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("stdin")?
        .write_all(stdin.as_bytes())?;
    let output = child.wait_with_output()?;
    assert!(output.status.success(), "{args:?}: {:?}", output.status);
    Ok(String::from_utf8(output.stdout)?)
}

/// MSB-001, MSB-002 and the acceptance of task #343: a session started before a restart calls
/// Yulius tools, git and `gh` after it, through the same bridge process and with no `/mcp`.
#[tokio::test(flavor = "multi_thread")]
async fn a_running_session_uses_mcp_git_and_gh_across_a_restart() -> TestResult {
    let h = harness()?;
    h.store.init_instance("https://yulius.test", "x").await?;
    configure_github_app(&h).await?;
    h.store.set_installation(Some(7)).await?;
    seed_github_user(&h, "u-a", "alice").await?;
    seed_sandbox(&h, "s1", "repo", "u-a").await?;
    h.github.set_perm("alice", "acme/repo", perms("write"));
    seed_task(&h, "t1", "s1", "u-a", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t1", TOKEN).await?;
    let port = std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port();

    let before = tokio::runtime::Runtime::new()?;
    serve_on(before.handle(), h.app.clone(), port);
    let mut bridge = Bridge::start(port)?;
    bridge.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "claude-code", "version": "2.1.292"}}}))?;
    let init = bridge
        .answer(Duration::from_secs(20))
        .map_err(|e| e.to_string())?;
    assert_eq!(init["result"]["serverInfo"]["name"], "yulius", "{init}");
    bridge.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
    bridge.send(&call(2, "steps_get"))?;
    let steps = bridge
        .answer(Duration::from_secs(20))
        .map_err(|e| e.to_string())?;
    assert_eq!(steps["result"]["isError"], false, "{steps}");

    // Yulius goes down: its process and every connection with it.
    before.shutdown_background();
    bridge.send(&call(3, "steps_get"))?;
    assert!(
        bridge
            .lines
            .recv_timeout(Duration::from_millis(500))
            .is_err(),
        "a call made while Yulius is down is held, not failed"
    );

    // Yulius is back: a new process on the same store and port.
    let after = App::new(h.app.config.clone(), h.app.deps.clone());
    serve_on(&tokio::runtime::Handle::current(), after.clone(), port);
    let held = tokio::task::spawn_blocking(move || {
        let answer = bridge.answer(Duration::from_secs(20));
        (bridge, answer)
    })
    .await?;
    let (mut bridge, held) = (held.0, held.1.map_err(|e| e.to_string())?);
    assert_eq!(held["id"], 3, "{held}");
    assert_eq!(held["result"]["isError"], false, "{held}");
    bridge.send(&call(4, "delivery_status"))?;
    let status = tokio::task::spawn_blocking(move || bridge.answer(Duration::from_secs(20)))
        .await?
        .map_err(|e| e.to_string())?;
    assert_eq!(status["result"]["isError"], false, "{status}");

    // git and gh, as the session's helpers ask for them.
    let git = tokio::task::spawn_blocking(move || {
        credential(
            port,
            &["git-credential", "get"],
            "protocol=https\nhost=github.com\npath=acme/repo.git\n\n",
        )
    })
    .await?
    .map_err(|e| e.to_string())?;
    assert!(git.contains("password="), "{git}");
    let gh = tokio::task::spawn_blocking(move || credential(port, &["gh-token"], ""))
        .await?
        .map_err(|e| e.to_string())?;
    assert!(!gh.trim().is_empty(), "gh got no token");
    // The egress proxy still knows the run by its token.
    assert!(after.egress_client(TOKEN).await.is_some());
    Ok(())
}

/// What a writer was given, readable while it is written to.
#[derive(Clone, Default)]
struct Shared(Arc<std::sync::Mutex<Vec<u8>>>);

impl Shared {
    fn text(&self) -> String {
        self.0
            .lock()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default()
    }
}

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut bytes) = self.0.lock() {
            bytes.extend_from_slice(buf);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// MSB-005: a `yulius wait` running when Yulius goes down keeps waiting through an outage far
/// longer than its old 5 tries, and goes on once Yulius is back.
#[tokio::test(flavor = "multi_thread")]
async fn a_wait_rides_out_a_restart() -> TestResult {
    let h = harness()?;
    seed_github_user(&h, "u-a", "alice").await?;
    seed_sandbox(&h, "s1", "repo", "u-a").await?;
    seed_task(&h, "t1", "s1", "u-a", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t1", TOKEN).await?;
    let port = std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port();
    let before = tokio::runtime::Runtime::new()?;
    serve_on(before.handle(), h.app.clone(), port);

    let out = Shared::default();
    let mut writer = out.clone();
    let wait = tokio::task::spawn_blocking(move || {
        let env = move |name: &str| match name {
            "YULIUS_URL" => Some(format!("http://127.0.0.1:{port}")),
            "YULIUS_TASK_TOKEN" => Some(TOKEN.to_owned()),
            _ => None,
        };
        let pace = yulius::wait_cli::Pace {
            total: Duration::from_secs(8),
            request_wait_secs: 1,
            backoff_cap: Duration::from_millis(50),
        };
        yulius::wait_cli::run_with(&mut writer, Some(&"ab".repeat(20)), pace, &env)
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while !out.text().contains("Waiting:") {
        assert!(
            Instant::now() < deadline,
            "the wait never started: {}",
            out.text()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    before.shutdown_background();
    // Down long enough for dozens of tries at this pace.
    while !out.text().contains("cannot be reached") {
        assert!(
            Instant::now() < deadline,
            "the outage was not seen: {}",
            out.text()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let after = App::new(h.app.config.clone(), h.app.deps.clone());
    serve_on(&tokio::runtime::Handle::current(), after, port);
    let code = wait.await?;
    let said = out.text();
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("Yulius is back."), "{said}");
    assert!(said.contains("Still waiting after"), "{said}");
    assert_eq!(
        said.matches("cannot be reached").count(),
        1,
        "said once: {said}"
    );
    Ok(())
}

/// Appends to the transcript of `id`'s session a turn that closed at `at` while Yulius was down:
/// what a dead `yulius wait` leaves (its exit woke the agent, which could do nothing and ended
/// the turn), with no Stop hook Yulius answered.
async fn turn_closed_while_down(
    app: &App,
    data: &std::path::Path,
    id: &yulius_domain::ids::TaskId,
    at: i64,
) -> TestResult {
    use yulius_app::ports::harness::HarnessPort as _;

    let record = app.deps.store.task(id).await?.ok_or("task")?;
    let attempt = app.deps.store.live_attempt(id).await?.ok_or("attempt")?;
    let session = attempt.provider_session_id.ok_or("session")?;
    let worktree = std::path::PathBuf::from(record.worktree_path.ok_or("worktree")?);
    let home = data.join("sandboxes").join("s-pool").join("home");
    let adapter =
        yulius::wiring::harness::HarnessAdapter::new(std::path::PathBuf::from(YULIUS), "x");
    let transcript = adapter.claude_transcript(&home, &worktree, &session);
    let time = yulius_app::auth::rfc3339(yulius_domain::time::Timestamp(at));
    let envelope = |kind: &str, uuid: &str| {
        json!({"type": kind, "uuid": uuid, "timestamp": time, "sessionId": session,
               "isSidechain": false, "userType": "external", "entrypoint": "cli"})
    };
    let mut user = envelope("user", "outage-u");
    user["message"] = json!({"role": "user", "content":
        "<task-notification>yulius wait: Yulius cannot be reached</task-notification>"});
    let mut answer = envelope("assistant", "outage-a");
    answer["message"] = json!({"id": "msg_outage", "type": "message", "role": "assistant",
        "model": "fake-model", "content": [{"type": "text", "text": "Yulius is down; I wait."}],
        "stop_reason": "end_turn", "usage": {"input_tokens": 1, "output_tokens": 1,
        "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}});
    let mut closed = envelope("system", "outage-d");
    closed["subtype"] = json!("turn_duration");
    closed["durationMs"] = json!(1000);
    let mut lines = std::fs::read_to_string(&transcript)?;
    for line in [user, answer, closed] {
        lines.push_str(&format!("{line}\n"));
    }
    std::fs::write(&transcript, lines)?;
    // Claude Code's SessionStart hook reached Yulius before the outage (the fake agent here runs
    // no hooks): the restarted server knows its prompt box takes text.
    std::fs::write(
        std::path::Path::new(&attempt.run_dir).join("session-started"),
        b"",
    )?;
    Ok(())
}

/// MSB-007: after a restart, the task whose turn closed while Yulius was down is told "Yulius is
/// back; continue" once; a task idle since before the outage is not.
#[test]
fn a_task_that_went_idle_during_the_outage_is_woken_once() -> TestResult {
    use crate::pool_supervision::{
        ScriptedUsage, build_with, new_task, runtime, screen, seed, stop, temp, wait_for,
    };
    use yulius_ptyhost::test_support::locate_fake_agent;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let agent = || crate::support::agents::FakeAgentOptions {
        transcript: true,
        answers_first_message: true,
        ..crate::support::agents::FakeAgentOptions::default()
    };

    // Two tasks answer their first message; then Yulius goes down.
    let first = runtime()?;
    let (cut, quiet, down_since) = first.block_on(async {
        let app = build_with(&data, &fake, Arc::new(ScriptedUsage::default()), agent())?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        let cut = new_task(&app, &sandbox, &user, "Cut").await?;
        let quiet = new_task(&app, &sandbox, &user, "Quiet").await?;
        for (id, title) in [(&cut, "Cut"), (&quiet, "Quiet")] {
            wait_for("the first answer", Duration::from_secs(30), || {
                let shown = screen(&app, id);
                async move { shown.contains(&format!("ECHO: {title}: answer with an echo")) }
            })
            .await?;
        }
        Ok::<_, Box<dyn std::error::Error>>((cut, quiet, app.now()))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let rt = runtime()?;
    rt.block_on(async {
        let app = build_with(&data, &fake, Arc::new(ScriptedUsage::default()), agent())?;
        // While Yulius was down, the cut task's wait died and its agent ended a turn.
        // Transcript times are whole seconds: the first one after Yulius went down.
        let closed = (down_since.0 / 1000 + 1) * 1000;
        turn_closed_while_down(&app, &data, &cut, closed).await?;
        while app.now().0 <= closed {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let started = app.now();
        app.reconcile().await?;
        // Each pass wakes what has replayed so far: it ends once the cut task is woken.
        let deadline = Instant::now() + Duration::from_secs(30);
        let woken = loop {
            let woken = app.wake_after_outage(down_since, started).await;
            if !woken.is_empty() || Instant::now() >= deadline {
                break woken;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        assert_eq!(woken, vec![cut.clone()], "only the task cut by the outage");
        wait_for("the wake-up typed", Duration::from_secs(30), || {
            let shown = screen(&app, &cut);
            async move { shown.contains("ECHO: Yulius is back; continue") }
        })
        .await?;
        // Once: its woken turn closed after this start.
        assert!(app.wake_after_outage(down_since, started).await.is_empty());
        assert!(
            !screen(&app, &quiet).contains("Yulius is back"),
            "{}",
            screen(&app, &quiet)
        );
        stop(&app, &cut).await?;
        stop(&app, &quiet).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
