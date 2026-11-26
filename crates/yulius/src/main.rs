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
        /// Give up after this many minutes, only while nothing it waits for is under way (no
        /// run at the head queued or running, no armed merge, no deploy).
        #[arg(long, default_value_t = 90)]
        minutes: u64,
    },
    /// Internal: the stdio bridge of a task run's Yulius MCP (SPEC-358), started by Claude Code.
    #[command(hide = true)]
    Mcp,
    /// In a task's session: call one Yulius tool (as its `yulius` MCP server would) and print
    /// its text; for when the session's Yulius MCP tools are unavailable (SPEC-432).
    Tool {
        /// The tool, e.g. `delivery_merge`.
        name: String,
        /// Its arguments as one JSON object, e.g. '{"reviewed_head": "<sha>"}'.
        arguments: Option<String>,
    },
    /// In a task's session: run a brokered command of the project (SPEC-626) outside the sandbox,
    /// with its key as a file you never see; prints its stdout and stderr and exits with its
    /// code (125: refused, rejected or not answered in time).
    #[command(disable_help_flag = true)]
    Run {
        /// The command's name (`commands_list`).
        name: String,
        /// Its arguments, after the command's fixed ones.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
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
        #[command(flatten)]
        options: yulius::remove_cli::RemovalOptions,
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
    /// Internal: exec Git confined to its own programs and the given folders (ADR-0322).
    #[cfg(target_os = "linux")]
    #[command(name = "git-confined", hide = true)]
    GitConfined {
        /// A folder Git may read and write below (repeatable).
        #[arg(long = "rw")]
        writable: Vec<std::path::PathBuf>,
        /// A folder or file Git may also read (repeatable).
        #[arg(long = "ro")]
        readable: Vec<std::path::PathBuf>,
        /// Git may start the system's shell (a transfer between two local repositories).
        #[arg(long)]
        shell: bool,
        /// The Git executable and its arguments.
        #[arg(last = true, required = true)]
        argv: Vec<std::ffi::OsString>,
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
        /// The run's guarded paths (SPEC-652): a JSON list of `GuardPath`, opened without
        /// following a link and handed to bwrap through its argument descriptors.
        #[arg(long)]
        guard: Option<String>,
        /// Start bwrap without the seccomp filter (a run that does not join a sandbox's holder).
        #[arg(long)]
        no_filter: bool,
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
    Onboard(yulius::cli::OnboardArgs),
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
        action: yulius::leftovers_cli::Action,
    },
    /// Manage the vault from this computer.
    #[command(visible_alias = "secret")]
    Secrets {
        /// What to do.
        #[command(subcommand)]
        action: SecretsAction,
    },
    /// Drive a Yulius instance from this computer with a personal API token (SPEC-247): log in,
    /// then call any tool its remote MCP serves.
    Remote {
        #[command(flatten)]
        opts: yulius::remote_cli::RemoteOpts,
        /// What to do.
        #[command(subcommand)]
        action: yulius::remote_cli::RemoteAction,
    },
    /// The project's variables (SPEC-275): its non-secret values (regions, environment names,
    /// ids, base URLs), in plain, in every task run's environment.
    #[command(visible_alias = "var")]
    Vars {
        #[command(flatten)]
        opts: yulius::remote_cli::RemoteOpts,
        /// What to do.
        #[command(subcommand)]
        action: yulius::remote_cli::VarsAction,
    },
    /// A project of the instance `yulius remote login` stored: import, onboard, read and change
    /// its files.
    Project {
        #[command(flatten)]
        opts: yulius::remote_cli::RemoteOpts,
        /// What to do.
        #[command(subcommand)]
        action: yulius::remote_cli::ProjectAction,
    },
    /// The project's tasks: create, list, message, restart, stop, archive, delete.
    Task {
        #[command(flatten)]
        opts: yulius::remote_cli::RemoteOpts,
        /// What to do.
        #[command(subcommand)]
        action: yulius::remote_cli::TaskAction,
    },
    /// The project's Yulius CI runs.
    Ci {
        #[command(flatten)]
        opts: yulius::remote_cli::RemoteOpts,
        /// What to do.
        #[command(subcommand)]
        action: yulius::remote_cli::CiAction,
    },
    /// Check this host's prerequisites (git, agent CLIs, containment, browser, editor, data dir).
    Doctor(Box<yulius::doctor::DoctorArgs>),
    /// Bring this host to every prerequisite of Docker in sandboxes (run as root; SPEC-203).
    Host {
        /// What to do.
        #[command(subcommand)]
        action: HostAction,
    },
    /// Copy the whole instance off this host, or install such a copy (run as root; SPEC-329).
    ///
    /// Straight to a new VM: `sudo yulius instance export | ssh root@new-vm yulius instance
    /// import`. To a file and back: `ssh root@old-vm yulius instance export > copy.tar`, then
    /// `ssh root@new-vm yulius instance import < copy.tar`.
    Instance {
        /// What to do.
        #[command(subcommand)]
        action: InstanceAction,
    },
    /// Print build information.
    Version,
    /// Apply this release's migrations to a copy of a store file and print its schema, from the
    /// one it had (staging's deploy measures a migration with it, SPEC-591).
    #[command(name = "store-migrate", hide = true)]
    StoreMigrate {
        /// The store file (a copy): it must exist.
        #[arg(long)]
        db: PathBuf,
    },
    /// Root helper: build or install the commit the server requested (yulius-update.service).
    #[command(name = "update-helper", hide = true)]
    UpdateHelper(Box<yulius::update_helper::HelperArgs>),
    /// Root helper: run the move Settings → Instance asked for (yulius-migrate.service).
    #[command(name = "migrate-helper", hide = true)]
    MigrateHelper(Box<yulius::migrate_helper::MoveHelperArgs>),
}

