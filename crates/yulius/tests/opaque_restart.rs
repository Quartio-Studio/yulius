//! SPEC-577. Keep the original peers while two real server processes use the same private data.
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::fs::DirBuilderExt as _;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use base64::Engine as _;
use nix::ifaddrs::getifaddrs;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::process::{Child, Command};

type Error = Box<dyn std::error::Error>;
const TOKEN: &str = "synthetic-restart-token";

#[derive(Clone, Copy)]
enum Route {
    Direct,
    Holder,
}

impl Route {
    fn service_args(self) -> &'static [&'static str] {
        match self {
            Self::Direct => &["--isolation", "legacy"],
            Self::Holder => &[
                "--contained",
                "--isolation",
                "namespaces",
                "--sandbox-egress",
                "none",
                "--sandbox-docker",
                "off",
            ],
        }
    }
}

// A bound, non-listening Linux socket keeps the port out of ephemeral allocation during
// startup and downtime. Each real server can listen on it using SO_REUSEADDR.
struct InternalPort {
    address: SocketAddr,
    _reservation: TcpSocket,
}

impl InternalPort {
    fn reserve() -> std::io::Result<Self> {
        let socket = TcpSocket::new_v4()?;
        socket.set_reuseaddr(true)?;
        socket.bind((Ipv4Addr::LOCALHOST, 0).into())?;
        Ok(Self {
            address: socket.local_addr()?,
            _reservation: socket,
        })
    }
}

async fn start(data: &Path, port: &InternalPort, route: Route) -> Result<Child, Error> {
    start_from(Path::new(env!("CARGO_BIN_EXE_yulius")), data, port, route).await
}

const LOG: &str = "fixture-serve.log";
const PREVIOUS_LOG: &str = "fixture-serve.previous.log";

