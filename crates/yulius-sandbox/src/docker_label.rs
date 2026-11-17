//! What a CI job creates through the sandbox's Docker front carries the job's label (SPEC-504
//! CLB-002, CLB-003, ADR-0359).
//!
//! The front knows a CI job's connection by its caller's cgroup ([`caller_job`]). For such a
//! connection it passes the client's bytes through [`pass_labelled`] instead of copying them:
//! each HTTP/1.1 request is read as a head and a body framed by `Content-Length` or `chunked`,
//! and the body of `POST [/v<version>]/containers/create` and `POST [/v<version>]/networks/create`
//! gets [`CI_JOB_LABEL`] set to the job's id. Everything else passes as it came. A call that
//! turns the connection into a stream (attach, exec start, a BuildKit session or gRPC, any
//! `Upgrade`) and anything that does not read as a request switch the connection to plain
//! copying from there on, so interactive streams are never held back. The engine's answers are
//! never read.

use std::io;

use tokio::io::{
    AsyncBufReadExt as _, AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _, BufReader,
};

/// The label a CI job's containers and networks carry: the job's id.
pub const CI_JOB_LABEL: &str = "yulius.ci.job";
/// Longest request head read; a longer one passes unread.
const HEAD_MAX: u64 = 64 << 10;
/// Longest create body labelled; a longer one passes unlabelled.
const BODY_MAX: usize = 4 << 20;
/// Longest chunk-size or trailer line.
const LINE_MAX: u64 = 4 << 10;

/// The CI job the process `pid` belongs to, read from `<proc>/<pid>/cgroup` through the handle
/// `proc` of the host's `/proc` (opened before the front joined the sandbox's mount namespace,
/// where `/proc` may show another PID namespace). `None` for any process outside a CI job's own
/// group ([`crate::cgroup::ci_job_of`]) and when the file cannot be read.
#[must_use]
pub fn caller_job(proc: &std::os::fd::OwnedFd, pid: u32) -> Option<String> {
    use nix::fcntl::{OFlag, openat};
    use nix::sys::stat::Mode;
    use std::io::Read as _;
    let fd = openat(
        proc,
        format!("{pid}/cgroup").as_str(),
        OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::empty(),
    )
    .ok()?;
    let mut text = String::new();
    std::fs::File::from(fd)
        .take(LINE_MAX)
        .read_to_string(&mut text)
        .ok()?;
    crate::cgroup::ci_job_of(&crate::cgroup::own_path(&text)?)
}

/// Copies the client's requests from `from` to `to` (the engine), labelling CI job `job`'s
/// creates (see the module docs), until `from` ends or either side fails; then shuts `to`'s
/// writing and gives `from` back.
pub async fn pass_labelled<R, W>(from: R, mut to: W, job: &str) -> R
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut reader = BufReader::new(from);
    if let Ok(Rest::Raw) = label_requests(&mut reader, &mut to, job).await {
        let _ = tokio::io::copy_buf(&mut reader, &mut to).await;
    }
    let _ = to.shutdown().await;
    reader.into_inner()
}

/// What follows the requests read.
enum Rest {
    /// The client's side ended.
    Ended,
    /// Bytes that are not requests (a stream, or something unread): copied as they come.
    Raw,
}

async fn label_requests<R, W>(reader: &mut BufReader<R>, to: &mut W, job: &str) -> io::Result<Rest>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    loop {
        let mut head = Vec::new();
        let complete = read_head(reader, &mut head).await?;
        if head.is_empty() {
            return Ok(Rest::Ended);
        }
        let request = complete.then(|| Request::parse(&head)).flatten();
        let Some(request) = request else {
            to.write_all(&head).await?;
            return Ok(Rest::Raw);
        };
        match (&request.body, request.creates()) {
            (Body::Length(length), true) if *length <= BODY_MAX as u64 => {
                let mut body = Vec::new();
                (&mut *reader).take(*length).read_to_end(&mut body).await?;
                if (body.len() as u64) < *length {
                    // Cut short: what came passes.
                    to.write_all(&head).await?;
                    to.write_all(&body).await?;
                    return Ok(Rest::Ended);
                }
                write_labelled(to, &head, body, job).await?;
            }
            (Body::Chunked, true) => match read_chunked(reader).await? {
                Chunked::Whole(body) => write_labelled(to, &head, body, job).await?,
                Chunked::Long {
                    read,
                    size_line,
                    size,
                } => {
                    // On as it came, the chunks read so far as one.
                    to.write_all(&head).await?;
                    if !read.is_empty() {
                        to.write_all(format!("{:x}\r\n", read.len()).as_bytes())
                            .await?;
                        to.write_all(&read).await?;
                        to.write_all(b"\r\n").await?;
                    }
                    to.write_all(&size_line).await?;
                    let whole = size.saturating_add(2);
                    let copied = tokio::io::copy(&mut (&mut *reader).take(whole), to).await?;
                    if copied < whole || !pass_chunked(reader, to).await? {
                        return Ok(Rest::Ended);
                    }
                }
                Chunked::Cut => return Ok(Rest::Ended),
            },
            _ => {
                to.write_all(&head).await?;
                if !pass_body(reader, to, &request.body).await? {
                    return Ok(Rest::Ended);
                }
            }
        }
        to.flush().await?;
        if request.hijacks() {
            return Ok(Rest::Raw);
        }
    }
}

