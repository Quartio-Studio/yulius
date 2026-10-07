//! Bubblewrap execution (SPEC-002 ADR-03; AC-NFR-6 groundwork). Linux only: argv construction is
//! unit-tested in `src/containment.rs` and the non-Linux refusal in `tests/supervision.rs`.
//! Skipped with an explicit message when `bwrap` is missing or cannot create namespaces (for
//! example when unprivileged user namespaces are disabled).

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::BTreeMap;
    use std::error::Error;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use yulius_contracts::ptyhost::{
        BubblewrapSpec, ClientFrame, Containment, ExitReason, HostFrame, LaunchSpec, RECORD_VERSION,
    };
    use yulius_ptyhost::test_support::locate_fake_agent;
    use yulius_ptyhost::{PtyClient, spawn_host, token_sha256_hex};

    const HOST_EXE: &str = env!("CARGO_BIN_EXE_yulius-ptyhost");
    const TOKEN: [u8; 32] = [3; 32];

    fn bwrap_usable() -> Result<(), String> {
        let probe = Command::new("bwrap")
            .args([
                "--die-with-parent",
                "--unshare-pid",
                "--ro-bind",
                "/",
                "/",
                "--proc",
                "/proc",
                "--",
                "/bin/true",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("bwrap not runnable: {e}"))?;
        if probe.status.success() {
            Ok(())
        } else {
            Err(format!(
                "bwrap cannot create namespaces here: {}",
                String::from_utf8_lossy(&probe.stderr).trim()
            ))
        }
    }

    #[tokio::test]
    async fn contained_agent_runs_inside_bwrap_and_hides_the_run_directory()
    -> Result<(), Box<dyn Error>> {
        if let Err(reason) = bwrap_usable() {
            eprintln!("SKIPPED contained_agent_runs_inside_bwrap: {reason}");
            return Ok(());
        }
        let fake = locate_fake_agent().ok_or("fake-agent not built")?;
        let fake_dir = fake
            .parent()
            .ok_or("no parent")?
            .to_string_lossy()
            .into_owned();
        let sandbox = tempfile::tempdir()?;
        let root = sandbox.path().to_string_lossy().into_owned();
        std::fs::create_dir_all(sandbox.path().join("home"))?;
        let runs = tempfile::tempdir()?;
        let run_dir = runs.path().join("run");
        let launch = LaunchSpec {
            version: RECORD_VERSION,
            attempt_id: "att-bwrap".into(),
            task_id: "task-bwrap".into(),
            argv: vec![
                fake.to_string_lossy().into_owned(),
                "--session-id".into(),
                "contained".into(),
                "--report-env".into(),
                "HOME".into(),
            ],
            cwd: root.clone(),
            env: BTreeMap::from([
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("TERM".into(), "xterm-256color".into()),
            ]),
            secret_env_names: Vec::new(),
            cols: 80,
            rows: 24,
            containment: Containment::Bubblewrap(BubblewrapSpec {
                sandbox_root: root.clone(),
                home: format!("{root}/home"),
                writable: Vec::new(),
                readonly: vec![fake_dir],
                share_net: false,
            }),
            ring_bytes: 0,
            token_sha256: token_sha256_hex(&TOKEN),
            linger_ms: 1_000,
        };
        let mut host = spawn_host(
            Path::new(HOST_EXE),
            &run_dir,
            &launch,
            &[],
            Duration::from_secs(10),
        )
        .await?;
        let (mut client, welcome, snapshot) = PtyClient::connect(&run_dir, &TOKEN, true).await?;
        assert_eq!(welcome.child_pid, host.status().child_pid, "child is bwrap");
        // Startup lines may predate the connection: they are on the snapshot screen.
        let mut screen = vt100::Parser::new(welcome.rows, welcome.cols, 0);
        screen.process(&snapshot.ok_or("snapshot missing")?.bytes);
        let mut output = screen.screen().contents();
        let run_dir_text = run_dir.to_string_lossy().into_owned();
        client
            .send(ClientFrame::Input(
                format!("{run_dir_text}\r/exit\r").into_bytes(),
            ))
            .await?;
        let mut exit = None;
        while let Some(frame) =
            tokio::time::timeout(Duration::from_secs(20), client.recv()).await??
        {
            match frame {
                HostFrame::Output { bytes, .. } => {
                    output.push_str(&String::from_utf8_lossy(&bytes));
                }
                HostFrame::Exit(record) => exit = Some(record),
                _ => {}
            }
        }
        let home_line = format!("ENV HOME=set len={}", root.len() + 5);
        assert!(
            output.contains(&home_line),
            "missing {home_line:?} in {output:?}"
        );
        assert!(
            output.contains(&format!("ECHO: {run_dir_text}")),
            "{output:?}"
        );
        let exit = exit.ok_or("no exit frame")?;
        assert_eq!((exit.reason, exit.code), (ExitReason::Exited, Some(0)));
        tokio::time::timeout(Duration::from_secs(10), host.wait()).await?;
        // The run directory is outside the sandbox and must not be visible inside it.
        let inside = Command::new("bwrap")
            .args(
                yulius_ptyhost::bwrap_argv(
                    match &launch.containment {
                        Containment::Bubblewrap(spec) => spec,
                        Containment::None => return Err("not contained".into()),
                    },
                    &["/bin/ls".to_owned(), run_dir_text.clone()],
                    &root,
                )[1..]
                    .iter(),
            )
            .output()?;
        assert!(
            !inside.status.success(),
            "run directory visible inside the sandbox"
        );
        Ok(())
    }
}
