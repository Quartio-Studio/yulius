//! What a move asks the running instance about its GitHub App (SPEC-466 MVC-021, MVC-022,
//! ADR-0302). The App's settings link needs the sealed credentials only the server opens, and
//! only GitHub knows whether a callback URL is registered: the root command asks the server over
//! its loopback listener, with the key the server writes for root's commands, and makes one
//! checklist line of the answer. The plan asks the old instance about the new URL; the finish
//! asks the new instance, through the staged build on the new host.

use std::io::{ErrorKind, Read as _, Write as _};
use std::path::Path;
use std::time::{Duration, Instant};

use yulius_app::github_callback::public_url_ok;
use yulius_app::leftovers::CLI_KEY_FILE;
use yulius_contracts::api::admin::{GitHubCallback, GitHubCallbackAnswer};
use yulius_contracts::update::HostOutcome;

/// The instance's answer: the facts, `None` without a GitHub App, or why it could not be asked.
pub type Asked = Result<Option<GitHubCallback>, String>;

/// How long the instance may take to connect, to take the request and to answer: it asks GitHub
/// one or two questions of at most 10 s each, after a check that may be on its way (as much
/// again).
const LIMIT: Duration = Duration::from_secs(45);
/// An answer is a few hundred bytes.
const MAX_ANSWER: usize = 64 * 1024;

/// Asks the instance whose data directory is `data_dir`, at its loopback listener `address`,
/// whether GitHub accepts the callback of `public_url` (its own without one).
///
/// # Errors
/// The key cannot be read, the instance does not answer, or it refuses.
pub fn asked_instance(data_dir: &Path, address: &str, public_url: Option<&str>) -> Asked {
    let path = data_dir.join(CLI_KEY_FILE);
    let key = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {} ({e}): does the server run?", path.display()))?;
    let key = key.trim();
    if key.is_empty() || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{} holds no key", path.display()));
    }
    ask(address, key, public_url, LIMIT)
}

/// One request to the loopback listener; connecting, sending and the whole answer each end after
/// `limit`.
pub(crate) fn ask(address: &str, key: &str, public_url: Option<&str>, limit: Duration) -> Asked {
    let said = |why: &dyn std::fmt::Display| format!("the instance at {address}: {why}");
    // Only what a move may name goes into the request line.
    if public_url.is_some_and(|url| !public_url_ok(url)) {
        return Err("the public URL is not `https://host`".to_owned());
    }
    let to: std::net::SocketAddr = address.parse().map_err(|e| said(&e))?;
    let mut stream = std::net::TcpStream::connect_timeout(&to, limit).map_err(|e| said(&e))?;
    stream
        .set_read_timeout(Some(limit))
        .and_then(|()| stream.set_write_timeout(Some(limit)))
        .map_err(|e| said(&e))?;
    let query = public_url
        .map(|url| format!("?public_url={url}"))
        .unwrap_or_default();
    write!(
        stream,
        "GET /internal/github-callback{query} HTTP/1.0\r\nHost: localhost\r\nAuthorization: \
         Bearer {key}\r\n\r\n"
    )
    .map_err(|e| said(&e))?;
    // The limit is for the whole answer, however slowly it comes.
    let late = || {
        said(&format_args!(
            "it did not answer within {} s",
            limit.as_secs()
        ))
    };
    let deadline = Instant::now() + limit;
    let mut answer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(late());
        }
        stream.set_read_timeout(Some(left)).map_err(|e| said(&e))?;
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) if answer.len() + read > MAX_ANSWER => {
                return Err(said(&"what it sent is not an answer"));
            }
            Ok(read) => answer.extend_from_slice(&chunk[..read]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return Err(late());
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(said(&e)),
        }
    }
    let answer = String::from_utf8_lossy(&answer);
    let (head, body) = answer.split_once("\r\n\r\n").unwrap_or((&answer, ""));
    let status = head.split_whitespace().nth(1).unwrap_or_default();
    match status {
        "200" => serde_json::from_str::<GitHubCallbackAnswer>(body.trim())
            .map(|answer| answer.github)
            .map_err(|_| said(&"what it sent is not an answer")),
        "401" => Err(said(&format_args!(
            "it refused the key of {CLI_KEY_FILE} (is this its data directory?)"
        ))),
        "404" => Err(said(&"it does not know the question (an older build runs)")),
        "" => Err(said(&"what it sent is not an answer")),
        other => Err(said(&format_args!("it answered {other}"))),
    }
}

/// The checklist's "GitHub App" line for a move to `new`, from what the instance said.
#[must_use]
pub fn line(asked: Asked, new: &str) -> (HostOutcome, String) {
    // The answer came from another process, and may quote GitHub: nothing a terminal obeys.
    let plain = |text: String| text.replace(|c: char| c.is_control(), " ");
    match asked {
        Ok(None) => (
            HostOutcome::Ok,
            "no GitHub App is connected: nothing to register on GitHub".to_owned(),
        ),
        Ok(Some(app)) if app.state == "registered" => (
            HostOutcome::Ok,
            plain(format!("GitHub accepts {} for sign-in", app.callback_url)),
        ),
        Ok(Some(app)) => {
            let what = format!(
                "open {} and add the callback URL {} and set the homepage {}. Keep the old \
                 callback until the old host is retired; password sign-in works meanwhile",
                app.settings_url, app.callback_url, app.homepage_url
            );
            let detail = if app.state == "missing" {
                format!("GitHub sign-in needs the new URL in the App: {what}")
            } else {
                format!(
                    "GitHub did not say whether it accepts the new URL ({}). If GitHub sign-in \
                     fails there: {what}",
                    app.reason.as_deref().unwrap_or("no answer")
                )
            };
            (HostOutcome::Manual, plain(detail))
        }
        Err(why) => (
            HostOutcome::Manual,
            plain(format!(
                "{} (the instance could not be asked for the App's own link: {why})",
                super::github_note(new)
            )),
        ),
    }
}
