//! Redaction of the terminals people watch (VLT-007): a task's, and a Superagent's (SPEC-147
//! SAM-004). PTY output passes a streaming redactor before the hub's screen model and every
//! viewer; a held tail that could begin a value is released 40 ms after the output stops, and
//! snapshots are redacted whole. A value is replaced by its label fitted to the value's width, so
//! the screen keeps its layout (SPEC-239).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use unicode_width::UnicodeWidthStr as _;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::vault::redact::{PlainMatcher, StreamRedactor, fitted_label, redact_all_with};

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

/// What stands for a value on a screen: its label fitted to the value's width, so a program's
/// cursor-addressed redraws land in the cells it meant (SPEC-239, ADR-0181). The width is the
/// one the hub's `vt100` screen gives the value.
fn screen_label(name: &str, original: &[u8]) -> Vec<u8> {
    fitted_label(name, String::from_utf8_lossy(original).width()).into_bytes()
}

/// The streaming redactor of a terminal over `matcher`.
pub(crate) fn screen_redactor(matcher: Arc<PlainMatcher>) -> StreamRedactor<Arc<PlainMatcher>> {
    StreamRedactor::new(matcher, 4).with_replacement(screen_label)
}

/// A whole terminal snapshot redacted with `matcher`.
fn redact_screen(matcher: &PlainMatcher, bytes: &[u8]) -> Vec<u8> {
    redact_all_with(matcher, bytes, screen_label).0
}

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
            Ok(matcher) => redact_screen(matcher.as_ref(), bytes),
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

#[cfg(test)]
mod tests {
    use super::*;
    use yulius_domain::vault::encode::needles;

    const NAME: &str = "PIPEDREAM_PROJECT_ENVIRONMENT";
    const VALUE: &str = "production";
    const COLS: u16 = 40;

    fn matcher() -> Arc<PlainMatcher> {
        Arc::new(PlainMatcher::new(vec![(NAME.into(), needles(VALUE))]))
    }

    /// The screen's rows after `bytes`.
    fn screen(bytes: &[u8]) -> Vec<String> {
        let mut parser = vt100::Parser::new(6, COLS, 0);
        parser.process(bytes);
        parser.screen().rows(0, COLS).collect()
    }

    /// Output drawn as Claude Code draws it: two lines, then the cursor goes back up two rows and
    /// one of them is rewritten in place, and a mark is put at a column of the first line.
    fn redraw() -> Vec<u8> {
        format!(
            "allowed for {VALUE}. Yulius tracks\r\nthree allowances\r\n\x1b[2A\r\x1b[2Kdev is not {VALUE}\x1b[1B\r\x1b[2Kfine\x1b[1A\x1b[30G!"
        )
        .into_bytes()
    }

    /// The unredacted screen with each cell of the value covered by `mask`.
    fn masked(rows: &[String], mask: &str) -> Vec<String> {
        rows.iter().map(|row| row.replace(VALUE, mask)).collect()
    }

    /// `rows` hold no value and equal `plain` in every cell outside the value's cells (a
    /// beginning of the value shorter than the hold minimum may show before the mask).
    fn assert_same_layout(rows: &[String], plain: &[String], context: &str) {
        assert_eq!(rows.len(), plain.len(), "{context}");
        let cells = |row: &str| -> Vec<char> {
            let mut cells: Vec<char> = row.chars().collect();
            cells.resize(usize::from(COLS), ' ');
            cells
        };
        for (row, want) in rows.iter().zip(plain) {
            assert!(!row.contains(VALUE), "{context}: {rows:?}");
            let (got, want_cells) = (cells(row), cells(want));
            let spans: Vec<usize> = want.match_indices(VALUE).map(|(at, _)| at).collect();
            for (column, (a, b)) in got.iter().zip(&want_cells).enumerate() {
                let in_value = spans
                    .iter()
                    .any(|at| (*at..at + VALUE.len()).contains(&column));
                assert!(in_value || a == b, "{context}: column {column}: {rows:?}");
            }
        }
    }

    /// TRM-003: a redraw over a redacted value lands where it would over the value, whatever
    /// the chunking (a held tail included).
    #[test]
    fn redraws_over_a_redacted_value_keep_their_cells() {
        let raw = redraw();
        let plain = screen(&raw);
        assert_eq!(plain[0].trim_end(), "dev is not production        !");
        let expected = masked(&plain, "[secret:P]");
        assert_eq!(screen(&redact_screen(&matcher(), &raw)), expected);
        for split in 0..=raw.len() {
            let mut stream = screen_redactor(matcher());
            let mut out = stream.push(&raw[..split]);
            out.extend(stream.push(&raw[split..]));
            out.extend(stream.flush());
            let rows = screen(&out);
            assert_same_layout(&rows, &plain, &format!("split at {split}"));
            if split == 0 || split == raw.len() {
                assert_eq!(rows, expected);
            }
        }
    }

    /// Where the cursor is after `bytes` on a screen wide enough for them.
    fn cursor_after(bytes: &[u8]) -> (u16, u16) {
        let mut parser = vt100::Parser::new(4, 200, 0);
        parser.process(bytes);
        parser.screen().cursor_position()
    }

    /// Values whose width the screen counts character by character (an emoji with a skin-tone
    /// modifier, a heart with a variation selector, a control character) leave the cursor where
    /// the value would, in a whole snapshot and split at every byte.
    #[test]
    fn values_take_the_columns_the_screen_counts() {
        for value in [
            "ok 👍🏽 deploy-key",
            "love ❤️ production",
            "tab\tseparated",
            "pässwörd-geheim",
        ] {
            let matcher = Arc::new(PlainMatcher::new(vec![("KEY".into(), needles(value))]));
            let raw = format!("a {value} b|").into_bytes();
            let want = cursor_after(&raw);
            assert_eq!(
                cursor_after(&redact_screen(&matcher, &raw)),
                want,
                "{value}"
            );
            for split in 0..=raw.len() {
                let mut stream = screen_redactor(Arc::clone(&matcher));
                let mut out = stream.push(&raw[..split]);
                out.extend(stream.flush());
                out.extend(stream.push(&raw[split..]));
                out.extend(stream.flush());
                assert!(
                    !String::from_utf8_lossy(&out).contains(value),
                    "{value} at {split}"
                );
                assert_eq!(cursor_after(&out), want, "{value} split at {split}");
            }
        }
    }

    /// A value of wide characters takes the columns the screen gives it.
    #[test]
    fn wide_values_keep_their_columns() {
        let wide = "秘密の値です"; // six characters, twelve columns
        let matcher = Arc::new(PlainMatcher::new(vec![("KEY".into(), needles(wide))]));
        let out = redact_screen(&matcher, format!("a {wide} b").as_bytes());
        assert_eq!(String::from_utf8_lossy(&out), "a [secret:KEY] b");
        let out = redact_screen(&matcher, format!("{wide}{wide}").as_bytes());
        assert_eq!(String::from_utf8_lossy(&out), "[secret:KEY][secret:KEY]");
    }
}
