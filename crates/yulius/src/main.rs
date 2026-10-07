//! The `yulius` binary: server, ptyhost, hooks, import and administration subcommands.

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
    /// Print build information.
    Version,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Version => println!("yulius {}", env!("CARGO_PKG_VERSION")),
    }
}
