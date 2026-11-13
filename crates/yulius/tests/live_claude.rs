//! Opt-in live compatibility check with the real Claude Code CLI (SPEC-002 §7.5, R-03).
//!
//! Serves the real internal endpoints in-process, writes the real launch files that Yulius
//! generates for Claude Code (hooks settings and MCP config) and runs `claude -p` against them
//! with the machine's existing Claude login. It proves that the CLI loads Yulius hooks, that
//! `yulius hook` reaches the server, that the Yulius MCP server works with the task bearer token
//! and that `--session-id` names the session.
//!
//! Costs a few cents of provider usage, so it is ignored by default:
//! `YULIUS_LIVE_CLAUDE=1 cargo test -p yulius --test main live_claude:: -- --ignored --nocapture`.
//! User settings are not loaded (`--setting-sources project` in an empty directory); the session
//! transcript this creates in the user's Claude config directory is removed afterwards.

use crate::support;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use support::{TestResult, harness};
use yulius::wiring::harness::HarnessAdapter;
use yulius_app::model::{
    AttemptRecord, LaunchKind, SandboxRecord, Secret, TaskOrigin, TaskRecord, UserRecord,
};
use yulius_app::ports::harness::{HarnessLaunch, HarnessPort, SessionMode};
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AttemptId, SandboxId, TaskId, UserId};
use yulius_domain::task::{Activity, AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;

const TOKEN: &str = "live-task-token-0123456789abcdef0123456789";

#[tokio::test]
#[ignore = "uses the real Claude Code CLI and provider quota; set YULIUS_LIVE_CLAUDE=1"]
async fn claude_code_runs_yulius_hooks_and_mcp() -> TestResult {
    if std::env::var("YULIUS_LIVE_CLAUDE").as_deref() != Ok("1") {
        return Err("set YULIUS_LIVE_CLAUDE=1 to run this live check".into());
    }
    let h = harness()?;
    let store = &h.app.deps.store;
    let user = UserRecord {
        id: UserId::parse("u1")?,
        github_login: Some("live".into()),
        github_id: Some(1),
        display_name: "Live".into(),
        avatar_url: None,
        is_superadmin: true,
        is_instance_admin: true,
        password_hash: None,
        created_at: Timestamp(1),
        disabled_at: None,
    };
    store.insert_user(&user).await?;
    let sandbox = SandboxRecord {
        id: SandboxId::parse("s1")?,
        slug: "live".into(),
        repo_owner: "acme".into(),
        repo_name: "live".into(),
        github_repo_id: Some(1),
        default_branch: "main".into(),
        profile_json: "{}".into(),
        port_base: 43000,
        port_count: 10,
        onboarding_state: "not_needed".into(),
        onboarding_task_id: None,
        created_by: user.id.clone(),
        created_at: Timestamp(1),
        removed_at: None,
    };
    store.insert_sandbox(&sandbox).await?;
    let now = h.app.now();
    let task = TaskRecord {
        id: TaskId::parse("t-live")?,
        sandbox_id: sandbox.id.clone(),
        number: 1,
        title: "Live compatibility probe".into(),
        prompt: "Probe".into(),
        harness: Harness::ClaudeCode,
        model: None,
        effort: None,
        workflow_id: None,
        status: TaskStatus::Running,
        activity: None,
        branch: None,
        worktree_path: None,
        spec_path: None,
        origin: TaskOrigin::Ui,
        automation_run: None,
        created_by: user.id.clone(),
        owner: user.id.clone(),
        created_at: now,
        updated_at: now,
        closed_at: None,
        archived_at: None,
        queued_at: None,
        gate_blocks: 0,
        wake_enabled: false,
        mode: yulius_app::model::TaskMode::Prompt,
        supertask: false,
    };
    store.insert_task(&task).await?;
    let work = tempfile::tempdir()?;
    let worktree = work.path().canonicalize()?.join("wt");
    let run_dir = work.path().canonicalize()?.join("run");
    std::fs::create_dir_all(&worktree)?;
    std::fs::create_dir_all(&run_dir)?;
    store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse("at-live")?,
            task_id: task.id.clone(),
            seq: 1,
            harness: Harness::ClaudeCode,
            account_id: None,
            provider_session_id: None,
            launch_kind: LaunchKind::New,
            model: None,
            state: AttemptState::Live,
            run_dir: run_dir.to_string_lossy().into_owned(),
            token_hash: h.app.deps.crypto.sha256_hex(TOKEN.as_bytes()),
            host_token: h.app.deps.crypto.seal(&[1u8; 32])?,
            ptyhost_pid: None,
            child_pid: None,
            started_at: now,
            ended_at: None,
            exit_code: None,
            exit_reason: None,
        })
        .await?;

    // The internal listener, served for the duration of the test only.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let internal = yulius_server::internal_router(h.app.clone())
        .into_make_service_with_connect_info::<SocketAddr>();
    let server = tokio::spawn(async move { axum::serve(listener, internal).await });

    let session = "5d1f3c2a-7b8e-4c9d-a0b1-c2d3e4f5a6b7".to_owned();
    let adapter = HarnessAdapter::new(PathBuf::from(env!("CARGO_BIN_EXE_yulius")), url.clone());
    let plan = adapter.plan(&HarnessLaunch {
        harness: Harness::ClaudeCode,
        mode: SessionMode::New {
            session_id: Some(session.clone()),
            prompt: None,
        },
        worktree: worktree.clone(),
        sandbox_home: work.path().join("home"),
        run_dir: run_dir.clone(),
        // Haiku keeps the probe cheap and, taking no effort level, gets no `--effort`.
        model: Some("claude-haiku-4-5".into()),
        effort: None,
        contained: false,
        mcp: Vec::new(),
        credential: Secret("unused-the-local-login-is-used".into()),
    })?;
    for file in &plan.plan.files {
        if let Some(parent) = file.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&file.path, &file.contents)?;
    }
    // The generated argv minus the program, plus print mode and cost bounds.
    let mut args: Vec<String> = plan.plan.argv.iter().skip(1).cloned().collect();
    args.extend(
        [
            "-p",
            "Call the yulius MCP tool steps_set with steps [\"Probe live compatibility\"]. Then reply with the single word DONE.",
            "--setting-sources",
            "project",
            "--max-budget-usd",
            "0.50",
            "--allowedTools",
            "mcp__yulius__steps_set",
        ]
        .map(str::to_owned),
    );
    let mut command = tokio::process::Command::new("claude");
    command
        .args(&args)
        .current_dir(&worktree)
        .stdin(Stdio::null())
        .env("YULIUS_TASK_TOKEN", TOKEN)
        .env("YULIUS_URL", &url)
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .env_remove("ANTHROPIC_API_KEY");
    for (key, value) in &plan.plan.env {
        if !matches!(key.as_str(), "CLAUDE_CONFIG_DIR" | "HOME" | "PATH") {
            command.env(key, value);
        }
    }
    let output = tokio::time::timeout(Duration::from_secs(240), command.output()).await??;
    let stdout = String::from_utf8_lossy(&output.stdout);
    eprintln!(
        "claude exit: {:?}\nstdout: {}",
        output.status.code(),
        stdout.trim()
    );

    let attempt = store
        .attempt(&AttemptId::parse("at-live")?)
        .await?
        .ok_or("attempt")?;
    let steps = store.steps(&task.id).await?;
    let record = store.task(&task.id).await?.ok_or("task")?;
    eprintln!(
        "session recorded by SessionStart hook: {:?}\nsteps via MCP: {:?}\nactivity after Stop hook: {:?}",
        attempt.provider_session_id,
        steps.iter().map(|s| s.title.clone()).collect::<Vec<_>>(),
        record.activity
    );

    // Clean up the transcript this created in the user's Claude config directory.
    let slug: String = worktree
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    if let Some(home) = std::env::var_os("HOME") {
        let project = PathBuf::from(home).join(".claude/projects").join(&slug);
        let transcript = project.join(format!("{session}.jsonl"));
        eprintln!("transcript at expected path: {}", transcript.is_file());
        let _ = std::fs::remove_file(&transcript);
        let _ = std::fs::remove_dir(&project);
    }
    server.abort();

    assert!(output.status.success(), "claude failed");
    assert_eq!(
        attempt.provider_session_id.as_deref(),
        Some(session.as_str())
    );
    assert_eq!(steps.len(), 1, "steps_set reached the Yulius MCP server");
    assert_eq!(
        record.activity,
        Some(Activity::Waiting),
        "Stop hook reached Yulius"
    );
    Ok(())
}

