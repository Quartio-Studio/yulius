//! Redaction of the terminals people watch (VLT-007): a task's, and a Superagent's (SPEC-147
//! SAM-004). PTY output passes a streaming redactor before the hub's screen model and every
//! viewer; a held tail that could begin a value is released 40 ms after the output stops, and
//! snapshots are redacted whole. A value is replaced by its label fitted to the value's width, so
//! the screen keeps its layout (SPEC-239).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use unicode_width::UnicodeWidthChar as _;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::vault::redact::{PlainMatcher, StreamRedactor, fitted_label, redact_all_with};

use crate::context::App;
use crate::error::AppResult;

/// Quiet time after which held bytes are shown.
const FLUSH_AFTER: Duration = Duration::from_millis(40);

#[derive(Default)]
struct Inner {
    /// Vault generation the redactor was built for (`None`: not built yet).
    generation: Option<u64>,
    redactor: Option<StreamRedactor<Arc<PlainMatcher>>>,
    /// Bumped by every push; a pending flush only runs if nothing came since.
    epoch: u64,
    /// The agent secrets could not be read: output is withheld until they can (VLT-007: never
    /// shown unredacted). The generation that failed and when to ask for it again.
    failed: Option<(u64, Instant)>,
}

/// What a terminal shows in place of output it cannot check for agent secrets.
const WITHHELD: &[u8] = b"\r\n[Yulius: the screen is withheld until agent secrets can be read]\r\n";

/// How long a terminal waits before it asks again for secrets that could not be read (a change
/// of a secret asks at once): a secret that no longer opens fails every time, and every chunk
/// of output would otherwise read and open all of them.
const RETRY_AFTER: Duration = Duration::from_secs(1);

impl TerminalScrub {
    /// Whether the terminal's output is withheld: what its screen shows is then not what the
    /// program drew, so nothing may be read off it or typed by it (the supervisor's waits).
    pub(crate) fn withheld(&self) -> bool {
        self.0.lock().map_or(true, |inner| inner.failed.is_some())
    }

    /// Records how the secrets were read for a whole screen: a failure withholds the stream
    /// that follows too (no second notice: the screen says it), a success after a failure makes
    /// the stream load its redactor anew.
    fn screen_loaded(&self, generation: u64, readable: bool, now: Instant) {
        let Ok(mut inner) = self.0.lock() else {
            return;
        };
        if let Some(redactor) = inner.redactor.as_mut() {
            let _ = redactor.flush();
        }
        if readable {
            if inner.failed.take().is_some() {
                inner.generation = None;
            }
        } else {
            inner.redactor = None;
            inner.generation = None;
            inner.failed = Some((generation, now + RETRY_AFTER));
        }
    }

    /// Whether the redactor has to be loaded for the vault's `generation` before the next
    /// chunk: it was built for another one, or its last load failed and it is time to ask again.
    fn needs_load(&self, generation: u64, now: Instant) -> bool {
        self.0.lock().map_or(true, |inner| match inner.failed {
            Some((failed, retry_at)) => failed != generation || now >= retry_at,
            None => inner.generation != Some(generation),
        })
    }

    /// Takes the redactor loaded for the vault's `generation` and returns what to show now:
    /// what the old redactor still held, and, when the load failed, the notice (once per
    /// failure, however long it lasts). Output is withheld until a load succeeds.
    fn loaded(
        &self,
        generation: u64,
        fresh: AppResult<Option<StreamRedactor<Arc<PlainMatcher>>>>,
        now: Instant,
    ) -> Vec<u8> {
        let Ok(mut inner) = self.0.lock() else {
            return Vec::new();
        };
        let mut show = inner
            .redactor
            .as_mut()
            .map(StreamRedactor::flush)
            .unwrap_or_default();
        match fresh {
            Ok(redactor) => {
                if inner.failed.take().is_some() {
                    tracing::info!("agent secrets can be read again: the terminal shows output");
                }
                inner.redactor = redactor;
                inner.generation = Some(generation);
            }
            Err(err) => {
                inner.redactor = None;
                inner.generation = None;
                if inner.failed.is_none() {
                    tracing::error!(error = %err, "terminal output withheld: agent secrets unreadable");
                    show.extend_from_slice(WITHHELD);
                }
                inner.failed = Some((generation, now + RETRY_AFTER));
            }
        }
        show
    }

    /// One chunk through the redactor: what to show, the push's epoch and whether a tail is
    /// held. Nothing while output is withheld, or when the state cannot be read.
    fn push(&self, bytes: &[u8]) -> (Vec<u8>, u64, bool) {
        let Ok(mut inner) = self.0.lock() else {
            return (Vec::new(), 0, false);
        };
        inner.epoch += 1;
        let epoch = inner.epoch;
        if inner.failed.is_some() {
            return (Vec::new(), epoch, false);
        }
        match inner.redactor.as_mut() {
            Some(redactor) => {
                let out = redactor.push(bytes);
                (out, epoch, redactor.holding())
            }
            None => (bytes.to_vec(), epoch, false),
        }
    }
}

/// Redaction state of one terminal (cheap to clone).
#[derive(Clone, Default)]
pub struct TerminalScrub(Arc<Mutex<Inner>>);

