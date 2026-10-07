//! Git credential-helper protocol against a loopback test server.

use std::error::Error;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::{self, JoinHandle};

use yulius_git::credential::{EXIT_OK, EXIT_UNAVAILABLE, EXIT_USAGE};
use yulius_git::run_credential_helper;

type TestResult = Result<(), Box<dyn Error>>;

/// Server thread returning the captured request text.
type Server = JoinHandle<Result<String, String>>;

const GIT_INPUT: &str = "protocol=https\nhost=github.com\npath=acme/app.git\n\n";

/// Accepts one connection, captures the request and replies with `response`.
fn one_shot_server(response: String) -> Result<(u16, Server), Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().map_err(|e| e.to_string())?;
        let request = read_request(&mut stream).map_err(|e| e.to_string())?;
        stream
            .write_all(response.as_bytes())
            .map_err(|e| e.to_string())?;
        Ok(request)
    });
    Ok((port, handle))
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&raw);
        if let Some(end) = text.find("\r\n\r\n") {
            let length = text[..end]
                .lines()
                .find_map(|line| line.strip_prefix("Content-Length: "))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if raw.len() >= end + 4 + length {
                break;
            }
        }
    }
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

fn env_for(url: String, token: Option<&'static str>) -> impl Fn(&str) -> Option<String> {
    move |key| match key {
        "YULIUS_URL" => Some(url.clone()),
        "YULIUS_TASK_TOKEN" => token.map(str::to_owned),
        _ => None,
    }
}

fn json_response(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[test]
fn get_asks_the_internal_listener_and_prints_the_token() -> TestResult {
    let (port, server) = one_shot_server(json_response(
        "200 OK",
        r#"{"token":"ghs_SyntheticFixtureToken","expires_at":1767225600}"#,
    ))?;
    let env = env_for(format!("http://127.0.0.1:{port}"), Some("task-token-123"));
    let mut output = Vec::new();
    let code = run_credential_helper("get", &mut GIT_INPUT.as_bytes(), &mut output, &env);
    assert_eq!(code, EXIT_OK);
    assert_eq!(
        String::from_utf8(output)?,
        "username=x-access-token\npassword=ghs_SyntheticFixtureToken\npassword_expiry_utc=1767225600\n"
    );
    let request = server.join().map_err(|_| "server panicked")??;
    assert!(
        request.starts_with("POST /internal/git-credential HTTP/1.1\r\n"),
        "{request}"
    );
    assert!(
        request.contains("\r\nAuthorization: Bearer task-token-123\r\n"),
        "{request}"
    );
    let body = request.split("\r\n\r\n").nth(1).ok_or("no body")?;
    let json: serde_json::Value = serde_json::from_str(body)?;
    assert_eq!(json["protocol"], "https");
    assert_eq!(json["host"], "github.com");
    assert_eq!(json["path"], "acme/app.git");
    Ok(())
}

#[test]
fn refusal_prints_nothing() -> TestResult {
    let (port, server) =
        one_shot_server(json_response("403 Forbidden", r#"{"error":"forbidden"}"#))?;
    let env = env_for(format!("http://127.0.0.1:{port}"), Some("task-token"));
    let mut output = Vec::new();
    let code = run_credential_helper("get", &mut GIT_INPUT.as_bytes(), &mut output, &env);
    assert_eq!(code, EXIT_UNAVAILABLE);
    assert!(output.is_empty());
    server.join().map_err(|_| "server panicked")??;
    Ok(())
}

#[test]
fn multi_line_token_is_rejected() -> TestResult {
    let (port, server) =
        one_shot_server(json_response("200 OK", r#"{"token":"abc\nusername=evil"}"#))?;
    let env = env_for(format!("http://127.0.0.1:{port}"), Some("task-token"));
    let mut output = Vec::new();
    let code = run_credential_helper("get", &mut GIT_INPUT.as_bytes(), &mut output, &env);
    assert_eq!(code, EXIT_UNAVAILABLE);
    assert!(output.is_empty());
    server.join().map_err(|_| "server panicked")??;
    Ok(())
}

#[test]
fn store_erase_and_unsupported_requests_never_contact_the_server() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let env = env_for(format!("http://127.0.0.1:{port}"), Some("task-token"));
    for op in ["store", "erase", "unknown-op"] {
        let mut output = Vec::new();
        let input = "protocol=https\nhost=github.com\nusername=x\npassword=secret\n\n";
        assert_eq!(
            run_credential_helper(op, &mut input.as_bytes(), &mut output, &env),
            EXIT_OK
        );
        assert!(output.is_empty());
    }
    let mut output = Vec::new();
    let plain_http = "protocol=http\nhost=github.com\n\n";
    assert_eq!(
        run_credential_helper("get", &mut plain_http.as_bytes(), &mut output, &env),
        EXIT_UNAVAILABLE
    );
    assert!(output.is_empty());
    match listener.accept() {
        Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(()),
        Ok(_) => Err("helper connected for a no-op or non-https request".into()),
        Err(error) => Err(error.into()),
    }
}

#[test]
fn configuration_errors_are_usage_failures() -> TestResult {
    let mut output = Vec::new();
    let remote = env_for("http://203.0.113.7:8080".to_owned(), Some("task-token"));
    assert_eq!(
        run_credential_helper("get", &mut GIT_INPUT.as_bytes(), &mut output, &remote),
        EXIT_USAGE
    );
    let tls = env_for("https://127.0.0.1:8443".to_owned(), Some("task-token"));
    assert_eq!(
        run_credential_helper("get", &mut GIT_INPUT.as_bytes(), &mut output, &tls),
        EXIT_USAGE
    );
    let no_token = env_for("http://127.0.0.1:9".to_owned(), None);
    assert_eq!(
        run_credential_helper("get", &mut GIT_INPUT.as_bytes(), &mut output, &no_token),
        EXIT_USAGE
    );
    let header_injection = env_for("http://127.0.0.1:9".to_owned(), Some("tok\r\nX-Evil: 1"));
    assert_eq!(
        run_credential_helper(
            "get",
            &mut GIT_INPUT.as_bytes(),
            &mut output,
            &header_injection
        ),
        EXIT_USAGE
    );
    let malformed = "no-equals-sign\n";
    assert_eq!(
        run_credential_helper("get", &mut malformed.as_bytes(), &mut output, &no_token),
        EXIT_USAGE
    );
    assert!(output.is_empty());
    Ok(())
}

#[test]
fn unreachable_server_is_unavailable() -> TestResult {
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.local_addr()?.port()
    };
    let env = env_for(format!("http://127.0.0.1:{port}"), Some("task-token"));
    let mut output = Vec::new();
    assert_eq!(
        run_credential_helper("get", &mut GIT_INPUT.as_bytes(), &mut output, &env),
        EXIT_UNAVAILABLE
    );
    assert!(output.is_empty());
    Ok(())
}
