//! Antigravity specifics of the supervisor: its shared folders, run home, run end and token files.

use std::path::{Path, PathBuf};

use yulius_domain::agent_cli::antigravity;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AttemptId, SandboxId};

use crate::context::App;
use crate::error::{AppError, AppResult};

/// What an Antigravity run says of the MCP servers it is given without: the HTTP servers that
/// need a token (SPEC-505), `None` when there is none.
pub(super) fn tokened_servers_left_out(
    mcp: &[crate::ports::harness::McpAttachment],
) -> Option<String> {
    let names: Vec<&str> = mcp
        .iter()
        .filter_map(|server| match server {
            crate::ports::harness::McpAttachment::Http {
                name,
                bearer: Some(_),
                ..
            } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    (!names.is_empty()).then(|| {
        format!(
            "Antigravity runs without the MCP servers that need a token ({}): it cannot be \
             given one without writing it to a file.",
            names.join(", ")
        )
    })
}

/// The sandbox's Antigravity folder (SPEC-505 AGY-003).
const ANTIGRAVITY_DIR: &str = "antigravity";

/// The folder below it shared by the homes.
const ANTIGRAVITY_SHARED_DIR: &str = "shared";

/// The CLI's folders below `.gemini/antigravity-cli` that every run's home links to the shared
/// ones.
const ANTIGRAVITY_SHARED: [&str; 2] = ["brain", "conversations"];

/// The Antigravity home of the run whose folder is `run_dir` (SPEC-505 AGY-003).
pub(super) fn antigravity_run_home(run_dir: &Path) -> PathBuf {
    run_dir.join(ANTIGRAVITY_RUN_HOME)
}

/// The bind of a contained Antigravity run: its home's `.gemini`, in the server's run directory,
/// over the sandbox home's `.gemini` (SPEC-505 AGY-003).
pub(super) fn antigravity_bind(
    run_home: &Path,
    sandbox_home: &Path,
) -> crate::ports::session::Bind {
    crate::ports::session::Bind {
        source: run_home.join(".gemini"),
        dest: sandbox_home.join(".gemini"),
        writable: true,
    }
}

/// Where the Antigravity CLI keeps a Google sign-in in the run's home (1.5.10). The same path as
/// `yulius_harness::launch::ANTIGRAVITY_TOKEN_FILE`, which the plan writes.
pub(super) fn antigravity_token_path(run_dir: &Path) -> PathBuf {
    let mut path = antigravity_run_home(run_dir);
    path.extend(antigravity::TOKEN_DIRS);
    path.join(antigravity::TOKEN_NAME)
}

/// The token file a launch wrote into its run's home, removed when the launch ends before its run
/// is live (a failed spawn, a subscription removed just after it was chosen); emptied once the
/// run is live, whose end takes it back (SPEC-505 AGY-008).
pub(super) struct AntigravityToken(pub(super) Option<PathBuf>);

impl Drop for AntigravityToken {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The sealed credential an Antigravity launch gave its run, in the run directory (out of the
/// bound `.gemini`, SPEC-505 AGY-008).
pub(super) const ANTIGRAVITY_GIVEN: &str = "antigravity-given";

/// The run's home below its run directory, as [`antigravity_run_home`] names it.
const ANTIGRAVITY_RUN_HOME: &str = "antigravity-home";

/// The folder holding the token file of the Antigravity home `home` below `root`, opened without
/// following a link (the CLI's process could change what lies below its `.gemini`); `None` when
/// it is missing.
fn antigravity_token_dir(root: &Path, home: &str) -> AppResult<Option<std::os::fd::OwnedFd>> {
    let names: Vec<&std::ffi::OsStr> = std::iter::once(home)
        .chain(antigravity::TOKEN_DIRS)
        .map(std::ffi::OsStr::new)
        .collect();
    match crate::files::open_dirs_below(root, &names, false) {
        Ok(dir) => Ok(Some(dir)),
        // Missing, or a folder the run's agent made a link or a file: no token of the run's.
        Err(nix::errno::Errno::ENOENT | nix::errno::Errno::ELOOP | nix::errno::Errno::ENOTDIR) => {
            Ok(None)
        }
        Err(err) => Err(crate::files::errno_error(err)),
    }
}

/// The token file an Antigravity run left in its home, read without following a link or waiting
/// on a FIFO, bounded; `None` when there is none or it is no plain file within the bound
/// (SPEC-505 AGY-008).
fn read_antigravity_token(run_dir: &Path) -> AppResult<Option<Vec<u8>>> {
    read_antigravity_token_in(run_dir, ANTIGRAVITY_RUN_HOME)
}

/// The token file the Antigravity CLI left in the home `home` below `root` (a run's, or a
/// sign-in's), read as [`read_antigravity_token`] says.
pub(crate) fn read_antigravity_token_in(root: &Path, home: &str) -> AppResult<Option<Vec<u8>>> {
    let Some(dir) = antigravity_token_dir(root, home)? else {
        return Ok(None);
    };
    match crate::files::read_bounded_at(
        &dir,
        std::ffi::OsStr::new(antigravity::TOKEN_NAME),
        yulius_domain::harness::ANTIGRAVITY_TOKEN_MAX_BYTES,
    ) {
        Ok(crate::files::Bounded::Bytes(mut bytes)) => Ok(Some(std::mem::take(&mut *bytes))),
        Ok(crate::files::Bounded::NotPlain | crate::files::Bounded::TooLarge) => Ok(None),
        Err(err)
            if err.kind() == std::io::ErrorKind::NotFound
                || err.raw_os_error() == Some(nix::libc::ELOOP) =>
        {
            Ok(None)
        }
        Err(err) => Err(crate::files::io_error(&err)),
    }
}

/// Removes the token file of the run whose folder is `run_dir`.
fn remove_antigravity_token(run_dir: &Path) -> AppResult<()> {
    let Some(dir) = antigravity_token_dir(run_dir, ANTIGRAVITY_RUN_HOME)? else {
        return Ok(());
    };
    crate::files::remove_at(&dir, std::ffi::OsStr::new(antigravity::TOKEN_NAME))
}

impl App {
    /// The sandbox's Antigravity conversations, shared by its Antigravity homes (SPEC-505
    /// AGY-003).
    pub(crate) fn antigravity_brain(&self, sandbox: &SandboxId) -> PathBuf {
        self.sandbox_dir(sandbox)
            .join(ANTIGRAVITY_DIR)
            .join(ANTIGRAVITY_SHARED_DIR)
            .join("brain")
    }

    /// The transcript of the Antigravity conversation `session_id` in `sandbox` (AGY-004).
    pub(crate) fn antigravity_transcript(&self, sandbox: &SandboxId, session_id: &str) -> PathBuf {
        self.deps
            .harness
            .antigravity_transcript(&self.antigravity_brain(sandbox), session_id)
    }

    /// The Antigravity home of a run, `<run_dir>/antigravity-home` (SPEC-505 AGY-003): the folder
    /// holding the `.gemini` the CLI reads, in the run's own folder, which only the server writes
    /// before the run (so a contained run binds a path no agent of the sandbox could have swapped
    /// for a link). Its `antigravity-cli/brain` and `conversations` link to the sandbox's shared
    /// ones, made through handles from the sandbox directory, so a conversation resumes on any
    /// Antigravity account; the links are absolute, made anew for each run.
    pub(super) fn antigravity_home(
        &self,
        sandbox: &SandboxId,
        run_dir: &Path,
    ) -> AppResult<PathBuf> {
        use std::ffi::OsStr;
        let root = self.sandbox_dir(sandbox);
        std::fs::create_dir_all(&root).map_err(AppError::internal)?;
        for shared in ANTIGRAVITY_SHARED {
            crate::files::open_dirs_below(
                &root,
                &[
                    OsStr::new(ANTIGRAVITY_DIR),
                    OsStr::new(ANTIGRAVITY_SHARED_DIR),
                    OsStr::new(shared),
                ],
                true,
            )
            .map_err(|err| match err {
                nix::errno::Errno::ELOOP | nix::errno::Errno::ENOTDIR => AppError::internal(
                    "the sandbox's Antigravity folder is a link or not a folder: Yulius follows no \
                     links",
                ),
                other => crate::files::errno_error(other),
            })?;
        }
        let home = antigravity_run_home(run_dir);
        let gemini = home.join(".gemini");
        for dir in [gemini.join("config"), gemini.join("antigravity-cli/cache")] {
            std::fs::create_dir_all(dir).map_err(AppError::internal)?;
        }
        let shared = root.join(ANTIGRAVITY_DIR).join(ANTIGRAVITY_SHARED_DIR);
        for name in ANTIGRAVITY_SHARED {
            std::os::unix::fs::symlink(
                shared.join(name),
                gemini.join("antigravity-cli").join(name),
            )
            .map_err(AppError::internal)?;
        }
        Ok(home)
    }

    /// After an Antigravity run ended or was lost (SPEC-505 AGY-008): the token file its CLI left
    /// in the run's home replaces the account's sign-in when it is a refresh of the one the launch
    /// gave the run and the account still holds that one (a sign-in a person made meanwhile, or
    /// another run's refresh, stays). The file is removed once that is settled; a refresh that
    /// could not be stored stays in the run directory, which no agent reaches after the run. The
    /// run's own agent could have written the file (ADR-0361).
    pub(super) async fn antigravity_run_ended(&self, attempt_id: &AttemptId) {
        let Ok(Some(attempt)) = self.deps.store.attempt(attempt_id).await else {
            return;
        };
        let Some(account_id) = attempt
            .account_id
            .filter(|_| attempt.harness == Harness::Antigravity)
        else {
            return;
        };
        let run_dir = PathBuf::from(&attempt.run_dir);
        let fresh = match read_antigravity_token(&run_dir) {
            Ok(Some(fresh)) => fresh,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(account = %account_id, error = %err, "the Antigravity run's sign-in could not be read");
                return;
            }
        };
        // Only a refresh of what the launch gave this run counts, and only while the account
        // still holds that: a sign-in a person made meanwhile, or another run's refresh, stays.
        let given = std::fs::read(run_dir.join(ANTIGRAVITY_GIVEN))
            .inspect_err(|err| {
                tracing::warn!(account = %account_id, error = %err, "what an Antigravity run was given is unknown: its sign-in is not taken back");
            })
            .ok()
            .map(crate::model::Sealed);
        let refreshed = given.as_ref().and_then(|sealed| {
            let plain = self.deps.crypto.open(sealed).ok()?;
            self.deps
                .harness
                .antigravity_refreshed_credential(&fresh, &plain)
        });
        let kept = match (given, refreshed) {
            (Some(given), Some(refreshed)) => match self.deps.crypto.seal(&refreshed) {
                Ok(sealed) => match self
                    .deps
                    .store
                    .refresh_account_credential(&account_id, &given, &sealed, self.now())
                    .await
                {
                    // Stored, or the account holds another sign-in now: the file is done.
                    Ok(_) => true,
                    Err(err) => {
                        tracing::warn!(account = %account_id, error = %err, "a refreshed Antigravity sign-in could not be stored");
                        false
                    }
                },
                Err(err) => {
                    tracing::error!(error = %err, "could not seal a refreshed Antigravity sign-in");
                    false
                }
            },
            // Unchanged, not a sign-in's, or nothing given: nothing to keep.
            _ => true,
        };
        if kept && let Err(err) = remove_antigravity_token(&run_dir) {
            tracing::warn!(account = %account_id, error = %err, "the Antigravity run's sign-in could not be removed");
        }
    }
}

#[cfg(test)]
mod tests {
    /// SPEC-505 AGY-003: a contained Antigravity run binds the `.gemini` of its run's own home,
    /// in the server's run directory, over the sandbox home's.
    #[test]
    fn an_antigravity_run_binds_its_own_home() {
        let run_dir = std::path::Path::new("/data/runs/a1");
        let bind = super::antigravity_bind(
            &super::antigravity_run_home(run_dir),
            std::path::Path::new("/data/sandboxes/s1/home"),
        );
        assert_eq!(
            bind.source,
            std::path::Path::new("/data/runs/a1/antigravity-home/.gemini")
        );
        assert_eq!(
            bind.dest,
            std::path::Path::new("/data/sandboxes/s1/home/.gemini")
        );
        assert!(bind.writable);
    }

    /// SPEC-505 AGY-008: a run's token file is read only when it is a plain file within its
    /// bound, never through a link (its agent could change what lies below its `.gemini`) or a
    /// FIFO, and removed without following one either.
    #[test]
    fn an_antigravity_token_is_read_without_following_links()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let run_dir = dir.path();
        let token = super::antigravity_token_path(run_dir);
        assert_eq!(super::read_antigravity_token(run_dir)?, None, "no home");
        std::fs::create_dir_all(token.parent().ok_or("parent")?)?;
        std::fs::write(&token, b"SYNTHETIC-token")?;
        assert_eq!(
            super::read_antigravity_token(run_dir)?.as_deref(),
            Some(&b"SYNTHETIC-token"[..])
        );
        std::fs::write(&token, vec![b'x'; 64 * 1024 + 1])?;
        assert_eq!(super::read_antigravity_token(run_dir)?, None, "oversize");
        std::fs::remove_file(&token)?;
        let elsewhere = dir.path().join("elsewhere");
        std::fs::write(&elsewhere, b"SYNTHETIC-not-the-run's")?;
        std::os::unix::fs::symlink(&elsewhere, &token)?;
        assert_eq!(super::read_antigravity_token(run_dir)?, None, "a link");
        super::remove_antigravity_token(run_dir)?;
        assert!(elsewhere.exists(), "the link went, not its target");
        nix::unistd::mkfifo(&token, nix::sys::stat::Mode::S_IRWXU)?;
        assert_eq!(super::read_antigravity_token(run_dir)?, None, "a FIFO");
        std::fs::remove_file(&token)?;
        // A folder on the way that is a link is not followed.
        let cli = token.parent().ok_or("parent")?;
        std::fs::remove_dir_all(cli)?;
        let other = dir.path().join("other");
        std::fs::create_dir_all(&other)?;
        std::fs::write(other.join("antigravity-oauth-token"), b"SYNTHETIC")?;
        std::os::unix::fs::symlink(&other, cli)?;
        assert_eq!(
            super::read_antigravity_token(run_dir)?,
            None,
            "a linked folder"
        );
        Ok(())
    }

    /// SPEC-505: an Antigravity run names the MCP servers it is given without (those needing a
    /// token), and says nothing when it is given them all.
    #[test]
    fn an_antigravity_run_names_the_servers_it_leaves_out() {
        use crate::ports::harness::McpAttachment;
        let http = |name: &str, bearer: Option<&str>| McpAttachment::Http {
            name: name.into(),
            url: "https://mcp.example.test".into(),
            bearer: bearer.map(str::to_owned),
        };
        let stdio = McpAttachment::Stdio {
            name: "local".into(),
            command: "x".into(),
            args: Vec::new(),
            env: vec![("K".into(), "SYNTHETIC".into())],
        };
        assert_eq!(
            super::tokened_servers_left_out(&[stdio.clone(), http("open", None)]),
            None
        );
        let said = super::tokened_servers_left_out(&[
            stdio,
            http("linear", Some("SYNTHETIC")),
            http("sentry", Some("SYNTHETIC")),
        ])
        .unwrap_or_default();
        assert!(said.contains("(linear, sentry)"), "{said}");
        assert!(!said.contains("SYNTHETIC"), "{said}");
    }
}
