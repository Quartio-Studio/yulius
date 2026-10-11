//! The `yulius` binary: server, process host, hooks, import and administration subcommands.

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
    /// Internal: a sandbox's namespace holder (SPEC-002 ADR-20).
    #[command(name = "sandbox-host", hide = true)]
    SandboxHost {
        /// What to do.
        #[command(subcommand)]
        action: yulius::sandbox_cli::HostAction,
    },
    /// Internal: exec `bwrap` with the sandbox's seccomp filter.
    #[command(name = "sandbox-exec", hide = true)]
    SandboxExec {
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
    /// Import a local repository and its recent Claude Code sessions into Yulius.
    Import {
        /// Repository path (default: current directory).
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Yulius URL.
        #[arg(long, env = "YULIUS_SERVER")]
        server: String,
        /// Session window, e.g. 14d, 36h, 2w (prompted when omitted).
        #[arg(long)]
        since: Option<String>,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
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
    },
    /// Print build information.
    Version,
}

fn main() -> ExitCode {
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
        Command::SandboxHost { action } => yulius::sandbox_cli::host(action),
        Command::SandboxExec { argv } => yulius::sandbox_cli::exec(&argv),
        Command::SandboxBase { action } => yulius::sandbox_cli::base(action),
        Command::Login { server } => yulius::cli::login(&server),
        Command::Import {
            path,
            server,
            since,
            yes,
        } => yulius::cli::import(path, &server, since, yes),
        Command::Doctor {
            data,
            contained,
            contained_readonly,
        } => yulius::doctor::doctor(&data, contained, &contained_readonly),
        Command::Version => {
            println!("yulius {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
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