/// Reads a request head (up to and with its empty line) into `head`, at most [`HEAD_MAX`];
/// whether it is whole. An empty `head` means the client's side ended.
async fn read_head<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
    head: &mut Vec<u8>,
) -> io::Result<bool> {
    loop {
        let left = HEAD_MAX.saturating_sub(head.len() as u64);
        if left == 0 {
            return Ok(false);
        }
        let read = (&mut *reader).take(left).read_until(b'\n', head).await?;
        if read == 0 || !head.ends_with(b"\n") {
            return Ok(false);
        }
        if head.ends_with(b"\r\n\r\n") || head.as_slice() == b"\r\n" {
            return Ok(head.as_slice() != b"\r\n");
        }
    }
}

/// How a request's body is framed.
#[derive(Debug, PartialEq, Eq)]
enum Body {
    None,
    Length(u64),
    Chunked,
}

/// What the labeller needs of a request head.
#[derive(Debug)]
struct Request {
    method: String,
    /// The path without its query and API version.
    path: String,
    body: Body,
    upgrade: bool,
}

impl Request {
    /// The request in `head`, `None` when it does not read as an HTTP/1.x request with a body
    /// framing understood here.
    fn parse(head: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(head).ok()?;
        let mut lines = text.split("\r\n");
        let mut first = lines.next()?.split(' ');
        let (method, target, version) = (first.next()?, first.next()?, first.next()?);
        if first.next().is_some()
            || method.is_empty()
            || !method.bytes().all(|b| b.is_ascii_uppercase())
            || !target.starts_with('/')
            || !version.starts_with("HTTP/1.")
        {
            return None;
        }
        let mut body = Body::None;
        let (mut length, mut upgrade) = (None, false);
        for line in lines.take_while(|line| !line.is_empty()) {
            let (name, value) = line.split_once(':')?;
            let value = value.trim();
            match name.trim().to_ascii_lowercase().as_str() {
                "content-length" => {
                    if length.is_some() {
                        return None;
                    }
                    length = Some(value.parse::<u64>().ok()?);
                }
                "transfer-encoding" => {
                    if !value.eq_ignore_ascii_case("chunked") {
                        return None;
                    }
                    body = Body::Chunked;
                }
                "upgrade" => upgrade = true,
                "connection" => {
                    upgrade |= value
                        .split(',')
                        .any(|token| token.trim().eq_ignore_ascii_case("upgrade"));
                }
                _ => {}
            }
        }
        match (&body, length) {
            (Body::Chunked, Some(_)) => return None,
            (Body::None, Some(length)) => body = Body::Length(length),
            _ => {}
        }
        Some(Self {
            method: method.to_owned(),
            path: api_path(target).to_owned(),
            body,
            upgrade,
        })
    }

    /// Whether it creates a container or a network.
    fn creates(&self) -> bool {
        self.method == "POST"
            && matches!(
                self.path.as_str(),
                "/containers/create" | "/networks/create"
            )
    }

    /// Whether the connection carries a stream after it.
    fn hijacks(&self) -> bool {
        let path = self.path.as_str();
        self.upgrade
            || matches!(path, "/session" | "/grpc")
            || (path.starts_with("/containers/")
                && (path.ends_with("/attach") || path.ends_with("/attach/ws")))
            || (path.starts_with("/exec/") && path.ends_with("/start"))
    }
}

