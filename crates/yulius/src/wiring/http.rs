//! The outbound HTTP client every adapter starts from, and its bounded body read.

/// A client builder with the ring TLS provider installed, Yulius's user agent and no redirects
/// followed; each adapter adds its own timeouts, `https_only` and retry policy.
pub fn client_builder() -> reqwest::ClientBuilder {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .user_agent(concat!("yulius/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
}

/// Why [`read_bounded`] returned no body.
#[derive(Debug)]
pub enum BodyError {
    /// The body could not be read.
    Read(reqwest::Error),
    /// The body is longer than allowed: what was read, with the chunk that passed the bound.
    TooLarge(Vec<u8>),
}

/// Reads `response`'s body, at most `max` bytes, also when it is chunked.
///
/// # Errors
/// [`BodyError::Read`] when reading fails, [`BodyError::TooLarge`] past `max` bytes.
pub async fn read_bounded(
    response: &mut reqwest::Response,
    max: usize,
) -> Result<Vec<u8>, BodyError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(BodyError::Read)? {
        let over = chunk.len() > max - bytes.len();
        bytes.extend_from_slice(&chunk);
        if over {
            return Err(BodyError::TooLarge(bytes));
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::{BodyError, client_builder, read_bounded};

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

    /// The body of one answer of `len` bytes, sent chunked in two halves, read with `max`.
    async fn body_of(len: usize, max: usize) -> TestResult<Result<Vec<u8>, BodyError>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request).await?;
            let mut reply = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
            for size in [len / 2, len - len / 2].into_iter().filter(|n| *n > 0) {
                reply.extend_from_slice(format!("{size:x}\r\n").as_bytes());
                reply.resize(reply.len() + size, b'x');
                reply.extend_from_slice(b"\r\n");
            }
            reply.extend_from_slice(b"0\r\n\r\n");
            socket.write_all(&reply).await
        });
        let client = client_builder().build()?;
        let mut response = client.get(format!("http://{addr}/")).send().await?;
        let body = read_bounded(&mut response, max).await;
        server.await??;
        Ok(body)
    }

    #[tokio::test]
    async fn reads_a_chunked_body_up_to_the_bound_and_refuses_one_byte_more() -> TestResult {
        assert_eq!(body_of(0, 8).await?.ok(), Some(Vec::new()));
        assert_eq!(body_of(8, 8).await?.ok(), Some(b"xxxxxxxx".to_vec()));
        match body_of(9, 8).await? {
            Err(BodyError::TooLarge(read)) => assert_eq!(read.len(), 9),
            other => return Err(format!("expected TooLarge, got {other:?}").into()),
        }
        Ok(())
    }
}
