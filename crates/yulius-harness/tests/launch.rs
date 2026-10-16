//! Launch-plan contracts: argv snapshots, configuration files and secret placement
//! (SPEC-002 §7.2, SEC-002, AC-MCP-1, AC-NFR-7).

use std::path::PathBuf;

use serde_json::{Value, json};
use yulius_domain::harness::Harness;
use yulius_harness::launch::{
    CockpitRequest, CockpitYulius, Credential, LaunchError, LaunchMode, LaunchPlan, LaunchRequest,
    McpServerSpec, McpTransport, PRIVATE_FILE_MODE, PlannedFile, plan_cockpit, plan_launch,
};
use yulius_harness::secret::Secret;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CLAUDE_TOKEN: &str = "sk-ant-oat01-SYNTHETIC_token_value_for_tests_only_0123456789";
const SESSION: &str = "3f2b1c9e-aaaa-4bbb-8ccc-0123456789ab";

fn servers() -> Vec<McpServerSpec> {
    vec![
        McpServerSpec {
            name: "apify".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@apify/actors-mcp-server".into()],
                env_vars: vec![("APIFY_TOKEN".into(), "YULIUS_MCP_APIFY_APIFY_TOKEN".into())],
            },
        },
        McpServerSpec {
            name: "quartio".into(),
            transport: McpTransport::Http {
                url: "https://mcp.example.test/mcp".into(),
                bearer_env: Some("QUARTIO_MCP_TOKEN".into()),
            },
        },
    ]
}

fn claude_request(mode: LaunchMode, contained: bool) -> LaunchRequest {
    LaunchRequest {
        harness: Harness::ClaudeCode,
        mode,
        worktree: PathBuf::from("/data/sandboxes/s1/worktrees/t1"),
        sandbox_home: PathBuf::from("/data/sandboxes/s1/home"),
        run_dir: PathBuf::from("/data/runs/a1"),
        model: Some("claude-opus-4-1".into()),
        effort: Some("high".into()),
        contained,
        yulius_exe: PathBuf::from("/opt/yulius/bin/yulius"),
        yulius_url: "http://127.0.0.1:7801/".into(),
        mcp_servers: servers(),
        credential: Credential::ClaudeOauthToken(Secret::new(CLAUDE_TOKEN.into())),
    }
}

fn file<'a>(plan: &'a LaunchPlan, name: &str) -> Result<&'a PlannedFile, String> {
    plan.files
        .iter()
        .find(|f| f.path.ends_with(name))
        .ok_or_else(|| format!("missing planned file {name}"))
}

fn json_of(file: &PlannedFile) -> Result<Value, serde_json::Error> {
    serde_json::from_slice(&file.contents)
}

fn assert_secret_absent(plan: &LaunchPlan, secret: &str) {
    assert!(
        !plan.argv.iter().any(|a| a.contains(secret)),
        "secret in argv"
    );
    assert!(
        !plan
            .env
            .iter()
            .any(|(k, v)| k.contains(secret) || v.contains(secret)),
        "secret in non-secret env"
    );
    assert!(
        !format!("{plan:?}").contains(secret),
        "secret in Debug output"
    );
}

