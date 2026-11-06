//! `yulius tool <name> [<json arguments>]`, run by a task's agent (SPEC-432 MCR-005, ADR-0276):
//! calls one Yulius tool through the session's `POST /mcp` with its task token, as the `yulius`
//! MCP server would, and prints the tool's text. It works when Claude Code has dropped the
//! session's Yulius MCP server, since it needs only the shell.
//!
//! Exit codes: 0 the tool answered, 1 the tool answered an error, 2 a usage or environment error
//! (or a refused token), 3 Yulius could not be reached or the call may have been cut.

use std::io::Write;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yulius_git::agent_call::{CallError, CallLimits, agent_call};

/// The route the call goes to.
const PATH: &str = "/mcp";
/// The largest answer read.
const MAX_ANSWER: usize = 32 * 1024 * 1024;

/// The bounds of a call.
#[derive(Debug, Clone, Copy)]
pub struct Pace {
    /// How long a call Yulius never read is tried again while it cannot be reached.
    pub outage_wait: Duration,
    /// Longest pause between two tries.
    pub backoff_cap: Duration,
    /// The bounds of one try.
    pub limits: CallLimits,
}

impl Default for Pace {
    fn default() -> Self {
        Self {
            outage_wait: Duration::from_secs(2 * 60),
            backoff_cap: Duration::from_secs(5),
            limits: CallLimits {
                connect: Duration::from_secs(5),
                continue_wait: Duration::from_secs(10),
                // The longest tool waits about 5 minutes (`sandbox_task_create`, SPEC-335).
                total: Duration::from_secs(30 * 60),
                max_body: MAX_ANSWER,
            },
        }
    }
}

/// Calls the tool `name` with `arguments` (a JSON object; none is `{}`); returns the exit code.
pub fn run(
    out: &mut dyn Write,
    name: &str,
    arguments: Option<&str>,
    pace: Pace,
    env: &dyn Fn(&str) -> Option<String>,
) -> i32 {
    let arguments = match arguments.map(serde_json::from_str::<Value>) {
        None => json!({}),
        Some(Ok(value @ Value::Object(_))) => value,
        Some(_) => {
            let _ = writeln!(
                out,
                "yulius tool: the arguments must be one JSON object, e.g. '{{\"number\": 3}}'"
            );
            return 2;
        }
    };
    if name.is_empty() {
        let _ = writeln!(
            out,
            "yulius tool: name the tool, e.g. `yulius tool steps_get`"
        );
        return 2;
    }
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments},
    })
    .to_string();
    let started = Instant::now();
    let mut pause = Duration::from_millis(250);
    let answer = loop {
        match agent_call(PATH, body.as_bytes(), env, pace.limits) {
            Ok(answer) => break answer,
            Err(CallError::Environment(why)) => {
                let _ = writeln!(out, "yulius tool: {why} (run it in a task's session)");
                return 2;
            }
            Err(CallError::Unsent(_)) if started.elapsed() + pause <= pace.outage_wait => {
                std::thread::sleep(pause);
                pause = (pause * 2).min(pace.backoff_cap);
            }
            Err(CallError::Unsent(why)) => {
                let _ = writeln!(
                    out,
                    "yulius tool: Yulius could not be reached for {} s ({why}): the call was not \
                     made. Try again later.",
                    pace.outage_wait.as_secs()
                );
                return 3;
            }
            Err(err) => {
                let _ = writeln!(
                    out,
                    "yulius tool: the connection to Yulius broke while {name} ran ({err}): it may \
                     have taken effect. Read its state before calling it again."
                );
                return 3;
            }
        }
    };
    if matches!(answer.status, 401 | 403) {
        let _ = writeln!(
            out,
            "yulius tool: Yulius refused this session's token (HTTP {}): the task's run ended or \
             its token changed",
            answer.status
        );
        return 2;
    }
    let Ok(answer) = serde_json::from_slice::<Value>(&answer.body) else {
        let _ = writeln!(
            out,
            "yulius tool: Yulius answered HTTP {} with no JSON",
            answer.status
        );
        return 3;
    };
    if let Some(message) = answer.pointer("/error/message").and_then(Value::as_str) {
        let _ = writeln!(out, "yulius tool: {message}");
        return 1;
    }
    let text = answer
        .pointer("/result/content")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let _ = writeln!(out, "{text}");
    if answer
        .pointer("/result/isError")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read};
    use std::net::{TcpListener, TcpStream};

    use super::*;

    fn pace() -> Pace {
        Pace {
            outage_wait: Duration::from_millis(300),
            backoff_cap: Duration::from_millis(50),
            limits: CallLimits {
                connect: Duration::from_secs(1),
                continue_wait: Duration::from_secs(2),
                total: Duration::from_secs(5),
                max_body: 4096,
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

    /// Answers one request with `status` and `body`; returns the request's body.
    fn answer_once(
        listener: TcpListener,
        status: &'static str,
        body: &'static str,
    ) -> std::thread::JoinHandle<std::io::Result<Value>> {
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept()?;
            let request = read_body(&mut stream)?;
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes())?;
            stream.write_all(body.as_bytes())?;
            Ok(serde_json::from_slice(&request).unwrap_or(Value::Null))
        })
    }

    fn read_body(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
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
        Ok(body)
    }

    fn call(port: u16, name: &str, arguments: Option<&str>) -> (i32, String) {
        let mut out = Vec::new();
        let code = run(&mut out, name, arguments, pace(), &env(port));
        (code, String::from_utf8_lossy(&out).into_owned())
    }

    #[test]
    fn a_tool_is_called_and_its_text_printed() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let server = answer_once(
            listener,
            "200 OK",
            r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"Progress: 3/8"}],"isError":false}}"#,
        );
        let (code, said) = call(port, "steps_get", Some(r#"{"number": 3}"#));
        assert_eq!(code, 0, "{said}");
        assert_eq!(said, "Progress: 3/8\n");
        let request = server.join().map_err(|_| "server panicked")??;
        assert_eq!(request["method"], json!("tools/call"));
        assert_eq!(request["params"]["name"], json!("steps_get"));
        assert_eq!(request["params"]["arguments"], json!({"number": 3}));
        Ok(())
    }

    #[test]
    fn a_tool_error_exits_1() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let server = answer_once(
            listener,
            "200 OK",
            r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"unknown tool: nope"}],"isError":true}}"#,
        );
        let (code, said) = call(port, "nope", None);
        assert_eq!(code, 1, "{said}");
        assert!(said.contains("unknown tool: nope"), "{said}");
        server.join().map_err(|_| "server panicked")??;
        Ok(())
    }

    #[test]
    fn a_refused_token_or_bad_arguments_exit_2() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let server = answer_once(listener, "401 Unauthorized", "");
        let (code, said) = call(port, "steps_get", None);
        assert_eq!(code, 2, "{said}");
        assert!(said.contains("refused this session's token"), "{said}");
        server.join().map_err(|_| "server panicked")??;
        let (code, said) = call(port, "steps_get", Some("[1]"));
        assert_eq!(code, 2, "{said}");
        let mut out = Vec::new();
        let code = run(&mut out, "steps_get", None, pace(), &|_| None);
        assert_eq!(code, 2, "{}", String::from_utf8_lossy(&out));
        Ok(())
    }

    #[test]
    fn an_unreachable_yulius_exits_3() -> Result<(), Box<dyn std::error::Error>> {
        let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
        let (code, said) = call(port, "steps_get", None);
        assert_eq!(code, 3, "{said}");
        assert!(said.contains("the call was not made"), "{said}");
        Ok(())
    }
}