#[derive(Debug, Subcommand)]
enum InstanceAction {
    /// Freeze every Yulius process, then stream the whole data directory to standard output as
    /// one tar of a size announced before the first byte; nothing is written on this host.
    Export(yulius::instance::export::ExportOptions),
    /// Install an export on this host: makes the service user and its ids, extracts the copy
    /// into the exported data directory as it arrives and checks it; never overwrites an
    /// instance. Then run `yulius host setup`.
    Import(yulius::instance::import::ImportOptions),
    /// Move the whole instance to another host with one command: check and set up the new
    /// host, carry every setting and key, copy the data while this instance runs, freeze it
    /// only to send what changed, prove the copy, start the new instance, redirect the old
    /// URL. Run it on the old host; run it again to continue.
    Migrate(Box<yulius::instance::migrate::Options>),
    /// A move's steps on the new host; run by `yulius instance migrate` over SSH.
    #[command(hide = true)]
    MigrateTarget {
        /// Install the copy arriving on standard input.
        #[arg(long)]
        receive: bool,
    },
}

#[derive(Debug, Subcommand)]
enum SecretsAction {
    /// Import secrets from a dotenv file, key files, stdin or typed prompts (SPEC-205): team
    /// secrets for people, or agent secrets, of a sandbox or team-wide (SPEC-216). Names are
    /// shown, values never; a secret that exists is skipped, never overwritten; nothing is sent
    /// before you type yes at the terminal, or with --yes and your agent's operate token
    /// (SPEC-256).
    Import(yulius::cli::SecretsImportArgs),
    /// Download the newest saved copy of an agent state (SPEC-271) as a .tar.gz, to a new file;
    /// the sandbox's copy stays the one written to.
    StateGet {
        /// The state's NAME.
        name: String,
        /// The file to write (refused when it exists).
        #[arg(long)]
        out: PathBuf,
        /// The project: owner/name, slug or id (default: this checkout's GitHub repository).
        #[arg(long)]
        sandbox: Option<String>,
        /// Yulius URL (default: the only server you signed in to).
        #[arg(long, env = "YULIUS_SERVER")]
        server: Option<String>,
    },
    /// The project's agent secrets through the remote MCP: names, descriptions and bound hosts,
    /// never values (SPEC-247).
    List {
        #[command(flatten)]
        opts: yulius::remote_cli::RemoteOpts,
    },
    /// Delete one of the project's agent secrets by NAME through the remote MCP, for example one
    /// imported by mistake (SPEC-274); audited, the value is never shown.
    Delete {
        /// The secret's NAME (`yulius secrets list`).
        name: String,
        #[command(flatten)]
        opts: yulius::remote_cli::RemoteOpts,
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
    /// Move the sessions an earlier layout left in the server unit's own cgroup into its
    /// sessions unit. Run as root by `<unit>-adopt.service` before every start of the server.
    #[command(hide = true)]
    AdoptSessions {
        /// The server's systemd unit.
        #[arg(long, default_value = "yulius.service")]
        service: String,
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
        HostAction::AdoptSessions { service } => {
            yulius::host_setup::session_units::adopt_main(&service)
        }
    }
}

fn main() -> ExitCode {
    // Claude Code's `CLAUDE_CODE_SHELL_PREFIX` (the run's `scrub-exec` script, SPEC-358) runs
    // `yulius --scrub-exec '<command>'` for every Bash command, hook and stdio MCP server of an
    // agent run: the command's output is redacted of agent secrets (SPEC-002 ADR-20). Handled
    // before argument parsing: the command is opaque text.
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
        Command::Mcp => {
            let code = yulius::mcp_bridge::run(
                &mut std::io::stdin().lock(),
                Box::new(std::io::stdout()),
                Box::new(|k| std::env::var(k).ok()),
                yulius::mcp_bridge::Settings::default(),
            );
            ExitCode::from(u8::try_from(code).unwrap_or(1))
        }
        Command::Tool { name, arguments } => {
            let code = yulius::tool_cli::run(
                &mut std::io::stdout(),
                &name,
                arguments.as_deref(),
                yulius::tool_cli::Pace::default(),
                &|k| std::env::var(k).ok(),
            );
            ExitCode::from(u8::try_from(code).unwrap_or(1))
        }
        Command::Run { name, args } => {
            let code = yulius::run_cli::run(
                &mut std::io::stdout(),
                &mut std::io::stderr(),
                &name,
                &args,
                yulius::tool_cli::Pace::default(),
                &|k| std::env::var(k).ok(),
            );
            ExitCode::from(u8::try_from(code).unwrap_or(1))
        }
        Command::SecurityScan { args } => yulius::security_scan::run(&args),
        Command::Remove { options, paths } => {
            let code = options.run(&mut std::io::stdout(), &paths, &|k| std::env::var(k).ok());
            ExitCode::from(u8::try_from(code).unwrap_or(1))
        }
        Command::GhToken => {
            let code = yulius_git::run_gh_token(&mut std::io::stdout(), &|k| std::env::var(k).ok());
            ExitCode::from(u8::try_from(code).unwrap_or(1))
        }
        Command::SandboxHost { action } => yulius::sandbox_cli::host(action),
        #[cfg(target_os = "linux")]
        Command::GitConfined {
            writable,
            readable,
            shell,
            argv,
        } => {
            let Some((git, args)) = argv.split_first() else {
                return ExitCode::from(2);
            };
            let error = yulius_git::confine::exec_confined(
                &writable,
                &readable,
                shell,
                std::path::Path::new(git),
                args,
            );
            eprintln!("yulius: {error}");
            ExitCode::from(126)
        }
        Command::SandboxExec {
            engine,
            cgroup,
            join,
            guard,
            no_filter,
            argv,
        } => yulius::sandbox_cli::exec(
            yulius::sandbox_cli::Exec {
                engine,
                cgroup: cgroup.as_deref(),
                join: join.as_deref(),
                guard: guard.as_deref(),
                filter: !no_filter,
            },
            &argv,
        ),
        Command::SandboxBase { action } => yulius::sandbox_cli::base(action),
        Command::Login { server } => yulius::cli::login(&server),
        Command::Onboard(args) => yulius::cli::onboard(args),
        Command::Leftovers {
            data_dir,
            url,
            action,
        } => {
            let code = yulius::leftovers_cli::run(&data_dir, &url, action);
            ExitCode::from(u8::try_from(code).unwrap_or(1))
        }
        Command::Secrets {
            action: SecretsAction::Import(args),
        } => yulius::cli::secrets_import(args),
        Command::Secrets {
            action:
                SecretsAction::StateGet {
                    name,
                    out,
                    sandbox,
                    server,
                },
        } => yulius::cli::state_get(&name, &out, sandbox.as_deref(), server.as_deref()),
        Command::Secrets {
            action: SecretsAction::List { opts },
        } => yulius::remote_cli::secrets_list(&opts),
        Command::Secrets {
            action: SecretsAction::Delete { name, opts },
        } => yulius::remote_cli::secrets_delete(&name, &opts),
        Command::Remote { opts, action } => yulius::remote_cli::remote(action, &opts),
        Command::Vars { opts, action } => yulius::remote_cli::vars_cmd(action, &opts),
        Command::Project { opts, action } => yulius::remote_cli::project_cmd(action, &opts),
        Command::Task { opts, action } => yulius::remote_cli::task_cmd(action, &opts),
        Command::Ci { opts, action } => yulius::remote_cli::ci_cmd(action, &opts),
        Command::Doctor(args) if args.docker_smoke => {
            yulius::docker_smoke::main(&args.data, args.json)
        }
        Command::Doctor(args) => yulius::doctor::doctor(
            &args.data,
            args.contained,
            &args.contained_readonly,
            args.proxy_secret_file.as_deref(),
            args.claude_updates,
        ),
        Command::Host { action } => host(action),
        Command::Instance {
            action: InstanceAction::Export(options),
        } => yulius::instance::export::main(&options),
        Command::Instance {
            action: InstanceAction::Import(options),
        } => yulius::instance::import::main(&options),
        Command::Instance {
            action: InstanceAction::Migrate(options),
        } => yulius::instance::migrate::main(&options),
        Command::Instance {
            action: InstanceAction::MigrateTarget { receive },
        } => yulius::instance::migrate::target::main(receive),
        Command::Version => {
            let commit = env!("YULIUS_BUILD_COMMIT");
            if commit.is_empty() {
                println!("yulius {}", env!("CARGO_PKG_VERSION"));
            } else {
                println!("yulius {} ({commit})", env!("CARGO_PKG_VERSION"));
            }
            ExitCode::SUCCESS
        }
        Command::StoreMigrate { db } => match yulius_store::migrate_file(&db) {
            Ok((from, to)) => {
                println!("schema {to} (from {from})");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("yulius store-migrate: {}: {err}", db.display());
                ExitCode::FAILURE
            }
        },
        Command::UpdateHelper(args) => yulius::update_helper::main(&args),
        Command::MigrateHelper(args) => yulius::migrate_helper::main(&args),
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

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser as _};
    use yulius::instance::migrate::Action;
    use yulius::migrate_helper::{MoveHelperArgs, command_args};
    use yulius_contracts::migrate::{MoveAction, MoveRequest};