#[test]
fn claude_new_session_argv_snapshot_and_files() -> TestResult {
    let plan = plan_launch(&claude_request(
        LaunchMode::New {
            session_id: Some(SESSION.into()),
            prompt: Some("Build the login page".into()),
        },
        true,
    ))?;
    assert_eq!(
        plan.argv,
        [
            "claude",
            "--session-id",
            SESSION,
            "--settings",
            "/data/runs/a1/claude-settings.json",
            "--mcp-config",
            "/data/runs/a1/mcp.json",
            "--strict-mcp-config",
            "--model",
            "claude-opus-4-1",
            "--effort",
            "high",
            "--dangerously-skip-permissions",
            "Build the login page",
        ]
    );
    assert_eq!(plan.cwd, PathBuf::from("/data/sandboxes/s1/worktrees/t1"));
    assert_eq!(plan.session_id.as_deref(), Some(SESSION));
    assert_eq!(
        plan.env,
        [
            (
                "CLAUDE_CONFIG_DIR".to_owned(),
                "/data/sandboxes/s1/home/.claude".to_owned()
            ),
            ("YULIUS_URL".to_owned(), "http://127.0.0.1:7801".to_owned()),
            ("DISABLE_AUTOUPDATER".to_owned(), "1".to_owned()),
            ("BASH_DEFAULT_TIMEOUT_MS".to_owned(), "600000".to_owned()),
            ("BASH_MAX_TIMEOUT_MS".to_owned(), "1800000".to_owned()),
        ]
    );
    assert_eq!(
        plan.secret_env,
        [(
            "CLAUDE_CODE_OAUTH_TOKEN".to_owned(),
            CLAUDE_TOKEN.to_owned()
        )]
    );
    assert_secret_absent(&plan, CLAUDE_TOKEN);
    for planned in &plan.files {
        assert_eq!(planned.mode, PRIVATE_FILE_MODE);
        assert!(!String::from_utf8_lossy(&planned.contents).contains(CLAUDE_TOKEN));
    }

    let settings = json_of(file(&plan, "claude-settings.json")?)?;
    // The turn boundaries may be held while no subscription has capacity (ADR-17): their CLI
    // timeout outlasts the hook client's 8-day hold; the others keep 15 s.
    for (event, timeout) in [
        ("SessionStart", 15),
        ("UserPromptSubmit", 694_800),
        ("Stop", 694_800),
        ("Notification", 15),
        ("SessionEnd", 15),
    ] {
        assert_eq!(
            settings["hooks"][event],
            json!([{ "hooks": [{
                "type": "command",
                "command": format!("'/opt/yulius/bin/yulius' hook claude {event}"),
                "timeout": timeout
            }] }])
        );
    }
    // The status line runs the same client: it prints the task's line and forwards the plan
    // usage Claude Code read from its own responses (ADR-17).
    assert_eq!(
        settings["statusLine"],
        json!({
            "type": "command",
            "command": "'/opt/yulius/bin/yulius' hook claude StatusLine",
            "padding": 0
        })
    );
    let mcp = json_of(file(&plan, "mcp.json")?)?;
    assert_eq!(
        mcp,
        json!({ "mcpServers": {
            "yulius": {
                "type": "http",
                "url": "http://127.0.0.1:7801/mcp",
                "headers": { "Authorization": "Bearer ${YULIUS_TASK_TOKEN}" }
            },
            "apify": {
                "type": "stdio",
                "command": "npx",
                "args": ["-y", "@apify/actors-mcp-server"],
                "env": { "APIFY_TOKEN": "${YULIUS_MCP_APIFY_APIFY_TOKEN}" }
            },
            "quartio": {
                "type": "http",
                "url": "https://mcp.example.test/mcp",
                "headers": { "Authorization": "Bearer ${QUARTIO_MCP_TOKEN}" }
            }
        }})
    );
    Ok(())
}

#[test]
fn claude_resume_uncontained_never_skips_permissions() -> TestResult {
    let mut request = claude_request(
        LaunchMode::Resume {
            session_id: SESSION.into(),
            prompt: Some("--continue after the limit".into()),
            continue_interrupted: false,
        },
        false,
    );
    request.model = None;
    request.effort = None;
    request.mcp_servers.clear();
    let plan = plan_launch(&request)?;
    assert_eq!(
        plan.argv,
        [
            "claude",
            "--resume",
            SESSION,
            "--settings",
            "/data/runs/a1/claude-settings.json",
            "--mcp-config",
            "/data/runs/a1/mcp.json",
            "--strict-mcp-config",
            // No model or effort named: Opus 5.5 at high effort, pinned so neither drifts with
            // Claude Code's aliases or a subscription's defaults (task #21).
            "--model",
            "claude-opus-5-5",
            "--effort",
            "high",
            "--",
            "--continue after the limit",
        ]
    );
    assert_eq!(plan.session_id.as_deref(), Some(SESSION));
    let mcp = json_of(file(&plan, "mcp.json")?)?;
    assert_eq!(
        mcp["mcpServers"].as_object().map(|m| m.len()),
        Some(1),
        "the yulius server is always attached"
    );
    Ok(())
}

