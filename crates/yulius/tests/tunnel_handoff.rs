//! SPEC-577. Exercise the production helper as a separate process.
use std::io;
use std::os::fd::{AsFd as _, OwnedFd};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream, UnixStream};
use yulius_sandbox::relay::custodian::{Pair, adopt, deposit};

#[tokio::test]
async fn a_successor_adopts_the_original_unix_and_tcp_peers_once() -> io::Result<()> {
    if let Some(directory) = std::env::var_os("YULIUS_FIXTURE_HANDOFF_DIRECTORY") {
        return deposit_in_the_old_process(PathBuf::from(directory)).await;
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        // A helper killed after bind left a name but holds no sockets or lock.
        drop(std::os::unix::net::UnixListener::bind(
            dir.path().join("tunnels.sock"),
        )?);
        let (mut client, left) = UnixStream::pair()?;
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        // Move the Unix endpoint to a distinct depositing process. It connects to the
        // synthetic destination and exits after the helper acknowledges the complete batch.
        let mut old = tokio::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "tunnel_handoff::a_successor_adopts_the_original_unix_and_tcp_peers_once",
            ])
            .env("YULIUS_FIXTURE_HANDOFF_DIRECTORY", dir.path())
            .env(
                "YULIUS_FIXTURE_HANDOFF_PEER",
                listener.local_addr()?.to_string(),
            )
            .stdin(Stdio::from(OwnedFd::from(left.into_std()?)))
            .stdout(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let (mut remote, _) = listener.accept().await?;
        assert!(
            old.wait().await?.success(),
            "the original depositing process exited"
        );
        client.write_all(b"left").await?;
        remote.write_all(b"peer").await?;
        let batch = adopt(dir.path()).await?;
        assert_eq!(batch.len(), 1);
        let pair = batch
            .into_iter()
            .next()
            .ok_or_else(|| io::Error::other("missing pair"))?;
        assert_eq!(pair.state, b"pending");
        let [left, right] = pair.sockets;
        let mut left = UnixStream::from_std(left.into())?;
        let mut right = TcpStream::from_std(right.into())?;
        assert_eq!(left.read_u32().await?.to_be_bytes(), *b"left");
        assert_eq!(right.read_u32().await?.to_be_bytes(), *b"peer");
        right.write_all(b"more").await?;
        left.write_all(b"done").await?;
        assert_eq!(remote.read_u32().await?.to_be_bytes(), *b"more");
        assert_eq!(client.read_u32().await?.to_be_bytes(), *b"done");
        assert!(
            adopt(dir.path())
                .await
                .is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
        );
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn an_overlapping_deposit_leaves_the_live_batch_for_its_successor() -> io::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let exe = Path::new(env!("CARGO_BIN_EXE_yulius"));
        let fixture = |state: &[u8]| -> io::Result<_> {
            let (a, left) = UnixStream::pair()?;
            let (b, right) = UnixStream::pair()?;
            Ok((
                [a, b],
                Pair {
                    expires: None,
                    sockets: [left.into_std()?.into(), right.into_std()?.into()],
                    state: state.to_vec(),
                },
            ))
        };
        let (_live, first) = fixture(b"first")?;
        deposit(exe, dir.path(), vec![first]).await?;
        let (mut rejected, second) = fixture(b"second")?;
        assert!(deposit(exe, dir.path(), vec![second]).await.is_err());
        for peer in &mut rejected {
            assert_eq!(peer.read(&mut [0]).await?, 0);
        }
        assert_eq!(
            adopt(dir.path()).await?.first().map(|p| p.state.as_slice()),
            Some(b"first".as_slice())
        );
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn a_stalled_successor_holds_the_only_helper_place_until_its_transfer_ends() -> io::Result<()>
{
    tokio::time::timeout(Duration::from_secs(10), async {
        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let (mut client, left) = UnixStream::pair()?;
        let (mut peer, right) = UnixStream::pair()?;
        let mut batch = Vec::new();
        // More private state than the Unix stream's send buffer. The first header confirms
        // the listener has accepted; leaving the rest unread stalls the helper in send.
        for _ in 0..16 {
            batch.push(Pair {
                expires: None,
                sockets: [
                    left.as_fd().try_clone_to_owned()?,
                    right.as_fd().try_clone_to_owned()?,
                ],
                state: vec![b'x'; 32 * 1024],
            });
        }
        drop((left, right));
        deposit(Path::new(env!("CARGO_BIN_EXE_yulius")), dir.path(), batch).await?;
        let mut successor = UnixStream::connect(dir.path().join("tunnels.sock")).await?;
        assert_eq!(successor.read_u8().await?, b'P');
        let lock = std::fs::File::open(dir.path())?;
        assert!(
            matches!(lock.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
            "another helper could publish during transfer"
        );
        drop(successor);
        assert_eq!(client.read(&mut [0]).await?, 0);
        assert_eq!(peer.read(&mut [0]).await?, 0);
        loop {
            match lock.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) => tokio::task::yield_now().await,
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    })
    .await?
}

// Called in the re-executed test with only a fixture-owned endpoint on stdin.
async fn deposit_in_the_old_process(directory: PathBuf) -> io::Result<()> {
    let addr = std::env::var("YULIUS_FIXTURE_HANDOFF_PEER").map_err(io::Error::other)?;
    let right = TcpStream::connect(addr).await?;
    let saved = Pair {
        expires: None,
        sockets: [
            std::io::stdin().as_fd().try_clone_to_owned()?,
            right.into_std()?.into(),
        ],
        state: b"pending".to_vec(),
    };
    deposit(
        Path::new(env!("CARGO_BIN_EXE_yulius")),
        &directory,
        vec![saved],
    )
    .await
}

#[tokio::test]
async fn one_tunnel_expires_in_custody_while_another_stays_adoptable() -> io::Result<()> {
    use yulius_sandbox::relay::custodian::monotonic_ms;
    tokio::time::timeout(Duration::from_secs(5), async {
        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let fixture = |expires: u64| -> io::Result<_> {
            let (a, left) = UnixStream::pair()?;
            let (b, right) = UnixStream::pair()?;
            Ok((
                [a, b],
                Pair {
                    expires: Some(expires),
                    sockets: [left.into_std()?.into(), right.into_std()?.into()],
                    state: b"synthetic copy state".to_vec(),
                },
            ))
        };
        let now = monotonic_ms();
        let (mut short, first) = fixture(now + 500)?;
        let ([mut long, _], second) = fixture(now + 60_000)?;
        deposit(
            Path::new(env!("CARGO_BIN_EXE_yulius")),
            dir.path(),
            vec![first, second],
        )
        .await?;
        for peer in &mut short {
            assert_eq!(
                peer.read(&mut [0]).await?,
                0,
                "the tunnel expired before a successor arrived"
            );
        }
        long.write_all(b"live").await?;
        let mut pairs = adopt(dir.path()).await?;
        assert_eq!(
            pairs.len(),
            1,
            "the successor receives only the longer-lived pair"
        );
        let [left, _] = pairs
            .pop()
            .ok_or_else(|| io::Error::other("missing live tunnel"))?
            .sockets;
        let mut left = UnixStream::from_std(left.into())?;
        assert_eq!(left.read_u32().await?.to_be_bytes(), *b"live");
        Ok(())
    })
    .await?
}
