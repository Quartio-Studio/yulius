//! Bound one socket read into Hyper. Once it finds a CONNECT's header boundary, the bytes
//! it has taken past that boundary fit one resumable direction; the remaining bytes are
//! still on the original socket. Header parsing can read as many chunks as Hyper allows.
use std::io::{self, IoSlice};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use yulius_sandbox::relay::CopyState;

/// The TCP transport on a Linux listener serving restartable CONNECT requests.
pub struct HttpInput<T = TcpStream>(
    /// Original socket, also used for the listener's TCP options and post-upgrade copy.
    pub T,
);

impl<T: AsyncRead + Unpin> AsyncRead for HttpInput<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let limit = buf.remaining().min(CopyState::BUFFER_BYTES);
        let mut part = ReadBuf::new(buf.initialize_unfilled_to(limit));
        let result = Pin::new(&mut self.0).poll_read(cx, &mut part);
        let filled = part.filled().len();
        buf.advance(filled);
        result
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for HttpInput<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write_vectored(cx, bufs)
    }
    fn is_write_vectored(&self) -> bool {
        self.0.is_write_vectored()
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt as _;

    #[derive(Default)]
    struct ScalarWriter {
        bytes: Vec<u8>,
        flushed: bool,
        closed: bool,
    }

    impl AsyncWrite for ScalarWriter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.bytes.extend_from_slice(bytes);
            Poll::Ready(Ok(bytes.len()))
        }
        fn poll_flush(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.flushed = true;
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.closed = true;
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn limiting_reads_keeps_the_transports_write_capabilities() -> io::Result<()> {
        let mut input = HttpInput(ScalarWriter::default());
        assert!(!input.is_write_vectored());
        input.write_all(b"marker").await?;
        assert_eq!(input.0.bytes, b"marker");
        input.flush().await?;
        assert!(input.0.flushed);
        input.shutdown().await?;
        assert!(input.0.closed);
        Ok(())
    }
}
