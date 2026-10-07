//! Fake openvscode-server for `yulius-editor` tests (feature `fake`). Synthetic only.
//!
//! Accepts the options the adapter passes, refuses `--without-connection-token`, requires the
//! token file to be private (mode without group/other bits), records its argv and each start
//! (`<user-data-dir>/fake-argv.txt`, `fake-starts.txt`, `fake-pid`), then serves HTTP on
//! `--host`/`--port`: `200` for `GET <base>/…` carrying `Cookie: vscode-tkn=<token>`, `403`
//! otherwise. Behaviour switches are files in the user-data directory:
//! `fake-fail-bind-once` (deleted, then exit 98 as if the port were taken) and `fake-forbid`
//! (always answer 403, so health checks never pass).

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

fn option(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn append(path: &Path, line: &str) {
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--without-connection-token") {
        eprintln!("fake-openvscode-server: refusing to run without a connection token");
        return ExitCode::from(3);
    }
    let (Some(host), Some(port), Some(token_file), Some(base), Some(user_data)) = (
        option(&args, "--host"),
        option(&args, "--port").and_then(|p| p.parse::<u16>().ok()),
        option(&args, "--connection-token-file"),
        option(&args, "--server-base-path"),
        option(&args, "--user-data-dir"),
    ) else {
        eprintln!("fake-openvscode-server: missing required option");
        return ExitCode::from(2);
    };
    let private =
        std::fs::metadata(&token_file).is_ok_and(|meta| meta.permissions().mode() & 0o077 == 0);
    let Ok(token) = std::fs::read_to_string(&token_file) else {
        eprintln!("fake-openvscode-server: cannot read the token file");
        return ExitCode::from(4);
    };
    if !private {
        eprintln!("fake-openvscode-server: token file must not be readable by others");
        return ExitCode::from(4);
    }
    let user_data = Path::new(&user_data);
    let _ = std::fs::write(user_data.join("fake-argv.txt"), args.join("\n"));
    let _ = std::fs::write(user_data.join("fake-pid"), std::process::id().to_string());
    append(&user_data.join("fake-starts.txt"), &port.to_string());
    if std::fs::remove_file(user_data.join("fake-fail-bind-once")).is_ok() {
        eprintln!("fake-openvscode-server: Error: listen EADDRINUSE: address already in use");
        return ExitCode::from(98);
    }
    let forbid = user_data.join("fake-forbid").exists();
    let listener = match TcpListener::bind((host.as_str(), port)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("fake-openvscode-server: Error: listen {error}");
            return ExitCode::from(98);
        }
    };
    let expected_cookie = format!("vscode-tkn={}", token.trim());
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut request = Vec::new();
        let mut buf = [0_u8; 1024];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") && request.len() < 16 * 1024 {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => request.extend_from_slice(&buf[..n]),
            }
        }
        let text = String::from_utf8_lossy(&request);
        let path = text.split_whitespace().nth(1).unwrap_or_default();
        let has_token = text.lines().any(|line| {
            line.to_ascii_lowercase().starts_with("cookie:")
                && line
                    .split([':', ';'])
                    .any(|part| part.trim() == expected_cookie)
        });
        let allowed = !forbid && path.starts_with(&format!("{base}/")) && has_token;
        let (status, body) = if allowed {
            ("200 OK", "fake editor")
        } else {
            ("403 Forbidden", "Forbidden")
        };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
    }
    ExitCode::SUCCESS
}