// The departing generation's log stays beside the successor's, for the handoff warnings.
async fn start_from(
    exe: &Path,
    data: &Path,
    port: &InternalPort,
    route: Route,
) -> Result<Child, Error> {
    let internal = port.address;
    let log_path = data.join(LOG);
    match std::fs::rename(&log_path, data.join(PREVIOUS_LOG)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    let log = std::fs::File::create(&log_path)?;
    let mut child = Command::new(exe)
        .args(["serve", "--bind", "127.0.0.1:0", "--internal-bind"])
        .arg(internal.to_string())
        .args(route.service_args())
        .args(["--ci", "off", "--data"])
        .arg(data)
        .env_clear()
        .env("HOME", data)
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .env("RUST_LOG", "warn,yulius::serve=info")
        .stdout(log.try_clone()?)
        .stderr(log)
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("starting the server from {}: {error}", exe.display()))?;
    let health = format!("http://{internal}/internal/healthz");
    let _ = rustls::crypto::ring::default_provider().install_default();
    let http = reqwest::Client::builder().no_proxy().build()?;
    loop {
        if let Some(status) = child.try_wait()? {
            let log = std::fs::read_to_string(&log_path)?;
            let message = log.lines().last().unwrap_or("no startup diagnostic");
            return Err(format!("server exited during startup with {status}: {message}").into());
        }
        if std::fs::read_to_string(&log_path)?.contains("yulius listening")
            && let Ok(response) = http.get(&health).send().await
            && response.status().is_success()
            && response.text().await? == "ok"
            && child.try_wait()?.is_none()
        {
            return Ok(child);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// Each server's lines about opaque tunnel handoff, adoption and restore, oldest first.
fn handoff_warnings(data: &Path) -> String {
    let lines: Vec<String> = [PREVIOUS_LOG, LOG]
        .iter()
        .filter_map(|name| std::fs::read_to_string(data.join(name)).ok())
        .flat_map(|log| {
            log.lines()
                .filter(|line| line.contains("opaque tunnel"))
                .map(|line| {
                    // Strip the formatter's colour escapes.
                    let mut plain = String::new();
                    let mut chars = line.chars();
                    while let Some(c) = chars.next() {
                        if c == '\u{1b}' {
                            chars.by_ref().find(|c| c.is_ascii_alphabetic());
                        } else {
                            plain.push(c);
                        }
                    }
                    plain
                })
                .collect::<Vec<_>>()
        })
        .collect();
    if lines.is_empty() {
        "none".into()
    } else {
        lines.join(" | ")
    }
}

// Name the read that failed and what both server generations said about the handoff.
fn failed_read<'a>(
    data: &'a Path,
    read: &'static str,
) -> impl FnOnce(std::io::Error) -> Error + 'a {
    move |error| {
        format!(
            "{read}: {error}; handoff warnings: {}",
            handoff_warnings(data)
        )
        .into()
    }
}

async fn stop(child: &mut Child) -> Result<(), Error> {
    kill(
        Pid::from_raw(child.id().ok_or("server exited")?.try_into()?),
        Signal::SIGTERM,
    )?;
    assert!(child.wait().await?.success());
    Ok(())
}

#[tokio::test]
async fn the_restart_fixture_keeps_its_port_before_the_child_listens() -> Result<(), Error> {
    let port = InternalPort::reserve()?;
    let internal = port.address;
    let other = TcpSocket::new_v4()?;
    assert_eq!(
        other.bind(internal).err().map(|error| error.kind()),
        Some(std::io::ErrorKind::AddrInUse),
        "the fixture released its port before the child could bind"
    );
    Ok(())
}

#[tokio::test]
async fn another_servers_health_response_cannot_start_the_restart_fixture() -> Result<(), Error> {
    let data = tempfile::tempdir()?;
    let port = InternalPort::reserve()?;
    let internal = port.address;
    // An explicit reusable bind can still take this port. Only our child can attest its bind.
    let foreign = TcpListener::bind(internal).await?;
    std::fs::write(data.path().join("fixture-serve.log"), "yulius listening\n")?;
    let router =
        axum::Router::new().route("/internal/healthz", axum::routing::get(|| async { "ok" }));
    let check = async {
        let error = match start(data.path(), &port, Route::Direct).await {
            Err(error) => error,
            Ok(mut child) => {
                let mut client = BufReader::new(TcpStream::connect(internal).await?);
                client
                    .get_mut()
                    .write_all(
                        b"CONNECT fixture.test:443 HTTP/1.1\r\nHost: fixture.test:443\r\n\r\n",
                    )
                    .await?;
                let mut status = String::new();
                client.read_line(&mut status).await?;
                child.kill().await?;
                return Err(format!(
                    "fixture accepted the other server as ready; CONNECT returned {}",
                    status.trim()
                )
                .into());
            }
        };
        let message = error.to_string();
        assert!(
            message.contains(&format!("cannot bind {internal}")),
            "{message}"
        );
        Ok(())
    };
    tokio::select! {
        checked = tokio::time::timeout(Duration::from_secs(30), check) => checked?,
        result = axum::serve(foreign, router).into_future() => Err(format!("fixture listener exited: {result:?}").into()),
    }
}

fn seed(data: &Path) -> Result<(), Error> {
    let db = rusqlite::Connection::open(data.join("yulius.db"))?;
    db.execute_batch("
        INSERT INTO users(id,display_name,created_at) VALUES('u1','Fixture',1);
        INSERT INTO sandboxes(id,slug,repo_owner,repo_name,default_branch,profile,port_base,port_count,
          onboarding_state,created_by,created_at) VALUES('s1','fixture','fixture','repo','dev','{}',20100,2,'not_needed','u1',1);
        INSERT INTO tasks(id,sandbox_id,number,title,prompt,harness,status,origin,created_by,owner_id,
          created_at,updated_at) VALUES('t1','s1',1,'Fixture','Synthetic transfer','claude','running','ui','u1','u1',1,1);")?;
    db.execute("INSERT INTO attempts(id,task_id,seq,harness,launch_kind,state,run_dir,token_hash,host_token,started_at)
        VALUES('a1','t1',1,'claude','new','live',?,?,x'00000000',1)",
        (data.join("run").to_string_lossy(), hex::encode(Sha256::digest(TOKEN))))?;
    Ok(())
}

struct Holder {
    forward: tokio::task::JoinHandle<()>,
    address: SocketAddr,
}

impl Drop for Holder {
    fn drop(&mut self) {
        self.forward.abort();
    }
}

async fn live_holder(data: &Path, port: u16) -> Result<Holder, Error> {
    use yulius_contracts::sandbox::{HOLDER_RECORD_VERSION, HolderRecord};
    use yulius_sandbox::holder::{StatePaths, boot_id, ns_inode};

    let state = StatePaths::new(data.join("runtime/sandboxes/s1"));
    std::fs::create_dir_all(&state.dir)?;
    let pid = std::process::id();
    let record = HolderRecord {
        version: HOLDER_RECORD_VERSION,
        sandbox: "s1".into(),
        base_id: "synthetic-base-1".into(),
        inner_pid: pid,
        bwrap_pid: 0,
        slirp_pid: None,
        boot_id: boot_id(),
        user_ns: ns_inode(pid, "user").ok_or("fixture user namespace")?,
        net_ns: ns_inode(pid, "net").ok_or("fixture network namespace")?,
        mnt_ns: ns_inode(pid, "mnt").ok_or("fixture mount namespace")?,
        started_at_ms: 1,
        system_root: data.display().to_string(),
        engine: false,
    };
    std::fs::write(state.record(), serde_json::to_vec(&record)?)?;
    // The alternate loopback keeps the holder and the server on the same port.
    let ip = Ipv4Addr::new(127, 0, 0, 2);
    let forward =
        yulius_sandbox::relay::serve_forward_at(ip, port, state.outbound(), false).await?;
    Ok(Holder {
        forward,
        address: (ip, port).into(),
    })
}

// Upkeep persists a new network-helper PID without replacing the holder or its Unix forward.
fn record_fixture_helper(data: &Path) -> Result<Child, Error> {
    use yulius_contracts::sandbox::HolderRecord;
    use yulius_sandbox::holder::{StatePaths, write_json};

    let path = StatePaths::new(data.join("runtime/sandboxes/s1")).record();
    let mut record: HolderRecord = serde_json::from_slice(&std::fs::read(&path)?)?;
    let child = Command::new("/bin/cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    record.slirp_pid = Some(child.id().ok_or("fixture helper exited")?);
    write_json(&path, &record)?;
    Ok(child)
}

async fn holder_for(
    route: Route,
    data: &Path,
    internal: SocketAddr,
) -> Result<Option<Holder>, Error> {
    match route {
        Route::Direct => Ok(None),
        Route::Holder => Ok(Some(live_holder(data, internal.port()).await?)),
    }
}

fn fixture_ip() -> Result<Ipv4Addr, Error> {
    getifaddrs()?
        .filter_map(|iface| iface.address?.as_sockaddr_in().map(|s| s.ip()))
        .find(|ip| yulius_domain::vault::egress_destination_allowed((*ip).into()))
        .ok_or_else(|| "the real server fixture needs a local non-loopback IPv4 address".into())
}

async fn read_head<R: tokio::io::AsyncRead + Unpin>(
    stream: &mut BufReader<R>,
    first_line: &str,
) -> Result<(), Error> {
    let mut line = String::new();
    stream.read_line(&mut line).await?;
    assert!(line.starts_with(first_line), "{line}");
    loop {
        line.clear();
        assert_ne!(stream.read_line(&mut line).await?, 0);
        if line == "\r\n" {
            return Ok(());
        }
    }
}

#[tokio::test]
async fn strict_namespace_startup_adopts_the_holder_or_reports_missing_prerequisites()
-> Result<(), Error> {
    tokio::time::timeout(Duration::from_secs(15), async {
        let dir = tempfile::tempdir()?;
        let port = InternalPort::reserve()?;
        let internal = port.address;
        let holder = live_holder(dir.path(), internal.port()).await?;
        match start(dir.path(), &port, Route::Holder).await {
            Ok(mut server) => {
                // A healthy legacy listener alone cannot serve this original holder.
                let health = format!("http://{}/internal/healthz", holder.address);
                let response = reqwest::Client::builder()
                    .no_proxy()
                    .build()?
                    .get(health)
                    .send()
                    .await?;
                assert_eq!(response.error_for_status()?.text().await?, "ok");
                check_quiet_shutdown(dir.path(), internal, holder.address, &mut server).await
            }
            Err(error) => {
                let log = std::fs::read_to_string(dir.path().join("fixture-serve.log"))?;
                if log.contains("sandbox isolation unavailable:")
                    && (log.contains("is not installed")
                        || log.contains("/etc/subuid")
                        || log.contains("/etc/subgid"))
                {
                    Ok(())
                } else {
                    Err(error)
                }
            }
        }
    })
    .await?
}

// The unfinished response pins HTTP drain. EOF on the separate idle health connection
// tells us the same real server has handled SIGTERM before we try a late Unix accept.
async fn check_quiet_shutdown(
    data: &Path,
    internal: SocketAddr,
    forward: SocketAddr,
    server: &mut Child,
) -> Result<(), Error> {
    seed(data)?;
    let destination = TcpListener::bind((fixture_ip()?, 0)).await?;
    let origin = destination.local_addr()?;
    let http = reqwest::Client::builder()
        .no_proxy()
        .proxy(reqwest::Proxy::http(format!("http://{forward}"))?.basic_auth("fixture", TOKEN))
        .build()?;
    let request = http.get(format!("http://{origin}/hold")).send();
    let respond = async {
        let mut peer = BufReader::new(destination.accept().await?.0);
        read_head(&mut peer, "GET /hold ").await?;
        peer.get_mut()
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nh")
            .await?;
        Ok::<_, Error>(peer)
    };
    let (download, peer) = tokio::join!(request, respond);
    let peer = peer?;
    let mut download = download?.error_for_status()?;
    assert_eq!(download.chunk().await?.as_deref(), Some(b"h".as_slice()));

    let mut idle = BufReader::new(TcpStream::connect(internal).await?);
    idle.get_mut()
        .write_all(b"GET /internal/healthz HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await?;
    read_head(&mut idle, "HTTP/1.1 200 ").await?;
    assert_eq!(idle.read_u16().await?.to_be_bytes(), *b"ok");
    kill(
        Pid::from_raw(server.id().ok_or("server exited")?.try_into()?),
        Signal::SIGTERM,
    )?;
    assert_eq!(idle.read(&mut [0]).await?, 0);
    assert!(server.try_wait()?.is_none());
    let mut late =
        tokio::net::UnixStream::connect(data.join("runtime/sandboxes/s1/out.sock")).await?;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), late.read(&mut [0]))
            .await
            .map_err(|_| "stopping server admitted a new outbound relay")??,
        0
    );
    drop(peer);
    drop(download);
    assert!(server.wait().await?.success());
    assert!(
        !std::fs::read_to_string(data.join("fixture-serve.log"))?
            .contains("connection was dropped at the limit")
    );
    Ok(())
}

#[tokio::test]
async fn established_direct_connect_carries_queued_and_fresh_bytes_across_server_exit()
-> Result<(), Error> {
    check_restart(Route::Direct).await
}

#[tokio::test]
#[ignore = "needs the Linux namespace server's subordinate IDs and installed helpers"]
async fn an_opaque_connect_through_the_same_holder_survives_server_exit() -> Result<(), Error> {
    check_restart(Route::Holder).await
}

#[tokio::test]
#[ignore = "needs the Linux namespace server's subordinate IDs and installed helpers"]
async fn a_stopped_client_keeps_its_original_tunnel_through_the_holder() -> Result<(), Error> {
    use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid};
    use std::os::fd::{AsFd as _, OwnedFd};

    const CHILD: &str = "YULIUS_FIXTURE_STOPPED_TUNNEL";
    // An ordered 1 MiB reply exceeds the origin's small send buffer during downtime.
    let payload: Vec<u8> = (0..262_144u32).flat_map(u32::to_be_bytes).collect();
    if std::env::var_os(CHILD).is_some() {
        // This process owns the client's original TCP connection while both servers turn over.
        let fd = std::io::stdin().as_fd().try_clone_to_owned()?;
        let mut stream = TcpStream::from_std(fd.into())?;
        stream.write_all(b"held").await?;
        kill(Pid::this(), Signal::SIGSTOP)?;
        assert_eq!(stream.read_u32().await?.to_be_bytes(), *b"down");
        let mut received = vec![0u8; payload.len()];
        stream.read_exact(&mut received).await?;
        assert!(
            received == payload,
            "the restored stream changed the 1 MiB reply"
        );
        stream.write_all(b"thaw").await?;
        stream.shutdown().await?;
        assert_eq!(stream.read_u32().await?.to_be_bytes(), *b"back");
        assert_eq!(stream.read(&mut [0]).await?, 0);
        return Ok(());
    }

    tokio::time::timeout(Duration::from_secs(30), async {
        let data = tempfile::tempdir()?;
        let port = InternalPort::reserve()?;
        let internal = port.address;
        let holder = live_holder(data.path(), internal.port()).await?;
        let listener = TcpListener::bind((fixture_ip()?, 0)).await?;
        let mut first = start(data.path(), &port, Route::Holder).await?;
        let first_pid = first.id();
        seed(data.path())?;
        let (client, mut peer) =
            open_tunnel(holder.address, listener.local_addr()?, &listener).await?;
        drop(listener);
        assert!(client.buffer().is_empty());
        let mut child = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "opaque_restart::a_stopped_client_keeps_its_original_tunnel_through_the_holder",
                "--ignored",
                "--nocapture",
            ])
            .env_clear()
            .env(CHILD, "1")
            .stdin(Stdio::from(OwnedFd::from(client.into_inner().into_std()?)))
            .kill_on_drop(true)
            .spawn()?;
        let pid = Pid::from_raw(child.id().ok_or("synthetic client exited")?.try_into()?);
        let flags = WaitPidFlag::WSTOPPED | WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT;
        assert_eq!(peer.read_u32().await?.to_be_bytes(), *b"held");
        let status = tokio::task::spawn_blocking(move || waitid(Id::Pid(pid), flags)).await??;
        assert_eq!(status, WaitStatus::Stopped(pid, Signal::SIGSTOP));
        stop(&mut first).await?;
        peer.write_all(b"down").await?;
        nix::sys::socket::setsockopt(&peer, nix::sys::socket::sockopt::SndBuf, &4096)?;
        let queued = peer.write(&payload).await?;
        assert!(
            queued > 0 && queued < payload.len(),
            "partial write during restart: {queued}"
        );
        let mut next = start(data.path(), &port, Route::Holder).await?;
        assert_ne!(next.id(), first_pid);
        assert_eq!(
            waitid(Id::Pid(pid), flags | WaitPidFlag::WNOHANG)?,
            WaitStatus::Stopped(pid, Signal::SIGSTOP)
        );
        kill(pid, Signal::SIGCONT)?;
        peer.write_all(&payload[queued..]).await?;
        assert_eq!(peer.read_u32().await?.to_be_bytes(), *b"thaw");
        assert_eq!(peer.read(&mut [0]).await?, 0);
        peer.write_all(b"back").await?;
        peer.shutdown().await?;
        assert!(child.wait().await?.success());
        stop(&mut next).await
    })
    .await?
}

