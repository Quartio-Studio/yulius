//! One call of a task's session to the Yulius server it belongs to (`YULIUS_URL`,
//! `YULIUS_TASK_TOKEN`), telling a call Yulius never read from one it may have acted on
//! (SPEC-358 MSB-002): the request goes with `Expect: 100-continue`, and its body only once the
//! server asks for it. A connection that fails before that reached no handler.

use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use crate::credential::{LoopbackUrl, decode_chunked, header_end, parse_head};

/// The bounds of one call.
#[derive(Debug, Clone, Copy)]
pub struct CallLimits {
    /// Connecting to the loopback listener.
    pub connect: Duration,
    /// How long the body waits for `100 Continue`; then it is sent anyway (RFC 9110 §10.1.1),
    /// and a failure after it counts as [`CallError::Lost`].
    pub continue_wait: Duration,
    /// The whole call, answer included.
    pub total: Duration,
    /// Largest answer body.
    pub max_body: usize,
}

/// A call that got no answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CallError {
    /// `YULIUS_URL` or `YULIUS_TASK_TOKEN` is missing or invalid.
    #[error("{0}")]
    Environment(&'static str),
    /// Yulius never read the body: no handler ran, so the call can be made again.
    #[error("Yulius cannot be reached ({0})")]
    Unsent(String),
    /// The answer was larger than [`CallLimits::max_body`]: sending it again would not help.
    #[error("Yulius answered more than {0} bytes")]
    TooLarge(usize),
    /// The body was sent and the answer did not come: the call may have taken effect.
    #[error("the connection to Yulius broke during the call ({0})")]
    Lost(String),
}

/// An answer: its HTTP status and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The HTTP status.
    pub status: u16,
    /// The body, de-chunked.
    pub body: Vec<u8>,
}

/// Posts `body` (JSON) to `path` of the session's Yulius with its task token.
///
/// # Errors
/// [`CallError`]: a bad environment, a call Yulius never read, or one whose answer was lost.
pub fn agent_call(
    path: &str,
    body: &[u8],
    env: &dyn Fn(&str) -> Option<String>,
    limits: CallLimits,
) -> Result<Answer, CallError> {
    let base = env("YULIUS_URL").ok_or(CallError::Environment("YULIUS_URL is not set"))?;
    let token =
        env("YULIUS_TASK_TOKEN").ok_or(CallError::Environment("YULIUS_TASK_TOKEN is not set"))?;
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(CallError::Environment(
            "YULIUS_TASK_TOKEN must be printable ASCII",
        ));
    }
    let endpoint = LoopbackUrl::parse(&base)
        .map_err(|_| CallError::Environment("YULIUS_URL must be http:// on a loopback address"))?;
    let deadline = Instant::now() + limits.total;
    let unsent =
        |what: &str, err: &std::io::Error| CallError::Unsent(format!("{what}: {}", err.kind()));
    let mut stream =
        TcpStream::connect_timeout(&endpoint.address, limits.connect.min(limits.total))
            .map_err(|e| unsent("connect", &e))?;
    let head = format!(
        "POST {}{path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\n\
         Expect: 100-continue\r\nConnection: close\r\nUser-Agent: yulius-agent\r\n\r\n",
        endpoint.base_path,
        endpoint.host_header,
        body.len()
    );
    stream
        .set_write_timeout(Some(limits.total))
        .map_err(|e| unsent("configure", &e))?;
    stream
        .write_all(head.as_bytes())
        .map_err(|e| unsent("write", &e))?;
    let mut reader = Reader::new(stream, limits.max_body);
    // Until the server asks for the body: an interim answer, a final one, or silence.
    let continue_by = (Instant::now() + limits.continue_wait).min(deadline);
    loop {
        match reader.head(continue_by) {
            Ok(Some(100)) => break,
            // 102 and 103 say nothing about the body: wait on.
            Ok(Some(status)) if (101..200).contains(&status) => reader.drop_interim(),
            // A final answer before the body: Yulius answered without reading it (a refused
            // token, say).
            Ok(Some(_)) => {
                return reader.answer(deadline).map_err(|e| match e {
                    ReadError::TooLarge(max) => CallError::TooLarge(max),
                    e => CallError::Unsent(e.to_string()),
                });
            }
            // No word within the wait: the body goes anyway, as a server that ignores the
            // expectation needs it.
            Ok(None) => break,
            Err(ReadError::TooLarge(max)) => return Err(CallError::TooLarge(max)),
            Err(err) => return Err(CallError::Unsent(err.to_string())),
        }
    }
    reader.drop_interim();
    // A body cut short never reaches a handler, so a failed write is still unsent.
    reader
        .stream
        .write_all(body)
        .map_err(|e| unsent("write the body", &e))?;
    loop {
        match reader.head(deadline) {
            Ok(Some(status)) if (100..200).contains(&status) => reader.drop_interim(),
            Ok(Some(_)) => break,
            Ok(None) => return Err(CallError::Lost("timed out".to_owned())),
            Err(ReadError::TooLarge(max)) => return Err(CallError::TooLarge(max)),
            Err(err) => return Err(CallError::Lost(err.to_string())),
        }
    }
    reader.answer(deadline).map_err(|e| match e {
        ReadError::TooLarge(max) => CallError::TooLarge(max),
        e => CallError::Lost(e.to_string()),
    })
}

