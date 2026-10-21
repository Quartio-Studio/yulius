//! `yulius remote`, `yulius project`, `yulius task`, `yulius ci` and `yulius secrets list`
//! (SPEC-247 RMT-030, RMT-031): a person drives a Yulius instance from their computer, or a
//! script or an agent without MCP does, through the instance's remote MCP with a personal API
//! token. Every command is one tool call; its text is printed, or its record with `--json`.
//!
//! Exit codes: 0 done, 1 the tool refused or failed (its text says why), 2 the request did not
//! reach a tool (no token, a refused token, the network, a bad argument).

use std::io::{self, IsTerminal, Read, Write};
use std::process::ExitCode;

use serde_json::{Map, Value, json};
use yulius_import::{RemoteClient, RemoteError, Secret, TokenStore, Url, discover_repo};

/// Options every remote command takes.
#[derive(Debug, Clone, Default, clap::Args)]
pub struct RemoteOpts {
    /// Yulius URL (default: the only instance `yulius remote login` stored).
    #[arg(long, env = "YULIUS_SERVER", global = true)]
    pub server: Option<String>,
    /// The project, owner/name (default: the GitHub `origin` of the current checkout).
    #[arg(long, short = 'p', env = "YULIUS_PROJECT", global = true)]
    pub project: Option<String>,
    /// Print the tool's record as JSON instead of its text.
    #[arg(long, global = true)]
    pub json: bool,
}

/// `yulius remote …`.
#[derive(Debug, clap::Subcommand)]
pub enum RemoteAction {
    /// Store a personal API token for an instance (make one on Settings → Connect your local
    /// agent); it is checked first.
    Login {
        /// The instance, e.g. `https://yulius.example.com`.
        url: String,
        /// Read the token from stdin instead of asking for it.
        #[arg(long)]
        token_stdin: bool,
    },
    /// Forget the instance's token (revoke it on Settings).
    Logout,
    /// The instance, who the token acts for and its scope.
    Status,
    /// The tools this token may call.
    Tools,
    /// Call any tool: `yulius remote call tasks_list '{"project": "owner/name"}'` (`-`: the
    /// arguments from stdin).
    Call {
        /// Tool name (`yulius remote tools`).
        tool: String,
        /// Arguments as a JSON object; `project` is added from --project or the checkout.
        args: Option<String>,
    },
}

/// `yulius project …`.
#[derive(Debug, clap::Subcommand)]
pub enum ProjectAction {
    /// The projects you can see.
    List,
    /// GitHub repositories you may add.
    Repositories,
    /// Add a GitHub repository (owner/name; default: this checkout's); onboarding waits for
    /// `yulius project onboard`.
    Import {
        /// owner/name.
        repo: Option<String>,
    },
    /// The project, its onboarding state and plan.
    Get,
    /// Start the project's onboarding.
    Onboard {
        /// A file (`-`: stdin) your agent wrote for the onboarding task: which secret is which,
        /// where and how deploys run; added to its request (SPEC-256).
        #[arg(long, value_name = "FILE")]
        notes: Option<String>,
    },
    /// List a directory of the integration branch.
    Files {
        /// Directory (default: the root).
        path: Option<String>,
        /// Branch or commit.
        #[arg(long = "ref")]
        reference: Option<String>,
    },
    /// Print a file of the integration branch.
    Cat {
        /// File path.
        path: String,
        /// Branch or commit.
        #[arg(long = "ref")]
        reference: Option<String>,
    },
    /// Open a change pull request writing local files to the same paths in the repository.
    Change {
        /// Commit and pull request title.
        #[arg(long, short = 'm')]
        title: String,
        /// Files to write, as repository-relative paths read from this directory.
        #[arg(required = true)]
        files: Vec<String>,
    },
    /// Merge a change pull request once its checks passed.
    Merge {
        /// Pull request number.
        number: u64,
    },
}

