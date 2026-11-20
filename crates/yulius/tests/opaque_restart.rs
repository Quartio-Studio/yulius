//! SPEC-577. Keep the original peers while two real server processes use the same private data.
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use base64::Engine as _;
use nix::ifaddrs::getifaddrs;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};

type Error = Box<dyn std::error::Error>;
const TOKEN: &str = "synthetic-restart-token";

async fn start(data: &Path, internal: SocketAddr) -> Result<Child, Error> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_yulius"))
        .args(["serve", "--bind", "127.0.0.1:0", "--internal-bind"])
        .arg(internal.to_string())
        .args(["--isolation", "legacy", "--ci", "off", "--data"])
        .arg(data)
        .env_clear()
        .env("HOME", data)
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let health = format!("http://{internal}/internal/healthz");
    let _ = rustls::crypto::ring::default_provider().install_default();
    let http = reqwest::Client::builder().no_proxy().build()?;
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(format!("server exited during startup with {status}").into());
        }
        if let Ok(response) = http.get(&health).send().await
            && response.text().await? == "ok"
        {
            return Ok(child);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
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

#[tokio::test]
async fn established_direct_connect_carries_queued_and_fresh_bytes_across_server_exit()
-> Result<(), Error> {
    tokio::time::timeout(Duration::from_secs(30), async {
        // Dial only this fixture's listening address. The production proxy denies loopback.
        let local = getifaddrs()?
            .filter_map(|iface| iface.address?.as_sockaddr_in().map(|s| s.ip()))
            .find(|ip| yulius_domain::vault::egress_destination_allowed((*ip).into()))
            .ok_or("the real server fixture needs a local non-loopback IPv4 address")?;
        let listener = TcpListener::bind((local, 0)).await?;
        let target = listener.local_addr()?;
        let internal = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?.local_addr()?;
        let dir = tempfile::tempdir()?;
        let mut first = start(dir.path(), internal).await?;
        let first_pid = first.id();
        seed(dir.path())?;
        let mut client = BufReader::new(TcpStream::connect(internal).await?);
        let auth = base64::engine::general_purpose::STANDARD.encode(format!("fixture:{TOKEN}"));
        let header_padding = "x".repeat(20 * 1024);
        let early: Vec<u8> = (b'a'..=b'w').cycle().take(96 * 1024).collect();
        let head = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Basic {auth}\r\nFixture-Padding: {header_padding}\r\n\r\nEARL");
        let mut request = head.into_bytes();
        request.extend_from_slice(&early);
        client.get_mut().write_all(&request).await?;
        let mut line = String::new();
        client.read_line(&mut line).await?;
        assert!(line.starts_with("HTTP/1.1 200"), "{line}");
        loop {
            line.clear();
            assert_ne!(client.read_line(&mut line).await?, 0);
            if line == "\r\n" {
                break;
            }
        }
        let (mut peer, _) = listener.accept().await?;
        drop(listener); // A replacement cannot redial the destination.
        assert_eq!(peer.read_u32().await?.to_be_bytes(), *b"EARL");
        peer.write_all(b"LIVE").await?;
        assert_eq!(client.read_u32().await?.to_be_bytes(), *b"LIVE");
        stop(&mut first).await?;
        peer.write_all(b"DOWN").await?;
        let mut next = start(dir.path(), internal).await?;
        assert_ne!(next.id(), first_pid);
        let queued = client.read_u32().await.map_err(|e| format!("original client reading downtime marker: {e}"))?;
        assert_eq!(queued.to_be_bytes(), *b"DOWN");
        client.get_mut().write_all(b"NEXT").await?;
        client.get_mut().shutdown().await?;
        let mut received = vec![0; early.len()];
        peer.read_exact(&mut received).await?;
        assert_eq!(received, early, "all early payload bytes arrived once, in order");
        assert_eq!(peer.read_u32().await?.to_be_bytes(), *b"NEXT");
        assert_eq!(peer.read(&mut [0]).await?, 0);
        peer.write_all(b"LAST").await?;
        peer.shutdown().await?;
        assert_eq!(client.read_u32().await?.to_be_bytes(), *b"LAST");
        assert_eq!(client.read(&mut [0]).await?, 0);
        stop(&mut next).await
    }).await?
}
