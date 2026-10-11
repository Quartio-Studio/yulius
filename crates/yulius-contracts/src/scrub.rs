//! Redaction fingerprints (SPEC-002 ADR-20, VLT-007): what processes inside a sandbox (the Bash
//! wrapper, the `PostToolUse` hook) receive to redact agent secrets without holding them.
//!
//! Served by `POST /internal/scrub/fingerprints` (task token) and written at launch to
//! `<run>/scrub.json` (the fallback while Yulius restarts). A needle is matched by its rolling
//! hash and confirmed by `SHA-256(salt || window)`; the needle itself never leaves the server.

use serde::{Deserialize, Serialize};

/// Current format version.
pub const FINGERPRINT_VERSION: u32 = 1;

/// The fingerprints of every agent secret that applies to a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FingerprintSet {
    /// [`FINGERPRINT_VERSION`].
    pub version: u32,
    /// Base of the rolling hash (mod 2^61 − 1).
    pub base: u64,
    /// Hex salt prefixed to a candidate before hashing.
    pub salt: String,
    /// Secret names; [`Fingerprint::secret`] indexes them.
    pub names: Vec<String>,
    /// One entry per needle (a value or one of its encodings).
    pub needles: Vec<Fingerprint>,
}

/// One needle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    /// Index into [`FingerprintSet::names`].
    pub secret: usize,
    /// Length in bytes.
    pub len: usize,
    /// Rolling hash with [`FingerprintSet::base`].
    pub rolling: u64,
    /// Hex `SHA-256(salt || needle)`.
    pub digest: String,
}

/// `POST /internal/scrub/report`: names a sandbox process redacted (audited once per run).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactionReport {
    /// `bash` (command output) or `tool` (a tool result).
    pub channel: String,
    /// Secret names.
    pub names: Vec<String>,
}
