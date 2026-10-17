//! The `yulius` binary: server, process host, hooks, onboarding (import) and administration
//! subcommands.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// Yulius: a self-hosted orchestrator of native Claude Code sessions.
#[derive(Debug, Parser)]
#[command(name = "yulius", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the Yulius server.
    Serve(Box<yulius::serve::ServeArgs>),
    /// Write a consistent copy of the database to a file.
    Backup {
        /// Data directory.
        #[arg(
            long = "data-dir",
            visible_alias = "data",
            env = "YULIUS_DATA",
            default_value = "/var/lib/yulius"
        )]
        data_dir: PathBuf,
        /// Destination file (must not exist).
        dest: PathBuf,
    },
    /// Internal: run the detached PTY host of one agent run.
    #[command(hide = true)]
    Ptyhost {
        /// Run directory holding launch.json.
        #[arg(long)]
        run_dir: PathBuf,
    },
    /// Internal: forward a Claude Code hook to Yulius (always exits 0).
    #[command(hide = true)]
    Hook {
        /// Harness (`claude`).
        harness: String,
        /// Hook event name.
        event: String,
    },
    /// Internal: git credential helper for task worktrees.
    #[command(name = "git-credential", hide = true)]
    GitCredential {
        /// Operation (`get`, `store`, `erase`).
        operation: String,
    },
    /// Internal: print the user's GitHub token for the `gh` shim of sandbox shells.
    #[command(name = "gh-token", hide = true)]
    GhToken,
    /// In a task's session: wait until the Yulius CI checks at this commit finish, the task's
    /// delivery moves or an armed merge is decided (start it in the background and end the turn).
    Wait {
        /// Give up after this many minutes.
        #[arg(long, default_value_t = 90)]
        minutes: u64,
    },
    /// Scan this repository with Vidoc's vidoc-1 (SPEC-195): the branch's changes against the
    /// integration branch, or the whole repository with --all; exit 0 passed, 1 failed, 2 not run.
    /// `yulius security-scan --help` lists the options.
    #[command(name = "security-scan", disable_help_flag = true)]
    SecurityScan {
        /// The scanner's options (--all, --base REF, --ci, --report FILE, --max-bytes N,
        /// --deadline-minutes N, --jobs N, --help).
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<std::ffi::OsString>,
    },
    /// In a task's session: remove files this task created (untracked or ignored paths of its
    /// worktree, paths of the session's own `/`, `/tmp`, `/run` and `/dev/shm`), refusing anything
    /// else.
    Remove {
        /// Files or directories to remove; a link is removed, not what it points to.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// Internal: a sandbox's namespace holder (SPEC-002 ADR-22).
    #[command(name = "sandbox-host", hide = true)]
    SandboxHost {
        /// What to do.
        #[command(subcommand)]
        action: yulius::sandbox_cli::HostAction,
    },
    /// Internal: exec `bwrap` with the sandbox's seccomp filter.
    #[command(name = "sandbox-exec", hide = true)]
    SandboxExec {
        /// The sandbox's container engine: its seccomp profile (ADR-0151).
        #[arg(long)]
        engine: bool,
        /// Join this cgroup directory first.
        #[arg(long)]
        cgroup: Option<std::path::PathBuf>,
        /// Join the sandbox described by this file first (a `SandboxJoin`, as the ptyhost does;
        /// `yulius doctor --docker-smoke`).
        #[arg(long)]
        join: Option<std::path::PathBuf>,
        /// `bwrap` and its arguments, after `--`.
        #[arg(last = true, required = true)]
        argv: Vec<std::ffi::OsString>,
    },
    /// Build or show the base of sandboxes' system layers (run as the service user).
    #[command(name = "sandbox-base")]
    SandboxBase {
        /// What to do.
        #[command(subcommand)]
        action: yulius::sandbox_cli::BaseAction,
    },
    /// Sign in this computer's CLI to a Yulius instance (browser approval).
    Login {
        /// Yulius URL, e.g. https://yulius.quartio.ai.
        #[arg(long, env = "YULIUS_SERVER")]
        server: String,
    },
    /// Onboard a local repository and your Claude Code sessions to Yulius: progress bars,
    /// secrets redacted on this computer, only new or changed sessions sent, onboarding state.
    #[command(visible_alias = "import")]
    Onboard {
        /// Repository path (default: current directory).
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Yulius URL (default: the only server you signed in to).
        #[arg(long, env = "YULIUS_SERVER")]
        server: Option<String>,
        /// Which sessions: all, recent, pick or none (asked when omitted; recent with --yes).
        #[arg(long, value_name = "WHICH")]
        sessions: Option<String>,
        /// Window of recent sessions, e.g. 14d, 36h, 2w.
        #[arg(long)]
        since: Option<String>,
        /// Upload sessions only, not unpublished branches or uncommitted changes.
        #[arg(long)]
        sessions_only: bool,
        /// Show what would be uploaded and upload nothing.
        #[arg(long)]
        dry_run: bool,
        /// Only show the sandbox's onboarding state and what is pending.
        #[arg(long)]
        status: bool,
        /// List every redacted secret's rule and key name.
        #[arg(long)]
        show_redactions: bool,
        /// Upload into a playground of yours (local Git on the server) instead of a GitHub
        /// repository's sandbox: no GitHub remote needed; every branch, the uncommitted work and
        /// the chosen sessions are sent, and nothing is pushed anywhere.
        #[arg(long)]
        playground: bool,
        /// The playground's name with --playground (default: the checkout directory's name); an
        /// existing playground of yours with that name receives the upload.
        #[arg(long, requires = "playground")]
        name: Option<String>,
        /// Do not ask anything.
        #[arg(long)]
        yes: bool,
        /// No colours (also NO_COLOR=1).
        #[arg(long)]
        no_color: bool,
    },
    /// List and delete leftovers on this server's disk (SPEC-215), as root or the service user,
    /// through the running server's loopback listener and its rules.
    Leftovers {
        /// Data directory (the server's key for this command is in it).
        #[arg(
            long = "data-dir",
            visible_alias = "data",
            env = "YULIUS_DATA",
            default_value = "/var/lib/yulius",
            global = true
        )]
        data_dir: PathBuf,
        /// The server's loopback listener.
        #[arg(long, default_value = yulius::leftovers_cli::DEFAULT_URL, global = true)]
        url: String,
        /// What to do.
        #[command(subcommand)]
        action: LeftoversAction,
    },
    /// Manage the vault from this computer.
    Secrets {
        /// What to do.
        #[command(subcommand)]
        action: SecretsAction,
    },
    /// Check this host's prerequisites (git, agent CLIs, containment, browser, editor, data dir).
    Doctor {
        /// Prove that sandboxes get Docker on this host, end to end, in a throwaway sandbox
        /// (SPEC-196): run it as root or the service user; it uses the current base.
        #[arg(long)]
        docker_smoke: bool,
        /// With --docker-smoke: one JSON line per check (what Settings → Instance reads).
        #[arg(long, requires = "docker_smoke")]
        json: bool,
        /// Data directory.
        #[arg(
            long = "data-dir",
            visible_alias = "data",
            env = "YULIUS_DATA",
            default_value = "/var/lib/yulius"
        )]
        data: PathBuf,
        /// Require bubblewrap containment.
        #[arg(long, env = "YULIUS_CONTAINED")]
        contained: bool,
        /// Extra paths visible inside containment (as for `serve`).
        #[arg(long, env = "YULIUS_CONTAINED_READONLY", value_delimiter = ':')]
        contained_readonly: Vec<PathBuf>,
        /// The reverse proxy's secret file (as for `serve`): checked, never shown.
        #[arg(long, env = "YULIUS_PROXY_SECRET_FILE")]
        proxy_secret_file: Option<PathBuf>,
    },
    /// Bring this host to every prerequisite of Docker in sandboxes (run as root; SPEC-203).
    Host {
        /// What to do.
        #[command(subcommand)]
        action: HostAction,
    },
    /// Print build information.
    Version,
    /// Root helper: build or install the commit the server requested (yulius-update.service).
    #[command(name = "update-helper", hide = true)]
    UpdateHelper(Box<yulius::update_helper::HelperArgs>),
}

