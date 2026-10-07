//! Resumable, checksummed chunk upload of import files (SPEC-001 IMP-001, AC-IMP-1).

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use reqwest::Url;
use sha2::{Digest, Sha256};
use yulius_contracts::import::{
    CHUNK_SHA256_HEADER, CreateImportRequest, CreateImportResponse, FileProgress, FileRef,
    ImportManifest, ImportResult, MAX_CHUNK_BYTES, ManifestError,
};

use crate::hash::{copy_into_hasher, sha256_hex};
use crate::http::{ApiClient, ApiError, CHUNK_TIMEOUT, JSON_TIMEOUT, RetryPolicy};
use crate::secret::Secret;

/// An upload failure.
#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    /// The API call failed.
    #[error(transparent)]
    Api(#[from] ApiError),
    /// The manifest violates the contract; nothing was sent.
    #[error("invalid import manifest: {0}")]
    Manifest(#[from] ManifestError),
    /// The local file no longer matches its manifest entry.
    #[error("{name} changed on disk after the manifest was built; run the import again")]
    LocalFileChanged {
        /// Manifest file name.
        name: String,
    },
    /// Reading the local file failed.
    #[error("cannot read {name}: {source}")]
    LocalIo {
        /// Manifest file name.
        name: String,
        /// Underlying error.
        #[source]
        source: io::Error,
    },
}

/// Client for the import endpoints of one server.
pub struct Uploader {
    api: ApiClient,
    chunk_size: u64,
}

impl Uploader {
    /// Creates an uploader for `server` authenticated with `token`.
    ///
    /// # Errors
    /// [`ApiError::InsecureServerUrl`], [`ApiError::InvalidServerUrl`] or [`ApiError::Client`].
    pub fn new(server: Url, token: Secret) -> Result<Self, ApiError> {
        Ok(Self {
            api: ApiClient::new(server, Some(token))?,
            chunk_size: MAX_CHUNK_BYTES,
        })
    }

    /// Replaces the retry policy (default: 3 attempts, backoff at most 10 s in total).
    #[must_use]
    pub fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.api.set_retry(retry);
        self
    }

    /// Sets the chunk size, clamped to `1..=MAX_CHUNK_BYTES` (default 8 MiB).
    #[must_use]
    pub fn with_chunk_size(mut self, bytes: u64) -> Self {
        self.chunk_size = bytes.clamp(1, MAX_CHUNK_BYTES);
        self
    }

    /// The server base URL.
    #[must_use]
    pub fn server(&self) -> &Url {
        self.api.base()
    }

    /// Validates `manifest` and creates (or resumes) an import job; the response lists the files
    /// the server still needs and where to resume each.
    ///
    /// # Errors
    /// [`UploadError::Manifest`] before any request, or an API failure.
    pub fn create_job(
        &self,
        manifest: &ImportManifest,
    ) -> Result<CreateImportResponse, UploadError> {
        manifest.validate()?;
        let url = self.api.endpoint(&["api", "import"])?;
        let body = CreateImportRequest {
            manifest: manifest.clone(),
        };
        let response: CreateImportResponse = self
            .api
            .execute(&|client| client.post(url.clone()).timeout(JSON_TIMEOUT).json(&body))?;
        if response.job_id.is_empty() {
            return Err(ApiError::Protocol("empty job id".to_owned()).into());
        }
        Ok(response)
    }

    /// Uploads `path` as manifest file `file` of `job`, starting at the server-reported
    /// `resume_from` offset. Each chunk carries its SHA-256; transient failures are retried per
    /// the retry policy (chunks are idempotent on the server). The next offset always follows the
    /// server's reported progress. Before sending, the file size must match the manifest; after
    /// the last chunk the whole file's digest must match too. `progress` receives
    /// `(bytes the server holds, total)` after every chunk. Returns the bytes sent.
    ///
    /// # Errors
    /// [`UploadError::LocalFileChanged`], [`UploadError::LocalIo`], or an API failure
    /// (for example [`ApiError::Unavailable`] after the retries for one chunk are exhausted; the
    /// server keeps the received prefix, so a later run resumes).
    pub fn upload_file(
        &self,
        job: &str,
        file: &FileRef,
        path: &Path,
        resume_from: u64,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<u64, UploadError> {
        let local_io = |source| UploadError::LocalIo {
            name: file.name.clone(),
            source,
        };
        let changed = || UploadError::LocalFileChanged {
            name: file.name.clone(),
        };
        let mut handle = File::open(path).map_err(local_io)?;
        let size = handle.metadata().map_err(local_io)?.len();
        if size != file.size {
            return Err(changed());
        }
        if resume_from > size {
            return Err(ApiError::Protocol(format!(
                "resume offset {resume_from} for {} is beyond its size {size}",
                file.name
            ))
            .into());
        }
        let mut hasher = Sha256::new();
        if copy_into_hasher(&mut handle, &mut hasher, resume_from).map_err(local_io)? != resume_from
        {
            return Err(changed());
        }
        let url = self
            .api
            .endpoint(&["api", "import", job, "files", &file.name])?;
        let capacity = usize::try_from(self.chunk_size.min(size - resume_from)).unwrap_or(0);
        let mut buffer = vec![0_u8; capacity];
        let mut offset = resume_from;
        let mut sent = 0_u64;
        progress(offset, size);
        // An empty file still needs one (empty) chunk so the server records it.
        let mut first = true;
        while offset < size || (size == 0 && first) {
            first = false;
            let len = self.chunk_size.min(size - offset);
            let chunk_len = usize::try_from(len).map_err(|_| changed())?;
            let chunk = &mut buffer[..chunk_len];
            handle.read_exact(chunk).map_err(|error| {
                if error.kind() == io::ErrorKind::UnexpectedEof {
                    changed()
                } else {
                    local_io(error)
                }
            })?;
            hasher.update(&*chunk);
            let received = self.put_chunk(&url, offset, chunk)?;
            if received.name != file.name {
                return Err(ApiError::Protocol(format!(
                    "progress for {:?} while uploading {:?}",
                    received.name, file.name
                ))
                .into());
            }
            let chunk_end = offset + len;
            if received.received_bytes < chunk_end || received.received_bytes > size {
                return Err(ApiError::Protocol(format!(
                    "server reported {} bytes of {} after a chunk ending at {chunk_end}",
                    received.received_bytes, file.name
                ))
                .into());
            }
            // The server may already hold more (an earlier attempt's chunks); follow it.
            let skip = received.received_bytes - chunk_end;
            if skip > 0
                && copy_into_hasher(&mut handle, &mut hasher, skip).map_err(local_io)? != skip
            {
                return Err(changed());
            }
            sent += len;
            offset = received.received_bytes;
            progress(offset, size);
        }
        if hex::encode(hasher.finalize()) != file.sha256 {
            return Err(changed());
        }
        Ok(sent)
    }

    fn put_chunk(&self, url: &Url, offset: u64, chunk: &[u8]) -> Result<FileProgress, ApiError> {
        let mut url = url.clone();
        url.query_pairs_mut()
            .clear()
            .append_pair("offset", &offset.to_string());
        let digest = sha256_hex(chunk);
        self.api.execute(&|client| {
            client
                .put(url.clone())
                .timeout(CHUNK_TIMEOUT)
                .header(CHUNK_SHA256_HEADER, digest.as_str())
                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                .body(chunk.to_vec())
        })
    }

    /// Asks the server to apply a fully uploaded job.
    ///
    /// # Errors
    /// An API failure (for example [`ApiError::Conflict`] while files are incomplete).
    pub fn complete(&self, job: &str) -> Result<ImportResult, UploadError> {
        let url = self.api.endpoint(&["api", "import", job, "complete"])?;
        Ok(self
            .api
            .execute(&|client| client.post(url.clone()).timeout(JSON_TIMEOUT))?)
    }
}
