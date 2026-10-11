//! ptyhost supervision evidence with the fake agent (SPEC-002 §7.1, §10; AC-NFR-1/2/4/5,
//! AC-TSK-3/6). Every test spawns the real `yulius-ptyhost` binary through `spawn_host`.

use std::collections::BTreeMap;
use std::error::Error;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use yulius_contracts::ptyhost::{
    BubblewrapSpec, ClientFrame, Containment, EXIT_FILE, ExitFile, ExitReason, ExitRecord,
    FrameDecoder, HostFrame, LAUNCH_FILE, LaunchSpec, PROTOCOL_VERSION, RECORD_VERSION,
    SOCKET_FILE, STATUS_FILE, StatusRecord, Welcome, encode_client, error_code,
};
use yulius_ptyhost::test_support::locate_fake_agent;
use yulius_ptyhost::{
    ClientError, PtyClient, RunDirState, SpawnError, SpawnedHost, current_boot_id, inspect_run_dir,
    spawn_host, terminate_recorded, token_sha256_hex,
};

type TestResult = Result<(), Box<dyn Error>>;
type Res<T> = Result<T, Box<dyn Error>>;

const HOST_EXE: &str = env!("CARGO_BIN_EXE_yulius-ptyhost");
const TOKEN: [u8; 32] = [7; 32];
const WAIT: Duration = Duration::from_secs(20);

fn fake_agent() -> Res<PathBuf> {
    locate_fake_agent().ok_or_else(|| {
        "fake-agent binary not found next to the test executable; \
         build it with `cargo build -p fake-agent` or run the workspace tests"
            .into()
    })
}

async fn within<T>(what: &str, future: impl Future<Output = T>) -> Res<T> {
    tokio::time::timeout(WAIT, future)
        .await
        .map_err(|_| format!("timed out waiting for {what}").into())
}

struct Run {
    _dir: tempfile::TempDir,
    run_dir: PathBuf,
    host: SpawnedHost,
}

fn spec(args: &[&str]) -> Res<LaunchSpec> {
    let mut argv = vec![fake_agent()?.to_string_lossy().into_owned()];
    argv.extend(args.iter().map(|a| (*a).to_owned()));
    Ok(LaunchSpec {
        version: RECORD_VERSION,
        attempt_id: "att-1".into(),
        task_id: "task-1".into(),
        argv,
        cwd: std::env::temp_dir().to_string_lossy().into_owned(),
        env: BTreeMap::from([
            ("TERM".into(), "xterm-256color".into()),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ]),
        secret_env_names: Vec::new(),
        cols: 80,
        rows: 24,
        containment: Containment::None,
        ring_bytes: 0,
        token_sha256: token_sha256_hex(&TOKEN),
        linger_ms: 2_000,
    })
}

async fn start_with(launch: &LaunchSpec, secrets: &[(String, String)]) -> Res<Run> {
    let dir = tempfile::tempdir()?;
    let run_dir = dir.path().join("run");
    let host = spawn_host(
        Path::new(HOST_EXE),
        &run_dir,
        launch,
        secrets,
        Duration::from_secs(10),
    )
    .await?;
    Ok(Run {
        _dir: dir,
        run_dir,
        host,
    })
}

async fn start(args: &[&str]) -> Res<Run> {
    start_with(&spec(args)?, &[]).await
}

/// A controlling connection plus a local vt100 replica fed with Snapshot and Output frames.
struct Session {
    client: PtyClient,
    welcome: Welcome,
    parser: vt100::Parser,
    last_seq: u64,
    text: String,
    checked: usize,
    exits: Vec<ExitRecord>,
    errors: Vec<(u16, String)>,
    pongs: Vec<u64>,
    closed: bool,
}

impl Session {
    async fn connect(run_dir: &Path) -> Res<Self> {
        let (client, welcome, snapshot) = PtyClient::connect(run_dir, &TOKEN, true).await?;
        let snapshot = snapshot.ok_or("snapshot missing")?;
        assert_eq!(
            snapshot.seq, welcome.seq,
            "snapshot and welcome agree on seq"
        );
        let mut parser = vt100::Parser::new(welcome.rows, welcome.cols, 0);
        parser.process(&snapshot.bytes);
        Ok(Self {
            client,
            last_seq: welcome.seq,
            welcome,
            parser,
            text: String::new(),
            checked: 0,
            exits: Vec::new(),
            errors: Vec::new(),
            pongs: Vec::new(),
            closed: false,
        })
    }