#[tokio::test]
#[ignore = "needs the Linux namespace server's subordinate IDs and installed helpers"]
async fn a_final_reply_already_sent_before_shutdown_drains_through_the_original_outbound_socket()
-> Result<(), Error> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let data = tempfile::tempdir()?;
        let port = InternalPort::reserve()?;
        let internal = port.address;
        let _holder = live_holder(data.path(), internal.port()).await?;
        let listener = TcpListener::bind((fixture_ip()?, 0)).await?;
        let mut first = start(data.path(), &port, Route::Holder).await?;
        seed(data.path())?;
        let mut unix =
            tokio::net::UnixStream::connect(data.path().join("runtime/sandboxes/s1/out.sock"))
                .await?;
        unix.write_u16(internal.port()).await?;
        assert_eq!(unix.read_u8().await?, yulius_contracts::sandbox::RELAY_OK);
        let (mut client, mut peer) =
            establish_tunnel(unix, listener.local_addr()?, &listener).await?;
        drop(listener);
        let local = format!("0100007F:{:04X}", internal.port());
        let internal_connections = || -> std::io::Result<Vec<(String, String)>> {
            Ok(std::fs::read_to_string("/proc/self/net/tcp")?
                .lines()
                .filter_map(|line| {
                    let mut fields = line.split_whitespace().skip(1);
                    (fields.next()? == local)
                        .then_some((fields.next()?.to_owned(), fields.next()?.to_owned()))
                })
                .collect())
        };
        let original = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let mut live = internal_connections()?
                    .into_iter()
                    .filter(|(_, state)| state == "01");
                if let Some((peer, _)) = live.next()
                    && live.next().is_none()
                {
                    return Ok::<_, Error>(peer);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| "missing unique internal TCP connection")??;
        client.get_mut().shutdown().await?;
        assert_eq!(peer.read(&mut [0]).await?, 0);
        // Leave the client unread while the proxy finishes writing into the internal TCP link.
        let reply: Vec<_> = (0..262_144u32).flat_map(u32::to_be_bytes).collect();
        peer.write_all(&reply).await?;
        peer.shutdown().await?;
        // The relay has sent its FIN. Wait until the proxy has forwarded the full reply and
        // finished its own write half. The unread original Unix socket still blocks the relay.
        tokio::time::timeout(Duration::from_secs(5), async {
            while internal_connections()?
                .iter()
                .any(|(peer, state)| *peer == original && matches!(state.as_str(), "01" | "08"))
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok::<_, Error>(())
        })
        .await
        .map_err(|_| "proxy did not finish its internal write half")??;
        stop(&mut first).await?;
        let mut next = start(data.path(), &port, Route::Holder).await?;
        let mut received = Vec::new();
        client
            .take(u64::try_from(reply.len())? + 1)
            .read_to_end(&mut received)
            .await?;
        assert_eq!(
            received.len(),
            reply.len(),
            "the relay lost some of the final reply"
        );
        assert!(
            received == reply,
            "the original client's queued reply changed"
        );
        stop(&mut next).await
    })
    .await?
}

