//! `yulius login`, `yulius onboard` / `yulius import` (client side of SPEC-001 IMP-001..009) and
//! `yulius secrets import` (SPEC-205).

use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use console::style;
use yulius_import::{
    DeviceLoginOptions, ImportError, ImportOptions, MAX_DOTENV_BYTES, Secret, SecretsImportError,
    SecretsImportOptions, SecretsIo, SecretsTarget, SessionChoice, TerminalIo, TokenStore, Url,
    VaultClient, device_login, import_secrets, parse_dotenv, prepare_target, prompt_secrets,
    run_import, set_colors,
};

fn install_tls() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn parse_server(server: &str) -> Result<Url, String> {
    Url::parse(server.trim_end_matches('/')).map_err(|e| format!("invalid --server URL: {e}"))
}

/// The server to talk to: `--server`/`YULIUS_SERVER`, else the only one signed in to.
fn resolve_server(server: Option<&str>, store: &TokenStore) -> Result<Url, String> {
    if let Some(server) = server.filter(|s| !s.trim().is_empty()) {
        return parse_server(server);
    }
    let known = store.servers().map_err(|e| e.to_string())?;
    match known.as_slice() {
        [only] => parse_server(only),
        [] => Err("pass --server <url> (for example --server https://yulius.example.com)".into()),
        many => Err(format!(
            "signed in to several servers ({}); pass --server <url>",
            many.join(", ")
        )),
    }
}

/// `yulius login --server <url>`.
pub fn login(server: &str) -> ExitCode {
    install_tls();
    let result = (|| -> Result<(), String> {
        let url = parse_server(server)?;
        let store = TokenStore::default_location().map_err(|e| e.to_string())?;
        let token = device_login(
            &url,
            &mut |start| {
                println!(
                    "Open {} and confirm the code {}",
                    start.verification_url, start.user_code
                );
            },
            &DeviceLoginOptions::default(),
        )
        .map_err(|e| e.to_string())?;
        store.save(&url, &token).map_err(|e| e.to_string())?;
        println!("Signed in to {url}.");
        Ok(())
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("yulius login: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Command-line options of `yulius onboard`.
#[derive(Debug, Clone, Default)]
pub struct OnboardArgs {
    /// Repository path.
    pub path: PathBuf,
    /// Server URL (`None`: the only signed-in server).
    pub server: Option<String>,
    /// `all`, `recent`, `pick` or `none`.
    pub sessions: Option<String>,
    /// Window of `recent`.
    pub since: Option<String>,
    /// Ask nothing.
    pub yes: bool,
    /// Sessions only (no unpublished branches or uncommitted changes).
    pub sessions_only: bool,
    /// Show the plan, upload nothing.
    pub dry_run: bool,
    /// Only show the onboarding state.
    pub status: bool,
    /// List every redaction.
    pub show_redactions: bool,
    /// Upload into a playground of the caller (SPEC-171).
    pub playground: bool,
    /// The playground's name (`None`: the checkout directory's name).
    pub playground_name: Option<String>,
    /// No colours.
    pub no_color: bool,
}

/// `yulius onboard [path] [--server <url>] [--sessions all|recent|pick|none] [--since 14d]
/// [--sessions-only] [--dry-run] [--status] [--show-redactions] [--playground [--name <name>]]
/// [--yes] [--no-color]`.
pub fn onboard(args: OnboardArgs) -> ExitCode {
    install_tls();
    set_colors(args.no_color);
    let result = (|| -> Result<(), String> {
        let store = TokenStore::default_location().map_err(|e| e.to_string())?;
        let server = resolve_server(args.server.as_deref(), &store)?;
        let sessions = match args.sessions.as_deref() {
            Some(text) => Some(SessionChoice::parse(text).ok_or_else(|| {
                format!("--sessions must be all, recent, pick or none, not {text:?}")
            })?),
            None => None,
        };
        let options = ImportOptions {
            sessions,
            since: args.since.clone(),
            assume_yes: args.yes,
            sessions_only: args.sessions_only,
            dry_run: args.dry_run,
            status_only: args.status,
            show_redactions: args.show_redactions,
            playground: args.playground,
            playground_name: args.playground_name.clone(),
            ..ImportOptions::new(args.path.clone(), server)
        };
        let mut io = TerminalIo::new(store);
        match run_import(&options, &mut io) {
            Ok(_) => Ok(()),
            Err(ImportError::Cancelled) => Err("cancelled; nothing was uploaded".into()),
            Err(err @ (ImportError::Upload(_) | ImportError::Api(_))) => Err(format!(
                "{err}\n  Run the same command again: finished files are kept and the upload resumes."
            )),
            Err(err) => Err(err.to_string()),
        }
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("\n{} {err}", style("yulius onboard:").red().bold());
            ExitCode::FAILURE
        }
    }
}

/// Which vault `yulius secrets import` fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum VaultChoice {
    /// Team secrets, for people.
    Team,
    /// Agent secrets of a sandbox, used by agents who never see them.
    Agent,
}

/// Command-line options of `yulius secrets import` (SPEC-205). None takes a value.
#[derive(Debug, Clone)]
pub struct SecretsImportArgs {
    /// The vault.
    pub to: VaultChoice,
    /// A dotenv file, `-` for stdin, `None` to type each secret.
    pub file: Option<PathBuf>,
    /// The sandbox of the secrets (id, slug or `owner/name`).
    pub sandbox: Option<String>,
    /// Team-wide secrets, for every sandbox (SPEC-216).
    pub instance: bool,
    /// Agent secrets a person allows each host and production deploys of, instead of agents
    /// setting them up (SPEC-216).
    pub confirm_hosts: bool,
    /// Server URL (`None`: the only signed-in server).
    pub server: Option<String>,
    /// No colours.
    pub no_color: bool,
}

/// The terminal of `yulius secrets import`: lines on stdout (and the terminal when stdout is
/// redirected), questions at `/dev/tty` (so stdin can carry the file), values read without echo.
struct TtyIo;

impl TtyIo {
    fn tty() -> io::Result<std::fs::File> {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
    }
}

impl SecretsIo for TtyIo {
    fn say(&mut self, line: &str) {
        println!("{line}");
        // With stdout redirected, the person answering at the terminal still sees the names.
        if !io::stdout().is_terminal()
            && let Ok(mut tty) = Self::tty()
        {
            let _ = writeln!(tty, "{line}");
        }
    }

    fn ask(&mut self, prompt: &str) -> io::Result<String> {
        let mut tty = Self::tty()?;
        tty.write_all(prompt.as_bytes())?;
        tty.flush()?;
        let mut answer = String::new();
        if BufReader::new(tty).read_line(&mut answer)? == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "end of input"));
        }
        Ok(answer.trim_end_matches(['\r', '\n']).to_owned())
    }

    fn ask_hidden(&mut self, prompt: &str) -> io::Result<Secret> {
        // The terminal itself, not stderr: `console` answers "" without asking when its
        // terminal is not attended, which stderr redirected to a file is not.
        let term = console::Term::read_write_pair(Self::tty()?, Self::tty()?);
        term.write_str(prompt)?;
        Ok(Secret::new(term.read_secure_line()?))
    }
}

