//! `yulius run NAME [ARGS…]`, run by a task's agent (SPEC-626 BRK-007): runs a brokered command
//! of the project outside the sandbox through `command_run`, waits with `command_result` while
//! it waits for a person or runs, then prints its stdout and stderr and exits with its code.
//!
//! Exit codes: the command's own; 125 a refusal, a rejection, an expired approval or a run that
//! did not end with a code (its reason on stderr); 2 a usage or environment error; 3 Yulius could
//! not be reached or the call may have been cut.

use std::io::Write;

use serde_json::{Value, json};

use crate::tool_cli::{Answer, Pace, call};

/// The exit code of a run that ended without its own.
pub const NOT_RUN: i32 = 125;

/// Runs the brokered command `name` with `args`; returns the exit code.
pub fn run(
    out: &mut dyn Write,
    err: &mut dyn Write,
    name: &str,
    args: &[String],
    pace: Pace,
    env: &dyn Fn(&str) -> Option<String>,
) -> i32 {
    if name.is_empty() {
        let _ = writeln!(
            err,
            "yulius run: name the command, e.g. `yulius run deploy status` (commands_list lists them)"
        );
        return 2;
    }
    let mut answer = call(
        "command_run",
        &json!({"name": name, "args": args}),
        pace,
        env,
    );
    loop {
        let text = match answer {
            Answer::Text(text) => text,
            Answer::ToolError(text) => {
                let _ = writeln!(err, "yulius run: {text}");
                return NOT_RUN;
            }
            Answer::Failed(code, why) => {
                let _ = writeln!(err, "yulius run: {why}");
                return code;
            }
        };
        let Ok(view) = serde_json::from_str::<Value>(&text) else {
            let _ = writeln!(
                err,
                "yulius run: Yulius answered something that is no run: {text}"
            );
            return 3;
        };
        let state = view["state"].as_str().unwrap_or_default();
        if matches!(state, "waiting" | "approved" | "running") {
            let Some(id) = view["id"].as_str() else {
                let _ = writeln!(err, "yulius run: a run without an id");
                return 3;
            };
            if state == "waiting" {
                let until = view["approval_until"].as_str().unwrap_or("its deadline");
                let _ = writeln!(
                    err,
                    "yulius run: waiting for a person to approve it (until {until})"
                );
            }
            answer = call("command_result", &json!({"id": id}), pace, env);
            continue;
        }
        let _ = write!(out, "{}", view["stdout"].as_str().unwrap_or_default());
        let _ = write!(err, "{}", view["stderr"].as_str().unwrap_or_default());
        if let Some(code) = view["exit_code"].as_i64().filter(|_| state == "done") {
            return i32::try_from(code).unwrap_or(NOT_RUN);
        }
        let reason = view["reason"].as_str().unwrap_or(state);
        let _ = writeln!(err, "yulius run: {state}: {reason}");
        return NOT_RUN;
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read};
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    use yulius_git::agent_call::CallLimits;

    use super::*;

    fn pace() -> Pace {
        Pace {
            outage_wait: Duration::from_millis(300),
            backoff_cap: Duration::from_millis(50),
            limits: CallLimits {
                connect: Duration::from_secs(1),
                continue_wait: Duration::from_secs(2),
                total: Duration::from_secs(5),
                max_body: 1 << 20,
            },
        }
    }

    fn env(port: u16) -> impl Fn(&str) -> Option<String> {
        move |name| match name {
            "YULIUS_URL" => Some(format!("http://127.0.0.1:{port}")),
            "YULIUS_TASK_TOKEN" => Some("tk_synthetic".to_owned()),
            _ => None,
        }
    }

    fn read_request(stream: &mut TcpStream) -> std::io::Result<Value> {
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line)?;
            let line = line.trim_end().to_ascii_lowercase();
            if line.is_empty() {
                break;
            }
            if let Some(n) = line.strip_prefix("content-length: ") {
                length = n.parse().unwrap_or(0);
            }
        }
        stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
        let mut body = vec![0; length];
        reader.read_exact(&mut body)?;
        Ok(serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    /// Answers one request per tool result in `results`, in order; returns the requests.
    fn serve(
        listener: TcpListener,
        results: Vec<Value>,
    ) -> std::thread::JoinHandle<std::io::Result<Vec<Value>>> {
        std::thread::spawn(move || {
            let mut requests = Vec::new();
            for result in results {
                let (mut stream, _) = listener.accept()?;
                requests.push(read_request(&mut stream)?);
                let body = json!({"jsonrpc": "2.0", "id": 1, "result": result}).to_string();
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                stream.write_all(head.as_bytes())?;
                stream.write_all(body.as_bytes())?;
            }
            Ok(requests)
        })
    }

    fn text(view: &Value) -> Value {
        json!({"content": [{"type": "text", "text": view.to_string()}]})
    }

    /// BRK-007: `yulius run` waits while the run waits or runs, then prints its streams and exits
    /// with its code; a refusal exits 125 with the reason.
    #[test]
    fn a_run_is_followed_to_its_end_and_exits_with_its_code()
    -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let server = serve(
            listener,
            vec![
                text(&json!({"id": "run-1", "state": "running"})),
                text(&json!({
                    "id": "run-1", "state": "done", "exit_code": 3,
                    "stdout": "deployed\n", "stderr": "warning\n",
                })),
            ],
        );
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let args = vec!["push".to_owned(), "--send".to_owned()];
        let code = run(&mut out, &mut err, "deploy", &args, pace(), &env(port));
        assert_eq!(code, 3);
        assert_eq!(String::from_utf8(out)?, "deployed\n");
        assert_eq!(String::from_utf8(err)?, "warning\n");
        let requests = server.join().map_err(|_| "server")??;
        assert_eq!(requests[0]["params"]["name"], "command_run");
        assert_eq!(
            requests[0]["params"]["arguments"],
            json!({"name": "deploy", "args": ["push", "--send"]})
        );
        assert_eq!(requests[1]["params"]["name"], "command_result");
        assert_eq!(requests[1]["params"]["arguments"], json!({"id": "run-1"}));

        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let server = serve(
            listener,
            vec![
                json!({"isError": true, "content": [{"type": "text", "text": "command refused: \"/etc/passwd\" looks like a file path"}]}),
            ],
        );
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(
            &mut out,
            &mut err,
            "deploy",
            &["/etc/passwd".to_owned()],
            pace(),
            &env(port),
        );
        assert_eq!(code, NOT_RUN);
        assert!(out.is_empty());
        assert!(String::from_utf8(err)?.contains("looks like a file path"));
        server.join().map_err(|_| "server")??;

        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let server = serve(
            listener,
            vec![text(
                &json!({"id": "run-2", "state": "timed_out", "reason": "stopped at its time limit of 2 s", "stdout": "", "stderr": ""}),
            )],
        );
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(&mut out, &mut err, "deploy", &[], pace(), &env(port));
        assert_eq!(code, NOT_RUN);
        assert!(String::from_utf8(err)?.contains("timed_out: stopped at its time limit"));
        server.join().map_err(|_| "server")??;
        Ok(())
    }
}