/// What the real CLI's debug log says about the language servers it loaded, once it says it:
/// the count of `Total LSP servers loaded: N` and the plugins named by
/// `Loaded N LSP server(s) from plugin: <name>`.
fn language_servers_loaded(log: &str) -> Option<(usize, Vec<String>)> {
    let total = log
        .lines()
        .find_map(|line| line.split_once("Total LSP servers loaded: "))?
        .1
        .trim()
        .parse()
        .ok()?;
    let mut plugins: Vec<String> = log
        .lines()
        .filter_map(|line| line.split_once(" LSP server(s) from plugin: "))
        .map(|(_, plugin)| plugin.trim().to_owned())
        .collect();
    plugins.sort();
    Some((total, plugins))
}

/// A sandbox directory for the real CLI: `home/.claude` (the config dir), `work` (the worktree,
/// trusted) and the marketplace `m` of four plugins. Three bring a language server, each in one
/// of the ways the CLI knows: the marketplace's entry (the official `rust-analyzer-lsp` plugin's
/// way), the plugin's manifest and `.lsp.json`; `skills-only` brings none.
struct LiveHome {
    root: PathBuf,
    home: PathBuf,
    config: PathBuf,
    work: PathBuf,
    runs: std::cell::Cell<usize>,
}

const LIVE_PLUGINS: [&str; 4] = ["by-catalog", "by-manifest", "by-file", "skills-only"];