/// Reads the server's answers off one connection.
struct Reader {
    stream: TcpStream,
    raw: Vec<u8>,
    /// The end of the head read last, once one was.
    head_end: Option<usize>,
    status: u16,
    max_body: usize,
}

#[derive(Debug, thiserror::Error)]
enum ReadError {
    #[error("{0}")]
    Io(String),
    #[error("closed by Yulius")]
    Closed,
    #[error("bad answer: {0}")]
    Bad(&'static str),
    #[error("answer larger than {0} bytes")]
    TooLarge(usize),
}

impl Reader {
    fn new(stream: TcpStream, max_body: usize) -> Self {
        Self {
            stream,
            raw: Vec::new(),
            head_end: None,
            status: 0,
            max_body,
        }
    }

    /// The status of the next head, once read; `None` when `until` passed first.
    fn head(&mut self, until: Instant) -> Result<Option<u16>, ReadError> {
        loop {
            if let Some(end) = header_end(&self.raw) {
                let (status, _, _) =
                    parse_head(&self.raw[..end - 4]).map_err(|_| ReadError::Bad("head"))?;
                self.head_end = Some(end);
                self.status = status;
                return Ok(Some(status));
            }
            match self.fill(until)? {
                Fill::Read => {}
                Fill::Timeout => return Ok(None),
                Fill::Eof => return Err(ReadError::Closed),
            }
        }
    }

    /// Forgets an interim (1xx) head read last.
    fn drop_interim(&mut self) {
        if let Some(end) = self.head_end.take() {
            self.raw.drain(..end);
        }
    }

    /// The rest of the final answer whose head was read last.
    fn answer(mut self, until: Instant) -> Result<Answer, ReadError> {
        let end = self.head_end.ok_or(ReadError::Bad("no head"))?;
        let (_, length, chunked) =
            parse_head(&self.raw[..end - 4]).map_err(|_| ReadError::Bad("head"))?;
        loop {
            let body = &self.raw[end..];
            if chunked {
                if let Some(body) = decode_chunked(body).map_err(|_| ReadError::Bad("chunk"))? {
                    return Ok(Answer {
                        status: self.status,
                        body,
                    });
                }
            } else if let Some(length) = length {
                if length > self.max_body {
                    return Err(ReadError::TooLarge(self.max_body));
                }
                if body.len() >= length {
                    return Ok(Answer {
                        status: self.status,
                        body: body[..length].to_vec(),
                    });
                }
            }
            match self.fill(until)? {
                Fill::Read => {}
                Fill::Timeout => return Err(ReadError::Io("timed out".to_owned())),
                // Without a length, the body ends with the connection.
                Fill::Eof if length.is_none() && !chunked => {
                    return Ok(Answer {
                        status: self.status,
                        body: self.raw[end..].to_vec(),
                    });
                }
                Fill::Eof => return Err(ReadError::Closed),
            }
        }
    }

