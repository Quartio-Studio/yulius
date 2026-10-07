//! Async client of a ptyhost socket, used by the server's session supervisor.

use std::io;
use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use yulius_contracts::ptyhost::{
    ClientFrame, FrameDecoder, HostFrame, PROTOCOL_VERSION, ProtocolError, SOCKET_FILE, Snapshot,
    TOKEN_BYTES, Welcome, encode_client,
};

/// Bound on connecting, authenticating and receiving the welcome (and snapshot).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

const READ_CHUNK: usize = 64 * 1024;

/// Client-side failure.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The socket could not be reached (no host, stale run directory, permission).
    #[error("cannot connect to the ptyhost socket: {0}")]
    Connect(#[source] io::Error),
    /// The handshake did not complete within [`CONNECT_TIMEOUT`].
    #[error("ptyhost handshake timed out")]
    Timeout,
    /// Reading or writing the socket failed.
    #[error("ptyhost socket I/O: {0}")]
    Io(#[source] io::Error),
    /// The host sent bytes that are not a valid frame; the connection is unusable.
    #[error("ptyhost protocol violation: {0}")]
    Protocol(#[source] ProtocolError),
    /// A frame to send violates the protocol bounds (nothing was sent).
    #[error("invalid frame: {0}")]
    InvalidFrame(#[source] ProtocolError),
    /// The host refused the connection (bad token, version, busy).
    #[error("ptyhost rejected the connection (code {code}): {message}")]
    Rejected {
        /// One of [`yulius_contracts::ptyhost::error_code`].
        code: u16,
        /// Host diagnostic.
        message: String,
    },
    /// The host closed the connection during the handshake.
    #[error("ptyhost closed the connection")]
    Closed,
    /// The host sent a frame that is not valid at this point of the handshake.
    #[error("unexpected frame from ptyhost: {0}")]
    UnexpectedFrame(&'static str),
}

/// Receiving half of a ptyhost connection.
#[derive(Debug)]
pub struct PtyReader {
    stream: OwnedReadHalf,
    decoder: FrameDecoder,
    buf: Box<[u8]>,
}

/// Sending half of a ptyhost connection.
#[derive(Debug)]
pub struct PtyWriter {
    stream: OwnedWriteHalf,
}

/// An authenticated connection to a ptyhost. Only one client controls a host at a time: a newer
/// authenticated connection replaces this one, which then receives an `Error` and EOF.
#[derive(Debug)]
pub struct PtyClient {
    reader: PtyReader,
    writer: PtyWriter,
}

impl PtyClient {
    /// Connects to `run_dir/pty.sock`, authenticates with `token` and returns the welcome and,
    /// when requested, the screen snapshot. Frames that follow are read with
    /// [`PtyClient::recv`]. The whole handshake is bounded by [`CONNECT_TIMEOUT`].
    ///
    /// # Errors
    /// [`ClientError::Rejected`] for a refused token or version, [`ClientError::Timeout`],
    /// [`ClientError::Connect`] when no host listens, or protocol/I/O errors.
    pub async fn connect(
        run_dir: &Path,
        token: &[u8; TOKEN_BYTES],
        want_snapshot: bool,
    ) -> Result<(Self, Welcome, Option<Snapshot>), ClientError> {
        tokio::time::timeout(
            CONNECT_TIMEOUT,
            Self::handshake(run_dir, token, want_snapshot),
        )
        .await
        .map_err(|_| ClientError::Timeout)?
    }

    async fn handshake(
        run_dir: &Path,
        token: &[u8; TOKEN_BYTES],
        want_snapshot: bool,
    ) -> Result<(Self, Welcome, Option<Snapshot>), ClientError> {
        let stream = UnixStream::connect(run_dir.join(SOCKET_FILE))
            .await
            .map_err(ClientError::Connect)?;
        let (read, write) = stream.into_split();
        let mut client = Self {
            reader: PtyReader {
                stream: read,
                decoder: FrameDecoder::new(),
                buf: vec![0u8; READ_CHUNK].into_boxed_slice(),
            },
            writer: PtyWriter { stream: write },
        };
        client
            .send(ClientFrame::Hello {
                version: PROTOCOL_VERSION,
                token: *token,
                want_snapshot,
            })
            .await?;
        let welcome = match client.recv().await? {
            Some(HostFrame::Welcome(welcome)) => welcome,
            other => return Err(handshake_failure(other, "expected Welcome")),
        };
        let snapshot = if want_snapshot {
            match client.recv().await? {
                Some(HostFrame::Snapshot(snapshot)) => Some(snapshot),
                other => return Err(handshake_failure(other, "expected Snapshot")),
            }
        } else {
            None
        };
        Ok((client, welcome, snapshot))
    }

    /// Sends one frame. Not cancel-safe: dropping the future may leave a partial frame.
    ///
    /// # Errors
    /// [`ClientError::InvalidFrame`] for out-of-bound frames, [`ClientError::Io`] otherwise.
    pub async fn send(&mut self, frame: ClientFrame) -> Result<(), ClientError> {
        self.writer.send(frame).await
    }

    /// Receives the next frame; `Ok(None)` when the host closed the connection. Cancel-safe.
    ///
    /// # Errors
    /// [`ClientError::Protocol`] for malformed input, [`ClientError::Io`] for socket errors.
    pub async fn recv(&mut self) -> Result<Option<HostFrame>, ClientError> {
        self.reader.recv().await
    }

    /// Splits the connection so reading and writing can run in separate tasks.
    #[must_use]
    pub fn into_split(self) -> (PtyReader, PtyWriter) {
        (self.reader, self.writer)
    }
}

fn handshake_failure(frame: Option<HostFrame>, expected: &'static str) -> ClientError {
    match frame {
        Some(HostFrame::Error { code, message }) => ClientError::Rejected { code, message },
        Some(_) => ClientError::UnexpectedFrame(expected),
        None => ClientError::Closed,
    }
}

impl PtyReader {
    /// Receives the next frame; `Ok(None)` when the host closed the connection. Cancel-safe:
    /// received bytes are kept in the decoder between calls.
    ///
    /// # Errors
    /// [`ClientError::Protocol`] for malformed input, [`ClientError::Io`] for socket errors.
    pub async fn recv(&mut self) -> Result<Option<HostFrame>, ClientError> {
        loop {
            if let Some(frame) = self.decoder.next_host().map_err(ClientError::Protocol)? {
                return Ok(Some(frame));
            }
            let n = match self.stream.read(&mut self.buf).await {
                Ok(n) => n,
                // A host that cut us off may reset the connection instead of closing it.
                Err(error) if error.kind() == io::ErrorKind::ConnectionReset => 0,
                Err(error) => return Err(ClientError::Io(error)),
            };
            if n == 0 {
                self.decoder.finish().map_err(ClientError::Protocol)?;
                return Ok(None);
            }
            self.decoder.push(self.buf.get(..n).unwrap_or_default());
        }
    }
}

impl PtyWriter {
    /// Sends one frame. Not cancel-safe: dropping the future may leave a partial frame.
    ///
    /// # Errors
    /// [`ClientError::InvalidFrame`] for out-of-bound frames, [`ClientError::Io`] otherwise.
    pub async fn send(&mut self, frame: ClientFrame) -> Result<(), ClientError> {
        frame.validate().map_err(ClientError::InvalidFrame)?;
        self.stream
            .write_all(&encode_client(&frame))
            .await
            .map_err(ClientError::Io)
    }

    /// Closes the sending direction; the host treats this as the client leaving.
    ///
    /// # Errors
    /// Socket errors.
    pub async fn shutdown(&mut self) -> Result<(), ClientError> {
        self.stream.shutdown().await.map_err(ClientError::Io)
    }
}