/// `target` without its query and its `/v<major>.<minor>` API version.
fn api_path(target: &str) -> &str {
    let path = target.split(['?', '#']).next().unwrap_or(target);
    if let Some(rest) = path.strip_prefix("/v")
        && let Some(slash) = rest.find('/')
        && rest[..slash].split_once('.').is_some_and(|(major, minor)| {
            [major, minor]
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        })
    {
        return &rest[slash..];
    }
    path
}

/// `body` (a create's JSON object) with [`CI_JOB_LABEL`] set to `job` among its `Labels`;
/// `None` when it is not a JSON object or its `Labels` is neither an object nor null.
fn labelled_body(body: &[u8], job: &str) -> Option<Vec<u8>> {
    let mut value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let object = value.as_object_mut()?;
    let labels = object
        .entry("Labels")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if labels.is_null() {
        *labels = serde_json::Value::Object(serde_json::Map::new());
    }
    labels
        .as_object_mut()?
        .insert(CI_JOB_LABEL.to_owned(), serde_json::Value::from(job));
    serde_json::to_vec(&value).ok()
}

/// Writes the create `head` with its body labelled (framed by `Content-Length`, without
/// `Expect`), or as it came when the body cannot be labelled.
async fn write_labelled<W: AsyncWrite + Unpin>(
    to: &mut W,
    head: &[u8],
    body: Vec<u8>,
    job: &str,
) -> io::Result<()> {
    let labelled = labelled_body(&body, job);
    let body = labelled.as_deref().unwrap_or(&body);
    let text = String::from_utf8_lossy(head);
    let mut out = String::with_capacity(head.len() + 32);
    for line in text.split("\r\n").filter(|line| !line.is_empty()) {
        let name = line
            .split_once(':')
            .map(|(name, _)| name.trim().to_ascii_lowercase());
        if matches!(
            name.as_deref(),
            Some("content-length" | "transfer-encoding" | "expect")
        ) {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    // A chunked body read whole goes on with its length, like any other.
    out.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    to.write_all(out.as_bytes()).await?;
    to.write_all(body).await
}

/// A chunked body read for labelling.
enum Chunked {
    /// The whole body, decoded.
    Whole(Vec<u8>),
    /// Too long to label: the chunks `read` before the one of `size` bytes whose `size_line`
    /// was read last; that chunk's bytes and the rest are still to read.
    Long {
        read: Vec<u8>,
        size_line: Vec<u8>,
        size: u64,
    },
    /// The client's side ended or the framing broke.
    Cut,
}

/// Reads one chunk-size line (bounded); `None` when it ended or does not read as one.
async fn chunk_size<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
    line: &mut Vec<u8>,
) -> io::Result<Option<u64>> {
    line.clear();
    (&mut *reader)
        .take(LINE_MAX)
        .read_until(b'\n', line)
        .await?;
    if !line.ends_with(b"\r\n") {
        return Ok(None);
    }
    let text = std::str::from_utf8(&line[..line.len() - 2]).unwrap_or_default();
    let size = text.split(';').next().unwrap_or_default().trim();
    Ok(u64::from_str_radix(size, 16).ok())
}

/// Reads a chunked body; trailers are dropped.
async fn read_chunked<R: AsyncRead + Unpin>(reader: &mut BufReader<R>) -> io::Result<Chunked> {
    let mut body = Vec::new();
    let mut line = Vec::new();
    loop {
        let Some(size) = chunk_size(reader, &mut line).await? else {
            return Ok(Chunked::Cut);
        };
        if size == 0 {
            loop {
                line.clear();
                (&mut *reader)
                    .take(LINE_MAX)
                    .read_until(b'\n', &mut line)
                    .await?;
                if !line.ends_with(b"\n") {
                    return Ok(Chunked::Cut);
                }
                if line.as_slice() == b"\r\n" {
                    return Ok(Chunked::Whole(body));
                }
            }
        }
        let start = body.len();
        let Ok(size) = usize::try_from(size) else {
            return Ok(Chunked::Cut);
        };
        if start.saturating_add(size) > BODY_MAX {
            return Ok(Chunked::Long {
                read: body,
                size_line: line,
                size: size as u64,
            });
        }
        body.resize(start + size, 0);
        if reader.read_exact(&mut body[start..]).await.is_err() {
            return Ok(Chunked::Cut);
        }
        let mut end = [0u8; 2];
        if reader.read_exact(&mut end).await.is_err() || &end != b"\r\n" {
            return Ok(Chunked::Cut);
        }
    }
}

/// Copies a chunked body's remaining chunks and trailers as they come; whether it ended whole.
async fn pass_chunked<R, W>(reader: &mut BufReader<R>, to: &mut W) -> io::Result<bool>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut line = Vec::new();
    loop {
        let Some(size) = chunk_size(reader, &mut line).await? else {
            to.write_all(&line).await?;
            return Ok(false);
        };
        to.write_all(&line).await?;
        if size == 0 {
            loop {
                line.clear();
                (&mut *reader)
                    .take(LINE_MAX)
                    .read_until(b'\n', &mut line)
                    .await?;
                to.write_all(&line).await?;
                if !line.ends_with(b"\n") {
                    return Ok(false);
                }
                if line.as_slice() == b"\r\n" {
                    return Ok(true);
                }
            }
        }
        // The chunk and its CRLF.
        let copied = tokio::io::copy(&mut (&mut *reader).take(size.saturating_add(2)), to).await?;
        if copied < size.saturating_add(2) {
            return Ok(false);
        }
    }
}