    fn screen(&self) -> String {
        self.parser.screen().contents()
    }

    /// Reads one frame; Output must continue the sequence without gaps.
    async fn step(&mut self) -> Res<()> {
        match within("a frame", self.client.recv()).await?? {
            Some(HostFrame::Output { seq, bytes }) => {
                assert_eq!(
                    seq,
                    self.last_seq + 1,
                    "output seq is gap-free and monotonic"
                );
                self.last_seq = seq;
                self.parser.process(&bytes);
                self.text.push_str(&String::from_utf8_lossy(&bytes));
                // Keep only a tail so memory stays bounded during floods.
                if self.text.len() > 128 * 1024 {
                    let mut cut = self.text.len() - 64 * 1024;
                    while !self.text.is_char_boundary(cut) {
                        cut += 1;
                    }
                    self.text.drain(..cut);
                    self.checked = self.checked.saturating_sub(cut);
                }
            }
            Some(HostFrame::Exit(record)) => self.exits.push(record),
            Some(HostFrame::Error { code, message }) => self.errors.push((code, message)),
            Some(HostFrame::Pong(nonce)) => self.pongs.push(nonce),
            Some(other) => return Err(format!("unexpected frame {other:?}").into()),
            None => self.closed = true,
        }
        Ok(())
    }

    /// Whether `needle` appeared in live output or is on the current screen.
    fn seen(&self, needle: &str) -> bool {
        self.text.contains(needle) || self.screen().contains(needle)
    }

    /// The rest of the first screen line starting with `prefix`.
    fn screen_line(&self, prefix: &str) -> Option<String> {
        self.screen()
            .lines()
            .find_map(|line| line.strip_prefix(prefix).map(|rest| rest.trim().to_owned()))
    }

    /// Searches only output that arrived since the last check (plus an overlap).
    fn seen_new(&mut self, needle: &str) -> bool {
        let mut start = self.checked.saturating_sub(needle.len());
        while !self.text.is_char_boundary(start) {
            start -= 1;
        }
        self.checked = self.text.len();
        self.text
            .get(start..)
            .is_some_and(|tail| tail.contains(needle))
    }

    async fn wait_for(&mut self, needle: &str) -> Res<()> {
        let deadline = Instant::now() + WAIT;
        if self.seen(needle) {
            self.checked = self.text.len();
            return Ok(());
        }
        while !self.seen_new(needle) {
            if self.closed || Instant::now() > deadline {
                return Err(
                    format!("{needle:?} never appeared; screen:\n{}", self.screen()).into(),
                );
            }
            self.step().await?;
        }
        Ok(())
    }

    async fn wait_closed(&mut self) -> Res<()> {
        while !self.closed {
            self.step().await?;
        }
        Ok(())
    }

    async fn line(&mut self, text: &str) -> Res<()> {
        self.client
            .send(ClientFrame::Input(format!("{text}\r").into_bytes()))
            .await?;
        Ok(())
    }
}

fn read_exit_file(run_dir: &Path) -> Res<ExitFile> {
    Ok(serde_json::from_slice(&std::fs::read(
        run_dir.join(EXIT_FILE),
    )?)?)
}