/// SPEC-034: a resume after a usage limit sends no prompt; Claude Code continues the interrupted
/// turn itself (`CLAUDE_CODE_RESUME_INTERRUPTED_TURN`). A plain resume does not set the flag.
#[test]
fn claude_resume_continuing_the_interrupted_turn_sends_no_prompt() -> TestResult {
    let continued = plan_launch(&claude_request(
        LaunchMode::Resume {
            session_id: SESSION.into(),
            prompt: Some("Continue.".into()),
            continue_interrupted: true,
        },
        false,
    ))?;
    assert_eq!(continued.argv.last().map(String::as_str), Some("high"));
    assert!(!continued.argv.iter().any(|a| a == "--" || a == "Continue."));
    assert!(
        continued
            .env
            .iter()
            .any(|(k, v)| k == "CLAUDE_CODE_RESUME_INTERRUPTED_TURN" && v == "1"),
        "{:?}",
        continued.env
    );
    let plain = plan_launch(&claude_request(
        LaunchMode::Resume {
            session_id: SESSION.into(),
            prompt: None,
            continue_interrupted: false,
        },
        false,
    ))?;
    assert!(
        !plain
            .env
            .iter()
            .any(|(k, _)| k == "CLAUDE_CODE_RESUME_INTERRUPTED_TURN")
    );
    Ok(())
}

#[test]
fn claude_new_session_without_id_or_prompt() -> TestResult {
    let plan = plan_launch(&claude_request(
        LaunchMode::New {
            session_id: None,
            prompt: Some(String::new()),
        },
        false,
    ))?;
    assert_eq!(plan.session_id, None);
    assert!(!plan.argv.iter().any(|a| a == "--session-id"));
    assert_eq!(plan.argv.last().map(String::as_str), Some("high"));
    Ok(())
}

#[test]
fn hook_command_quotes_the_executable_path() -> TestResult {
    let mut request = claude_request(
        LaunchMode::New {
            session_id: None,
            prompt: None,
        },
        false,
    );
    request.yulius_exe = PathBuf::from("/opt/it's here/yulius");
    let plan = plan_launch(&request)?;
    let settings = json_of(file(&plan, "claude-settings.json")?)?;
    assert_eq!(
        settings["hooks"]["Stop"][0]["hooks"][0]["command"],
        "'/opt/it'\\''s here/yulius' hook claude Stop"
    );
    Ok(())
}

#[test]
fn invalid_requests_are_rejected_without_echoing_values() {
    let new = || LaunchMode::New {
        session_id: None,
        prompt: None,
    };
    let bad_session = claude_request(
        LaunchMode::Resume {
            session_id: "--help".into(),
            prompt: None,
            continue_interrupted: false,
        },
        false,
    );
    assert_eq!(
        plan_launch(&bad_session),
        Err(LaunchError::Invalid("session_id"))
    );
    let bad_prompt = claude_request(
        LaunchMode::New {
            session_id: None,
            prompt: Some("a\0b".into()),
        },
        false,
    );
    assert_eq!(
        plan_launch(&bad_prompt),
        Err(LaunchError::Invalid("prompt"))
    );
    let bad_model = LaunchRequest {
        model: Some("--dangerously-skip-permissions".into()),
        ..claude_request(new(), false)
    };
    assert_eq!(plan_launch(&bad_model), Err(LaunchError::Invalid("model")));
    let bad_url = LaunchRequest {
        yulius_url: "file:///etc/passwd".into(),
        ..claude_request(new(), false)
    };
    assert_eq!(
        plan_launch(&bad_url),
        Err(LaunchError::Invalid("yulius_url"))
    );

    let mut reserved = claude_request(new(), false);
    reserved.mcp_servers.push(McpServerSpec {
        name: "yulius".into(),
        transport: McpTransport::Http {
            url: "http://evil.test/mcp".into(),
            bearer_env: None,
        },
    });
    assert_eq!(
        plan_launch(&reserved),
        Err(LaunchError::DuplicateMcpServer("yulius".into()))
    );
    let mut duplicate = claude_request(new(), false);
    duplicate.mcp_servers.push(servers()[0].clone());
    assert_eq!(
        plan_launch(&duplicate),
        Err(LaunchError::DuplicateMcpServer("apify".into()))
    );
    let mut bad_name = claude_request(new(), false);
    bad_name.mcp_servers[0].name = "a.b".into();
    assert_eq!(
        plan_launch(&bad_name),
        Err(LaunchError::Invalid("mcp server name"))
    );
    let mut bad_env = claude_request(new(), false);
    bad_env.mcp_servers[0] = McpServerSpec {
        name: "x".into(),
        transport: McpTransport::Stdio {
            command: "x".into(),
            args: vec![],
            env_vars: vec![("BAD-NAME".into(), "YULIUS_MCP_X_BAD".into())],
        },
    };
    assert_eq!(
        plan_launch(&bad_env),
        Err(LaunchError::Invalid("mcp env name"))
    );
    bad_env.mcp_servers[0].transport = McpTransport::Stdio {
        command: "x".into(),
        args: vec![],
        env_vars: vec![("TOKEN".into(), "BAD VAR".into())],
    };
    assert_eq!(
        plan_launch(&bad_env),
        Err(LaunchError::Invalid("mcp env name"))
    );
    let error = format!("{:?}", plan_launch(&bad_prompt));
    assert!(!error.contains("a\\0b"));
}