async fn check_restart(route: Route) -> Result<(), Error> {
    tokio::time::timeout(Duration::from_secs(30), async {
        // Dial only this fixture's listening address. The production proxy denies loopback.
        let local = fixture_ip()?;
        let listener = TcpListener::bind((local, 0)).await?;
        let target = listener.local_addr()?;
        let port = InternalPort::reserve()?;
        let internal = port.address;
        let dir = tempfile::tempdir()?;
        let holder = holder_for(route, dir.path(), internal).await?;
        let endpoint = holder.as_ref().map_or(internal, |h| h.address);
        let mut helper = holder.as_ref().map(|_| record_fixture_helper(dir.path())).transpose()?;
        let mut first = start(dir.path(), &port, route).await?;
        let first_pid = first.id();
        assert!(!std::fs::read_to_string(dir.path().join("fixture-serve.log"))?
            .contains("cannot adopt opaque tunnels"), "an ordinary cold start is quiet");
        // A previous successful handoff leaves its private service directory in place.
        std::fs::DirBuilder::new().mode(0o700).create(dir.path().join("tunnel-handoff"))?;
        seed(dir.path())?;
        if let Some(old) = helper.as_mut() {
            let replacement = record_fixture_helper(dir.path())?;
            old.kill().await?;
            *old = replacement;
        }
        let mut client = BufReader::new(TcpStream::connect(endpoint).await?);
        let auth = base64::engine::general_purpose::STANDARD.encode(format!("fixture:{TOKEN}"));
        let header_padding = "x".repeat(20 * 1024);
        let early: Vec<u8> = (b'a'..=b'w').cycle().take(96 * 1024).collect();
        let head = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Basic {auth}\r\nFixture-Padding: {header_padding}\r\n\r\nEARL");
        let mut request = head.into_bytes();
        request.extend_from_slice(&early);
        client.get_mut().write_all(&request).await?;
        read_head(&mut client,"HTTP/1.1 200").await?;
        let (mut peer, _) = listener.accept().await?;
        drop(listener); // A replacement cannot redial the destination.
        assert_eq!(peer.read_u32().await?.to_be_bytes(), *b"EARL");
        peer.write_all(b"LIVE").await?;
        assert_eq!(client.read_u32().await?.to_be_bytes(), *b"LIVE");
        stop(&mut first).await?;
        peer.write_all(b"DOWN").await?;
        let mut next = start(dir.path(), &port, route).await?;
        assert_ne!(next.id(), first_pid);
        let queued = client.read_u32().await.map_err(failed_read(dir.path(), "original client reading downtime marker"))?;
        assert_eq!(queued.to_be_bytes(), *b"DOWN");
        client.get_mut().write_all(b"NEXT").await?;
        client.get_mut().shutdown().await?;
        let mut received = vec![0; early.len()];
        peer.read_exact(&mut received).await.map_err(failed_read(dir.path(), "destination reading early bytes"))?;
        assert_eq!(received, early, "all early payload bytes arrived once, in order");
        let next_marker = peer.read_u32().await.map_err(failed_read(dir.path(), "destination reading NEXT"))?;
        assert_eq!(next_marker.to_be_bytes(), *b"NEXT");
        assert_eq!(peer.read(&mut [0]).await?, 0);
        peer.write_all(b"LAST").await?;
        peer.shutdown().await?;
        let last = client.read_u32().await.map_err(failed_read(dir.path(), "original client reading LAST"))?;
        assert_eq!(last.to_be_bytes(), *b"LAST");
        assert_eq!(client.read(&mut [0]).await?, 0);
        stop(&mut next).await?;
        if let Some(mut helper) = helper {
            helper.kill().await?;
        }
        Ok(())
    }).await?
}

