//! The `yulius` binary: server, process host, hooks, import and administration subcommands.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// Yulius: a self-hosted orchestrator of native Claude Code and Codex sessions.
#[derive(Debug, Parser)]
#[command(name = "yulius", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the Yulius server.
    Serve(yulius::serve::ServeArgs),
    /// Write a consistent copy of the database to a file.
    Backup {
        /// Data directory.
        #[arg(long, env = "YULIUS_DATA", default_value = "/var/lib/yulius")]
        data_dir: PathBuf,
        /// Destination file (must not exist).
        dest: PathBuf,
    },
    /// Print build information.
    Version,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Version => {
            println!("yulius {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Command::Serve(args) => run_async(yulius::serve::serve(args)),
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
