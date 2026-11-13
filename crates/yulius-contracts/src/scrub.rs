//! Redaction fingerprints (SPEC-002 ADR-20, VLT-007): what processes inside a sandbox (the Bash
//! wrapper, the `PostToolUse` hook) receive to redact agent secrets without holding them.
//!
//! Served by `POST /internal/scrub/fingerprints` (task token) and written at launch to
//! `<run>/scrub.json` (the fallback while Yulius restarts). A candidate is found by a needle's
//! tag (a few bits of a hash of its first eight bytes) and confirmed by
//! `SHA-256(salt || window)`; the needle itself never leaves the server. A sandbox's set is the
//! same on every request until a secret changes, so asking again tells nothing more (ADR-0313).

use serde::{Deserialize, Serialize};

/// Current format version.
pub const FINGERPRINT_VERSION: u32 = 2;

/// The fingerprints of every agent secret that applies to a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FingerprintSet {
    /// [`FINGERPRINT_VERSION`].
    pub version: u32,
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
    /// The needle's tag (`yulius_domain::vault::redact::needle_tag`).
    pub tag: u16,
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