#[tokio::test]
async fn an_invalid_handoff_directory_reports_the_kind_and_still_serves() -> Result<(), Error> {
    tokio::time::timeout(Duration::from_secs(15), async {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("tunnel-handoff"), b"synthetic fixture")?;
        let port = InternalPort::reserve()?;
        let mut server = start(dir.path(), &port, Route::Direct).await?;
        stop(&mut server).await?;
        let mut next = start(dir.path(), &port, Route::Direct).await?;
        stop(&mut next).await?;
        // A failed restart read reports both generations' warnings as plain text.
        let warnings = handoff_warnings(dir.path());
        assert_eq!(
            warnings
                .matches("cannot adopt opaque tunnels kind=PermissionDenied")
                .count(),
            2,
            "{warnings}"
        );
        Ok(())
    })
    .await?
}

async fn open_tunnel(
    internal: SocketAddr,
    target: SocketAddr,
    listener: &TcpListener,
) -> Result<(BufReader<TcpStream>, TcpStream), Error> {
    establish_tunnel(TcpStream::connect(internal).await?, target, listener).await
}

async fn establish_tunnel<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    stream: S,
    target: SocketAddr,
    listener: &TcpListener,
) -> Result<(BufReader<S>, TcpStream), Error> {
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("fixture:{TOKEN}"));
    let connect = format!(
        "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Basic {auth}\r\n\r\n"
    );
    let mut client = BufReader::new(stream);
    client.get_mut().write_all(connect.as_bytes()).await?;
    read_head(&mut client, "HTTP/1.1 200").await?;
    let (mut peer, _) = listener.accept().await?;
    client.get_mut().write_all(b"LIVE").await?;
    assert_eq!(peer.read_u32().await?.to_be_bytes(), *b"LIVE");
    peer.write_all(b"BACK").await?;
    assert_eq!(client.read_u32().await?.to_be_bytes(), *b"BACK");
    Ok((client, peer))
}