#[derive(Debug, Subcommand)]
enum LeftoversAction {
    /// Rescan and list every leftover: id, size, safe / needs you / report only, kind, path,
    /// owner and why.
    List {
        /// The server's JSON instead of the list.
        #[arg(long)]
        json: bool,
    },
    /// Rescan, then delete every safe leftover or the given ids, after a typed yes.
    Delete {
        /// Every leftover Yulius deletes by itself.
        #[arg(long, conflicts_with = "ids")]
        all_safe: bool,
        /// Ids from `yulius leftovers list`.
        ids: Vec<String>,
        /// Do not ask (scripts).
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
enum SecretsAction {
    /// Import secrets from a dotenv file, stdin or typed prompts (SPEC-205): team secrets for
    /// people, or agent secrets, of a sandbox or team-wide (SPEC-216). Names are shown, values
    /// never; a secret that exists is skipped, never overwritten; nothing is sent before you type
    /// yes at the terminal.
    Import {
        /// Team secrets (for people) or agent secrets (for agents, who never see them).
        #[arg(long, value_enum)]
        to: yulius::cli::VaultChoice,
        /// A dotenv file (NAME=value lines; a comment above a key becomes its description), `-`
        /// for stdin; without it, type each name and its hidden value.
        file: Option<PathBuf>,
        /// The sandbox of the secrets: owner/name, slug or id (default: this checkout's GitHub
        /// repository).
        #[arg(long)]
        sandbox: Option<String>,
        /// Team-wide secrets, for every sandbox (agent secrets: instance administrators).
        #[arg(long)]
        instance: bool,
        /// Agent secrets: you allow each host and production deploys on the Secrets page,
        /// instead of agents setting them up (the default).
        #[arg(long)]
        confirm_hosts: bool,
        /// Yulius URL (default: the only server you signed in to).
        #[arg(long, env = "YULIUS_SERVER")]
        server: Option<String>,
        /// No colours (also NO_COLOR=1).
        #[arg(long)]
        no_color: bool,
    },
}

#[derive(Debug, Subcommand)]
enum HostAction {
    /// Install and fix what sandboxes need for Docker (Docker Engine with buildx and compose, the
    /// host's own Docker units stopped, kernel modules, a needrestart rule, the service user and
    /// its ids, the unit's delegation, the binary's owner, the base), then run the contained
    /// doctor and the Docker smoke test. Idempotent; safe while sandboxes run.
    Setup {
        /// Data directory.
        #[arg(
            long = "data-dir",
            visible_alias = "data",
            env = "YULIUS_DATA",
            default_value = "/var/lib/yulius"
        )]
        data: PathBuf,
        /// The service user to create when the data directory does not exist yet.
        #[arg(long, default_value = "yulius")]
        user: String,
        /// The public URL, to write yulius.service when it is not installed.
        #[arg(long)]
        public_url: Option<String>,
        /// Change nothing; print what would change.
        #[arg(long)]
        dry_run: bool,
        /// Skip the Docker smoke test at the end (it pulls from Docker Hub).
        #[arg(long)]
        no_smoke: bool,
        /// The server's systemd unit.
        #[arg(long, env = "YULIUS_UPDATE_SERVICE", default_value = "yulius.service")]
        service: String,
        /// The update helper's state directory: the report is kept there when it exists.
        #[arg(
            long,
            env = "YULIUS_UPDATE_STATE",
            default_value = "/var/lib/yulius-update"
        )]
        update_state: PathBuf,
    },
    /// Check the same prerequisites, applying only safe fixes (the update helper runs it after
    /// each update).
    Check {
        /// Data directory.
        #[arg(
            long = "data-dir",
            visible_alias = "data",
            env = "YULIUS_DATA",
            default_value = "/var/lib/yulius"
        )]
        data: PathBuf,
        /// Print one JSON report instead of lines.
        #[arg(long)]
        json: bool,
        /// The server's systemd unit.
        #[arg(long, env = "YULIUS_UPDATE_SERVICE", default_value = "yulius.service")]
        service: String,
        /// The update helper's state directory: the report is kept there when it exists.
        #[arg(
            long,
            env = "YULIUS_UPDATE_STATE",
            default_value = "/var/lib/yulius-update"
        )]
        update_state: PathBuf,
    },
}

