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

use yulius_domain::task::TaskStatus;

use crate::support::{
    harness, perms, seed_github_user, seed_live_attempt, seed_sandbox, seed_task,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
/// An error that crosses `spawn_blocking`.
type SendError = Box<dyn std::error::Error + Send + Sync>;

/// The run's task token.
const TOKEN: &str = "tt";
const YULIUS: &str = env!("CARGO_BIN_EXE_yulius");

/// Serves `app`'s internal listener on `port` from `runtime`; returns once it listens.
fn serve_on(runtime: &tokio::runtime::Handle, app: Arc<App>, port: u16) {
    let (bound, listening) = mpsc::channel();
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
        let _ = bound.send(());
        let router = yulius_server::internal_router(app)
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))));
        let _ = axum::serve(listener, router).await;
    });
    // Waited for on this thread: the server runs on the runtime's workers.
    let _ = listening.recv_timeout(Duration::from_secs(15));
}

/// The session's `yulius mcp`, with its answers as lines.
struct Bridge {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl Bridge {
    fn spawn(mut command: Command) -> Result<Self, Box<dyn std::error::Error>> {
        let mut child = command
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

/// The environment of the process `pid` (the run's agent): what Claude Code passes its
/// stdio MCP servers and Bash commands.
fn environ(pid: u32) -> Result<Vec<(String, String)>, SendError> {
    let raw = std::fs::read(format!("/proc/{pid}/environ"))?;
    Ok(raw
        .split(|b| *b == 0)
        .filter_map(|entry| {
            let entry = String::from_utf8_lossy(entry);
            entry
                .split_once('=')
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
        })
        .collect())
}

fn value(env: &[(String, String)], key: &str) -> Result<String, SendError> {
    env.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
        .ok_or_else(|| format!("{key} is not in the run's environment").into())
}

fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// `command` as Claude Code runs a stdio MCP server or a Bash command in the run: through its
/// `CLAUDE_CODE_SHELL_PREFIX`, with the run's environment.
fn in_run(env: &[(String, String)], command: &str) -> Result<Command, SendError> {
    let mut process = Command::new(value(env, "CLAUDE_CODE_SHELL_PREFIX")?);
    process.arg(command).env_clear().envs(env.iter().cloned());
    Ok(process)
}

/// The output of `command` run as a Bash command of the run.
fn bash(env: &[(String, String)], command: &str) -> Result<String, SendError> {
    let output = in_run(env, command)?.stdin(Stdio::null()).output()?;
    assert!(output.status.success(), "{command}: {output:?}");
    Ok(String::from_utf8(output.stdout)?)
}

/// MSB-001, MSB-002 and the acceptance of task #343: a session running when Yulius restarts
/// calls Yulius tools, git and `gh` after the restart, through the same bridge process and with no
/// `/mcp`. The session is a real reattached run (detached ptyhost, fake agent), and its bridge,
/// git and `gh` are spawned from the run's own MCP config and environment, through its shell
/// prefix, as Claude Code 2.1.292 spawns them.
#[test]
fn a_running_session_uses_mcp_git_and_gh_across_a_restart() -> TestResult {
    use crate::pool_supervision::{
        ScriptedUsage, build_served, new_task, runtime, screen, seed, stop, temp, wait_for,
    };
    use crate::support::FakeGitHub;

    use yulius_ptyhost::test_support::locate_fake_agent;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let port = std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port();
    let url = format!("http://127.0.0.1:{port}");
    let github = Arc::new(FakeGitHub::default());
    github.set_perm("admin", "acme/pool", perms("write"));
    let agent = || crate::support::agents::FakeAgentOptions {
        transcript: true,
        answers_first_message: true,
        ..crate::support::agents::FakeAgentOptions::default()
    };
    let build = || {
        build_served(
            &data,
            &fake,
            Arc::new(ScriptedUsage::default()),
            agent(),
            github.clone(),
            &url,
        )
    };

    // Yulius runs the task; its session starts.
    let first = runtime()?;
    let app = first.block_on(async { build() })?;
    serve_on(first.handle(), app.clone(), port);
    let (id, env, mcp) = first.block_on(async {
        app.deps
            .store
            .init_instance("https://yulius.test", "x")
            .await?;
        let cfg = json!({"app_id": 1, "slug": "yulius-test", "client_id": "c",
            "client_secret": "s", "private_key_pem": "k", "webhook_secret": null});
        let sealed = app.deps.crypto.seal(cfg.to_string().as_bytes())?;
        app.deps
            .store
            .set_github_app(&sealed, "acme", "org")
            .await?;
        app.deps.store.set_installation(Some(7)).await?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        let id = new_task(&app, &sandbox, &user, "Restart").await?;
        wait_for("the first answer", Duration::from_secs(30), || {
            let shown = screen(&app, &id);
            async move { shown.contains("ECHO: Restart: answer with an echo") }
        })
        .await?;
        let attempt = app.deps.store.live_attempt(&id).await?.ok_or("attempt")?;
        let pid = attempt.child_pid.ok_or("no agent pid")?;
        let env = environ(pid).map_err(|e| e.to_string())?;
        let mcp: Value = serde_json::from_slice(&std::fs::read(
            std::path::Path::new(&attempt.run_dir).join("mcp.json"),
        )?)?;
        Ok::<_, Box<dyn std::error::Error>>((id, env, mcp))
    })?;
    let token = value(&env, "YULIUS_TASK_TOKEN").map_err(|e| e.to_string())?;

    // Claude Code starts the Yulius MCP as the run's config says, through the shell prefix.
    let yulius = &mcp["mcpServers"]["yulius"];
    assert_eq!(yulius["type"], "stdio", "{mcp}");
    let mut line = vec![quote(yulius["command"].as_str().ok_or("command")?)];
    for arg in yulius["args"].as_array().ok_or("args")? {
        line.push(quote(arg.as_str().ok_or("arg")?));
    }
    let mut bridge = Bridge::spawn(in_run(&env, &line.join(" ")).map_err(|e| e.to_string())?)?;
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

    // Yulius goes down: its process and every connection with it. The run lives on.
    first.shutdown_timeout(Duration::from_secs(5));
    bridge.send(&call(3, "steps_get"))?;
    assert!(
        bridge
            .lines
            .recv_timeout(Duration::from_millis(500))
            .is_err(),
        "a call made while Yulius is down is held, not failed"
    );

    // Yulius is back: a new process on the same data and port reattaches the run.
    let second = runtime()?;
    let after = second.block_on(async {
        let after = build()?;
        after.reconcile().await?;
        Ok::<_, Box<dyn std::error::Error>>(after)
    })?;
    serve_on(second.handle(), after.clone(), port);
    let held = bridge
        .answer(Duration::from_secs(20))
        .map_err(|e| e.to_string())?;
    assert_eq!(held["id"], 3, "{held}");
    assert_eq!(held["result"]["isError"], false, "{held}");
    bridge.send(&call(4, "delivery_status"))?;
    let status = bridge
        .answer(Duration::from_secs(20))
        .map_err(|e| e.to_string())?;
    assert_eq!(status["result"]["isError"], false, "{status}");

    // git and gh, as the agent's Bash tool runs them.
    let git = bash(
        &env,
        "printf 'protocol=https\\nhost=github.com\\npath=acme/pool.git\\n\\n' | git credential fill",
    )
    .map_err(|e| e.to_string())?;
    assert!(git.contains("password="), "{git}");
    let gh = bash(&env, &format!("{} gh-token", quote(YULIUS))).map_err(|e| e.to_string())?;
    assert!(!gh.trim().is_empty(), "gh got no token");
    second.block_on(async {
        // The egress proxy still knows the run by its token.
        assert!(after.egress_client(&token).await.is_some());
        assert_eq!(
            after.deps.store.attempts(&id).await?.len(),
            1,
            "the same run"
        );
        stop(&after, &id).await
    })?;
    second.shutdown_timeout(Duration::from_secs(5));
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

/// Runs `yulius wait` at `port` with `total` and the session's token; its code, output and time.
fn wait_at(port: u16, total: Duration) -> (i32, String, Duration) {
    let mut out = Shared::default();
    let env = move |name: &str| match name {
        "YULIUS_URL" => Some(format!("http://127.0.0.1:{port}")),
        "YULIUS_TASK_TOKEN" => Some(TOKEN.to_owned()),
        _ => None,
    };
    let pace = yulius::wait_cli::Pace {
        total,
        request_wait_secs: 1,
        backoff_cap: Duration::from_millis(50),
    };
    let began = Instant::now();
    let code = yulius::wait_cli::run_with(&mut out, Some(&"ab".repeat(20)), pace, &env);
    (code, out.text(), began.elapsed())
}

/// MSB-005: a wait whose deadline passes while Yulius is down ends 1, saying so.
#[test]
fn a_wait_ends_at_its_deadline_while_yulius_is_down() -> TestResult {
    let port = std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port();
    let (code, said, _) = wait_at(port, Duration::from_secs(1));
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("could not be reached for"), "{said}");
    Ok(())
}

/// MSB-005: a refused token ends the wait at once, outage or not.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_token_ends_the_wait_at_once() -> TestResult {
    let h = harness()?;
    let port = std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port();
    serve_on(&tokio::runtime::Handle::current(), h.app.clone(), port);
    let (code, said, took) =
        tokio::task::spawn_blocking(move || wait_at(port, Duration::from_secs(30))).await?;
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("token was refused"), "{said}");
    assert!(took < Duration::from_secs(10), "{took:?}");
    Ok(())
}

/// Review round 1: an answer that is not an outage (a 500 every time) ends the wait after a few
/// tries, not at its deadline as if Yulius were restarting.
#[test]
fn a_server_failing_every_request_ends_the_wait_after_a_few_tries() -> TestResult {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut buffer = [0_u8; 4096];
            let _ = std::io::Read::read(&mut stream, &mut buffer);
            let _ = stream.write_all(
                b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    let (code, said, took) = wait_at(port, Duration::from_secs(60));
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("HTTP 500"), "{said}");
    assert!(!said.contains("cannot be reached"), "not an outage: {said}");
    assert!(took < Duration::from_secs(20), "{took:?}");
    Ok(())
}
