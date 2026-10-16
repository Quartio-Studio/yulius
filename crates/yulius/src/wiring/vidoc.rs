//! Vidoc's Messages API (<https://model.vidocsecurity.com>) for the instance key's test
//! (SPEC-195 VDK-002): one `GET /v1/models` with `x-api-key` and `anthropic-version`, no
//! redirects, one deadline, a bounded answer that must list `vidoc-1`. The key never appears in a
//! log, an error or a debug string. Scans themselves run in sandboxes, through the egress proxy
//! (`scripts/vidoc_scan.py`).

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use yulius_app::model::Secret;
use yulius_app::ports::vidoc::{VidocCheck, VidocConnector};

/// The API's base address.
pub const VIDOC_API: &str = "https://model.vidocsecurity.com";
/// The model the scan uses.
pub const VIDOC_MODEL: &str = "vidoc-1";
/// Deadline of the test request (connect, request and the whole body).
const DEADLINE: Duration = Duration::from_secs(10);
/// Largest answer read.
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// Tests Vidoc keys.
#[derive(Debug, Clone)]
pub struct VidocHttp {
    base: String,
    https_only: bool,
}

impl Default for VidocHttp {
    fn default() -> Self {
        Self {
            base: VIDOC_API.to_owned(),
            https_only: true,
        }
    }
}

impl VidocHttp {
    /// Another base address (tests).
    #[must_use]
    pub fn with_base(base: String) -> Self {
        Self {
            base,
            https_only: false,
        }
    }

    async fn models(&self, key: &Secret) -> Result<(u16, Vec<u8>), VidocCheck> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .user_agent(concat!("yulius/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .https_only(self.https_only)
            .timeout(DEADLINE)
            .build()
            .map_err(|_| VidocCheck::Unreachable)?;
        let mut response = http
            .get(format!("{}/v1/models", self.base))
            .header("x-api-key", key.expose())
            .header("anthropic-version", "2023-06-01")
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    VidocCheck::Timeout
                } else {
                    VidocCheck::Unreachable
                }
            })?;
        let status = response.status().as_u16();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| {
            if e.is_timeout() {
                VidocCheck::Timeout
            } else {
                VidocCheck::Unreachable
            }
        })? {
            if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(VidocCheck::Invalid);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((status, bytes))
    }
}

/// Whether a `/v1/models` answer lists `vidoc-1`.
fn lists_vidoc_1(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("data")?.as_array().cloned())
        .is_some_and(|models| {
            models
                .iter()
                .any(|m| m.get("id").and_then(Value::as_str) == Some(VIDOC_MODEL))
        })
}

#[async_trait]
impl VidocConnector for VidocHttp {
    async fn check(&self, key: &Secret) -> VidocCheck {
        match self.models(key).await {
            Ok((200, body)) if lists_vidoc_1(&body) => VidocCheck::Ok,
            Ok((200, _)) => VidocCheck::Invalid,
            Ok((401 | 403, _)) => VidocCheck::Refused,
            Ok(_) => VidocCheck::Unreachable,
            Err(check) => check,
        }
    }
}

/// The key of `YULIUS_VIDOC_API_KEY_FILE`: `Ok(None)` when unset, `Err` (a message without the
/// key) when set but unusable (an owner-only file of 8–4096 printable characters, the rule of
/// every Yulius secret file).
///
/// # Errors
/// Why the file cannot be used.
pub fn config_key(path: Option<&Path>) -> Result<Option<Secret>, String> {
    match path {
        None => Ok(None),
        Some(path) => crate::wiring::jev::read_secret_file(path)
            .map(Some)
            .map_err(|e| e.replace("Jev key", "Vidoc key")),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    const KEY: &str = "synthetic-vidoc-key-0001";
    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A server answering every request with `status` and `body`, recording request heads.
    async fn fake(
        status: u16,
        body: &str,
    ) -> Result<(String, Arc<Mutex<Vec<String>>>), std::io::Error> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (s, body) = (seen.clone(), body.to_owned());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = vec![0u8; 16 * 1024];
                let mut got = Vec::new();
                while let Ok(n) = socket.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..n]);
                    if got.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                if let Ok(mut seen) = s.lock() {
                    seen.push(String::from_utf8_lossy(&got).to_lowercase());
                }
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            }
        });
        Ok((base, seen))
    }

    #[tokio::test]
    async fn a_key_that_lists_vidoc_1_works() -> TestResult {
        let (base, seen) = fake(
            200,
            r#"{"object":"list","data":[{"id":"vidoc-1","object":"model"}]}"#,
        )
        .await?;
        let check = VidocHttp::with_base(base).check(&Secret(KEY.into())).await;
        assert_eq!(check, VidocCheck::Ok);
        let head = seen.lock().map_err(|_| "lock")?.join("\n");
        assert!(head.starts_with("get /v1/models "), "{head}");
        assert!(head.contains(&format!("x-api-key: {KEY}")), "{head}");
        assert!(head.contains("anthropic-version: 2023-06-01"), "{head}");
        assert!(
            !head.contains("authorization"),
            "the key travels in x-api-key only"
        );
        Ok(())
    }

    #[tokio::test]
    async fn refusals_errors_and_other_answers_are_named() -> TestResult {
        for (status, body, expected) in [
            (401, "{}", VidocCheck::Refused),
            (403, "{}", VidocCheck::Refused),
            (500, "{}", VidocCheck::Unreachable),
            (429, "{}", VidocCheck::Unreachable),
            (200, r#"{"data":[{"id":"other"}]}"#, VidocCheck::Invalid),
            (200, "not json", VidocCheck::Invalid),
        ] {
            let (base, _) = fake(status, body).await?;
            let check = VidocHttp::with_base(base).check(&Secret(KEY.into())).await;
            assert_eq!(check, expected, "{status} {body}");
        }
        let check = VidocHttp::with_base("http://127.0.0.1:9".into())
            .check(&Secret(KEY.into()))
            .await;
        assert_eq!(check, VidocCheck::Unreachable);
        Ok(())
    }

    #[test]
    fn the_key_file_follows_the_secret_file_rules() -> TestResult {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(config_key(None), Ok(None));
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("vidoc.key");
        std::fs::write(&path, format!("{KEY}\n"))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;
        let err = config_key(Some(&path)).err().unwrap_or_default();
        assert!(err.contains("0600"), "{err}");
        assert!(!err.contains(KEY));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        assert_eq!(
            config_key(Some(&path))?.map(|k| k.expose().to_owned()),
            Some(KEY.to_owned())
        );
        Ok(())
    }
}