    fn fill(&mut self, until: Instant) -> Result<Fill, ReadError> {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(Fill::Timeout);
        }
        self.stream
            .set_read_timeout(Some(left))
            .map_err(|e| ReadError::Io(format!("configure: {}", e.kind())))?;
        let mut chunk = [0_u8; 64 * 1024];
        match self.stream.read(&mut chunk) {
            Ok(0) => Ok(Fill::Eof),
            Ok(read) => {
                self.raw.extend_from_slice(&chunk[..read]);
                if self.raw.len() > self.max_body.saturating_add(64 * 1024) {
                    return Err(ReadError::TooLarge(self.max_body));
                }
                Ok(Fill::Read)
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                Ok(Fill::Timeout)
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => Ok(Fill::Read),
            Err(e) => Err(ReadError::Io(format!("read: {}", e.kind()))),
        }
    }
}

enum Fill {
    Read,
    Timeout,
    Eof,
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    const LIMITS: CallLimits = CallLimits {
        connect: Duration::from_secs(2),
        continue_wait: Duration::from_secs(5),
        total: Duration::from_secs(10),
        max_body: 1024,
    };

    fn env_for(port: u16) -> impl Fn(&str) -> Option<String> {
        move |name| match name {
            "YULIUS_URL" => Some(format!("http://127.0.0.1:{port}")),
            "YULIUS_TASK_TOKEN" => Some("tk_synthetic".to_owned()),
            _ => None,
        }
    }

    /// Reads one request head; whether it carried `Expect: 100-continue`.
    fn read_head(reader: &mut BufReader<TcpStream>) -> std::io::Result<(bool, usize)> {
        let (mut expect, mut length) = (false, 0);
        loop {
            let mut line = String::new();
            reader.read_line(&mut line)?;
            let line = line.trim_end();
            if line.is_empty() {
                return Ok((expect, length));
            }
            let lower = line.to_ascii_lowercase();
            expect |= lower == "expect: 100-continue";
            if let Some(n) = lower.strip_prefix("content-length: ") {
                length = n.parse().unwrap_or(0);
            }
        }
    }

    /// A one-connection server running `serve` on the accepted connection.
    fn server(
        serve: impl FnOnce(BufReader<TcpStream>) -> std::io::Result<()> + Send + 'static,
    ) -> std::io::Result<(u16, thread::JoinHandle<std::io::Result<()>>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let handle = thread::spawn(move || {
            let (stream, _) = listener.accept()?;
            serve(BufReader::new(stream))
        });
        Ok((port, handle))
    }

    #[test]
    fn a_server_that_closes_before_asking_for_the_body_never_got_the_call() -> std::io::Result<()> {
        let (port, server) = server(|mut reader| {
            read_head(&mut reader)?;
            Ok(())
        })?;
        let result = agent_call("/mcp", b"{}", &env_for(port), LIMITS);
        assert!(matches!(result, Err(CallError::Unsent(_))), "{result:?}");
        server
            .join()
            .map_err(|_| std::io::Error::other("server panicked"))?
    }

    #[test]
    fn nothing_listening_is_unsent() -> std::io::Result<()> {
        let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
        let result = agent_call("/mcp", b"{}", &env_for(port), LIMITS);
        assert!(matches!(result, Err(CallError::Unsent(_))), "{result:?}");
        Ok(())
    }

    #[test]
    fn a_server_that_read_the_body_and_closed_may_have_acted() -> std::io::Result<()> {
        let (port, server) = server(|mut reader| {
            let (expect, length) = read_head(&mut reader)?;
            assert!(expect, "the call asks before sending its body");
            reader
                .get_mut()
                .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
            let mut body = vec![0; length];
            reader.read_exact(&mut body)?;
            Ok(())
        })?;
        let result = agent_call("/mcp", b"{\"id\":1}", &env_for(port), LIMITS);
        assert!(matches!(result, Err(CallError::Lost(_))), "{result:?}");
        server
            .join()
            .map_err(|_| std::io::Error::other("server panicked"))?
    }

