//! `yulius mcp`, the stdio bridge of a task run's Yulius MCP (SPEC-358, ADR-0244): Claude Code
//! starts it as a stdio server, and it forwards each JSON-RPC line to the run's `POST /mcp`. The
//! process lives as long as the session, so a Yulius restart is a slow call, never a failed
//! server that only `/mcp` would bring back.
//!
//! Contract:
//! - a message Yulius never read ([`CallError::Unsent`]) is sent again with a backoff until
//!   [`Settings::outage_wait`] (MSB-002);
//! - one Yulius may have read ([`CallError::Lost`]) is sent again only when it changes nothing
//!   (`initialize`, `ping`, `tools/list`, a notification); a `tools/call` answers that it may have
//!   taken effect (MSB-003);
//! - a held request Claude Code cancels is never sent; at most [`MAX_IN_FLIGHT`] messages are in
//!   flight; lines and answers are bounded; each answer is one line (MSB-004).

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yulius_git::agent_call::{CallError, CallLimits, agent_call};

/// The route the bridge forwards to.
const PATH: &str = "/mcp";
/// Most messages in flight at once.
pub const MAX_IN_FLIGHT: usize = 64;
/// Longest line read from Claude Code.
const MAX_LINE: usize = 4 * 1024 * 1024;
/// Largest answer passed back.
const MAX_ANSWER: usize = 32 * 1024 * 1024;
/// Methods that change nothing, sent again even when Yulius may have read them.
const SAFE_TO_RESEND: [&str; 3] = ["initialize", "ping", "tools/list"];
/// How long the calls in flight may finish once Claude Code closed stdin.
const DRAIN: Duration = Duration::from_secs(10);

/// The bridge's bounds.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    /// How long a message Yulius cannot be reached for is held before it is answered as not made.
    pub outage_wait: Duration,
    /// Longest pause between two tries.
    pub backoff_cap: Duration,
    /// The bounds of one try.
    pub limits: CallLimits,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            outage_wait: Duration::from_secs(10 * 60),
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

type Env = dyn Fn(&str) -> Option<String> + Send + Sync;

struct Shared {
    env: Box<Env>,
    out: Mutex<Box<dyn Write + Send>>,
    /// The requests in flight, by id, and whether Claude Code cancelled each.
    pending: Mutex<HashMap<String, bool>>,
    /// Messages being forwarded, and its signal when one ends.
    in_flight: Mutex<usize>,
    ended: Condvar,
    /// Whether the last try found Yulius unreachable, so the log says so once per outage.
    down: AtomicBool,
    settings: Settings,
}

/// Runs the bridge until `input` ends; returns the exit code.
pub fn run(
    input: &mut dyn BufRead,
    output: Box<dyn Write + Send>,
    env: Box<Env>,
    settings: Settings,
) -> i32 {
    let shared = Arc::new(Shared {
        env,
        out: Mutex::new(output),
        pending: Mutex::new(HashMap::new()),
        in_flight: Mutex::new(0),
        ended: Condvar::new(),
        down: AtomicBool::new(false),
        settings,
    });
    loop {
        match read_line(input) {
            Ok(Line::End) => {
                shared.drain();
                return 0;
            }
            Ok(Line::TooLong) => shared.answer(
                &Value::Null,
                -32600,
                &format!("message longer than {MAX_LINE} bytes"),
            ),
            Ok(Line::Text(line)) => dispatch(&shared, line),
            Err(err) => {
                eprintln!("yulius mcp: stdin: {err}");
                return 1;
            }
        }
    }
}

enum Line {
    Text(Vec<u8>),
    TooLong,
    End,
}