impl LiveHome {
    fn new(dir: &std::path::Path, name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let root = dir.join(name);
        let (home, work) = (root.join("home"), root.join("work"));
        let config = home.join(".claude");
        std::fs::create_dir_all(config.join("plugins"))?;
        std::fs::create_dir_all(&work)?;
        // The worktree is trusted, as every launch of Yulius makes it.
        std::fs::write(
            config.join(".claude.json"),
            serde_json::json!({
                "hasCompletedOnboarding": true,
                "projects": { work.to_string_lossy(): {
                    "hasTrustDialogAccepted": true, "hasCompletedProjectOnboarding": true,
                } },
            })
            .to_string(),
        )?;
        Ok(Self {
            root,
            home,
            config,
            work,
            runs: std::cell::Cell::new(0),
        })
    }

    /// The marketplace `m` as the folder `at`, its plugins in `at/plugins/<name>`.
    fn marketplace(&self, at: &std::path::Path) -> TestResult {
        let server = |extension: &str| {
            serde_json::json!({ "srv": {
                "command": "yulius-live-check-no-such-language-server",
                "extensionToLanguage": { extension: "x" },
            } })
        };
        let mut entries = Vec::new();
        for name in LIVE_PLUGINS {
            let source = at.join("plugins").join(name);
            std::fs::create_dir_all(&source)?;
            std::fs::write(source.join("README.md"), name)?;
            let mut entry = serde_json::json!({
                "name": name, "source": format!("./plugins/{name}"), "description": name,
                "version": "1.0.0",
            });
            let mut manifest = serde_json::json!({
                "name": name, "version": "1.0.0", "description": name,
            });
            match name {
                "by-catalog" => {
                    entry["strict"] = false.into();
                    entry["lspServers"] = server(".xa");
                }
                "by-manifest" => manifest["lspServers"] = server(".xb"),
                "by-file" => std::fs::write(source.join(".lsp.json"), server(".xc").to_string())?,
                _ => {}
            }
            // A plugin described by its marketplace entry alone has no manifest of its own.
            if name != "by-catalog" {
                std::fs::create_dir_all(source.join(".claude-plugin"))?;
                std::fs::write(
                    source.join(".claude-plugin/plugin.json"),
                    manifest.to_string(),
                )?;
            }
            entries.push(entry);
        }
        std::fs::create_dir_all(at.join(".claude-plugin"))?;
        std::fs::write(
            at.join(".claude-plugin/marketplace.json"),
            serde_json::json!({ "name": "m", "owner": { "name": "check" }, "plugins": entries })
                .to_string(),
        )?;
        Ok(())
    }

