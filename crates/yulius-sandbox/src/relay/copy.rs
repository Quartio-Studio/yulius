//! Buffered state of an established opaque tunnel or its host-side relay (SPEC-577).

use std::io::{self, Cursor};

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _, copy_buf, split};

const BUFFER_BYTES: usize = 8192;

/// Unsent bytes and write-half completion for each direction of one connected socket pair.
/// Keep this state with the pair when suspending a transfer. It contains private traffic.
#[derive(Default)]
pub struct CopyState {
    to_peer: Direction,
    to_client: Direction,
}

struct Direction {
    pending: Cursor<Vec<u8>>,
    closed: bool,
}

impl Default for Direction {
    fn default() -> Self {
        Self {
            pending: Cursor::new(Vec::with_capacity(BUFFER_BYTES)),
            closed: false,
        }
    }
}

impl Direction {
    async fn transfer(
        &mut self,
        mut source: impl AsyncRead + Unpin,
        mut destination: impl AsyncWrite + Unpin,
    ) -> io::Result<()> {
        while !self.closed {
            if self.pending.get_ref().is_empty()
                && source.read_buf(self.pending.get_mut()).await? == 0
            {
                destination.shutdown().await?;
                self.closed = true;
                break;
            }
            // copy_buf advances the cursor after each successful write. Cancelling this future
            // during a partial write or flush leaves the cursor at the first unsent byte.
            copy_buf(&mut self.pending, &mut destination).await?;
            self.pending.get_mut().clear();
            self.pending.set_position(0);
        }
        Ok(())
    }
}

impl CopyState {
    /// Copies in both directions until both write halves close or an I/O error occurs.
    /// Dropping the future pauses the copy. Call again with this state and the same sockets
    /// in the same order to resume. Each direction buffers up to 8 KiB.
    ///
    /// # Errors
    /// Propagates socket read, write, flush and shutdown failures.
    pub async fn transfer<A, B>(&mut self, client: &mut A, peer: &mut B) -> io::Result<()>
    where
        A: AsyncRead + AsyncWrite + Unpin,
        B: AsyncRead + AsyncWrite + Unpin,
    {
        let (client_read, client_write) = split(client);
        let (peer_read, peer_write) = split(peer);
        tokio::try_join!(
            self.to_peer.transfer(client_read, peer_write),
            self.to_client.transfer(peer_read, client_write)
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::future::poll_fn;
    use std::io;
    use std::task::Poll;
    use std::time::Duration;

    use super::CopyState;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, duplex};

    // A single poll consumes immediately available input and stops at backpressure.
    async fn pause(copy: impl Future<Output = io::Result<()>>) {
        let mut copy = std::pin::pin!(copy);
        assert!(
            poll_fn(|cx| Poll::Ready(copy.as_mut().poll(cx)))
                .await
                .is_pending()
        );
    }

    #[tokio::test]
    async fn repeated_pauses_preserve_partial_writes_in_both_directions() -> io::Result<()> {
        tokio::time::timeout(Duration::from_secs(5), async {
            let (mut client, mut left) = duplex(4);
            let (mut right, mut peer) = duplex(4);
            // Leave room for one byte in each destination, with four bytes ready at each source.
            left.write_all(b"aaa").await?;
            right.write_all(b"bbb").await?;
            client.write_all(b"1234").await?;
            peer.write_all(b"WXYZ").await?;
            let mut copy = CopyState::default();
            pause(copy.transfer(&mut left, &mut right)).await;
            assert_eq!(client.read_u32().await?.to_be_bytes(), *b"aaaW");
            assert_eq!(peer.read_u32().await?.to_be_bytes(), *b"bbb1");

            // Each destination can take its three pending bytes and one of the next four.
            client.write_all(b"5678").await?;
            peer.write_all(b"abcd").await?;
            pause(copy.transfer(&mut left, &mut right)).await;
            assert_eq!(client.read_u32().await?.to_be_bytes(), *b"XYZa");
            assert_eq!(peer.read_u32().await?.to_be_bytes(), *b"2345");

            client.shutdown().await?;
            peer.shutdown().await?;
            let mut from_client = Vec::new();
            let mut from_peer = Vec::new();
            tokio::try_join!(
                copy.transfer(&mut left, &mut right),
                peer.read_to_end(&mut from_client),
                client.read_to_end(&mut from_peer)
            )?;
            assert_eq!(from_client, b"678");
            assert_eq!(from_peer, b"bcd");
            Ok(())
        })
        .await?
    }

    #[tokio::test]
    async fn a_half_closed_peer_can_finish_its_reply_after_a_pause() -> io::Result<()> {
        tokio::time::timeout(Duration::from_secs(5), async {
            let (mut client, mut left) = duplex(4);
            let (mut right, mut peer) = duplex(4);
            left.write_all(b"aaa").await?;
            client.shutdown().await?;
            peer.write_all(b"pong").await?;
            let mut copy = CopyState::default();
            pause(copy.transfer(&mut left, &mut right)).await;
            assert_eq!(
                peer.read(&mut [0]).await?,
                0,
                "the client's FIN reached the peer"
            );
            assert_eq!(client.read_u32().await?.to_be_bytes(), *b"aaap");

            peer.write_all(b"done").await?;
            peer.shutdown().await?;
            let mut reply = Vec::new();
            tokio::try_join!(
                copy.transfer(&mut left, &mut right),
                client.read_to_end(&mut reply)
            )?;
            assert_eq!(reply, b"ongdone");
            Ok(())
        })
        .await?
    }

    #[tokio::test]
    async fn a_blocked_destination_bounds_the_bytes_read_from_the_source() -> io::Result<()> {
        tokio::time::timeout(Duration::from_secs(5), async {
            let bytes = vec![b'x'; 16_384];
            let (mut client, mut left) = duplex(bytes.len());
            let (mut right, _peer) = duplex(1);
            right.write_all(b"!").await?;
            client.write_all(&bytes).await?;
            let mut copy = CopyState::default();
            pause(copy.transfer(&mut left, &mut right)).await;
            // The room now available in the originally full source socket measures read-ahead.
            let room = client.write(&bytes).await?;
            assert!((1..=8192).contains(&room), "read ahead {room} bytes");
            Ok(())
        })
        .await?
    }

    #[tokio::test]
    async fn a_destination_write_error_ends_the_copy_with_the_other_peer_still_open()
    -> io::Result<()> {
        let (mut client, mut left) = duplex(4);
        let (mut right, peer) = duplex(4);
        drop(peer);
        client.write_all(b"ping").await?;
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            CopyState::default().transfer(&mut left, &mut right),
        )
        .await?;
        assert!(result.is_err_and(|error| error.kind() == io::ErrorKind::BrokenPipe));
        Ok(())
    }
}