#[tokio::test]
async fn a_destination_reset_during_restart_keeps_the_other_established_tunnel() -> Result<(), Error>
{
    check_reset_during_restart(Route::Direct).await
}

#[tokio::test]
#[ignore = "needs the Linux namespace server's subordinate IDs and installed helpers"]
async fn losing_one_destination_during_restart_releases_its_relay_and_keeps_the_other()
-> Result<(), Error> {
    check_reset_during_restart(Route::Holder).await
}

async fn check_reset_during_restart(route: Route) -> Result<(), Error> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let local = fixture_ip()?;
        let listener = TcpListener::bind((local, 0)).await?;
        let target = listener.local_addr()?;
        let port = InternalPort::reserve()?;
        let internal = port.address;
        let dir = tempfile::tempdir()?;
        let holder = holder_for(route, dir.path(), internal).await?;
        let endpoint = holder.as_ref().map_or(internal, |h| h.address);
        let mut first = start(dir.path(), &port, route).await?;
        seed(dir.path())?;
        let (mut ended, resetting) = open_tunnel(endpoint, target, &listener).await?;
        let (mut client, mut peer) = open_tunnel(endpoint, target, &listener).await?;
        drop(listener);
        stop(&mut first).await?;
        nix::sys::socket::setsockopt(
            &resetting,
            nix::sys::socket::sockopt::Linger,
            &nix::libc::linger {
                l_onoff: 1,
                l_linger: 0,
            },
        )?;
        drop(resetting);
        peer.write_all(b"DOWN").await?;
        let mut next = start(dir.path(), &port, route).await?;
        let down = client.read_u32().await.map_err(failed_read(
            dir.path(),
            "surviving client reading DOWN after the restart",
        ))?;
        assert_eq!(down.to_be_bytes(), *b"DOWN");
        client.get_mut().write_all(b"NEXT").await?;
        let next_marker = peer.read_u32().await.map_err(failed_read(
            dir.path(),
            "surviving destination reading NEXT after the restart",
        ))?;
        assert_eq!(next_marker.to_be_bytes(), *b"NEXT");
        let end = ended.read(&mut [0]).await.map_err(failed_read(
            dir.path(),
            "reset tunnel's client reading its end",
        ))?;
        assert_eq!(end, 0, "the reset destination's client tunnel ends");
        stop(&mut next).await
    })
    .await?
}