    /// `known_marketplaces.json`: the marketplace `m` is the folder `at`.
    fn known(&self, at: &std::path::Path) -> TestResult {
        std::fs::write(
            self.config.join("plugins/known_marketplaces.json"),
            serde_json::json!({ "m": {
                "source": { "source": "directory", "path": at },
                "installLocation": at,
                "lastUpdated": "2026-11-13T00:00:00.000Z",
            } })
            .to_string(),
        )?;
        Ok(())
    }

    /// Installs every plugin of the marketplace at `market` as Claude Code does: a copy of its
    /// folder below `copies` and a record that names the copy.
    fn install(&self, market: &std::path::Path, copies: &std::path::Path) -> TestResult {
        let mut installed = serde_json::Map::new();
        for name in LIVE_PLUGINS {
            let source = market.join("plugins").join(name);
            let copy = copies.join(name).join("1.0.0");
            std::fs::create_dir_all(copy.join(".claude-plugin"))?;
            for file in ["README.md", ".lsp.json", ".claude-plugin/plugin.json"] {
                if source.join(file).exists() {
                    std::fs::copy(source.join(file), copy.join(file))?;
                }
            }
            if name == "by-catalog" {
                std::fs::remove_dir(copy.join(".claude-plugin"))?;
            }
            installed.insert(
                format!("{name}@m"),
                serde_json::json!([{
                    "scope": "user", "installPath": copy, "version": "1.0.0",
                    "installedAt": "2026-11-13T00:00:00.000Z",
                    "lastUpdated": "2026-11-13T00:00:00.000Z",
                }]),
            );
        }
        std::fs::write(
            self.config.join("plugins/installed_plugins.json"),
            serde_json::json!({ "version": 2, "plugins": installed }).to_string(),
        )?;
        Ok(())
    }

    /// Writes the settings file `file` that switches every plugin on, with the keys of `more`.
    fn switch_on(&self, file: &std::path::Path, more: serde_json::Value) -> TestResult {
        let on: serde_json::Map<String, serde_json::Value> = LIVE_PLUGINS
            .iter()
            .map(|name| (format!("{name}@m"), true.into()))
            .collect();
        let mut settings = serde_json::json!({ "enabledPlugins": on });
        if let (Some(settings), Some(more)) = (settings.as_object_mut(), more.as_object()) {
            settings.extend(more.clone());
        }
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(file, settings.to_string())?;
        Ok(())
    }