#[test]
fn planned_file_write_is_private_and_replaces_atomically() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("claude-settings.json");
    std::fs::write(&path, b"old")?;
    let planned = PlannedFile {
        path: path.clone(),
        contents: CLAUDE_TOKEN.as_bytes().to_vec(),
        mode: PRIVATE_FILE_MODE,
    };
    planned.write()?;
    assert_eq!(std::fs::read(&path)?, CLAUDE_TOKEN.as_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path)?.permissions().mode() & 0o777;
        // The replacement file is created with the planned mode, not the old file's mode.
        assert_eq!(mode, 0o600);
    }
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())?.collect::<Result<_, _>>()?;
    assert_eq!(leftovers.len(), 1, "no temporary file left behind");
    let missing_parent = PlannedFile {
        path: dir.path().join("no/such/dir/file"),
        ..planned
    };
    assert!(missing_parent.write().is_err());
    assert!(!format!("{missing_parent:?}").contains("SYNTHETIC"));
    Ok(())
}

#[test]
fn a_named_model_without_effort_runs_at_high_effort_unless_it_takes_none() -> TestResult {
    let mut request = claude_request(
        LaunchMode::New {
            session_id: None,
            prompt: None,
        },
        false,
    );
    request.effort = None;
    request.model = Some("claude-sonnet-5-5".into());
    let argv = plan_launch(&request)?.argv;
    let at = |flag: &str| argv.iter().position(|a| a == flag);
    assert_eq!(
        at("--effort")
            .and_then(|i| argv.get(i + 1))
            .map(String::as_str),
        Some("high"),
        "{argv:?}"
    );
    // Haiku takes no effort level, so none is sent.
    request.model = Some("claude-haiku-4-5".into());
    let argv = plan_launch(&request)?.argv;
    assert!(!argv.iter().any(|a| a == "--effort"), "{argv:?}");
    // An effort the task chose is kept.
    request.model = None;
    request.effort = Some("max".into());
    let argv = plan_launch(&request)?.argv;
    assert!(
        argv.windows(2).any(|w| w == ["--effort", "max"]),
        "{argv:?}"
    );
    assert!(
        argv.windows(2).any(|w| w == ["--model", "claude-opus-5-5"]),
        "{argv:?}"
    );
    Ok(())
}