// An update, package upgrade or rebuild can remove or rewrite the file at the server's install
// path while the old server is still running. The tunnel handoff must not depend on that file.
#[tokio::test]
async fn a_removed_install_path_still_hands_the_tunnel_to_the_next_server() -> Result<(), Error> {
    check_install_path_change(false).await
}

// A copying build cache writes a new file at the path, which cannot run until it is closed.
#[tokio::test]
async fn an_install_path_being_rewritten_still_hands_the_tunnel_to_the_next_server()
-> Result<(), Error> {
    check_install_path_change(true).await
}

async fn check_install_path_change(rewriting: bool) -> Result<(), Error> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let built = Path::new(env!("CARGO_BIN_EXE_yulius"));
        // A hard link needs the binary's own filesystem.
        let install = tempfile::tempdir_in(built.parent().ok_or("binary directory")?)?;
        let exe = install.path().join("yulius");
        std::fs::hard_link(built, &exe)?;
        let listener = TcpListener::bind((fixture_ip()?, 0)).await?;
        let target = listener.local_addr()?;
        let port = InternalPort::reserve()?;
        let dir = tempfile::tempdir()?;
        let mut first = start_from(&exe, dir.path(), &port, Route::Direct).await?;
        seed(dir.path())?;
        let (mut client, mut peer) = open_tunnel(port.address, target, &listener).await?;
        drop(listener);
        std::fs::remove_file(&exe)?;
        let partial = if rewriting {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt as _;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o700)
                .open(&exe)?;
            file.write_all(b"\x7fELF")?;
            Some(file)
        } else {
            None
        };
        stop(&mut first).await?;
        drop(partial);
        peer.write_all(b"DOWN").await?;
        let mut next = start(dir.path(), &port, Route::Direct).await?;
        let down = client.read_u32().await.map_err(failed_read(
            dir.path(),
            "client reading DOWN after the restart",
        ))?;
        assert_eq!(down.to_be_bytes(), *b"DOWN");
        client.get_mut().write_all(b"NEXT").await?;
        let next_marker = peer.read_u32().await.map_err(failed_read(
            dir.path(),
            "destination reading NEXT after the restart",
        ))?;
        assert_eq!(next_marker.to_be_bytes(), *b"NEXT");
        stop(&mut next).await
    })
    .await?
}