/// `yulius task …`.
#[derive(Debug, clap::Subcommand)]
pub enum TaskAction {
    /// The project's tasks.
    List {
        /// all, active, recent or archived.
        #[arg(long, default_value = "all")]
        scope: String,
    },
    /// One task.
    Get {
        /// Task number.
        number: u64,
    },
    /// Create and start a task (`-`: the request from stdin).
    Create {
        /// The request.
        prompt: String,
        /// Title.
        #[arg(long)]
        title: Option<String>,
        /// Workflow id or name.
        #[arg(long)]
        workflow: Option<String>,
        /// Start after this task is done.
        #[arg(long)]
        after: Option<u64>,
        /// Prepare it; it waits for Start now.
        #[arg(long)]
        prepared: bool,
    },
    /// Tell a task something (`-`: the text from stdin).
    Message {
        /// Task number.
        number: u64,
        /// The message.
        text: String,
    },
    /// Continue a stopped or finished task.
    Restart {
        /// Task number.
        number: u64,
    },
    /// Stop a task.
    Stop {
        /// Task number.
        number: u64,
    },
    /// Archive a task.
    Archive {
        /// Task number.
        number: u64,
    },
    /// Delete a task, its worktree and its local branch.
    Delete {
        /// Task number.
        number: u64,
    },
    /// A task's pull request and checks.
    Delivery {
        /// Task number.
        number: u64,
    },
    /// Read a task's transcript.
    Transcript {
        /// Task number.
        number: u64,
        /// First line.
        #[arg(long, default_value_t = 0)]
        from: u64,
    },
}

/// `yulius ci …`.
#[derive(Debug, clap::Subcommand)]
pub enum CiAction {
    /// The jobs in force and the latest runs.
    Runs,
    /// One run and its failing output.
    Run {
        /// Run id.
        run_id: String,
    },
    /// Run it again.
    Rerun {
        /// Run id.
        run_id: String,
    },
}

/// Exit code of a request that did not reach a tool.
const NOT_RUN: u8 = 2;

fn fail(command: &str, err: impl std::fmt::Display) -> ExitCode {
    eprintln!("yulius {command}: {err}");
    ExitCode::from(NOT_RUN)
}

fn parse_url(url: &str) -> Result<Url, String> {
    Url::parse(url.trim().trim_end_matches('/')).map_err(|e| format!("invalid URL {url:?}: {e}"))
}

fn store() -> Result<TokenStore, String> {
    TokenStore::default_location()
        .map(|store| store.remote())
        .map_err(|e| e.to_string())
}

/// The instance and a client for it: `--server`, else the only one stored.
fn client(opts: &RemoteOpts) -> Result<(Url, RemoteClient), String> {
    let store = store()?;
    let server = match opts.server.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(server) => parse_url(server)?,
        None => match store.servers().map_err(|e| e.to_string())?.as_slice() {
            [only] => parse_url(only)?,
            [] => return Err("no instance yet: run `yulius remote login <url>`".into()),
            many => {
                return Err(format!(
                    "tokens for several instances ({}); pass --server <url>",
                    many.join(", ")
                ));
            }
        },
    };
    let token = store
        .load(&server)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no token for {server}: run `yulius remote login {server}`"))?;
    let client = RemoteClient::new(&server, token).map_err(|e| e.to_string())?;
    Ok((server, client))
}

/// `--project`, else the GitHub `origin` of the checkout around the current directory.
fn project(opts: &RemoteOpts) -> Result<String, String> {
    if let Some(project) = opts.project.as_deref().filter(|p| !p.trim().is_empty()) {
        return Ok(project.trim().to_owned());
    }
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    discover_repo(&here)
        .map(|repo| format!("{}/{}", repo.owner, repo.name))
        .map_err(|_| "pass --project owner/name (this directory is not a GitHub checkout)".into())
}