    /// Runs the real `claude -p` in the worktree with `args`, against a closed local port (no
    /// provider request can be made), until its debug log says which language servers it loaded;
    /// the process is then ended.
    async fn session(
        &self,
        args: &[String],
    ) -> Result<(usize, Vec<String>), Box<dyn std::error::Error>> {
        self.runs.set(self.runs.get() + 1);
        let log = self.root.join(format!("debug-{}.log", self.runs.get()));
        let mut child = tokio::process::Command::new("claude")
            .args(args)
            .arg("--debug-file")
            .arg(&log)
            .args(["-p", "hi"])
            .current_dir(&self.work)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("CLAUDE_CONFIG_DIR", &self.config)
            // Not a key and not a reachable API: nothing leaves this machine.
            .env("ANTHROPIC_API_KEY", "yulius-live-check-not-a-key")
            .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:9")
            .env("DISABLE_AUTOUPDATER", "1")
            .env("DISABLE_TELEMETRY", "1")
            .env("DISABLE_ERROR_REPORTING", "1")
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let loaded = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let text = tokio::fs::read_to_string(&log).await.unwrap_or_default();
                if let Some(loaded) = language_servers_loaded(&text) {
                    return loaded;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        child.kill().await?;
        Ok(
            loaded
                .map_err(|_| "the CLI's debug log never said which language servers it loaded")?,
        )
    }

    /// The `--settings` Yulius plans for a task run of this home and worktree, written where the
    /// plan says. `/bin/true` stands in for the hooks' `yulius`.
    fn planned_settings(&self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let adapter = HarnessAdapter::new(PathBuf::from("/bin/true"), "http://127.0.0.1:9");
        let plan = adapter.plan(&HarnessLaunch {
            harness: Harness::ClaudeCode,
            mode: SessionMode::New {
                session_id: None,
                prompt: None,
            },
            worktree: self.work.clone(),
            sandbox_home: self.home.clone(),
            run_dir: self.root.join("run"),
            model: None,
            effort: None,
            contained: false,
            mcp: Vec::new(),
            credential: Secret("unused-no-provider-request-is-made".into()),
        })?;
        let settings = plan
            .plan
            .files
            .iter()
            .find(|file| file.path.ends_with("claude-settings.json"))
            .ok_or("the run's settings file")?;
        std::fs::create_dir_all(self.root.join("run"))?;
        std::fs::write(&settings.path, &settings.contents)?;
        Ok(vec![
            "--settings".to_owned(),
            settings.path.to_string_lossy().into_owned(),
        ])
    }
}

/// SPEC-485 LSP-004 (ADR-0327): in each of eight layouts in which the real CLI loads a plugin's
/// language server, it loads none when started with the settings Yulius plans for a run of that
/// home and worktree, and the plugin without a language server stays on. Each layout is first shown to be
/// one the CLI loads three servers from (the control), so a layout a later Claude Code no longer
/// loads, or loads differently, fails here too.
///
/// No account and no provider request:
/// `YULIUS_LIVE_CLAUDE=1 cargo test -p yulius --test main live_claude::a_run -- --ignored`.
#[tokio::test]
#[ignore = "launches the real Claude Code CLI; set YULIUS_LIVE_CLAUDE=1"]
async fn a_runs_settings_keep_the_real_cli_from_loading_a_language_server() -> TestResult {
    if std::env::var("YULIUS_LIVE_CLAUDE").as_deref() != Ok("1") {
        return Err("set YULIUS_LIVE_CLAUDE=1 to run this live check".into());
    }
    let dir = tempfile::tempdir()?;
    let dir = dir.path().canonicalize()?;
    let all_three = (
        3,
        vec![
            "by-catalog".to_owned(),
            "by-file".to_owned(),
            "by-manifest".to_owned(),
        ],
    );
    let mut homes = Vec::new();

    // What Claude Code's install dialog leaves: a cloned marketplace, the plugins installed
    // into the cache and switched on in the home's settings.
    let home = LiveHome::new(&dir, "cloned")?;
    let market = home.config.join("plugins/marketplaces/m");
    home.marketplace(&market)?;
    home.known(&market)?;
    home.install(&market, &home.config.join("plugins/cache/m"))?;
    home.switch_on(&home.config.join("settings.json"), serde_json::json!({}))?;
    homes.push(home);

    // `claude plugin marketplace add <folder>` and `claude plugin install`: the marketplace
    // stays where it lies; the copy of the plugin its entry alone describes holds no servers.
    let home = LiveHome::new(&dir, "folder-marketplace")?;
    let market = home.work.join("tools/market");
    home.marketplace(&market)?;
    home.known(&market)?;
    home.install(&market, &home.config.join("plugins/cache/m"))?;
    home.switch_on(&home.config.join("settings.json"), serde_json::json!({}))?;
    homes.push(home);

    // Switched on in the home's settings with no install record: loaded in place.
    let home = LiveHome::new(&dir, "no-install-record")?;
    let market = home.work.join("tools/market");
    home.marketplace(&market)?;
    home.known(&market)?;
    home.switch_on(&home.config.join("settings.json"), serde_json::json!({}))?;
    homes.push(home);

    // Switched on by the repository, and by a worktree's local settings.
    for file in ["settings.json", "settings.local.json"] {
        let home = LiveHome::new(&dir, &format!("repository-{file}"))?;
        let market = home.root.join("market");
        home.marketplace(&market)?;
        home.known(&market)?;
        home.switch_on(&home.work.join(".claude").join(file), serde_json::json!({}))?;
        homes.push(home);
    }

    // Installed copies outside the cache, and nothing but the copies says that two of the
    // plugins bring a server: their folders in the marketplace are gone.
    let home = LiveHome::new(&dir, "copies-elsewhere")?;
    let market = home.config.join("plugins/marketplaces/m");
    home.marketplace(&market)?;
    home.known(&market)?;
    home.install(&market, &home.root.join("copies"))?;
    // A cloned marketplace: Claude Code then loads the installed copies, not the clone.
    std::fs::write(
        home.config.join("plugins/known_marketplaces.json"),
        serde_json::json!({ "m": {
            "source": { "source": "github", "repo": "yulius-live-check/no-such-repository" },
            "installLocation": market,
            "lastUpdated": "2026-11-13T00:00:00.000Z",
        } })
        .to_string(),
    )?;
    for name in ["by-manifest", "by-file"] {
        std::fs::remove_dir_all(market.join("plugins").join(name))?;
    }
    home.switch_on(&home.config.join("settings.json"), serde_json::json!({}))?;
    homes.push(home);

    // A repository that brings its own marketplace by an absolute path: loaded in the very
    // session that registers it.
    let home = LiveHome::new(&dir, "repository-marketplace-absolute")?;
    let market = home.work.join("tools/market");
    home.marketplace(&market)?;
    home.switch_on(
        &home.work.join(".claude/settings.json"),
        serde_json::json!({ "extraKnownMarketplaces": { "m": {
            "source": { "source": "directory", "path": market },
        } } }),
    )?;
    homes.push(home);

    // The same by a path relative to the repository. Run without a terminal, as here, the
    // session that registers it loads nothing and the next one does; in a terminal the
    // registering session loads the servers too (measured on 2.1.323). So the run's settings are
    // planned before anything is registered, and must hold after.
    let home = LiveHome::new(&dir, "repository-marketplace")?;
    home.marketplace(&home.work.join("tools/market"))?;
    home.switch_on(
        &home.work.join(".claude/settings.json"),
        serde_json::json!({ "extraKnownMarketplaces": { "m": {
            "source": { "source": "directory", "path": "./tools/market" },
        } } }),
    )?;
    let planned_unregistered = home.planned_settings()?;
    assert!(
        !home.config.join("plugins/known_marketplaces.json").exists(),
        "nothing is registered when the run is planned"
    );
    assert_eq!(home.session(&[]).await?.0, 0, "this session registers");
    assert_eq!(
        home.session(&[]).await?,
        all_three,
        "repository-marketplace: the control, the real CLI loads the three language servers"
    );
    assert_eq!(
        home.session(&planned_unregistered).await?,
        (0, Vec::new()),
        "repository-marketplace: with the settings planned before the registration"
    );

    for home in &homes {
        let layout = home.root.file_name().unwrap_or_default().to_string_lossy();
        assert_eq!(
            home.session(&[]).await?,
            all_three,
            "{layout}: the control, the real CLI loads the three language servers"
        );
        let args = home.planned_settings()?;
        assert_eq!(
            home.session(&args).await?,
            (0, Vec::new()),
            "{layout}: with the run's settings no language server is loaded"
        );
    }

    // The plugin without a language server is still on, and the home's file is as it was.
    let home = &homes[0];
    let args = home.planned_settings()?;
    let listed = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::process::Command::new("claude")
            .args(&args)
            .args(["plugin", "list", "--json"])
            .current_dir(&home.work)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &home.home)
            .env("CLAUDE_CONFIG_DIR", &home.config)
            .env("DISABLE_AUTOUPDATER", "1")
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    let listed: serde_json::Value = serde_json::from_slice(&listed.stdout)?;
    let enabled = |id: &str| {
        listed
            .as_array()
            .and_then(|plugins| plugins.iter().find(|plugin| plugin["id"] == id))
            .map(|plugin| plugin["enabled"].clone())
    };
    assert_eq!(enabled("skills-only@m"), Some(true.into()), "{listed}");
    for id in ["by-catalog@m", "by-manifest@m", "by-file@m"] {
        assert_eq!(enabled(id), Some(false.into()), "{id}: {listed}");
    }
    let kept: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.config.join("settings.json"))?)?;
    assert_eq!(kept["enabledPlugins"]["by-catalog@m"], true);
    Ok(())
}