#[test]
fn a_cockpit_is_plain_claude_code_with_the_token_only_in_its_environment() -> TestResult {
    let request = CockpitRequest {
        cwd: PathBuf::from("/data/sandboxes/s1/repo"),
        config_dir: PathBuf::from("/home/alice/.claude"),
        resume: None,
        continue_interrupted: false,
        credential: Credential::ClaudeOauthToken(Secret::new(CLAUDE_TOKEN.into())),
        yulius: None,
        project_mcp_servers: Vec::new(),
        mcp_servers: Vec::new(),
        redaction_exe: None,
    };
    let plan = plan_cockpit(&request)?;
    // No Yulius settings (hooks, status line), MCP, prompt or approval bypass (SPEC-021
    // CKP-002): Claude Code as a person starts it, on the default Opus 5.5 at high effort
    // (SPEC-023 DEF-004); `/model` and `/effort` still change it.
    assert_eq!(
        plan.argv,
        ["claude", "--model", "claude-opus-5-5", "--effort", "high"]
    );
    assert_eq!(plan.cwd, PathBuf::from("/data/sandboxes/s1/repo"));
    assert!(plan.files.is_empty());
    assert!(
        plan.env
            .contains(&("CLAUDE_CONFIG_DIR".into(), "/home/alice/.claude".into()))
    );
    assert!(
        plan.env
            .contains(&("DISABLE_AUTOUPDATER".into(), "1".into()))
    );
    assert!(!plan.env.iter().any(|(k, _)| k.starts_with("YULIUS")));
    assert_eq!(
        plan.secret_env,
        vec![(
            "CLAUDE_CODE_OAUTH_TOKEN".to_owned(),
            CLAUDE_TOKEN.to_owned()
        )]
    );
    assert!(!format!("{plan:?}").contains("SYNTHETIC"));

    let resumed = plan_cockpit(&CockpitRequest {
        resume: Some(SESSION.into()),
        ..request.clone()
    })?;
    assert_eq!(
        resumed.argv,
        [
            "claude",
            "--resume",
            SESSION,
            "--model",
            "claude-opus-5-5",
            "--effort",
            "high"
        ]
    );
    assert_eq!(resumed.session_id.as_deref(), Some(SESSION));
    assert!(
        !resumed
            .env
            .iter()
            .any(|(k, _)| k == "CLAUDE_CODE_RESUME_INTERRUPTED_TURN")
    );

    // SPEC-074 SUP-004: moved to another subscription after a usage limit, the Cockpit resumes
    // its conversation and redoes the cut turn by itself (SPEC-034 LIM-002): the flag, no prompt.
    let continued = plan_cockpit(&CockpitRequest {
        resume: Some(SESSION.into()),
        continue_interrupted: true,
        ..request.clone()
    })?;
    assert_eq!(continued.argv, resumed.argv);
    assert!(
        continued
            .env
            .contains(&("CLAUDE_CODE_RESUME_INTERRUPTED_TURN".into(), "1".into()))
    );
    // Without a conversation there is no turn to continue.
    let fresh = plan_cockpit(&CockpitRequest {
        continue_interrupted: true,
        ..request.clone()
    })?;
    assert!(
        !fresh
            .env
            .iter()
            .any(|(k, _)| k == "CLAUDE_CODE_RESUME_INTERRUPTED_TURN")
    );

    for bad in ["--help", "a b", ""] {
        assert_eq!(
            plan_cockpit(&CockpitRequest {
                resume: Some(bad.into()),
                ..request.clone()
            }),
            Err(LaunchError::Invalid("session_id"))
        );
    }
    assert_eq!(
        plan_cockpit(&CockpitRequest {
            credential: Credential::ClaudeOauthToken(Secret::new("a\nb".into())),
            ..request
        }),
        Err(LaunchError::Invalid("credential"))
    );
    Ok(())
}

