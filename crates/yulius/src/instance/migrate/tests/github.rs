//! The move asks the running instance about its GitHub App (SPEC-466 MVC-021, MVC-022): one
//! bounded request to the loopback listener with the server's key, and one checklist line.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use yulius_app::leftovers::CLI_KEY_FILE;
use yulius_contracts::api::admin::GitHubCallback;
use yulius_contracts::update::HostOutcome;

use crate::instance::migrate::github::{ask, asked_instance, line};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const NEW: &str = "https://new.example";

pub fn facts(state: &str, public_url: &str) -> GitHubCallback {
    GitHubCallback {
        state: state.to_owned(),
        reason: (state == "unknown").then(|| "GitHub: timed out".to_owned()),
        callback_url: format!("{public_url}/api/auth/github/callback"),
        homepage_url: public_url.to_owned(),
        settings_url: "https://github.com/organizations/acme/settings/apps/yulius-acme".into(),
        checked_at: None,
    }
}

/// A loopback listener that answers one request; what it was asked.
fn serve_once(
    status: &'static str,
    body: String,
) -> std::io::Result<(String, JoinHandle<std::io::Result<String>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?.to_string();
    let served = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept()?;
        let mut request = Vec::new();
        let mut byte = [0u8; 1];
        while !request.ends_with(b"\r\n\r\n") && stream.read(&mut byte)? == 1 {
            request.push(byte[0]);
        }
        write!(
            stream,
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )?;
        Ok(String::from_utf8_lossy(&request).into_owned())
    });
    Ok((address, served))
}

fn data_dir_with_key() -> std::io::Result<tempfile::TempDir> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join(CLI_KEY_FILE), "abc123\n")?;
    Ok(dir)
}