/// What stands for a value on a screen: its label fitted to the columns the value takes past
/// what was already shown, so a program's cursor-addressed redraws land in the cells it meant
/// (SPEC-239, ADR-0181).
fn screen_label(name: &str, form: &[u8], shown: usize) -> Vec<u8> {
    let width = screen_width(form).saturating_sub(drawn_width(&form[..shown.min(form.len())]));
    fitted_label(name, width).into_bytes()
}

/// Columns `bytes` take on the hub's `vt100` screen, which adds up each character's width (no
/// grapheme rules) and draws no control character below 256.
fn screen_width(bytes: &[u8]) -> usize {
    String::from_utf8_lossy(bytes)
        .chars()
        .map(|c| c.width().unwrap_or(usize::from(u32::from(c) >= 256)))
        .sum()
}

/// Columns a shown beginning took: its whole characters (an unfinished one is dropped by the
/// screen once the label's first byte comes).
fn drawn_width(shown: &[u8]) -> usize {
    let whole = match std::str::from_utf8(shown) {
        Ok(_) => shown.len(),
        Err(err) => err.valid_up_to(),
    };
    screen_width(&shown[..whole])
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
        let generation = self.vault_generation();
        let matcher = self.agent_secret_matcher(sandbox).await;
        scrub.screen_loaded(generation, matcher.is_ok(), Instant::now());
        match matcher {
            Ok(matcher) => redact_screen(matcher.as_ref(), bytes),
            Err(err) => {
                tracing::error!(error = %err, "terminal snapshot withheld: agent secrets unreadable");
                WITHHELD.to_vec()
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
        if scrub.needs_load(generation, Instant::now()) {
            let fresh = self.terminal_redactor(sandbox).await;
            let show = scrub.loaded(generation, fresh, Instant::now());
            if !show.is_empty() {
                sink(seq, &show);
            }
        }
        let (out, epoch, holding) = scrub.push(bytes);
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

    /// VLT-007 (SPEC-483 V4): when the agent secrets cannot be read for the terminal, its
    /// output is withheld with one notice, never shown unredacted; it asks again after a
    /// moment or when a secret changes, not at every chunk; once they can be read the output
    /// flows, redacted.
    #[test]
    fn output_is_withheld_while_agent_secrets_cannot_be_read() {
        let scrub = TerminalScrub::default();
        let failed = || Err(crate::error::AppError::Internal("store".into()));
        let text = format!("TOKEN={VALUE}\r\n");
        let start = Instant::now();
        let soon = start + RETRY_AFTER / 2;
        let later = start + RETRY_AFTER;

        assert!(scrub.needs_load(1, start), "nothing is loaded yet");
        assert_eq!(scrub.loaded(1, failed(), start), WITHHELD);
        assert_eq!(scrub.push(text.as_bytes()).0, b"");
        // The next chunks do not ask again (a secret that never opens would be read and
        // opened at each), until a moment passed or a secret changed.
        assert!(!scrub.needs_load(1, soon));
        assert!(scrub.needs_load(2, soon), "a secret changed");
        assert!(scrub.needs_load(1, later));
        assert_eq!(
            scrub.loaded(1, failed(), later),
            b"",
            "the notice shows once"
        );
        assert_eq!(scrub.push(text.as_bytes()).0, b"");
        assert!(!scrub.needs_load(1, later + RETRY_AFTER / 2));

        let again = later + RETRY_AFTER;
        assert!(scrub.needs_load(1, again));
        assert_eq!(
            scrub.loaded(1, Ok(Some(screen_redactor(matcher()))), again),
            b""
        );
        assert!(!scrub.needs_load(1, again + RETRY_AFTER));
        let shown = scrub.push(text.as_bytes()).0;
        let shown = String::from_utf8_lossy(&shown);
        assert!(
            shown.starts_with("TOKEN=[secret:") && !shown.contains(VALUE),
            "{shown}"
        );
        assert!(!scrub.withheld());
        // A later failure says so again.
        assert_eq!(scrub.loaded(2, failed(), again), WITHHELD);
        assert!(scrub.withheld());
        // Without agent secrets there is nothing to redact: the output passes.
        assert_eq!(scrub.loaded(3, Ok(None), again), b"");
        assert_eq!(scrub.push(b"plain\r\n").0, b"plain\r\n");

        // A whole screen that could not be checked withholds the stream after it, without a
        // second notice (the screen says it); one that could makes the stream load anew.
        scrub.screen_loaded(4, false, again);
        assert!(scrub.withheld());
        assert_eq!(scrub.push(text.as_bytes()).0, b"");
        assert_eq!(scrub.loaded(4, failed(), again + RETRY_AFTER), b"");
        scrub.screen_loaded(4, true, again + RETRY_AFTER);
        assert!(!scrub.withheld());
        assert!(scrub.needs_load(4, again + RETRY_AFTER));
    }

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
    /// modifier, a heart with a variation selector) and a value cut inside a character leave
    /// the cursor where the value would, in a whole snapshot and split at every byte. (A tab
    /// moves to a tab stop no printable mask can reach; agent secrets hold none in practice.)
    #[test]
    fn values_take_the_columns_the_screen_counts() {
        for value in ["ok 👍🏽 deploy-key", "love ❤️ production", "pässwörd-geheim"] {
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