/// `text`, or stdin when it is `-`.
fn text_arg(text: &str) -> Result<String, String> {
    if text != "-" {
        return Ok(text.to_owned());
    }
    let mut out = String::new();
    io::stdin()
        .take(1 << 20)
        .read_to_string(&mut out)
        .map_err(|e| format!("cannot read stdin: {e}"))?;
    Ok(out)
}

/// The text of the file at `path`, or stdin for `-`, at most 1 MiB.
fn file_arg(path: &str) -> Result<String, String> {
    if path == "-" {
        return text_arg(path);
    }
    let mut out = String::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(1 << 20).read_to_string(&mut out))
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    Ok(out)
}

/// Calls `tool` and prints its answer; `project` is added to `args` when the tool needs one.
fn run(command: &str, opts: &RemoteOpts, tool: &str, args: Value, with_project: bool) -> ExitCode {
    let mut args = match args {
        Value::Object(map) => map,
        Value::Null => Map::new(),
        _ => return fail(command, "the arguments must be a JSON object"),
    };
    if with_project && !args.contains_key("project") {
        match project(opts) {
            Ok(project) => {
                args.insert("project".into(), Value::String(project));
            }
            Err(err) => return fail(command, err),
        }
    }
    let (_, client) = match client(opts) {
        Ok(found) => found,
        Err(err) => return fail(command, err),
    };
    match client.call(tool, Value::Object(args)) {
        Ok(outcome) => {
            if opts.json && !outcome.is_error {
                let record = outcome.record.unwrap_or(Value::Null);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&record).unwrap_or_default()
                );
            } else if outcome.is_error {
                eprintln!("{}", outcome.text);
            } else {
                println!("{}", outcome.text);
            }
            if outcome.is_error {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(err) => fail(command, err),
    }
}

/// `yulius remote …`.
#[must_use]
pub fn remote(action: RemoteAction, opts: &RemoteOpts) -> ExitCode {
    match action {
        RemoteAction::Login { url, token_stdin } => login(&url, token_stdin),
        RemoteAction::Logout => {
            let result = (|| -> Result<String, String> {
                let (server, _) = client(opts)?;
                store()?.remove(&server).map_err(|e| e.to_string())?;
                Ok(format!(
                    "Forgot the token for {server}. Revoke it on Settings → Connect your local agent."
                ))
            })();
            match result {
                Ok(said) => {
                    println!("{said}");
                    ExitCode::SUCCESS
                }
                Err(err) => fail("remote logout", err),
            }
        }
        RemoteAction::Status => match client(opts).and_then(|(server, client)| {
            client
                .initialize()
                .map(|info| (server, info))
                .map_err(|e| e.to_string())
        }) {
            Ok((server, info)) => {
                let meta = &info["_meta"];
                println!(
                    "{server}: Yulius {}, signed in as {} ({} token)",
                    info["serverInfo"]["version"].as_str().unwrap_or("?"),
                    meta["yulius/person"].as_str().unwrap_or("?"),
                    meta["yulius/scope"].as_str().unwrap_or("?"),
                );
                ExitCode::SUCCESS
            }
            Err(err) => fail("remote status", err),
        },
        RemoteAction::Tools => {
            match client(opts).and_then(|(_, c)| c.tools().map_err(|e| e.to_string())) {
                Ok(tools) => {
                    if opts.json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&tools).unwrap_or_default()
                        );
                    } else {
                        for tool in tools {
                            let description = tool["description"].as_str().unwrap_or("");
                            let first = description.split(". ").next().unwrap_or(description);
                            println!("{:<28} {first}", tool["name"].as_str().unwrap_or("?"));
                        }
                    }
                    ExitCode::SUCCESS
                }
                Err(err) => fail("remote tools", err),
            }
        }
        RemoteAction::Call { tool, args } => {
            let args = match args.as_deref().map(text_arg).transpose() {
                Ok(text) => text,
                Err(err) => return fail("remote call", err),
            };
            let args: Value = match args.as_deref().map(serde_json::from_str).transpose() {
                Ok(args) => args.unwrap_or(Value::Null),
                Err(err) => {
                    return fail("remote call", format!("the arguments are not JSON: {err}"));
                }
            };
            // A project tool's `project` comes from --project or the checkout when absent.
            let wants_project = opts.project.is_some()
                || client(opts)
                    .ok()
                    .and_then(|(_, c)| c.tools().ok())
                    .and_then(|tools| {
                        tools
                            .into_iter()
                            .find(|t| t["name"] == tool.as_str())
                            .map(|t| {
                                t["inputSchema"]["required"]
                                    .as_array()
                                    .is_some_and(|r| r.iter().any(|p| p == "project"))
                            })
                    })
                    .unwrap_or(false);
            run("remote call", opts, &tool, args, wants_project)
        }
    }
}

