//! Redaction of the terminals people watch (VLT-007): a task's, and a Superagent's (SPEC-147
//! SAM-004). PTY output passes a streaming redactor before the hub's screen model and every
//! viewer; a held tail that could begin a value is released 40 ms after the output stops, and
//! snapshots are redacted whole.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::vault::redact::{PlainMatcher, StreamRedactor, redact_all};

use crate::context::App;

/// Quiet time after which held bytes are shown.
const FLUSH_AFTER: Duration = Duration::from_millis(40);

#[derive(Default)]
struct Inner {
    /// Vault generation the redactor was built for (`None`: not built yet).
    generation: Option<u64>,
    redactor: Option<StreamRedactor<Arc<PlainMatcher>>>,
    /// Bumped by every push; a pending flush only runs if nothing came since.
    epoch: u64,
}

/// Redaction state of one terminal (cheap to clone).
#[derive(Clone, Default)]
pub struct TerminalScrub(Arc<Mutex<Inner>>);

impl App {
    /// Redacts a whole terminal snapshot for `sandbox` and forgets any held output.
    pub(crate) async fn terminal_snapshot(
        &self,
        scrub: &TerminalScrub,
        sandbox: &SandboxId,
        bytes: &[u8],
    ) -> Vec<u8> {
        if let Ok(mut inner) = scrub.0.lock()
            && let Some(redactor) = inner.redactor.as_mut()
        {
            let _ = redactor.flush();
        }
        match self.agent_secret_matcher(sandbox).await {
            Ok(matcher) => redact_all(matcher.as_ref(), bytes).0,
            Err(err) => {
                tracing::error!(error = %err, "terminal snapshot withheld: agent secrets unreadable");
                b"\r\n[Yulius: the screen is withheld until agent secrets can be read]\r\n".to_vec()
            }
        }
    }

    /// Redacts one chunk of PTY output and feeds it to the task's hub; schedules the release
    /// of a held tail.
    pub(crate) async fn terminal_output(
        self: &Arc<Self>,
        scrub: &TerminalScrub,
        task: &TaskId,
        sandbox: &SandboxId,
        seq: u64,
        bytes: &[u8],
    ) {
        let app = Arc::clone(self);
        let task = task.clone();
        self.scrub_output(scrub, sandbox, seq, bytes, move |seq, out| {
            app.runtime(&task).hub.output(seq, out);
        })
        .await;
    }

    /// Redacts one chunk of PTY output and feeds it to `sink` (a hub); schedules the release of
    /// a held tail.
    pub(crate) async fn scrub_output<F>(
        &self,
        scrub: &TerminalScrub,
        sandbox: &SandboxId,
        seq: u64,
        bytes: &[u8],
        sink: F,
    ) where
        F: Fn(u64, &[u8]) + Send + Sync + 'static,
    {
        let generation = self.vault_generation();
        let stale = scrub
            .0
            .lock()
            .map(|inner| inner.generation != Some(generation))
            .unwrap_or(true);
        if stale {
            let fresh = self.terminal_redactor(sandbox).await;
            let released = scrub.0.lock().ok().map(|mut inner| {
                let held = inner
                    .redactor
                    .as_mut()
                    .map(StreamRedactor::flush)
                    .unwrap_or_default();
                inner.redactor = fresh;
                inner.generation = Some(generation);
                held
            });
            if let Some(held) = released.filter(|h| !h.is_empty()) {
                sink(seq, &held);
            }
        }
        let (out, epoch, holding) = match scrub.0.lock() {
            Ok(mut inner) => {
                inner.epoch += 1;
                let epoch = inner.epoch;
                match inner.redactor.as_mut() {
                    Some(redactor) => {
                        let out = redactor.push(bytes);
                        (out, epoch, redactor.holding())
                    }
                    None => (bytes.to_vec(), epoch, false),
                }
            }
            Err(_) => (bytes.to_vec(), 0, false),
        };
        if !out.is_empty() {
            sink(seq, &out);
        }
        if holding {
            let scrub = scrub.clone();
            tokio::spawn(async move {
                tokio::time::sleep(FLUSH_AFTER).await;
                let held = scrub.0.lock().ok().and_then(|mut inner| {
                    (inner.epoch == epoch)
                        .then(|| inner.redactor.as_mut().map(StreamRedactor::flush))
                        .flatten()
                });
                if let Some(held) = held.filter(|h| !h.is_empty()) {
                    sink(seq, &held);
                }
            });
        }
    }
}