    #[test]
    fn the_body_follows_the_continue_and_the_answer_is_read() -> std::io::Result<()> {
        let (port, server) = server(|mut reader| {
            let (_, length) = read_head(&mut reader)?;
            reader
                .get_mut()
                .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
            let mut body = vec![0; length];
            reader.read_exact(&mut body)?;
            assert_eq!(body, b"{\"id\":1}");
            reader.get_mut().write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 9\r\n\r\n{\"id\":1}\n",
            )?;
            Ok(())
        })?;
        let answer = agent_call("/mcp", b"{\"id\":1}", &env_for(port), LIMITS);
        assert_eq!(
            answer,
            Ok(Answer {
                status: 200,
                body: b"{\"id\":1}\n".to_vec()
            })
        );
        server
            .join()
            .map_err(|_| std::io::Error::other("server panicked"))?
    }

    #[test]
    fn a_final_answer_before_the_body_is_the_answer() -> std::io::Result<()> {
        let (port, server) = server(|mut reader| {
            read_head(&mut reader)?;
            reader
                .get_mut()
                .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")?;
            Ok(())
        })?;
        let answer = agent_call("/mcp", b"{}", &env_for(port), LIMITS);
        assert_eq!(answer.map(|a| a.status), Ok(401));
        server
            .join()
            .map_err(|_| std::io::Error::other("server panicked"))?
    }

    /// A server that ignores the expectation still gets the body, after the wait.
    #[test]
    fn a_silent_server_gets_the_body_after_the_wait() -> std::io::Result<()> {
        let (port, server) = server(|mut reader| {
            let (_, length) = read_head(&mut reader)?;
            let mut body = vec![0; length];
            reader.read_exact(&mut body)?;
            reader
                .get_mut()
                .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")?;
            Ok(())
        })?;
        let limits = CallLimits {
            continue_wait: Duration::from_millis(100),
            ..LIMITS
        };
        let answer = agent_call("/mcp", b"{}", &env_for(port), limits);
        assert_eq!(answer.map(|a| a.status), Ok(202));
        server
            .join()
            .map_err(|_| std::io::Error::other("server panicked"))?
    }

    #[test]
    fn an_answer_past_the_bound_is_refused() -> std::io::Result<()> {
        let (port, server) = server(|mut reader| {
            let (_, length) = read_head(&mut reader)?;
            reader
                .get_mut()
                .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
            let mut body = vec![0; length];
            reader.read_exact(&mut body)?;
            reader
                .get_mut()
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4096\r\n\r\n")?;
            Ok(())
        })?;
        let result = agent_call("/mcp", b"{}", &env_for(port), LIMITS);
        assert!(
            matches!(&result, Err(CallError::TooLarge(1024))),
            "{result:?}"
        );
        server
            .join()
            .map_err(|_| std::io::Error::other("server panicked"))?
    }

    /// An interim 103 before the 100 is skipped, not read again forever (review round 1).
    #[test]
    fn an_early_hint_before_the_continue_is_skipped() -> std::io::Result<()> {
        let (port, server) = server(|mut reader| {
            let (_, length) = read_head(&mut reader)?;
            reader.get_mut().write_all(
                b"HTTP/1.1 103 Early Hints\r\nLink: </x>\r\n\r\nHTTP/1.1 100 Continue\r\n\r\n",
            )?;
            let mut body = vec![0; length];
            reader.read_exact(&mut body)?;
            reader.get_mut().write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n{\"a\r\n3\r\n\":1\r\n1\r\n}\r\n0\r\n\r\n",
            )?;
            Ok(())
        })?;
        let answer = agent_call("/mcp", b"{}", &env_for(port), LIMITS);
        assert_eq!(
            answer,
            Ok(Answer {
                status: 200,
                body: b"{\"a\":1}".to_vec()
            }),
            "the chunked answer, de-chunked"
        );
        server
            .join()
            .map_err(|_| std::io::Error::other("server panicked"))?
    }

    #[test]
    fn a_missing_environment_is_said() {
        let result = agent_call("/mcp", b"{}", &|_| None, LIMITS);
        assert_eq!(result, Err(CallError::Environment("YULIUS_URL is not set")));
    }
}