/// A process is gone when it no longer exists or only remains as a zombie.
fn gone(pid: i32) -> bool {
    if kill(Pid::from_raw(pid), None) == Err(Errno::ESRCH) {
        return true;
    }
    Command::new("/bin/ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().starts_with('Z'))
        .unwrap_or(false)
}

async fn wait_gone(pid: i32) -> Res<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !gone(pid) {
        if Instant::now() > deadline {
            return Err(format!("process {pid} is still alive").into());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(())
}

fn rss_kib(pid: u32) -> Option<u64> {
    let output = Command::new("/bin/ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

async fn raw_connect(run_dir: &Path) -> Res<UnixStream> {
    Ok(UnixStream::connect(run_dir.join(SOCKET_FILE)).await?)
}

/// Reads host frames until EOF, bounded by `limit`.
async fn frames_until_eof(stream: &mut UnixStream, limit: Duration) -> Res<Vec<HostFrame>> {
    let mut decoder = FrameDecoder::new();
    let mut frames = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    tokio::time::timeout(limit, async {
        loop {
            let n = stream.read(&mut buf).await.unwrap_or(0);
            if n == 0 {
                return Ok::<_, Box<dyn Error>>(());
            }
            decoder.push(&buf[..n]);
            while let Some(frame) = decoder.next_host()? {
                frames.push(frame);
            }
        }
    })
    .await
    .map_err(|_| "connection was not closed")??;
    Ok(frames)
}

fn hello(version: u16, token: [u8; 32], want_snapshot: bool) -> Vec<u8> {
    encode_client(&ClientFrame::Hello {
        version,
        token,
        want_snapshot,
    })
}

// 1 -------------------------------------------------------------------------------------------

#[tokio::test]
async fn echo_round_trip_welcome_snapshot_resize_and_clean_exit() -> TestResult {
    let mut run = start(&["--session-id", "s1"]).await?;
    let mut session = Session::connect(&run.run_dir).await?;
    assert_eq!(session.welcome.attempt_id, "att-1");
    assert_eq!(session.welcome.child_pid, run.host.status().child_pid);
    assert_eq!((session.welcome.cols, session.welcome.rows), (80, 24));
    assert_eq!(run.host.status().ptyhost_pid, run.host.host_pid());
    // Private run directory: 0700, records and socket 0600.
    let mode = |name: &str| -> Res<u32> {
        Ok(std::fs::metadata(run.run_dir.join(name))?
            .permissions()
            .mode()
            & 0o777)
    };
    assert_eq!(mode("")?, 0o700);
    for name in [LAUNCH_FILE, STATUS_FILE, SOCKET_FILE] {
        assert_eq!(mode(name)?, 0o600, "{name}");
    }

    session.wait_for("FAKE-AGENT READY session=s1").await?;
    session.line("hello world").await?;
    session.wait_for("ECHO: hello world").await?;

    session.client.send(ClientFrame::Ping(42)).await?;
    while session.pongs.is_empty() {
        session.step().await?;
    }
    assert_eq!(session.pongs, vec![42]);

    session
        .client
        .send(ClientFrame::Resize {
            cols: 100,
            rows: 30,
        })
        .await?;
    session.line("/size").await?;
    session.wait_for("SIZE 30 100").await?;

    // A fresh controlling connection sees the new size.
    drop(session);
    let mut session = Session::connect(&run.run_dir).await?;
    assert_eq!((session.welcome.cols, session.welcome.rows), (100, 30));

    // Leaving the agent ends the run (AC-TSK-3): exit 0, recorded once, delivered, host exits.
    session.line("/exit").await?;
    session.wait_closed().await?;
    assert_eq!(session.exits.len(), 1);
    let exit = &session.exits[0];
    assert_eq!(
        (exit.code, exit.signal, exit.reason),
        (Some(0), None, ExitReason::Exited)
    );
    assert_eq!(&read_exit_file(&run.run_dir)?.exit, exit);
    let host_exit = within("host exit", run.host.wait()).await?;
    assert_eq!(host_exit.code, Some(0));
    assert_eq!(
        std::fs::metadata(run.run_dir.join(EXIT_FILE))?
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(!run.run_dir.join(SOCKET_FILE).exists(), "socket removed");
    Ok(())
}

// 2 -------------------------------------------------------------------------------------------

#[tokio::test]
async fn reconnect_after_disconnect_shows_current_screen_and_session_survives() -> TestResult {
    let run = start(&[]).await?;
    let mut first = Session::connect(&run.run_dir).await?;
    first.wait_for("FAKE-AGENT READY").await?;
    first.line("hello").await?;
    first.wait_for("ECHO: hello\r\n").await?;
    let seen_screen = first.screen();
    let seen_cursor = first.parser.screen().cursor_position();
    let seen_seq = first.last_seq;
    drop(first);

    // Snapshot after reconnect equals the screen the previous client had (AC-NFR-1, NFR-004).
    let second = Session::connect(&run.run_dir).await?;
    assert_eq!(second.screen(), seen_screen);
    assert_eq!(second.parser.screen().cursor_position(), seen_cursor);
    assert_eq!(second.welcome.seq, seen_seq);

    // Output produced while nobody is connected shows up in the next snapshot (TSK-004).
    let mut second = second;
    second.line("/later 300 LATE-LINE").await?;
    second.wait_for("/later 300 LATE-LINE\r\n").await?;
    drop(second);
    let deadline = Instant::now() + WAIT;
    let mut third = loop {
        let candidate = Session::connect(&run.run_dir).await?;
        if candidate.screen().contains("LATE-LINE\n") || candidate.screen().ends_with("LATE-LINE") {
            break candidate;
        }
        if Instant::now() > deadline {
            return Err("LATE-LINE never reached the snapshot".into());
        }
        drop(candidate);
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(third.welcome.child_pid, run.host.status().child_pid);
    third.line("again").await?;
    third.wait_for("ECHO: again").await?;
    assert!(run.host.host_exit().is_none(), "host still running");
    Ok(())
}

// 3 -------------------------------------------------------------------------------------------

#[tokio::test]
async fn replacement_client_takes_over_and_old_client_is_closed() -> TestResult {
    let run = start(&[]).await?;
    let mut old = Session::connect(&run.run_dir).await?;
    old.wait_for("FAKE-AGENT READY").await?;

    let mut new = Session::connect(&run.run_dir).await?;
    old.wait_closed().await?;
    assert_eq!(
        old.errors.len(),
        1,
        "old client receives one Error then EOF"
    );
    assert_eq!(old.errors[0].0, error_code::REPLACED);

    new.line("from new").await?;
    new.wait_for("ECHO: from new").await?;
    // Input from the replaced connection cannot reach the agent any more.
    // The write may still land in the socket buffer; the host no longer reads it.
    let _ = old
        .client
        .send(ClientFrame::Input(b"from old\r".to_vec()))
        .await;
    new.line("marker").await?;
    new.wait_for("ECHO: marker").await?;
    assert!(!new.text.contains("from old"));
    Ok(())
}

// 4 -------------------------------------------------------------------------------------------

#[tokio::test]
async fn slow_client_is_dropped_while_output_continues_and_memory_stays_bounded() -> TestResult {
    const FLOOD: u64 = 10 << 20; // to stdout and to stderr: 20 MiB through the PTY
    let mut run = start(&[]).await?;
    let host_pid = run.host.host_pid();
    let baseline = rss_kib(host_pid).ok_or("no rss")?;

    // A client that authenticates, starts the flood and never reads.
    let mut slow = raw_connect(&run.run_dir).await?;
    slow.write_all(&hello(PROTOCOL_VERSION, TOKEN, true))
        .await?;
    slow.write_all(&encode_client(&ClientFrame::Input(
        format!("/flood {FLOOD}\r").into_bytes(),
    )))
    .await?;

    let flood_started = Instant::now();
    // Detect the drop without reading: once the host shut the socket, writes fail.
    let deadline = Instant::now() + Duration::from_secs(50);
    let mut peak = baseline;
    let mut nonce = 0;
    loop {
        peak = peak.max(rss_kib(host_pid).unwrap_or(0));
        nonce += 1;
        if slow
            .write_all(&encode_client(&ClientFrame::Ping(nonce)))
            .await
            .is_err()
        {
            break;
        }
        assert!(run.host.host_exit().is_none(), "host must keep running");
        if Instant::now() > deadline {
            return Err("slow client was never dropped".into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Only what was already in flight can still be read: far less than the flood.
    let mut received = 0usize;
    let mut buf = vec![0u8; 64 * 1024];
    while let Ok(Ok(n)) = tokio::time::timeout(Duration::from_secs(5), slow.read(&mut buf)).await {
        if n == 0 {
            break;
        }
        received += n;
    }
    eprintln!("slow client received {received} bytes; host RSS {baseline} KiB -> peak {peak} KiB");
    assert!(received < FLOOD as usize, "received {received} bytes");
    assert!(
        peak < baseline + 64 * 1024,
        "host RSS grew from {baseline} KiB to {peak} KiB"
    );

    // Reconnect: fresh snapshot, the flood completes, the session still answers (AC-NFR-4).
    // A reader slower than the flood may legitimately be cut off again; it just reconnects.
    let dropped_after = flood_started.elapsed();
    // Debug builds parse vt100 slowly (about 1-10 MiB/s depending on machine load), hence the
    // generous bound; release builds are far faster.
    tokio::time::timeout(Duration::from_secs(90), wait_flood_done(&run.run_dir))
        .await
        .map_err(|_| "flood never completed")??;
    eprintln!(
        "slow client dropped after {dropped_after:?}; flood done after {:?}",
        flood_started.elapsed()
    );
    let mut session = Session::connect(&run.run_dir).await?;
    session.line("after flood").await?;
    session.wait_for("ECHO: after flood").await?;
    assert!(run.host.host_exit().is_none());
    session.line("/exit").await?;
    session.wait_closed().await?;
    within("host exit", run.host.wait()).await?;
    Ok(())
}

/// Follows the session (reconnecting when dropped) until `FLOOD DONE` was printed.
async fn wait_flood_done(run_dir: &Path) -> TestResult {
    const MARK: &[u8] = b"FLOOD DONE";
    loop {
        let (mut client, welcome, snapshot) = PtyClient::connect(run_dir, &TOKEN, true).await?;
        let mut screen = vt100::Parser::new(welcome.rows, welcome.cols, 0);
        screen.process(&snapshot.ok_or("snapshot missing")?.bytes);
        if screen.screen().contents().contains("FLOOD DONE") {
            return Ok(());
        }
        let mut tail: Vec<u8> = Vec::new();
        while let Some(frame) = client.recv().await? {
            if let HostFrame::Output { bytes, .. } = frame {
                tail.extend_from_slice(&bytes);
                if tail.windows(MARK.len()).any(|w| w == MARK) {
                    return Ok(());
                }
                let keep = tail.len().saturating_sub(MARK.len());
                tail.drain(..keep);
            }
        }
    }
}

// 5 -------------------------------------------------------------------------------------------

#[tokio::test]
async fn shutdown_escalates_past_ignored_signals_and_reaps_descendants() -> TestResult {
    let mut run = start(&["--mode", "fork-child", "--ignore-term", "--seconds", "60"]).await?;
    let child_pid = i32::try_from(run.host.status().child_pid)?;
    let mut session = Session::connect(&run.run_dir).await?;
    session.wait_for("CHILD ").await?;
    // Wait until the whole line (and the echo loop) is up: the agent answers input next.
    session.line("ping").await?;
    session.wait_for("ECHO: ping").await?;
    let descendant: i32 = session
        .screen_line("CHILD ")
        .ok_or("no descendant pid")?
        .parse()?;
    assert!(!gone(descendant), "descendant running before shutdown");

    let grace = 300;
    let started = Instant::now();
    session
        .client
        .send(ClientFrame::Shutdown { grace_ms: grace })
        .await?;
    session.wait_closed().await?;
    let elapsed = started.elapsed();

    // Interrupt and SIGTERM were ignored, so only SIGKILL ended it (AC-NFR-5).
    assert_eq!(session.exits.len(), 1, "exactly one terminal outcome");
    let exit = &session.exits[0];
    assert_eq!(exit.reason, ExitReason::Signalled);
    assert_eq!(exit.signal, Some(9));
    assert!(elapsed >= Duration::from_millis(2 * u64::from(grace)));
    assert_eq!(&read_exit_file(&run.run_dir)?.exit, exit);
    assert!(!run.run_dir.join(".exit.json.tmp").exists());
    within("host exit", run.host.wait()).await?;
    wait_gone(child_pid).await?;
    wait_gone(descendant).await?;
    Ok(())
}

#[tokio::test]
async fn kill_signal_stops_the_agent_immediately() -> TestResult {
    let mut run = start(&["--mode", "ignore-term"]).await?;
    let mut session = Session::connect(&run.run_dir).await?;
    session.wait_for("FAKE-AGENT READY").await?;
    session
        .client
        .send(ClientFrame::Signal(
            yulius_contracts::ptyhost::SignalKind::Terminate,
        ))
        .await?;
    session.line("still here").await?;
    session.wait_for("ECHO: still here").await?;
    session
        .client
        .send(ClientFrame::Signal(
            yulius_contracts::ptyhost::SignalKind::Kill,
        ))
        .await?;
    session.wait_closed().await?;
    assert_eq!(session.exits.len(), 1);
    assert_eq!(session.exits[0].signal, Some(9));
    within("host exit", run.host.wait()).await?;
    Ok(())
}

// 6 -------------------------------------------------------------------------------------------

#[tokio::test]
async fn early_exit_is_recorded_delivered_and_ends_the_host_after_delivery() -> TestResult {
    let mut launch = spec(&["--mode", "exit-early", "--code", "7"])?;
    launch.linger_ms = 20_000;
    let mut run = start_with(&launch, &[]).await?;
    let started = Instant::now();
    let mut session = Session::connect(&run.run_dir).await?;
    session.wait_closed().await?;
    assert_eq!(session.exits.len(), 1);
    let exit = &session.exits[0];
    assert_eq!((exit.code, exit.reason), (Some(7), ExitReason::Exited));
    assert_eq!(read_exit_file(&run.run_dir)?.exit.code, Some(7));
    // The host leaves as soon as the Exit was delivered, long before the 20 s linger.
    within("host exit", run.host.wait()).await?;
    assert!(started.elapsed() < Duration::from_secs(10));
    Ok(())
}

#[tokio::test]
async fn early_exit_without_client_lingers_then_exits() -> TestResult {
    let mut launch = spec(&["--mode", "exit-early", "--code", "5"])?;
    launch.linger_ms = 300;
    let mut run = start_with(&launch, &[]).await?;
    let host_exit = within("host exit", run.host.wait()).await?;
    assert_eq!(host_exit.code, Some(0));
    assert_eq!(read_exit_file(&run.run_dir)?.exit.code, Some(5));
    assert!(!run.run_dir.join(SOCKET_FILE).exists());
    match inspect_run_dir(&run.run_dir) {
        RunDirState::Exited(record) => assert_eq!(record.code, Some(5)),
        other => return Err(format!("expected Exited, got {other:?}").into()),
    }
    Ok(())
}

// 7 -------------------------------------------------------------------------------------------

async fn expect_spawn_failed(launch: &LaunchSpec, detail_part: &str) -> TestResult {
    let dir = tempfile::tempdir()?;
    let run_dir = dir.path().join("run");
    let result = spawn_host(
        Path::new(HOST_EXE),
        &run_dir,
        launch,
        &[],
        Duration::from_secs(10),
    )
    .await;
    match result {
        Err(SpawnError::HostFailed { exit: Some(exit) }) => {
            assert_eq!(exit.reason, ExitReason::SpawnFailed);
            let detail = exit.detail.unwrap_or_default();
            assert!(detail.contains(detail_part), "detail {detail:?}");
        }
        other => return Err(format!("expected HostFailed, got {other:?}").into()),
    }
    let recorded = read_exit_file(&run_dir)?;
    assert_eq!(recorded.exit.reason, ExitReason::SpawnFailed);
    assert!(!run_dir.join(STATUS_FILE).exists());
    assert!(!run_dir.join(SOCKET_FILE).exists());
    Ok(())
}

#[tokio::test]
async fn spawn_failures_are_recorded_with_a_diagnostic() -> TestResult {
    let mut missing = spec(&[])?;
    missing.argv = vec!["/nonexistent/agent-binary".into()];
    expect_spawn_failed(&missing, "/nonexistent/agent-binary").await?;

    let mut bad_cwd = spec(&[])?;
    bad_cwd.cwd = "/nonexistent/working-dir".into();
    expect_spawn_failed(&bad_cwd, "working directory").await?;

    if !cfg!(target_os = "linux") {
        let mut contained = spec(&[])?;
        contained.containment = Containment::Bubblewrap(BubblewrapSpec {
            sandbox_root: "/tmp/sandbox".into(),
            home: "/tmp/sandbox/home".into(),
            writable: Vec::new(),
            readonly: Vec::new(),
            share_net: true,
            binds: Vec::new(),
            identity: None,
            join: None,
        });
        expect_spawn_failed(&contained, "only supported on Linux").await?;
    }
    Ok(())
}

#[tokio::test]
async fn spawn_host_rejects_bad_inputs_before_starting() -> TestResult {
    let dir = tempfile::tempdir()?;
    let run_dir = dir.path().join("run");
    let launch = spec(&[])?;
    let missing_exe = spawn_host(
        Path::new("/nonexistent/yulius"),
        &run_dir,
        &launch,
        &[],
        Duration::from_secs(5),
    )
    .await;
    assert!(
        matches!(missing_exe, Err(SpawnError::Io { context, .. }) if context == "spawn ptyhost")
    );
    // launch.json was written, so the directory cannot be reused for another start.
    assert!(run_dir.join(LAUNCH_FILE).exists());
    let reused = spawn_host(Path::new(HOST_EXE), &run_dir, &launch, &[], WAIT).await;
    assert!(matches!(reused, Err(SpawnError::RunDirInUse)));

    let mut secret = spec(&[])?;
    secret.secret_env_names = vec!["FAKE_SECRET".into()];
    let other_dir = dir.path().join("other");
    let undeclared = spawn_host(Path::new(HOST_EXE), &other_dir, &secret, &[], WAIT).await;
    assert!(matches!(undeclared, Err(SpawnError::InvalidSecret { .. })));
    assert!(!other_dir.exists(), "nothing written for rejected input");
    Ok(())
}

#[tokio::test]
async fn secrets_reach_only_the_child_environment() -> TestResult {
    const VALUE: &str = "synthetic-secret-value-42";
    let mut launch = spec(&[
        "--report-env",
        "FAKE_SECRET",
        "--report-env",
        "HOME",
        "--report-env",
        "TERM",
    ])?;
    launch.secret_env_names = vec!["FAKE_SECRET".into()];
    let secrets = vec![("FAKE_SECRET".to_owned(), VALUE.to_owned())];
    let mut run = start_with(&launch, &secrets).await?;
    let mut session = Session::connect(&run.run_dir).await?;
    session.wait_for("ENV TERM=").await?;
    assert!(session.seen(&format!("ENV FAKE_SECRET=set len={}", VALUE.len())));
    // The child environment is exactly the launch env plus secrets: HOME is not inherited.
    assert!(session.seen("ENV HOME=unset"));
    assert!(!session.seen(VALUE));
    let argv = Command::new("/bin/ps")
        .args(["-o", "command=", "-p", &run.host.host_pid().to_string()])
        .output()?;
    assert!(!String::from_utf8_lossy(&argv.stdout).contains(VALUE));
    session.line("/exit").await?;
    session.wait_closed().await?;
    within("host exit", run.host.wait()).await?;
    for name in [LAUNCH_FILE, STATUS_FILE, EXIT_FILE] {
        let content = std::fs::read_to_string(run.run_dir.join(name))?;
        assert!(!content.contains(VALUE), "{name} leaks the secret");
    }
    Ok(())
}

// 8 -------------------------------------------------------------------------------------------

#[tokio::test]
async fn unauthenticated_connections_are_refused_without_disturbing_the_controller() -> TestResult {
    let run = start(&[]).await?;
    let mut controller = Session::connect(&run.run_dir).await?;
    controller.wait_for("FAKE-AGENT READY").await?;

    let bad_token = PtyClient::connect(&run.run_dir, &[9; 32], true).await;
    assert!(
        matches!(bad_token, Err(ClientError::Rejected { code, .. }) if code == error_code::AUTH_FAILED)
    );

    let mut wrong_version = raw_connect(&run.run_dir).await?;
    wrong_version.write_all(&hello(2, TOKEN, false)).await?;
    let frames = frames_until_eof(&mut wrong_version, WAIT).await?;
    assert!(
        matches!(frames.as_slice(), [HostFrame::Error { code, .. }] if *code == error_code::UNSUPPORTED_VERSION)
    );

    let mut not_hello = raw_connect(&run.run_dir).await?;
    not_hello
        .write_all(&encode_client(&ClientFrame::Ping(1)))
        .await?;
    let frames = frames_until_eof(&mut not_hello, WAIT).await?;
    assert!(
        matches!(frames.as_slice(), [HostFrame::Error { code, .. }] if *code == error_code::PROTOCOL)
    );

    let mut silent = raw_connect(&run.run_dir).await?;
    let started = Instant::now();
    let frames = frames_until_eof(&mut silent, Duration::from_secs(8)).await?;
    assert!(
        matches!(frames.as_slice(), [HostFrame::Error { code, .. }] if *code == error_code::HELLO_TIMEOUT)
    );
    assert!(started.elapsed() >= Duration::from_millis(4_500));

    // None of the refused connections replaced the controller.
    controller.line("still in control").await?;
    controller.wait_for("ECHO: still in control").await?;
    assert!(controller.errors.is_empty());
    Ok(())
}

// 9 -------------------------------------------------------------------------------------------

fn write_status(run_dir: &Path, status: &StatusRecord) -> TestResult {
    std::fs::create_dir_all(run_dir)?;
    std::fs::write(run_dir.join(STATUS_FILE), serde_json::to_vec(status)?)?;
    Ok(())
}

#[tokio::test]
async fn reconciliation_classifies_run_dirs_and_terminates_live_hosts() -> TestResult {
    let dir = tempfile::tempdir()?;
    assert_eq!(
        inspect_run_dir(&dir.path().join("absent")),
        RunDirState::Missing
    );
    std::fs::write(dir.path().join(LAUNCH_FILE), b"{}")?;
    assert_eq!(
        inspect_run_dir(dir.path()),
        RunDirState::Missing,
        "launch only"
    );

    // Recorded and alive: a live host and child with matching identities.
    let mut run = start(&[]).await?;
    let status = run.host.status().clone();
    match inspect_run_dir(&run.run_dir) {
        RunDirState::Recorded {
            status: recorded,
            host_alive: true,
            child_alive: true,
            same_boot: true,
        } => assert_eq!(recorded, status),
        other => return Err(format!("expected live Recorded, got {other:?}").into()),
    }

    // Server-restart reconciliation of an unusable session: kill it, never respawn.
    terminate_recorded(&status)?;
    within("host exit", run.host.wait()).await?;
    wait_gone(i32::try_from(status.child_pid)?).await?;
    match inspect_run_dir(&run.run_dir) {
        RunDirState::Recorded {
            host_alive: false,
            child_alive: false,
            ..
        } => {}
        RunDirState::Exited(record) => assert_eq!(record.signal, Some(9)),
        other => return Err(format!("expected dead run, got {other:?}").into()),
    }

    // Recorded and dead: pids of a reaped process.
    let mut reaped = Command::new("/bin/sh").args(["-c", "exit 0"]).spawn()?;
    let reaped_pid = reaped.id();
    reaped.wait()?;
    let boot_id = current_boot_id().ok_or("no boot id")?;
    let dead = StatusRecord {
        version: RECORD_VERSION,
        attempt_id: "att-dead".into(),
        ptyhost_pid: reaped_pid,
        child_pid: reaped_pid,
        started_at_ms: 0,
        boot_id: boot_id.clone(),
        token_sha256: "0".repeat(64),
    };
    let dead_dir = dir.path().join("dead");
    write_status(&dead_dir, &dead)?;
    assert!(matches!(
        inspect_run_dir(&dead_dir),
        RunDirState::Recorded {
            host_alive: false,
            child_alive: false,
            same_boot: true,
            ..
        }
    ));

    // A live process with a reused pid is not the recorded host: it started after the record.
    let me = std::process::id();
    let reused = StatusRecord {
        ptyhost_pid: me,
        child_pid: me,
        started_at_ms: 1_000,
        ..dead.clone()
    };
    let reused_dir = dir.path().join("reused");
    write_status(&reused_dir, &reused)?;
    assert!(matches!(
        inspect_run_dir(&reused_dir),
        RunDirState::Recorded {
            host_alive: false,
            child_alive: false,
            ..
        }
    ));
    terminate_recorded(&reused)?; // must not signal this test process

    // Another boot: nothing can be alive, nothing is signalled.
    let other_boot = StatusRecord {
        boot_id: "another-boot".into(),
        started_at_ms: i64::MAX / 2,
        ..reused
    };
    let boot_dir = dir.path().join("boot");
    write_status(&boot_dir, &other_boot)?;
    assert!(matches!(
        inspect_run_dir(&boot_dir),
        RunDirState::Recorded {
            same_boot: false,
            host_alive: false,
            ..
        }
    ));
    terminate_recorded(&other_boot)?;
    assert!(!gone(i32::try_from(me)?));
    Ok(())
}

#[tokio::test]
async fn deleting_the_run_directory_ends_the_orphaned_run() -> TestResult {
    let mut run = start(&[]).await?;
    let child = i32::try_from(run.host.status().child_pid)?;
    assert!(kill(Pid::from_raw(child), None).is_ok(), "agent running");
    std::fs::remove_dir_all(&run.run_dir)?;
    // Two missed checks (2 s apart) start the shutdown; the echo agent obeys the interrupt.
    let exit = within("the orphaned host to exit", run.host.wait()).await?;
    eprintln!("host exit: {exit:?}");
    let deadline = Instant::now() + WAIT;
    while kill(Pid::from_raw(child), None) != Err(Errno::ESRCH) {
        if Instant::now() > deadline {
            return Err("the orphaned agent is still running".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}