#[tokio::test]
async fn an_unfinished_http_download_cannot_delay_opaque_handoff_or_shutdown() -> Result<(), Error>
{
    tokio::time::timeout(Duration::from_secs(25), async {
        let local = fixture_ip()?;
        let dest = TcpListener::bind((local, 0)).await?;
        let port = InternalPort::reserve()?;
        let internal = port.address;
        let dir = tempfile::tempdir()?;
        let mut first = start(dir.path(), &port, Route::Direct).await?;
        seed(dir.path())?;
        let (mut client, mut peer) = open_tunnel(internal, dest.local_addr()?, &dest).await?;
        let origin = dest.local_addr()?;
        let http = reqwest::Client::builder()
            .no_proxy()
            .proxy(reqwest::Proxy::http(format!("http://{internal}"))?.basic_auth("fixture", TOKEN))
            .build()?;
        let request = http.get(format!("http://{origin}/fixture")).send();
        let respond = async {
            let (socket, _) = dest.accept().await?;
            drop(dest);
            let mut upstream = BufReader::new(socket);
            read_head(&mut upstream, "GET /fixture ").await?;
            upstream
                .get_mut()
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 128\r\n\r\n")
                .await?;
            upstream.get_mut().write_all(&[b'x'; 64]).await?;
            Ok::<_, Error>(upstream)
        };
        let (download, origin_socket) = tokio::join!(request, respond);
        let _held_origin = origin_socket?;
        let mut download = download?.error_for_status()?;
        // Read application bytes through the HTTP decoder and keep the unfinished body open.
        let chunk = download.chunk().await?.ok_or("download ended early")?;
        assert!(!chunk.is_empty() && chunk.iter().all(|&byte| byte == b'x'));
        tokio::time::timeout(Duration::from_secs(12), stop(&mut first))
            .await
            .map_err(|_| "unfinished HTTP download kept the old service running")??;
        peer.write_all(b"DOWN").await?;
        let mut next = start(dir.path(), &port, Route::Direct).await?;
        let down = client.read_u32().await.map_err(failed_read(
            dir.path(),
            "tunnel client reading DOWN after the restart",
        ))?;
        assert_eq!(down.to_be_bytes(), *b"DOWN");
        client.get_mut().write_all(b"NEXT").await?;
        let next_marker = peer.read_u32().await.map_err(failed_read(
            dir.path(),
            "tunnel destination reading NEXT after the restart",
        ))?;
        assert_eq!(next_marker.to_be_bytes(), *b"NEXT");
        stop(&mut next).await
    })
    .await?
}

#[tokio::test]
async fn final_bytes_and_fin_during_restart_reach_the_half_closed_peer() -> Result<(), Error> {
    check_final_fin(Route::Direct).await
}

#[tokio::test]
#[ignore = "needs the Linux namespace server's subordinate IDs and installed helpers"]
async fn final_bytes_and_fin_reach_the_half_closed_peer_through_its_holder() -> Result<(), Error> {
    check_final_fin(Route::Holder).await
}

async fn check_final_fin(route: Route) -> Result<(), Error> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let listener = TcpListener::bind((fixture_ip()?, 0)).await?;
        let target = listener.local_addr()?;
        let port = InternalPort::reserve()?;
        let internal = port.address;
        let dir = tempfile::tempdir()?;
        let holder = holder_for(route, dir.path(), internal).await?;
        let endpoint = holder.as_ref().map_or(internal, |h| h.address);
        let mut first = start(dir.path(), &port, route).await?;
        seed(dir.path())?;
        let (mut client, mut origin) = open_tunnel(endpoint, target, &listener).await?;
        let (mut sender, mut recipient) = open_tunnel(endpoint, target, &listener).await?;
        drop(listener);
        client.get_mut().shutdown().await?;
        assert_eq!(origin.read(&mut [0]).await?, 0);
        recipient.shutdown().await?;
        assert_eq!(sender.read(&mut [0]).await?, 0);
        stop(&mut first).await?;
        origin.write_all(b"last reply").await?;
        origin.shutdown().await?;
        sender.get_mut().write_all(b"last request").await?;
        sender.get_mut().shutdown().await?;
        let mut next = start(dir.path(), &port, route).await?;
        let mut delivered = Vec::new();
        client
            .read_to_end(&mut delivered)
            .await
            .map_err(failed_read(
                dir.path(),
                "half-closed client reading the final reply",
            ))?;
        assert_eq!(
            delivered, b"last reply",
            "destination bytes preceding its final FIN"
        );
        delivered.clear();
        recipient
            .read_to_end(&mut delivered)
            .await
            .map_err(failed_read(
                dir.path(),
                "half-closed destination reading the final request",
            ))?;
        assert_eq!(
            delivered, b"last request",
            "client bytes preceding its final FIN"
        );
        stop(&mut next).await
    })
    .await?
}