#[test]
fn a_cockpit_reaches_the_yulius_tools_with_its_token_only_in_its_environment() -> TestResult {
    let request = CockpitRequest {
        cwd: PathBuf::from("/data/sandboxes/s1/repo"),
        config_dir: PathBuf::from("/home/alice/.claude"),
        resume: Some(SESSION.into()),
        continue_interrupted: false,
        credential: Credential::ClaudeOauthToken(Secret::new(CLAUDE_TOKEN.into())),
        yulius: Some(CockpitYulius {
            url: "http://127.0.0.1:8081/".into(),
            token: Secret::new("ck_SYNTHETIC".into()),
        }),
        project_mcp_servers: Vec::new(),
        mcp_servers: Vec::new(),
        redaction_exe: None,
    };
    let plan = plan_cockpit(&request)?;
    // SPEC-037 MGT-004: the Yulius MCP server inline, its bearer read from the environment; the
    // read-only tools allowed, everything else still asked; no settings, strict config or bypass.
    let config = r#"{"mcpServers":{"yulius":{"headers":{"Authorization":"Bearer ${YULIUS_COCKPIT_TOKEN}"},"type":"http","url":"http://127.0.0.1:8081/mcp"}}}"#;
    assert_eq!(
        plan.argv,
        [
            "claude",
            "--resume",
            SESSION,
            "--mcp-config",
            config,
            "--allowedTools",
            "mcp__yulius__tasks_list,mcp__yulius__task_get,mcp__yulius__automations_list,mcp__yulius__workflows_list,mcp__yulius__sessions_search,mcp__yulius__session_read,mcp__yulius__secrets_list,mcp__yulius__secret_verify",
            "--model",
            "claude-opus-5-5",
            "--effort",
            "high"
        ]
    );
    assert!(plan.files.is_empty());
    assert!(!plan.argv.iter().any(|a| a.contains("SYNTHETIC")));
    assert!(!plan.env.iter().any(|(_, v)| v.contains("SYNTHETIC")));
    assert_eq!(
        plan.secret_env,
        vec![
            (
                "CLAUDE_CODE_OAUTH_TOKEN".to_owned(),
                CLAUDE_TOKEN.to_owned()
            ),
            ("YULIUS_COCKPIT_TOKEN".to_owned(), "ck_SYNTHETIC".to_owned()),
        ]
    );
    assert!(!format!("{plan:?}").contains("SYNTHETIC"));

    for (url, token) in [
        ("ftp://x", "ck_1"),
        ("http://x\"y", "ck_1"),
        ("http://x", "a\nb"),
        ("http://x", ""),
    ] {
        assert!(
            plan_cockpit(&CockpitRequest {
                yulius: Some(CockpitYulius {
                    url: url.into(),
                    token: Secret::new(token.into()),
                }),
                ..request.clone()
            })
            .is_err(),
            "{url} {token:?}"
        );
    }
    Ok(())
}

/// SPEC-095 PMC-004: the servers of the repository's `.mcp.json` are Yulius's to attach, so the
/// Cockpit starts with Claude Code rejecting each of them instead of asking the person; the names
/// travel as JSON in one argv element. Without such servers nothing changes.
#[test]
fn a_cockpit_rejects_the_repository_mcp_servers_instead_of_asking() -> TestResult {
    let request = CockpitRequest {
        cwd: PathBuf::from("/data/sandboxes/s1/repo"),
        config_dir: PathBuf::from("/home/alice/.claude"),
        resume: None,
        continue_interrupted: false,
        credential: Credential::ClaudeOauthToken(Secret::new(CLAUDE_TOKEN.into())),
        yulius: None,
        project_mcp_servers: vec![
            "supabase".into(),
            "pipedream-gmail".into(),
            "it's \"odd\"; $(x)".into(),
        ],
        mcp_servers: Vec::new(),
        redaction_exe: None,
    };
    let plan = plan_cockpit(&request)?;
    let at = plan
        .argv
        .iter()
        .position(|a| a == "--settings")
        .ok_or("--settings")?;
    let settings: serde_json::Value = serde_json::from_str(&plan.argv[at + 1])?;
    assert_eq!(
        settings,
        serde_json::json!({ "disabledMcpjsonServers": [
            "supabase", "pipedream-gmail", "it's \"odd\"; $(x)"
        ] })
    );
    assert_eq!(
        &plan.argv[at + 2..],
        ["--model", "claude-opus-5-5", "--effort", "high"]
    );
    assert!(!plan.argv.iter().any(|a| a == "--strict-mcp-config"));
    Ok(())
}