/// `yulius remote login <url> [--token-stdin]`.
fn login(url: &str, token_stdin: bool) -> ExitCode {
    let result = (|| -> Result<String, String> {
        let server = parse_url(url)?;
        let token = if token_stdin || !io::stdin().is_terminal() {
            let mut text = String::new();
            io::stdin()
                .take(4096)
                .read_to_string(&mut text)
                .map_err(|e| format!("cannot read the token: {e}"))?;
            text
        } else {
            print!("Token for {server} (Settings → Connect your local agent): ");
            io::stdout().flush().map_err(|e| e.to_string())?;
            console::Term::stdout()
                .read_secure_line()
                .map_err(|e| format!("cannot read the token: {e}"))?
        };
        let token = Secret::new(token.trim().to_owned());
        if !token.expose().starts_with("yul_") {
            return Err("that is not a Yulius API token (they start with yul_)".into());
        }
        let client = RemoteClient::new(&server, token.clone()).map_err(|e| e.to_string())?;
        let info = client.initialize().map_err(|e| match e {
            RemoteError::Unauthenticated => {
                "the instance refused the token: check it, or make a new one on Settings".into()
            }
            other => other.to_string(),
        })?;
        store()?.save(&server, &token).map_err(|e| e.to_string())?;
        let meta = &info["_meta"];
        Ok(format!(
            "Signed in to {server} as {} ({} token). Try `yulius project list`.",
            meta["yulius/person"].as_str().unwrap_or("?"),
            meta["yulius/scope"].as_str().unwrap_or("?"),
        ))
    })();
    match result {
        Ok(said) => {
            println!("{said}");
            ExitCode::SUCCESS
        }
        Err(err) => fail("remote login", err),
    }
}