/// Copies a body framed as `body`; whether it ended whole.
async fn pass_body<R, W>(reader: &mut BufReader<R>, to: &mut W, body: &Body) -> io::Result<bool>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    match body {
        Body::None => Ok(true),
        Body::Length(length) => {
            let copied = tokio::io::copy(&mut (&mut *reader).take(*length), to).await?;
            Ok(copied == *length)
        }
        Body::Chunked => pass_chunked(reader, to).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// What the engine receives when a CI job's client sends `input`.
    async fn through(input: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let _ = pass_labelled(input, &mut out, "job-1").await;
        out
    }

    fn post(path: &str, body: &str) -> String {
        format!(
            "POST {path} HTTP/1.1\r\nHost: api.moby.localhost\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
    }

    /// The JSON body of the single request in `bytes`, checking its `Content-Length`.
    fn body_of(bytes: &[u8]) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let text = std::str::from_utf8(bytes)?;
        let (head, body) = text.split_once("\r\n\r\n").ok_or("no head")?;
        let length = head
            .lines()
            .find_map(|l| l.strip_prefix("Content-Length: "))
            .ok_or("no length")?;
        assert_eq!(length.parse::<usize>()?, body.len(), "{text}");
        assert!(
            !head.to_ascii_lowercase().contains("transfer-encoding"),
            "{head}"
        );
        Ok(serde_json::from_str(body)?)
    }

    /// SPEC-504 CLB-003: on a kept-alive connection, a container's and a network's create get
    /// the job's label beside their own labels; every other call passes byte for byte.
    #[tokio::test]
    async fn creates_on_a_kept_alive_connection_get_the_jobs_label() -> TestResult {
        let ping = "GET /_ping HTTP/1.1\r\nHost: api.moby.localhost\r\n\r\n";
        let container = post(
            "/v1.47/containers/create?name=browser",
            r#"{"Image":"quartio-browser","Labels":{"app":"quartio","yulius.ci.job":"forged"},"HostConfig":{"Memory":2147483648,"NanoCpus":1500000000}}"#,
        );
        let start = post("/v1.47/containers/browser/start", "");
        let network = post(
            "/networks/create",
            r#"{"Name":"quartio-test-1","Driver":"bridge"}"#,
        );
        let delete = "DELETE /v1.47/containers/old?force=1 HTTP/1.1\r\nHost: x\r\n\r\n";
        let input = [ping, &container, &start, &network, delete].concat();
        let out = String::from_utf8(through(input.as_bytes()).await)?;
        assert!(out.starts_with(ping), "{out}");
        assert!(out.ends_with(delete), "{out}");
        assert!(out.contains(&start), "{out}");
        let at = |what: &str| out.find(what).ok_or(format!("no {what}: {out}"));
        let container_at = at("POST /v1.47/containers/create")?;
        let start_at = at("POST /v1.47/containers/browser/start")?;
        let created = body_of(&out.as_bytes()[container_at..start_at])?;
        assert_eq!(created["Labels"]["app"], "quartio");
        assert_eq!(
            created["Labels"][CI_JOB_LABEL], "job-1",
            "the job's own, not forged"
        );
        assert_eq!(created["HostConfig"]["NanoCpus"], 1_500_000_000_u64);
        let network_at = at("POST /networks/create")?;
        let delete_at = at("DELETE ")?;
        let made = body_of(&out.as_bytes()[network_at..delete_at])?;
        assert_eq!(made["Labels"][CI_JOB_LABEL], "job-1");
        assert_eq!(made["Name"], "quartio-test-1");
        Ok(())
    }

    /// CLB-003: `Labels: null`, a chunked create and an `Expect` header: labelled, framed by
    /// length.
    #[tokio::test]
    async fn a_chunked_or_null_labelled_create_is_labelled() -> TestResult {
        let input = "POST /v1.44/containers/create HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\
                     Expect: 100-continue\r\n\r\n6\r\n{\"Labe\r\n10\r\nls\":null,\"Image\"\r\n5\r\n:\"a\"}\r\n0\r\n\r\n";
        let out = through(input.as_bytes()).await;
        let body = body_of(&out)?;
        assert_eq!(body["Labels"][CI_JOB_LABEL], "job-1");
        assert_eq!(body["Image"], "a");
        assert!(!String::from_utf8_lossy(&out).contains("Expect"));
        Ok(())
    }

    /// CLB-003: a create whose body is not a JSON object, or is over 4 MiB, passes as it came;
    /// so does a chunked one over 4 MiB, still chunked and whole.
    #[tokio::test]
    async fn an_unlabellable_create_passes_as_it_came() -> TestResult {
        for body in ["not json", "[1,2]", r#"{"Labels":"text"}"#] {
            let input = post("/containers/create", body);
            assert_eq!(through(input.as_bytes()).await, input.as_bytes(), "{body}");
        }
        let long = format!(r#"{{"Image":"{}"}}"#, "a".repeat(BODY_MAX));
        let input = post("/containers/create", &long);
        assert_eq!(through(input.as_bytes()).await, input.as_bytes());
        let mut chunked =
            b"POST /containers/create HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        let parts = [&long.as_bytes()[..10], &long.as_bytes()[10..]];
        for part in parts {
            chunked.extend(format!("{:x}\r\n", part.len()).as_bytes());
            chunked.extend(part);
            chunked.extend(b"\r\n");
        }
        chunked.extend(b"0\r\n\r\n");
        chunked.extend(b"GET /_ping HTTP/1.1\r\n\r\n");
        let out = through(&chunked).await;
        assert_eq!(out, chunked, "the chunks read go on as they came");
        Ok(())
    }

    /// CLB-003: after a call that turns the connection into a stream, the client's bytes are
    /// copied unread, even ones that look like a create.
    #[tokio::test]
    async fn a_hijacked_connection_passes_unread() -> TestResult {
        let create = post("/containers/create", r#"{"Image":"a"}"#);
        for call in [
            "POST /v1.47/containers/abc/attach?stream=1&stdin=1 HTTP/1.1\r\nHost: x\r\n\r\n",
            "POST /exec/abc/start HTTP/1.1\r\nContent-Length: 2\r\n\r\n{}",
            "GET /containers/abc/attach/ws HTTP/1.1\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n",
            "POST /session HTTP/1.1\r\nUpgrade: h2c\r\nConnection: Upgrade\r\n\r\n",
            "POST /v1.47/grpc HTTP/1.1\r\n\r\n",
            "POST /v1.47/build HTTP/1.1\r\nConnection: keep-alive, Upgrade\r\nUpgrade: tcp\r\n\r\n",
        ] {
            let input = format!("{call}{create}");
            assert_eq!(through(input.as_bytes()).await, input.as_bytes(), "{call}");
        }
        Ok(())
    }

    /// CLB-003: what does not read as a request passes unread, from there on.
    #[tokio::test]
    async fn what_is_not_a_request_passes_unread() -> TestResult {
        let create = post("/containers/create", r#"{"Image":"a"}"#);
        for start in [
            "garbage\r\n\r\n",
            "post /containers/create HTTP/1.1\r\n\r\n",
            "POST /containers/create HTTP/2\r\n\r\n",
            "POST /x HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx",
            "POST /x HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n",
            "POST /x HTTP/1.1\r\nBroken header\r\n\r\n",
        ] {
            let input = format!("{start}{create}");
            assert_eq!(through(input.as_bytes()).await, input.as_bytes(), "{start}");
        }
        // Cut short: what came passes.
        for cut in [
            "POST /containers/create HTTP/1.1\r\nContent-Le",
            "POST /containers/create HTTP/1.1\r\nContent-Length: 50\r\n\r\n{\"Im",
        ] {
            assert_eq!(through(cut.as_bytes()).await, cut.as_bytes(), "{cut}");
        }
        Ok(())
    }

    /// CLB-003: bodies of other calls, by length or chunked, pass byte for byte, and the next
    /// request after them is still read.
    #[tokio::test]
    async fn other_bodies_pass_and_the_next_request_is_read() -> TestResult {
        let build = "POST /v1.47/build?t=x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n\
                     3;ext=1\r\nabc\r\n0\r\nTrailer: t\r\n\r\n";
        let load = "POST /images/load HTTP/1.1\r\nContent-Length: 5\r\n\r\nPOST ";
        let create = post("/containers/create", r#"{"Image":"a"}"#);
        let input = format!("{build}{load}{create}");
        let out = through(input.as_bytes()).await;
        let prefix = format!("{build}{load}");
        assert!(out.starts_with(prefix.as_bytes()));
        let body = body_of(&out[prefix.len()..])?;
        assert_eq!(body["Labels"][CI_JOB_LABEL], "job-1");
        Ok(())
    }

    /// CLB-003: a stream's bytes reach the engine as they come, not held for a line's end.
    #[tokio::test]
    async fn a_streams_bytes_are_not_held() -> TestResult {
        let (mut client, front_side) = tokio::io::duplex(1024);
        let (engine_side, mut engine) = tokio::io::duplex(1024);
        let pass = tokio::spawn(async move { pass_labelled(front_side, engine_side, "j").await });
        let attach = b"POST /containers/c/attach?stdin=1 HTTP/1.1\r\n\r\n";
        client.write_all(attach).await?;
        client.write_all(b"ls").await?;
        let mut got = vec![0u8; attach.len() + 2];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            engine.read_exact(&mut got),
        )
        .await??;
        assert_eq!(&got[attach.len()..], b"ls");
        drop(client);
        pass.await?;
        Ok(())
    }

    #[test]
    fn the_api_path_drops_the_query_and_the_version() {
        for (target, path) in [
            ("/v1.47/containers/create?name=x", "/containers/create"),
            ("/containers/create", "/containers/create"),
            ("/v1.47/networks/create", "/networks/create"),
            ("/vx.1/containers/create", "/vx.1/containers/create"),
            ("/v1./containers/create", "/v1./containers/create"),
            ("/volumes/create", "/volumes/create"),
        ] {
            assert_eq!(api_path(target), path, "{target}");
        }
        let creates = |line: &str| {
            Request::parse(format!("{line}\r\n\r\n").as_bytes()).is_some_and(|r| r.creates())
        };
        assert!(creates("POST /v1.24/containers/create HTTP/1.1"));
        assert!(creates("POST /networks/create?x=1 HTTP/1.0"));
        assert!(!creates("POST /containers/create2 HTTP/1.1"));
        assert!(
            !creates("POST /volumes/create HTTP/1.1"),
            "volumes are never touched"
        );
        assert!(!creates("GET /containers/create HTTP/1.1"));
    }

    /// CLB-002: a process is a CI job's when its cgroup is a CI job's own group.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_caller_is_read_from_its_cgroup() -> TestResult {
        use std::os::fd::OwnedFd;
        let proc = tempfile::tempdir()?;
        for (pid, line) in [
            (
                10,
                "0::/system.slice/yulius.service/sandboxes/s1/ci/ci-job-j7\n",
            ),
            (11, "0::/system.slice/yulius.service/sandboxes/s1/agents\n"),
            (12, "0::/system.slice/yulius.service/sandboxes/s1/ci\n"),
        ] {
            std::fs::create_dir(proc.path().join(pid.to_string()))?;
            std::fs::write(proc.path().join(format!("{pid}/cgroup")), line)?;
        }
        let dir = OwnedFd::from(std::fs::File::open(proc.path())?);
        assert_eq!(caller_job(&dir, 10).as_deref(), Some("j7"));
        assert_eq!(caller_job(&dir, 11), None);
        assert_eq!(caller_job(&dir, 12), None);
        assert_eq!(caller_job(&dir, 13), None, "gone");
        Ok(())
    }
}