#[test]
fn the_instance_is_asked_with_its_key_about_the_new_url() -> TestResult {
    let dir = data_dir_with_key()?;
    let body = serde_json::json!({"github": facts("missing", NEW)}).to_string();
    let (address, served) = serve_once("200 OK", body)?;
    let answer = asked_instance(dir.path(), &address, Some(NEW))?;
    assert_eq!(answer, Some(facts("missing", NEW)));
    let request = served.join().map_err(|_| "the listener died")??;
    assert!(
        request.starts_with(
            "GET /internal/github-callback?public_url=https://new.example HTTP/1.0\r\n"
        ),
        "{request}"
    );
    assert!(
        request.contains("\r\nAuthorization: Bearer abc123\r\n"),
        "{request}"
    );

    // Its own URL, and an instance without a GitHub App.
    let (address, served) = serve_once("200 OK", r#"{"github":null}"#.into())?;
    assert_eq!(asked_instance(dir.path(), &address, None)?, None);
    let request = served.join().map_err(|_| "the listener died")??;
    assert!(
        request.starts_with("GET /internal/github-callback HTTP/1.0\r\n"),
        "{request}"
    );
    Ok(())
}

#[test]
fn an_instance_that_cannot_be_asked_says_why() -> TestResult {
    let dir = data_dir_with_key()?;
    // A server that refuses the key, one of an older build, one that answers nonsense.
    for (status, body, why) in [
        ("401 Unauthorized", "{}", "refused the key"),
        ("404 Not Found", "", "an older build"),
        ("200 OK", "<html>", "not an answer"),
    ] {
        let (address, served) = serve_once(status, body.into())?;
        let refused = asked_instance(dir.path(), &address, Some(NEW))
            .err()
            .ok_or("an answer")?;
        assert!(refused.contains(why), "{status}: {refused}");
        served.join().map_err(|_| "the listener died")??;
    }
    // No server: the port of a listener that is gone.
    let gone = TcpListener::bind("127.0.0.1:0")?.local_addr()?.to_string();
    let refused = asked_instance(dir.path(), &gone, None)
        .err()
        .ok_or("an answer")?;
    assert!(refused.contains(&gone), "{refused}");
    // No key: not this instance's data directory, or the server never started.
    let empty = tempfile::tempdir()?;
    let refused = asked_instance(empty.path(), &gone, None)
        .err()
        .ok_or("an answer")?;
    assert!(refused.contains(CLI_KEY_FILE), "{refused}");
    // A key file that holds something else is never sent.
    std::fs::write(
        empty.path().join(CLI_KEY_FILE),
        "not a key\r\nX-Injected: 1",
    )?;
    let refused = asked_instance(empty.path(), &gone, None)
        .err()
        .ok_or("an answer")?;
    assert!(refused.contains("holds no key"), "{refused}");
    Ok(())
}

#[test]
fn a_server_that_does_not_answer_ends_the_question() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?.to_string();
    let started = Instant::now();
    let silent = ask(&address, "abc123", None, Duration::from_millis(300));
    assert!(silent.is_err(), "{silent:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
    drop(listener);
    Ok(())
}

#[test]
fn the_checklist_line_says_what_github_answered() {
    let (outcome, detail) = line(Ok(Some(facts("registered", NEW))), NEW);
    assert_eq!(outcome, HostOutcome::Ok);
    assert_eq!(
        detail,
        "GitHub accepts https://new.example/api/auth/github/callback for sign-in"
    );

    let (outcome, detail) = line(Ok(Some(facts("missing", NEW))), NEW);
    assert_eq!(outcome, HostOutcome::Manual);
    for part in [
        "open https://github.com/organizations/acme/settings/apps/yulius-acme",
        "add the callback URL https://new.example/api/auth/github/callback",
        "set the homepage https://new.example",
        "Keep the old callback until the old host is retired",
        "password sign-in works meanwhile",
    ] {
        assert!(detail.contains(part), "{part}: {detail}");
    }

    let (outcome, detail) = line(Ok(Some(facts("unknown", NEW))), NEW);
    assert_eq!(outcome, HostOutcome::Manual);
    assert!(detail.contains("GitHub: timed out"), "{detail}");
    assert!(
        detail.contains("https://github.com/organizations/acme/settings/apps/yulius-acme"),
        "{detail}"
    );

    let (outcome, detail) = line(Ok(None), NEW);
    assert_eq!(outcome, HostOutcome::Ok);
    assert!(detail.contains("no GitHub App"), "{detail}");

    // The instance could not be asked: the general note, with why.
    let (outcome, detail) = line(Err("connection refused".into()), NEW);
    assert_eq!(outcome, HostOutcome::Manual);
    assert!(
        detail.contains("https://new.example/api/auth/github/callback"),
        "{detail}"
    );
    assert!(detail.contains("settings/apps/<app>"), "{detail}");
    assert!(detail.contains("connection refused"), "{detail}");
    // Nothing a terminal would obey travels in a line.
    let mut odd = facts("unknown", NEW);
    odd.reason = Some("it said\u{1b}[2J\nmanual  fake".into());
    let (_, detail) = line(Ok(Some(odd)), NEW);
    assert!(!detail.chars().any(char::is_control), "{detail:?}");
}

/// MVC-022: the step of the new host asks the server there, with the key of its data directory.
#[test]
fn the_new_host_s_step_asks_its_own_instance() -> TestResult {
    use crate::instance::migrate::target::Request;
    let dir = data_dir_with_key()?;
    let body = serde_json::json!({"github": facts("registered", NEW)}).to_string();
    let (address, served) = serve_once("200 OK", body)?;
    let request = Request::Callback {
        data_dir: dir.path().to_path_buf(),
        address,
        public_url: NEW.to_owned(),
    };
    let reply = super::target::step(&mut super::fake::new_host(), &request);
    let said = served.join().map_err(|_| "the listener died")??;
    // Its own URL: the new instance keeps the answer for its Instance page.
    assert!(
        said.starts_with("GET /internal/github-callback HTTP/1.0\r\n"),
        "{said}"
    );
    assert_eq!(reply.lines.len(), 1);
    assert_eq!(reply.lines[0].name, "GitHub App");
    assert_eq!(reply.lines[0].outcome, HostOutcome::Ok);

    // No server there (yet): the general note for the new URL, not a failed step.
    let gone = TcpListener::bind("127.0.0.1:0")?.local_addr()?.to_string();
    let request = Request::Callback {
        data_dir: dir.path().to_path_buf(),
        address: gone,
        public_url: NEW.to_owned(),
    };
    let reply = super::target::step(&mut super::fake::new_host(), &request);
    assert_eq!(reply.lines[0].outcome, HostOutcome::Manual);
    assert!(
        reply.lines[0]
            .detail
            .contains("https://new.example/api/auth/github/callback"),
        "{}",
        reply.lines[0].detail
    );
    Ok(())
}

/// Only a public URL as a move names it goes into the request line: nothing is sent otherwise.
#[test]
fn a_url_that_is_not_a_host_is_never_sent() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?.to_string();
    for bad in [
        "https://new.example HTTP/1.0\r\nX-Injected: 1",
        "https://new.example/path?x=1",
        "http://new.example",
    ] {
        let refused = ask(&address, "abc123", Some(bad), Duration::from_millis(300))
            .err()
            .ok_or("an answer")?;
        assert!(refused.contains("not `https://host`"), "{refused}");
    }
    // Nobody connected.
    assert!(listener.accept().is_err());
    Ok(())
}

/// The limit is for the whole answer: a server that keeps sending a byte now and then does not
/// keep the move waiting.
#[test]
fn an_answer_that_trickles_ends_at_the_limit() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?.to_string();
    let trickles = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            // Until the asker hangs up.
            while stream.write_all(b"H").is_ok() {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    });
    let started = Instant::now();
    let asked = ask(&address, "abc123", None, Duration::from_millis(400));
    let took = started.elapsed();
    let why = asked.err().ok_or("an answer")?;
    assert!(why.contains("did not answer within"), "{why}");
    assert!(took < Duration::from_secs(10), "{took:?}");
    trickles.join().map_err(|_| "the listener died")?;
    Ok(())
}