/// `yulius project …`.
#[must_use]
pub fn project_cmd(action: ProjectAction, opts: &RemoteOpts) -> ExitCode {
    let command = "project";
    match action {
        ProjectAction::List => run(command, opts, "projects_list", Value::Null, false),
        ProjectAction::Repositories => run(command, opts, "repositories_list", Value::Null, false),
        ProjectAction::Import { repo } => {
            let repo = match repo.map(Ok).unwrap_or_else(|| project(opts)) {
                Ok(repo) => repo,
                Err(err) => return fail(command, err),
            };
            run(
                command,
                opts,
                "project_import",
                json!({"repo": repo}),
                false,
            )
        }
        ProjectAction::Get => run(command, opts, "project_get", Value::Null, true),
        ProjectAction::Onboard { notes } => {
            let notes = match notes.as_deref().map(file_arg).transpose() {
                Ok(notes) => notes,
                Err(err) => return fail(command, err),
            };
            run(
                command,
                opts,
                "onboarding_start",
                notes.map_or(Value::Null, |notes| json!({"notes": notes})),
                true,
            )
        }
        ProjectAction::Files { path, reference } => run(
            command,
            opts,
            "project_files",
            json!({"path": path, "ref": reference}),
            true,
        ),
        ProjectAction::Cat { path, reference } => {
            if opts.json {
                return run(
                    command,
                    opts,
                    "project_file_read",
                    json!({"path": path, "ref": reference}),
                    true,
                );
            }
            // The file itself, as `cat` prints it.
            let (_, client) = match client(opts) {
                Ok(found) => found,
                Err(err) => return fail(command, err),
            };
            let project = match project(opts) {
                Ok(project) => project,
                Err(err) => return fail(command, err),
            };
            match client.call(
                "project_file_read",
                json!({"project": project, "path": path, "ref": reference}),
            ) {
                Ok(outcome) if !outcome.is_error => {
                    let content = outcome
                        .record
                        .as_ref()
                        .and_then(|r| r["content"].as_str())
                        .unwrap_or_default()
                        .to_owned();
                    print!("{content}");
                    ExitCode::SUCCESS
                }
                Ok(outcome) => {
                    eprintln!("{}", outcome.text);
                    ExitCode::FAILURE
                }
                Err(err) => fail(command, err),
            }
        }
        ProjectAction::Change { title, files } => {
            let mut list = Vec::new();
            for path in files {
                match std::fs::read_to_string(&path) {
                    Ok(content) => list.push(json!({"path": path, "content": content})),
                    Err(err) => return fail(command, format!("cannot read {path}: {err}")),
                }
            }
            run(
                command,
                opts,
                "project_change",
                json!({"title": title, "files": list}),
                true,
            )
        }
        ProjectAction::Merge { number } => run(
            command,
            opts,
            "change_merge",
            json!({"number": number}),
            true,
        ),
    }
}

/// `yulius task …`.
#[must_use]
pub fn task_cmd(action: TaskAction, opts: &RemoteOpts) -> ExitCode {
    let command = "task";
    let (tool, args) = match action {
        TaskAction::List { scope } => ("tasks_list", json!({"scope": scope})),
        TaskAction::Get { number } => ("task_get", json!({"number": number})),
        TaskAction::Create {
            prompt,
            title,
            workflow,
            after,
            prepared,
        } => {
            let prompt = match text_arg(&prompt) {
                Ok(prompt) => prompt,
                Err(err) => return fail(command, err),
            };
            let mut args = json!({"prompt": prompt});
            for (key, value) in [
                ("title", title.map(Value::from)),
                ("workflow", workflow.map(Value::from)),
                ("after_task", after.map(Value::from)),
                ("prepared", prepared.then_some(Value::Bool(true))),
            ] {
                if let Some(value) = value {
                    args[key] = value;
                }
            }
            ("task_create", args)
        }
        TaskAction::Message { number, text } => match text_arg(&text) {
            Ok(text) => ("task_message", json!({"number": number, "text": text})),
            Err(err) => return fail(command, err),
        },
        TaskAction::Restart { number } => ("task_restart", json!({"number": number})),
        TaskAction::Stop { number } => ("task_stop", json!({"number": number})),
        TaskAction::Archive { number } => ("task_archive", json!({"number": number})),
        TaskAction::Delete { number } => ("task_delete", json!({"number": number})),
        TaskAction::Delivery { number } => ("task_delivery", json!({"number": number})),
        TaskAction::Transcript { number, from } => {
            ("session_read", json!({"task_number": number, "from": from}))
        }
    };
    run(command, opts, tool, args, true)
}

/// `yulius ci …`.
#[must_use]
pub fn ci_cmd(action: CiAction, opts: &RemoteOpts) -> ExitCode {
    let (tool, args) = match action {
        CiAction::Runs => ("ci_runs", Value::Null),
        CiAction::Run { run_id } => ("ci_run", json!({"run_id": run_id})),
        CiAction::Rerun { run_id } => ("ci_rerun", json!({"run_id": run_id})),
    };
    run("ci", opts, tool, args, true)
}

/// `yulius secrets list`: the project's agent secrets, names and bindings only.
#[must_use]
pub fn secrets_list(opts: &RemoteOpts) -> ExitCode {
    run("secrets list", opts, "secrets_list", Value::Null, true)
}