/// SPEC-147 SAM-001/002 (ADR-0121): the Superagent gets the sandbox's MCP servers next to `yulius`,
/// in the task run's shape (values only as `${VAR}` references), and the task runs' `PostToolUse`
/// redaction hook as its only hook, beside the rejected `.mcp.json` servers.
#[test]
fn a_cockpit_gets_the_sandbox_mcp_servers_and_the_redaction_hook() -> TestResult {
    let request = CockpitRequest {
        cwd: PathBuf::from("/data/sandboxes/s1/repo"),
        config_dir: PathBuf::from("/home/alice/.claude"),
        resume: None,
        continue_interrupted: false,
        credential: Credential::ClaudeOauthToken(Secret::new(CLAUDE_TOKEN.into())),
        yulius: Some(CockpitYulius {
            url: "http://127.0.0.1:8081".into(),
            token: Secret::new("ck_SYNTHETIC".into()),
        }),
        project_mcp_servers: vec!["apify".into()],
        mcp_servers: servers(),
        redaction_exe: Some(PathBuf::from("/opt/yulius it's/yulius")),
    };
    let plan = plan_cockpit(&request)?;
    let value_of = |flag: &str| -> Result<Value, Box<dyn std::error::Error>> {
        let at = plan
            .argv
            .iter()
            .position(|a| a == flag)
            .ok_or(format!("{flag} missing"))?;
        Ok(serde_json::from_str(&plan.argv[at + 1])?)
    };
    let mcp = value_of("--mcp-config")?;
    assert_eq!(
        mcp["mcpServers"]["yulius"]["headers"]["Authorization"],
        "Bearer ${YULIUS_COCKPIT_TOKEN}"
    );
    assert_eq!(
        mcp["mcpServers"]["apify"],
        json!({
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "@apify/actors-mcp-server"],
            "env": { "APIFY_TOKEN": "${YULIUS_MCP_APIFY_APIFY_TOKEN}" }
        })
    );
    assert_eq!(
        mcp["mcpServers"]["quartio"],
        json!({
            "type": "http",
            "url": "https://mcp.example.test/mcp",
            "headers": { "Authorization": "Bearer ${QUARTIO_MCP_TOKEN}" }
        })
    );
    let settings = value_of("--settings")?;
    assert_eq!(settings["disabledMcpjsonServers"], json!(["apify"]));
    assert_eq!(
        settings["hooks"],
        json!({ "PostToolUse": [{ "matcher": "*", "hooks": [{
            "type": "command",
            "command": r"'/opt/yulius it'\''s/yulius' hook claude PostToolUse",
            "timeout": 60
        }] }] })
    );
    assert_eq!(
        settings.as_object().map(serde_json::Map::len),
        Some(2),
        "no other setting: {settings}"
    );
    assert!(!plan.argv.iter().any(|a| a == "--strict-mcp-config"));
    assert!(plan.files.is_empty());

    // Without the Yulius server the sandbox's servers still come, with nothing pre-allowed;
    // without servers, redaction or rejected servers there is no config and no setting.
    let alone = plan_cockpit(&CockpitRequest {
        yulius: None,
        project_mcp_servers: Vec::new(),
        redaction_exe: None,
        ..request.clone()
    })?;
    let at = alone
        .argv
        .iter()
        .position(|a| a == "--mcp-config")
        .ok_or("--mcp-config")?;
    let only: Value = serde_json::from_str(&alone.argv[at + 1])?;
    assert!(only["mcpServers"]["yulius"].is_null());
    assert!(only["mcpServers"]["apify"].is_object());
    assert!(
        !alone
            .argv
            .iter()
            .any(|a| a == "--allowedTools" || a == "--settings")
    );

    // A server is validated as for a task run.
    let named = |name: &str| McpServerSpec {
        name: name.into(),
        transport: McpTransport::Http {
            url: "https://x.test/mcp".into(),
            bearer_env: None,
        },
    };
    for (servers, error) in [
        (
            vec![named("yulius")],
            LaunchError::DuplicateMcpServer("yulius".into()),
        ),
        (
            vec![named("a"), named("a")],
            LaunchError::DuplicateMcpServer("a".into()),
        ),
        (
            vec![named("bad name")],
            LaunchError::Invalid("mcp server name"),
        ),
        (
            vec![McpServerSpec {
                name: "s".into(),
                transport: McpTransport::Stdio {
                    command: "npx".into(),
                    args: Vec::new(),
                    env_vars: vec![("A".into(), "B-C".into())],
                },
            }],
            LaunchError::Invalid("mcp env name"),
        ),
    ] {
        assert_eq!(
            plan_cockpit(&CockpitRequest {
                mcp_servers: servers,
                ..request.clone()
            }),
            Err(error)
        );
    }
    Ok(())
}