fn host(action: HostAction) -> ExitCode {
    use yulius::host_setup::{Mode, Options, main};
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("/usr/local/bin/yulius"));
    match action {
        HostAction::Setup {
            data,
            user,
            public_url,
            dry_run,
            no_smoke,
            service,
            update_state,
        } => main(
            if dry_run { Mode::DryRun } else { Mode::Setup },
            Options {
                data_dir: data,
                user,
                public_url,
                exe,
                smoke: !no_smoke,
                service,
                update_state: Some(update_state),
            },
            false,
        ),
        HostAction::Check {
            data,
            json,
            service,
            update_state,
        } => main(
            Mode::Check,
            Options {
                data_dir: data,
                user: "yulius".into(),
                public_url: None,
                exe,
                smoke: false,
                service,
                update_state: Some(update_state),
            },
            json,
        ),
    }
}

fn main() -> ExitCode {
    // Claude Code's `CLAUDE_CODE_SHELL_PREFIX` runs `yulius --scrub-exec '<command>'` for every
    // Bash command of an agent run: the command's output is redacted of agent secrets
    // (SPEC-002 ADR-20). Handled before argument parsing: the command is opaque text.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some(yulius_app::vault::SCRUB_EXEC_FLAG) {
        // Under memory pressure the kernel kills the agent's command (a build it can rerun)
        // before the agent itself: rust-lld's OOM killed `claude` and ended tasks #10 and #15
        // (F 2026-10-12). Raising one's own score needs no privilege; children inherit it.
        let _ignored = std::fs::write("/proc/self/oom_score_adj", "500");
        let code =
            yulius_harness::scrub_client::run_scrub_exec(&args[1..], &|k| std::env::var(k).ok());
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    let cli = Cli::parse();
    match cli.command {
        Command::Ptyhost { run_dir } => match yulius_ptyhost::run_host(&run_dir) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("yulius ptyhost: {err}");
                ExitCode::FAILURE
            }
        },
        Command::Hook { harness, event } => {
            let code = yulius_harness::hook_client::run_hook(
                &harness,
                &event,
                &mut std::io::stdin(),
                &mut std::io::stdout(),
                &|k| std::env::var(k).ok(),
            );
            ExitCode::from(u8::try_from(code).unwrap_or(0))
        }
        Command::GitCredential { operation } => {
            let code = yulius_git::run_credential_helper(
                &operation,
                &mut std::io::stdin(),
                &mut std::io::stdout(),
                &|k| std::env::var(k).ok(),
            );
            ExitCode::from(u8::try_from(code).unwrap_or(1))
        }
        Command::Wait { minutes } => {
            let code =
                yulius::wait_cli::run(&mut std::io::stdout(), minutes, &|k| std::env::var(k).ok());
            ExitCode::from(u8::try_from(code).unwrap_or(1))
        }
        Command::SecurityScan { args } => yulius::security_scan::run(&args),
        Command::Remove { paths } => {
            let code =
                yulius::remove_cli::run(&mut std::io::stdout(), &paths, &|k| std::env::var(k).ok());
            ExitCode::from(u8::try_from(code).unwrap_or(1))
        }
        Command::GhToken => {
            let code = yulius_git::run_gh_token(&mut std::io::stdout(), &|k| std::env::var(k).ok());
            ExitCode::from(u8::try_from(code).unwrap_or(1))
        }
        Command::SandboxHost { action } => yulius::sandbox_cli::host(action),
        Command::SandboxExec {
            engine,
            cgroup,
            join,
            argv,
        } => yulius::sandbox_cli::exec(engine, cgroup.as_deref(), join.as_deref(), &argv),
        Command::SandboxBase { action } => yulius::sandbox_cli::base(action),
        Command::Login { server } => yulius::cli::login(&server),
        Command::Onboard {
            path,
            server,
            sessions,
            since,
            sessions_only,
            dry_run,
            status,
            show_redactions,
            playground,
            name,
            yes,
            no_color,
        } => yulius::cli::onboard(yulius::cli::OnboardArgs {
            path,
            server,
            sessions,
            since,
            yes,
            sessions_only,
            dry_run,
            status,
            show_redactions,
            playground,
            playground_name: name,
            no_color,
        }),
        Command::Leftovers {
            data_dir,
            url,
            action,
        } => {
            let action = match action {
                LeftoversAction::List { json } => yulius::leftovers_cli::Action::List { json },
                LeftoversAction::Delete { all_safe, ids, yes } => {
                    yulius::leftovers_cli::Action::Delete { all_safe, ids, yes }
                }
            };
            let code = yulius::leftovers_cli::run(&data_dir, &url, action);
            ExitCode::from(u8::try_from(code).unwrap_or(1))
        }
        Command::Secrets {
            action:
                SecretsAction::Import {
                    to,
                    file,
                    sandbox,
                    instance,
                    confirm_hosts,
                    server,
                    no_color,
                },
        } => yulius::cli::secrets_import(yulius::cli::SecretsImportArgs {
            to,
            file,
            sandbox,
            instance,
            confirm_hosts,
            server,
            no_color,
        }),
        Command::Doctor {
            docker_smoke: true,
            data,
            json,
            ..
        } => yulius::docker_smoke::main(&data, json),
        Command::Doctor {
            docker_smoke: false,
            data,
            contained,
            contained_readonly,
            proxy_secret_file,
            ..
        } => yulius::doctor::doctor(
            &data,
            contained,
            &contained_readonly,
            proxy_secret_file.as_deref(),
        ),
        Command::Host { action } => host(action),
        Command::Version => {
            let commit = env!("YULIUS_BUILD_COMMIT");
            if commit.is_empty() {
                println!("yulius {}", env!("CARGO_PKG_VERSION"));
            } else {
                println!("yulius {} ({commit})", env!("CARGO_PKG_VERSION"));
            }
            ExitCode::SUCCESS
        }
        Command::UpdateHelper(args) => yulius::update_helper::main(&args),
        Command::Serve(args) => run_async(yulius::serve::serve(*args)),
        Command::Backup { data_dir, dest } => run_async(yulius::serve::backup(data_dir, dest)),
    }
}

fn run_async<F>(future: F) -> ExitCode
where
    F: std::future::Future<Output = Result<(), String>>,
{
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("yulius: failed to start runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(future) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("yulius: {err}");
            ExitCode::FAILURE
        }
    }
}