    use super::{Cli, Command, InstanceAction};

    /// SPEC-470 MVB-004: what the move helper runs is the command's own syntax. Each value of a
    /// request arrives as the option it is meant for, a plan says no `--yes`, a move does, and
    /// an abort names nothing of the request.
    #[test]
    fn the_move_helpers_arguments_are_the_commands_own() -> Result<(), Box<dyn std::error::Error>> {
        let args = MoveHelperArgs {
            data_dir: "/srv/yulius".into(),
            state_dir: "/srv/yulius-migrate".into(),
            service: "yulius-b.service".into(),
            update_service: "yulius-b-update.service".into(),
            targets: Vec::new(),
            systemctl: "systemctl".into(),
            program: None,
        };
        let request = |action| MoveRequest {
            version: 1,
            id: "0123abcd-request".into(),
            action,
            target: Some("root@new.example".into()),
            ssh_port: Some(2222),
            public_url: Some("https://new.example".into()),
            data_disks: vec!["/dev/vdc".into(), "/dev/vdd".into()],
            max_downtime: None,
            requested_at: 0,
        };
        let parsed = |request: &MoveRequest| {
            let argv = std::iter::once("yulius".to_owned()).chain(command_args(&args, request));
            Cli::try_parse_from(argv).map(|cli| cli.command)
        };
        for (action, says_yes) in [(MoveAction::Plan, false), (MoveAction::Move, true)] {
            let mut json = serde_json::to_value(request(action))?;
            json["max_downtime"] = 8.into();
            let bounded: MoveRequest = serde_json::from_value(json)?;
            let Command::Instance {
                action: InstanceAction::Migrate(options),
            } = parsed(&bounded)?
            else {
                return Err("not `instance migrate`".into());
            };
            assert_eq!(options.target.as_deref(), Some("root@new.example"));
            assert_eq!(options.port, 2222);
            assert_eq!(options.public_url.as_deref(), Some("https://new.example"));
            assert_eq!(options.data_disks, ["/dev/vdc", "/dev/vdd"]);
            assert_eq!(options.yes, says_yes, "{action:?}");
            assert_eq!(options.max_downtime, Some(8), "{action:?}");
            // Root's helper is nobody's sign-in: only the move's own key reaches the new host.
            assert!(options.no_sign_in, "{action:?}");
            // And says which request it answers, for the checklist it writes.
            assert_eq!(options.request.as_deref(), Some("0123abcd-request"));
            assert!(!options.status && !options.abort);
            assert_eq!(options.action(), Action::Run);
            assert_eq!(options.data_dir, std::path::Path::new("/srv/yulius"));
            assert_eq!(options.service, "yulius-b.service");
            assert_eq!(options.update_service, "yulius-b-update.service");
            assert_eq!(
                options.state_dir,
                std::path::Path::new("/srv/yulius-migrate")
            );
        }
        let abort = MoveRequest {
            target: None,
            ssh_port: None,
            public_url: None,
            data_disks: Vec::new(),
            ..request(MoveAction::Abort)
        };
        let Command::Instance {
            action: InstanceAction::Migrate(options),
        } = parsed(&abort)?
        else {
            return Err("not `instance migrate`".into());
        };
        assert!(options.abort && !options.yes && options.no_sign_in);
        assert_eq!(options.action(), Action::Abort);
        assert_eq!(options.request.as_deref(), Some("0123abcd-request"));
        assert_eq!(options.target, None);
        assert_eq!(options.service, "yulius-b.service");
        assert_eq!(
            options.state_dir,
            std::path::Path::new("/srv/yulius-migrate")
        );
        // An id is the request's own form: nothing else reaches the status through it.
        for bad in ["short", "with space-0123", "0123abcd/../x", ""] {
            let refused = Cli::try_parse_from([
                "yulius",
                "instance",
                "migrate",
                &format!("--request-id={bad}"),
                "--",
                "root@new.example",
            ]);
            assert!(refused.is_err(), "{bad:?} was taken as a request's id");
        }
        // The helper itself is a command of this binary, with its unit's defaults.
        let Command::MigrateHelper(helper) = Cli::try_parse_from([
            "yulius",
            "migrate-helper",
            "--targets",
            "a.example,b.example",
        ])?
        .command
        else {
            return Err("not `migrate-helper`".into());
        };
        assert_eq!(helper.targets, ["a.example", "b.example"]);
        assert_eq!(helper.update_service, "yulius-update.service");
        Ok(())
    }

    /// SPEC-451 WRP-002: every subcommand documented "In a task's session" gets through the
    /// agent's `yulius` wrapper at `/yulius/bin`, which is first on a session's `PATH`, and
    /// every command the wrapper passes is a subcommand.
    #[test]
    fn the_agent_wrapper_passes_every_session_command() {
        let shim = yulius_app::identity::agent_yulius_shim(std::path::Path::new("/bin/yulius"));
        let passed: Vec<&str> = shim
            .lines()
            .find_map(|line| line.trim().split_once(") exec "))
            .map(|(names, _)| names.split('|').collect())
            .unwrap_or_default();
        let cli = Cli::command();
        for name in &passed {
            assert!(
                cli.find_subcommand(name).is_some(),
                "{name} is no subcommand"
            );
        }
        let session: Vec<String> = cli
            .get_subcommands()
            .filter(|command| {
                command
                    .get_about()
                    .is_some_and(|about| about.to_string().starts_with("In a task's session"))
            })
            .map(|command| command.get_name().to_owned())
            .collect();
        assert!(session.len() >= 3, "{session:?}");
        for name in &session {
            assert!(passed.contains(&name.as_str()), "{name} not in {passed:?}");
        }
    }
}