/// Reads the dotenv text of `file` (`-`: stdin), at most [`MAX_DOTENV_BYTES`], into a buffer
/// that is wiped when dropped.
fn read_source(file: &std::path::Path) -> Result<Secret, String> {
    let reader: Box<dyn Read> = if file.as_os_str() == "-" {
        Box::new(io::stdin())
    } else {
        Box::new(
            std::fs::File::open(file)
                .map_err(|e| format!("cannot read {}: {e}", file.display()))?,
        )
    };
    let mut bytes = Vec::new();
    let read = reader
        .take(MAX_DOTENV_BYTES as u64 + 1)
        .read_to_end(&mut bytes);
    let text = String::from_utf8(bytes).map_err(|e| {
        let mut bytes = e.into_bytes();
        zeroize::Zeroize::zeroize(&mut bytes);
        format!("{} is not UTF-8 text", file.display())
    });
    let text = Secret::new(text?);
    read.map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    if text.expose().len() > MAX_DOTENV_BYTES {
        return Err(format!("{} is larger than 1 MiB", file.display()));
    }
    Ok(text)
}

/// `yulius secrets import --to team|agent [FILE|-] [--sandbox <owner/name>] [--instance]
/// [--confirm-hosts]
/// [--server <url>] [--no-color]`: exit 0 when every secret was imported or already there.
pub fn secrets_import(args: SecretsImportArgs) -> ExitCode {
    install_tls();
    set_colors(args.no_color);
    let result = (|| -> Result<bool, String> {
        if args.instance && args.to == VaultChoice::Team && args.sandbox.is_some() {
            return Err("team-wide team secrets (--instance) belong to no --sandbox".into());
        }
        if args.confirm_hosts && args.to != VaultChoice::Agent {
            return Err("--confirm-hosts applies to --to agent".into());
        }
        let parsed = match &args.file {
            Some(file) => {
                let text = read_source(file)?;
                let entries = parse_dotenv(text.expose())
                    .map_err(|e| format!("{}: {e}; nothing was imported", file.display()))?;
                Some(entries)
            }
            None => None,
        };
        let store = TokenStore::default_location().map_err(|e| e.to_string())?;
        let server = resolve_server(args.server.as_deref(), &store)?;
        let token = store
            .load(&server)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                format!("not signed in to {server}; run `yulius login --server {server}`")
            })?;
        let client = VaultClient::new(server, token).map_err(|e| e.to_string())?;
        let options = SecretsImportOptions {
            target: match args.to {
                VaultChoice::Team => SecretsTarget::Team {
                    sandbox: args.sandbox.clone(),
                    instance: args.instance,
                },
                VaultChoice::Agent => SecretsTarget::Agent {
                    sandbox: args.sandbox.clone(),
                    instance: args.instance,
                    confirm_hosts: args.confirm_hosts,
                },
            },
            checkout: std::env::current_dir().map_err(|e| e.to_string())?,
            source_file: args
                .file
                .as_ref()
                .filter(|f| f.as_os_str() != "-")
                .map(|f| f.display().to_string()),
        };
        // Checked before anything is typed: a wrong --sandbox throws nothing away.
        let target =
            prepare_target(&options, &client).map_err(|e| format!("{e}; nothing was imported"))?;
        let mut io = TtyIo;
        let entries = match parsed {
            Some(entries) => entries,
            None => prompt_secrets(&mut io).map_err(|e| e.to_string())?,
        };
        if entries.is_empty() {
            io.say("No secrets given; nothing was imported.");
            return Ok(true);
        }
        match import_secrets(entries, &target, &client, &mut io) {
            Ok(summary) => Ok(summary.ok()),
            Err(SecretsImportError::Cancelled) => Err("cancelled; nothing was imported".into()),
            Err(err) => Err(format!("{err}; nothing was imported")),
        }
    })();
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("{} {err}", style("yulius secrets import:").red().bold());
            ExitCode::FAILURE
        }
    }
}
