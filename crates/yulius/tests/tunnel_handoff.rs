//! SPEC-577. Exercise the production helper as a separate process.
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream, UnixStream};
use yulius_sandbox::relay::custodian::{Pair, adopt, deposit};

#[tokio::test]
async fn a_successor_adopts_the_original_unix_and_tcp_peers_once() -> io::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        // A helper killed after bind left a name but holds no sockets or lock.
        drop(std::os::unix::net::UnixListener::bind(
            dir.path().join("tunnels.sock"),
        )?);
        let (mut client, left) = UnixStream::pair()?;
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let (right, (mut remote, _)) = tokio::try_join!(
            TcpStream::connect(listener.local_addr()?),
            listener.accept()
        )?;
        let saved = Pair {
            sockets: [left.into_std()?.into(), right.into_std()?.into()],
            state: b"pending".to_vec(),
        };
        deposit(
            Path::new(env!("CARGO_BIN_EXE_yulius")),
            dir.path(),
            vec![saved],
        )
        .await?;
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
