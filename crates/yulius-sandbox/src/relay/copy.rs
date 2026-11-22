//! Buffered state of an established opaque tunnel or its host-side relay (SPEC-577).

use std::io::{self, BufRead as _, Cursor};

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
    phase: Phase,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Copying,
    ShuttingDown,
    Closed,
}

impl Default for Direction {
    fn default() -> Self {
        Self {
            pending: Cursor::new(Vec::with_capacity(BUFFER_BYTES)),
            phase: Phase::Copying,
        }
    }
}

impl Direction {
    async fn transfer(
        &mut self,
        mut source: impl AsyncRead + Unpin,
        mut destination: impl AsyncWrite + Unpin,
    ) -> io::Result<()> {
        while self.phase == Phase::Copying {
            if self.pending.get_ref().is_empty() {
                match source.read_buf(self.pending.get_mut()).await {
                    Ok(0) => {
                        self.phase = Phase::ShuttingDown;
                        break;
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                        tokio::task::yield_now().await;
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }
            // copy_buf advances the cursor after each successful write. Cancelling this future
            // during a partial write or flush leaves the cursor at the first unsent byte.
            copy_buf(&mut self.pending, &mut destination).await?;
            self.pending.get_mut().clear();
            self.pending.set_position(0);
        }
        if self.phase == Phase::ShuttingDown {
            destination.shutdown().await?;
            self.phase = Phase::Closed;
        }
        Ok(())
    }
}

impl CopyState {
    /// Maximum pending bytes in each direction, including HTTP upgrade lookahead.
    pub const BUFFER_BYTES: usize = BUFFER_BYTES;

    /// Begin with the opaque bytes already read by the HTTP listener after the headers.
    /// No forwarding has to complete before the connection can accept a checkpoint.
    ///
    /// # Errors
    /// Rejects a prefix larger than the listener's single-read bound.
    pub fn from_prefix(prefix: &[u8]) -> io::Result<Self> {
        if prefix.len() > BUFFER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "CONNECT lookahead exceeds the copy buffer",
            ));
        }
        let mut state = Self::default();
        state.to_peer.pending.get_mut().extend_from_slice(prefix);
        Ok(state)
    }

    /// Append the two write-half phases and exactly the unsent bytes to private handoff state.
    ///
    /// # Errors
    /// The pending byte count must fit the direction's buffer.
    pub fn encode(self, output: &mut Vec<u8>) -> io::Result<()> {
        for mut direction in [self.to_peer, self.to_client] {
            let bytes = direction.pending.fill_buf()?;
            let size = u16::try_from(bytes.len()).map_err(io::Error::other)?;
            output.push(match direction.phase {
                Phase::Copying => 0,
                Phase::ShuttingDown => 1,
                Phase::Closed => 2,
            });
            output.extend_from_slice(&size.to_be_bytes());
            output.extend_from_slice(bytes);
        }
        Ok(())
    }

    /// Recreate the bounded copy buffers and their original half-close positions.
    ///
    /// # Errors
    /// Rejects truncated, oversized, inconsistent or trailing private state.
    pub fn decode(mut input: &[u8]) -> io::Result<Self> {
        let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid tunnel copy state");
        let mut direction = || {
            let ([phase, high, low], rest) = input.split_first_chunk::<3>().ok_or_else(invalid)?;
            let len = usize::from(u16::from_be_bytes([*high, *low]));
            if len > BUFFER_BYTES {
                return Err(invalid());
            }
            let phase = match (phase, len) {
                (0, _) => Phase::Copying,
                (1, 0) => Phase::ShuttingDown,
                (2, 0) => Phase::Closed,
                _ => return Err(invalid()),
            };
            let (pending, rest) = rest.split_at_checked(len).ok_or_else(invalid)?;
            input = rest;
            let mut bytes = Vec::with_capacity(BUFFER_BYTES);
            bytes.extend_from_slice(pending);
            Ok(Direction {
                phase,
                pending: Cursor::new(bytes),
            })
        };
        let state = Self {
            to_peer: direction()?,
            to_client: direction()?,
        };
        if !input.is_empty() {
            return Err(invalid());
        }
        Ok(state)
    }

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

    #[test]
    fn copy_records_accept_the_bound_and_reject_invalid_phases_and_lengths() -> io::Result<()> {
        let mut valid = vec![0, 0x20, 0];
        valid.resize(3 + super::BUFFER_BYTES, 0x42);
        valid.extend_from_slice(&[1, 0, 0]);
        let mut encoded = Vec::new();
        CopyState::decode(&valid)?.encode(&mut encoded)?;
        assert_eq!(encoded, valid);
        let mut oversized = valid.clone();
        oversized[2] = 1;
        oversized.insert(3, 0x42);
        assert!(CopyState::decode(&oversized).is_err());
        for bad in [
            &[][..],
            &[0, 0, 2, 1],
            &[0, 0x20, 1],
            &[1, 0, 1, 0, 0, 0, 0],
            &[2, 0, 1, 0, 0, 0, 0],
            &[3, 0, 0, 0, 0, 0],
            &[0, 0, 0, 0, 0, 0, 1],
        ] {
            assert!(CopyState::decode(bad).is_err(), "{bad:?}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_stalled_maximum_upgrade_prefix_checkpoints_while_the_peer_can_reply()
    -> io::Result<()> {
        let (mut client, mut left) = duplex(4);
        let (mut right, mut peer) = duplex(4);
        let prefix = vec![b'X'; CopyState::BUFFER_BYTES];
        assert!(CopyState::from_prefix(&vec![0; CopyState::BUFFER_BYTES + 1]).is_err());
        peer.write_all(b"HELO").await?;
        let mut copy = CopyState::from_prefix(&prefix)?;
        pause(copy.transfer(&mut left, &mut right)).await;
        assert_eq!(client.read_u32().await?.to_be_bytes(), *b"HELO");
        // The destination is full, with most of the original header's lookahead still pending.
        let mut encoded = Vec::new();
        copy.encode(&mut encoded)?;
        let mut resumed = CopyState::decode(&encoded)?;
        client.write_all(b"TAIL").await?;
        client.shutdown().await?;
        peer.shutdown().await?;
        let mut received = Vec::new();
        tokio::time::timeout(Duration::from_secs(3), async {
            tokio::try_join!(
                resumed.transfer(&mut left, &mut right),
                peer.read_to_end(&mut received)
            )
        })
        .await??;
        assert_eq!(received.len(), prefix.len() + 4);
        assert!(received.starts_with(&prefix));
        assert!(received.ends_with(b"TAIL"));
        Ok(())
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

            // Model reconstructing the copy in a new server process.
            let mut encoded = Vec::new();
            copy.encode(&mut encoded)?;
            copy = CopyState::decode(&encoded)?;

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

            let mut encoded = Vec::new();
            copy.encode(&mut encoded)?;
            copy = CopyState::decode(&encoded)?;
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

    struct ScriptedRead<I>(I);

    impl<I: Iterator<Item = io::Result<()>> + Unpin> tokio::io::AsyncRead for ScriptedRead<I> {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            _: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(self.0.next().unwrap_or(Ok(())))
        }
    }

    #[tokio::test]
    async fn interrupted_reads_retry_and_fatal_read_errors_end_the_copy() -> io::Result<()> {
        use tokio::io::{empty, join, sink};
        let copy_after = |error| async move {
            let source =
                ScriptedRead([Err(io::Error::from(error))].into_iter()).chain(&b"marker"[..]);
            let mut client = join(source, sink());
            let mut peer = join(empty(), Vec::new());
            CopyState::default()
                .transfer(&mut client, &mut peer)
                .await?;
            io::Result::Ok(peer.into_inner().1)
        };
        assert_eq!(copy_after(io::ErrorKind::Interrupted).await?, b"marker");
        assert!(
            copy_after(io::ErrorKind::ConnectionReset)
                .await
                .is_err_and(|error| { error.kind() == io::ErrorKind::ConnectionReset })
        );
        Ok(())
    }

    struct PendingShutdownOnce {
        calls: usize,
    }

    impl tokio::io::AsyncWrite for PendingShutdownOnce {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(bytes.len()))
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> Poll<io::Result<()>> {
            self.calls += 1;
            if self.calls == 1 {
                cx.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(Ok(()))
            }
        }
    }

    #[tokio::test]
    async fn pausing_during_shutdown_resumes_the_shutdown_without_reading_past_eof()
    -> io::Result<()> {
        let eof_then_error =
            ScriptedRead([Ok(()), Err(io::ErrorKind::ConnectionReset.into())].into_iter());
        let mut client = tokio::io::join(eof_then_error, Vec::new());
        let mut peer = tokio::io::join(&b"reply"[..], PendingShutdownOnce { calls: 0 });
        let mut copy = CopyState::default();
        pause(copy.transfer(&mut client, &mut peer)).await;
        let mut encoded = Vec::new();
        copy.encode(&mut encoded)?;
        copy = CopyState::decode(&encoded)?;
        copy.transfer(&mut client, &mut peer).await?;
        assert_eq!(
            peer.into_inner().1.calls,
            2,
            "the pending shutdown completed on resume"
        );
        assert_eq!(client.into_inner().1, b"reply");
        Ok(())
    }
}
