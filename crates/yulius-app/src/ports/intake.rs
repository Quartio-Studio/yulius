//! Task intake ports (SPEC-001 TSK-011, TSK-013; SPEC-002 ADR-16): the Jev Choice evaluator and
//! the Claude split proposer. Both are optional: without them intake checks nothing and blocks
//! nothing.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use yulius_domain::intake::{ChoiceAnswer, ChoiceQuestion, ProposedTask};

use crate::model::Secret;

/// Why a Jev evaluation produced no answer. Every variant means "check nothing"; none carries
/// provider data or the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JevFailure {
    /// The input was refused before any request (shape, size, or it contains the key).
    Input(&'static str),
    /// The provider answered with an HTTP error or could not be reached.
    Unavailable,
    /// No complete answer within the deadline.
    Timeout,
    /// The answer broke the contract (the closed name of the failed check).
    Invalid(&'static str),
}

impl std::fmt::Display for JevFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input(reason) => write!(f, "jev input refused ({reason})"),
            Self::Unavailable => f.write_str("jev unavailable"),
            Self::Timeout => f.write_str("jev timed out"),
            Self::Invalid(reason) => write!(f, "jev answer invalid ({reason})"),
        }
    }
}

/// TypeSafe's Jev (`jev-1.13.0`): one Choice question per request, one request per call, no
/// retries.
#[async_trait]
pub trait ChoiceEvaluator: Send + Sync {
    /// Asks `question` about `state` within `deadline` (connect, send and the whole body).
    ///
    /// # Errors
    /// [`JevFailure`]; callers treat every failure as "no answer".
    async fn evaluate(
        &self,
        state: &str,
        question: &ChoiceQuestion<'_>,
        deadline: Duration,
    ) -> Result<ChoiceAnswer, JevFailure>;
}

/// What one small test request with a key showed (SPEC-001 ADM-002).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JevCheck {
    /// A complete, correct answer.
    Ok,
    /// The key was refused (HTTP 401 or 403).
    Refused,
    /// Another HTTP error, or Jev could not be reached.
    Unreachable,
    /// No answer within the deadline.
    Timeout,
    /// An answer outside the contract.
    Invalid,
}

/// Builds Jev evaluators from keys; the composition owns the HTTP client and the key format.
#[async_trait]
pub trait JevConnector: Send + Sync {
    /// An evaluator that uses `key`.
    ///
    /// # Errors
    /// A message (never containing the key) when the key is malformed.
    fn connect(&self, key: &Secret) -> Result<Arc<dyn ChoiceEvaluator>, String>;
    /// One small request with `key`.
    async fn check(&self, key: &Secret) -> JevCheck;
}

/// Why no split proposal was produced (secret-free, short).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposerFailure(pub String);

impl std::fmt::Display for ProposerFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Asks Claude (a headless Claude Code run on a connected account) for a split of a request.
#[async_trait]
pub trait SplitProposer: Send + Sync {
    /// The proposed tasks for `request`, shaped but not yet checked (the domain checks them).
    ///
    /// # Errors
    /// [`ProposerFailure`] when the run fails, times out or returns no usable JSON.
    async fn propose(
        &self,
        credential: &Secret,
        request: &str,
    ) -> Result<Vec<ProposedTask>, ProposerFailure>;
}
