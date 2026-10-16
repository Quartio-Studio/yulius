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
    /// Check this host's prerequisites (git, agent CLIs, containment, browser, editor, data dir).
    Doctor {
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
    /// Print build information.
    Version,
    /// Root helper: build or install the commit the server requested (yulius-update.service).
    #[command(name = "update-helper", hide = true)]
    UpdateHelper(Box<yulius::update_helper::HelperArgs>),
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
        Command::GhToken => {
            let code = yulius_git::run_gh_token(&mut std::io::stdout(), &|k| std::env::var(k).ok());
            ExitCode::from(u8::try_from(code).unwrap_or(1))
        }
        Command::SandboxHost { action } => yulius::sandbox_cli::host(action),
        Command::SandboxExec {
            engine,
            cgroup,
            argv,
        } => yulius::sandbox_cli::exec(engine, cgroup.as_deref(), &argv),
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
        Command::Doctor {
            data,
            contained,
            contained_readonly,
            proxy_secret_file,
        } => yulius::doctor::doctor(
            &data,
            contained,
            &contained_readonly,
            proxy_secret_file.as_deref(),
        ),
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