/// One line of at most [`MAX_LINE`] bytes; a longer one is skipped to its end.
fn read_line(input: &mut dyn BufRead) -> std::io::Result<Line> {
    let mut line = Vec::new();
    let mut too_long = false;
    loop {
        let buffer = match input.fill_buf() {
            Ok(buffer) => buffer,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        if buffer.is_empty() {
            return Ok(if too_long {
                Line::TooLong
            } else if line.is_empty() {
                Line::End
            } else {
                Line::Text(line)
            });
        }
        let (taken, done) = match buffer.iter().position(|b| *b == b'\n') {
            Some(at) => (at + 1, true),
            None => (buffer.len(), false),
        };
        if !too_long {
            line.extend_from_slice(&buffer[..taken]);
            if line.len() > MAX_LINE {
                too_long = true;
                line = Vec::new();
            }
        }
        input.consume(taken);
        if done {
            return Ok(if too_long {
                Line::TooLong
            } else {
                Line::Text(line)
            });
        }
    }
}

fn dispatch(shared: &Arc<Shared>, line: Vec<u8>) {
    if line.iter().all(u8::is_ascii_whitespace) {
        return;
    }
    let Ok(message) = serde_json::from_slice::<Value>(&line) else {
        shared.answer(&Value::Null, -32700, "parse error");
        return;
    };
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let Some(method) = method else {
        // An answer to a request of the server's: Yulius sends none.
        return;
    };
    let id = message.get("id").cloned();
    if method == "notifications/cancelled" {
        if let Some(request) = message.pointer("/params/requestId")
            && let Ok(mut pending) = shared.pending.lock()
            && let Some(cancelled) = pending.get_mut(&request.to_string())
        {
            *cancelled = true;
        }
        return;
    }
    if !shared.enter() {
        if let Some(id) = &id {
            shared.answer(
                id,
                -32000,
                &format!("more than {MAX_IN_FLIGHT} Yulius calls at once: try again"),
            );
        }
        return;
    }
    if let Some(id) = &id
        && let Ok(mut pending) = shared.pending.lock()
    {
        pending.insert(id.to_string(), false);
    }
    let shared = Arc::clone(shared);
    std::thread::spawn(move || {
        forward(&shared, &method, id.as_ref(), &line);
        if let Some(id) = &id
            && let Ok(mut pending) = shared.pending.lock()
        {
            pending.remove(&id.to_string());
        }
        shared.leave();
    });
}

/// Sends one message until it is answered, it may not be sent again, or the outage bound passes.
fn forward(shared: &Shared, method: &str, id: Option<&Value>, line: &[u8]) {
    let settings = shared.settings;
    let started = Instant::now();
    let mut pause = Duration::from_millis(250);
    loop {
        if id.is_some_and(|id| shared.is_cancelled(id)) {
            return;
        }
        let failure = match agent_call(PATH, line, shared.env.as_ref(), settings.limits) {
            Ok(answer) => {
                shared.up();
                // A request Claude Code cancelled meanwhile is not answered (MCP cancellation).
                if !id.is_some_and(|id| shared.is_cancelled(id)) {
                    shared.deliver(id, answer.status, &answer.body);
                }
                return;
            }
            Err(CallError::TooLarge(max)) => {
                shared.up();
                if let Some(id) = id {
                    shared.answer(
                        id,
                        -32603,
                        &format!("Yulius answered more than {max} bytes: ask for less"),
                    );
                }
                return;
            }
            Err(CallError::Environment(why)) => {
                if let Some(id) = id {
                    shared.answer(id, -32603, why);
                }
                return;
            }
            Err(CallError::Lost(why)) if !resendable(method, id) => {
                shared.up();
                if let Some(id) = id {
                    shared.answer(
                        id,
                        -32000,
                        &format!(
                            "Yulius restarted or the connection broke while this call ran \
                             ({why}): it may have taken effect. Read its state (for example \
                             with delivery_status, ci_status or steps_get) before calling it again."
                        ),
                    );
                }
                return;
            }
            Err(err @ (CallError::Unsent(_) | CallError::Lost(_))) => err,
        };
        if started.elapsed() + pause > settings.outage_wait {
            if let Some(id) = id {
                shared.answer(
                    id,
                    -32000,
                    &format!(
                        "Yulius could not be reached for {} s ({failure}): the call was not made. \
                         Try again later; this session's Yulius tools work again once it is back.",
                        settings.outage_wait.as_secs()
                    ),
                );
            } else {
                eprintln!("yulius mcp: {method} was not delivered: {failure}");
            }
            return;
        }
        shared.went_down(&failure);
        std::thread::sleep(pause);
        pause = (pause * 2).min(settings.backoff_cap);
    }
}

/// Whether a message Yulius may have read is sent again: one that changes nothing.
fn resendable(method: &str, id: Option<&Value>) -> bool {
    id.is_none() || SAFE_TO_RESEND.contains(&method)
}

impl Shared {
    /// Counts a message in flight; `false` past [`MAX_IN_FLIGHT`].
    fn enter(&self) -> bool {
        let Ok(mut count) = self.in_flight.lock() else {
            return false;
        };
        if *count >= MAX_IN_FLIGHT {
            return false;
        }
        *count += 1;
        true
    }

    fn leave(&self) {
        if let Ok(mut count) = self.in_flight.lock() {
            *count = count.saturating_sub(1);
        }
        self.ended.notify_all();
    }

    /// Waits (at most [`DRAIN`]) for the messages in flight to be answered.
    fn drain(&self) {
        if let Ok(count) = self.in_flight.lock() {
            let _ = self
                .ended
                .wait_timeout_while(count, DRAIN, |count| *count > 0);
        }
    }

    fn is_cancelled(&self, id: &Value) -> bool {
        self.pending
            .lock()
            .is_ok_and(|pending| pending.get(&id.to_string()) == Some(&true))
    }

    fn went_down(&self, failure: &CallError) {
        if !self.down.swap(true, Ordering::SeqCst) {
            eprintln!("yulius mcp: {failure}; holding calls until it is back");
        }
    }

    fn up(&self) {
        if self.down.swap(false, Ordering::SeqCst) {
            eprintln!("yulius mcp: Yulius is back");
        }
    }

    /// Passes Yulius's answer on: a JSON body as one line, nothing for an accepted notification.
    fn deliver(&self, id: Option<&Value>, status: u16, body: &[u8]) {
        let Some(id) = id else {
            return;
        };
        match status {
            200 => match serde_json::from_slice::<Value>(body) {
                Ok(answer) => self.write(&answer),
                Err(_) => self.answer(id, -32603, "Yulius answered something that is not JSON"),
            },
            401 | 403 => self.answer(
                id,
                -32001,
                &format!(
                    "Yulius refused this session's token (HTTP {status}): the task's run ended or \
                     its token changed"
                ),
            ),
            status => self.answer(id, -32603, &format!("Yulius answered HTTP {status}")),
        }
    }

    fn answer(&self, id: &Value, code: i64, message: &str) {
        self.write(
            &json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
        );
    }

    fn write(&self, message: &Value) {
        let mut line = message.to_string();
        line.push('\n');
        if let Ok(mut out) = self.out.lock() {
            let _ = out.write_all(line.as_bytes());
            let _ = out.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Read};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;

    use super::*;

    /// Collects what the bridge writes, line by line.
    struct Lines(mpsc::Sender<String>);

    impl Write for Lines {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            for line in String::from_utf8_lossy(buf).lines() {
                let _ = self.0.send(line.to_owned());
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn settings(outage: Duration) -> Settings {
        Settings {
            outage_wait: outage,
            backoff_cap: Duration::from_millis(50),
            limits: CallLimits {
                connect: Duration::from_secs(1),
                continue_wait: Duration::from_secs(2),
                total: Duration::from_secs(5),
                max_body: 1024,
            },
        }
    }

    fn env(port: u16) -> Box<Env> {
        Box::new(move |name| match name {
            "YULIUS_URL" => Some(format!("http://127.0.0.1:{port}")),
            "YULIUS_TASK_TOKEN" => Some("tk_synthetic".to_owned()),
            _ => None,
        })
    }

    fn bridge(input: &'static str, port: u16, outage: Duration) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut input = input.as_bytes();
            run(&mut input, Box::new(Lines(tx)), env(port), settings(outage));
        });
        rx
    }

    fn read_request(stream: &mut TcpStream) -> std::io::Result<()> {
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
        Ok(())
    }

    fn respond(stream: &mut TcpStream, body: &str) -> std::io::Result<()> {
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
        stream.write_all(head.as_bytes())?;
        stream.write_all(body.as_bytes())
    }

    fn free_port() -> std::io::Result<u16> {
        Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
    }

    fn first(rx: &mpsc::Receiver<String>) -> Result<Value, Box<dyn std::error::Error>> {
        let line = rx.recv_timeout(Duration::from_secs(10))?;
        Ok(serde_json::from_str(&line)?)
    }

    #[test]
    fn a_call_held_through_an_outage_is_answered_once_yulius_is_back()
    -> Result<(), Box<dyn std::error::Error>> {
        let port = free_port()?;
        let rx = bridge(
            "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/call\"}\n",
            port,
            Duration::from_secs(30),
        );
        // Nothing listens yet: the call is held, not answered.
        assert!(rx.recv_timeout(Duration::from_millis(400)).is_err());
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let (mut stream, _) = listener.accept()?;
        read_request(&mut stream)?;
        // A body with a line break still reaches Claude Code as one line.
        respond(&mut stream, "{\"jsonrpc\":\"2.0\",\"id\":7,\n\"result\":1}")?;
        let answer = first(&rx)?;
        assert_eq!(answer, json!({"jsonrpc": "2.0", "id": 7, "result": 1}));
        Ok(())
    }

    #[test]
    fn a_tool_call_cut_after_yulius_read_it_is_not_sent_again()
    -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let rx = bridge(
            "{\"jsonrpc\":\"2.0\",\"id\":\"a\",\"method\":\"tools/call\"}\n",
            port,
            Duration::from_secs(30),
        );
        let (mut stream, _) = listener.accept()?;
        read_request(&mut stream)?;
        drop(stream);
        let answer = first(&rx)?;
        let message = answer
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(message.contains("may have taken effect"), "{answer}");
        assert_eq!(answer["id"], json!("a"));
        listener.set_nonblocking(true)?;
        std::thread::sleep(Duration::from_millis(300));
        assert!(listener.accept().is_err(), "the call was sent again");
        Ok(())
    }

    #[test]
    fn a_tool_list_cut_after_yulius_read_it_is_sent_again() -> Result<(), Box<dyn std::error::Error>>
    {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let rx = bridge(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n",
            port,
            Duration::from_secs(30),
        );
        let (mut stream, _) = listener.accept()?;
        read_request(&mut stream)?;
        drop(stream);
        let (mut stream, _) = listener.accept()?;
        read_request(&mut stream)?;
        respond(&mut stream, "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}")?;
        assert_eq!(first(&rx)?["result"], json!({}));
        Ok(())
    }

    #[test]
    fn past_the_outage_bound_the_call_is_answered_as_not_made()
    -> Result<(), Box<dyn std::error::Error>> {
        let port = free_port()?;
        let rx = bridge(
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\"}\n",
            port,
            Duration::from_millis(300),
        );
        let answer = first(&rx)?;
        let message = answer
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(message.contains("the call was not made"), "{answer}");
        Ok(())
    }

    #[test]
    fn a_cancelled_held_call_is_never_sent() -> Result<(), Box<dyn std::error::Error>> {
        let port = free_port()?;
        let (tx, rx) = mpsc::channel();
        let (lines_tx, lines_rx) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut input = BufReader::new(ChannelReader(lines_rx, Vec::new()));
            run(
                &mut input,
                Box::new(Lines(tx)),
                env(port),
                settings(Duration::from_secs(30)),
            );
        });
        lines_tx.send(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\"}\n".to_vec())?;
        std::thread::sleep(Duration::from_millis(200));
        lines_tx.send(
            b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":3}}\n"
                .to_vec(),
        )?;
        std::thread::sleep(Duration::from_millis(200));
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        listener.set_nonblocking(true)?;
        std::thread::sleep(Duration::from_millis(500));
        assert!(listener.accept().is_err(), "a cancelled call was sent");
        assert!(rx.try_recv().is_err(), "a cancelled call was answered");
        Ok(())
    }

    #[test]
    fn a_refused_token_is_said_and_a_bad_line_answered() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let rx = bridge(
            "not json\n{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/list\"}\n",
            port,
            Duration::from_secs(30),
        );
        assert_eq!(first(&rx)?["error"]["code"], json!(-32700));
        let (mut stream, _) = listener.accept()?;
        let mut head = [0_u8; 1];
        stream.read_exact(&mut head)?;
        stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")?;
        let answer = first(&rx)?;
        assert_eq!(answer["error"]["code"], json!(-32001), "{answer}");
        Ok(())
    }

    /// A call in flight when stdin ends is still answered before the bridge exits.
    #[test]
    fn the_calls_in_flight_are_answered_after_stdin_ends() -> Result<(), Box<dyn std::error::Error>>
    {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let server = std::thread::spawn(move || -> std::io::Result<()> {
            let (mut stream, _) = listener.accept()?;
            read_request(&mut stream)?;
            std::thread::sleep(Duration::from_millis(200));
            respond(&mut stream, "{\"jsonrpc\":\"2.0\",\"id\":5,\"result\":{}}")
        });
        let (tx, rx) = mpsc::channel();
        let mut input = "{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"ping\"}\n".as_bytes();
        run(
            &mut input,
            Box::new(Lines(tx)),
            env(port),
            settings(Duration::from_secs(30)),
        );
        // `run` returned: the answer must be written already.
        let answer: Value = serde_json::from_str(&rx.try_recv()?)?;
        assert_eq!(answer["id"], json!(5));
        server.join().map_err(|_| "server panicked")??;
        Ok(())
    }

    /// An answer past the bound is said at once, even for a method safe to resend (review round
    /// 1: it was resent for the whole outage bound).
    #[test]
    fn an_oversized_answer_is_said_at_once() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let rx = bridge(
            "{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"tools/list\"}\n",
            port,
            Duration::from_secs(30),
        );
        let (mut stream, _) = listener.accept()?;
        read_request(&mut stream)?;
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 999999\r\n\r\n")?;
        let answer = first(&rx)?;
        let message = answer
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(message.contains("more than 1024 bytes"), "{answer}");
        Ok(())
    }

    /// A call Claude Code cancels while Yulius works on it gets no answer (MCP: a cancelled
    /// request is not answered).
    #[test]
    fn a_call_cancelled_in_flight_is_not_answered() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let (tx, rx) = mpsc::channel();
        let (lines_tx, lines_rx) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut input = BufReader::new(ChannelReader(lines_rx, Vec::new()));
            run(
                &mut input,
                Box::new(Lines(tx)),
                env(port),
                settings(Duration::from_secs(30)),
            );
        });
        lines_tx.send(b"{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"tools/call\"}\n".to_vec())?;
        let (mut stream, _) = listener.accept()?;
        read_request(&mut stream)?;
        lines_tx.send(
            b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":9}}\n"
                .to_vec(),
        )?;
        // The cancel is read before the answer goes out: a ping after it is answered first.
        lines_tx.send(b"{\"jsonrpc\":\"2.0\",\"id\":10,\"method\":\"ping\"}\n".to_vec())?;
        let (mut ping, _) = listener.accept()?;
        read_request(&mut ping)?;
        respond(&mut ping, "{\"jsonrpc\":\"2.0\",\"id\":10,\"result\":{}}")?;
        assert_eq!(first(&rx)?["id"], json!(10));
        respond(&mut stream, "{\"jsonrpc\":\"2.0\",\"id\":9,\"result\":{}}")?;
        drop(lines_tx);
        assert!(
            rx.recv_timeout(Duration::from_secs(2)).is_err(),
            "a cancelled call was answered"
        );
        Ok(())
    }

    /// stdin fed line by line, ending when the sender is dropped.
    struct ChannelReader(mpsc::Receiver<Vec<u8>>, Vec<u8>);

    impl Read for ChannelReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.1.is_empty() {
                match self.0.recv() {
                    Ok(bytes) => self.1 = bytes,
                    Err(_) => return Ok(0),
                }
            }
            let n = buf.len().min(self.1.len());
            buf[..n].copy_from_slice(&self.1[..n]);
            self.1.drain(..n);
            Ok(n)
        }
    }
}
