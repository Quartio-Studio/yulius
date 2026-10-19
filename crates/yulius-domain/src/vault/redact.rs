//! Exact-value redaction (SPEC-002 ADR-20, VLT-007): every occurrence of an agent secret's value
//! or of one of its encodings ([`super::encode::needles`]) becomes `[secret:NAME]`.
//!
//! Two matchers share one streaming redactor:
//! - [`PlainMatcher`] holds the needles themselves (the server: proxy responses, MCP results,
//!   the terminal, served transcripts). It knows prefixes, so a stream holds back only a tail
//!   that could begin a value.
//! - [`HashedMatcher`] holds fingerprints only — a rolling hash to find candidates and a salted
//!   SHA-256 digest (computed by the caller's injected function) to confirm them — so code
//!   running inside the sandbox (the Bash wrapper, the `PostToolUse` hook) never holds a value
//!   the agent could read. Needles never contain a line break, so a stream holds back at most
//!   the last `max_len - 1` bytes of an unfinished line.

use std::collections::{BTreeSet, HashMap};

/// The replacement for a value of `name`.
#[must_use]
pub fn label(name: &str) -> String {
    format!("[secret:{name}]")
}

/// [`label`] fitted to `width` columns, for a terminal whose redraws address cells: padded with
/// `*` after the label when wider, cut and closed with `]` when narrower, `*` below two columns.
/// Names are ASCII, so the label is one column per character.
#[must_use]
pub fn fitted_label(name: &str, width: usize) -> String {
    let full = label(name);
    let columns = full.chars().count();
    if width >= columns {
        format!("{full}{}", "*".repeat(width - columns))
    } else if width >= 2 {
        let cut: String = full.chars().take(width - 1).collect();
        format!("{cut}]")
    } else {
        "*".repeat(width)
    }
}

/// What stands for `form`, a form of the secret `name`, in the output, of which a stream already
/// emitted the first `shown` bytes (a beginning shorter than its hold minimum): the replacement
/// takes the place of `form[shown..]`.
pub type Replacement = fn(name: &str, form: &[u8], shown: usize) -> Vec<u8>;

/// The [`Replacement`] of text channels: the whole [`label`].
#[must_use]
pub fn label_replacement(name: &str, _form: &[u8], _shown: usize) -> Vec<u8> {
    label(name).into_bytes()
}

/// One occurrence: `data[start..end]` is a form of secret number `secret`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    /// First byte.
    pub start: usize,
    /// One past the last byte.
    pub end: usize,
    /// Index into [`Matcher::names`].
    pub secret: usize,
}

/// Finds secret forms in bytes.
pub trait Matcher {
    /// Non-overlapping occurrences, leftmost first and the longest at each position.
    fn find(&self, data: &[u8]) -> Vec<Match>;
    /// Longest needle.
    fn max_len(&self) -> usize;
    /// Secret names, indexed by [`Match::secret`].
    fn names(&self) -> &[String];
    /// Length of the longest suffix of `data`, at least `min` bytes, that is a proper prefix of
    /// a needle; `None` when the matcher cannot know (fingerprints).
    fn pending_prefix(&self, data: &[u8], min: usize) -> Option<usize>;
    /// Whether there is nothing to look for.
    fn is_empty(&self) -> bool {
        self.max_len() == 0
    }
}

impl<M: Matcher + ?Sized> Matcher for std::sync::Arc<M> {
    fn find(&self, data: &[u8]) -> Vec<Match> {
        (**self).find(data)
    }
    fn max_len(&self) -> usize {
        (**self).max_len()
    }
    fn names(&self) -> &[String] {
        (**self).names()
    }
    fn pending_prefix(&self, data: &[u8], min: usize) -> Option<usize> {
        (**self).pending_prefix(data, min)
    }
}

/// Equality of `data`'s first `needle.len()` bytes and `needle` that reads every byte (no early
/// exit on the first difference, so the time does not tell how much of a value matched).
fn starts_with_ct(data: &[u8], needle: &[u8]) -> bool {
    if data.len() < needle.len() {
        return false;
    }
    let diff = data
        .iter()
        .zip(needle)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    std::hint::black_box(diff) == 0
}

/// Picks leftmost-longest non-overlapping matches from unordered candidates.
fn select(mut candidates: Vec<Match>) -> Vec<Match> {
    candidates.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let mut out: Vec<Match> = Vec::with_capacity(candidates.len());
    let mut last_end = 0;
    for candidate in candidates {
        if candidate.start >= last_end {
            last_end = candidate.end;
            out.push(candidate);
        }
    }
    out
}

/// Replaces `matches` (sorted, non-overlapping, inside `data`) with their labels.
#[must_use]
pub fn apply(data: &[u8], matches: &[Match], names: &[String]) -> Vec<u8> {
    apply_with(data, matches, names, label_replacement)
}

/// [`apply`] with `replace` standing for each match.
#[must_use]
pub fn apply_with(
    data: &[u8],
    matches: &[Match],
    names: &[String],
    replace: Replacement,
) -> Vec<u8> {
    splice(data, matches, names, |m, name| {
        replace(name, &data[m.start..m.end], 0)
    })
}

/// Replaces `matches` (sorted, non-overlapping, inside `data`) with what `replacement` gives.
fn splice(
    data: &[u8],
    matches: &[Match],
    names: &[String],
    mut replacement: impl FnMut(&Match, &str) -> Vec<u8>,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut at = 0;
    for m in matches {
        out.extend_from_slice(&data[at..m.start]);
        let name = names.get(m.secret).map_or("?", String::as_str);
        out.extend(replacement(m, name));
        at = m.end;
    }
    out.extend_from_slice(&data[at..]);
    out
}

/// Redacts a whole text; returns it and the names found.
pub fn redact_all(matcher: &dyn Matcher, data: &[u8]) -> (Vec<u8>, Vec<String>) {
    redact_all_with(matcher, data, label_replacement)
}

/// [`redact_all`] with `replace` standing for each match.
pub fn redact_all_with(
    matcher: &dyn Matcher,
    data: &[u8],
    replace: Replacement,
) -> (Vec<u8>, Vec<String>) {
    if matcher.is_empty() {
        return (data.to_vec(), Vec::new());
    }
    let matches = matcher.find(data);
    let used: BTreeSet<usize> = matches.iter().map(|m| m.secret).collect();
    let names = used
        .into_iter()
        .filter_map(|i| matcher.names().get(i).cloned())
        .collect();
    (apply_with(data, &matches, matcher.names(), replace), names)
}

/// [`redact_all`] for text (needles are whole UTF-8 sequences, so the result stays valid).
pub fn redact_text(matcher: &dyn Matcher, text: &str) -> (String, Vec<String>) {
    let (bytes, names) = redact_all(matcher, text.as_bytes());
    (String::from_utf8_lossy(&bytes).into_owned(), names)
}

/// A matcher over the needles themselves.
#[derive(Clone, Default)]
pub struct PlainMatcher {
    needles: Vec<(Vec<u8>, usize)>,
    by_first: HashMap<u8, Vec<usize>>,
    max_len: usize,
    names: Vec<String>,
}

impl std::fmt::Debug for PlainMatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlainMatcher")
            .field("names", &self.names)
            .field("needles", &self.needles.len())
            .finish()
    }
}

impl PlainMatcher {
    /// Builds from `(name, needles)` per secret.
    #[must_use]
    pub fn new(secrets: Vec<(String, Vec<Vec<u8>>)>) -> Self {
        let mut matcher = Self::default();
        for (index, (name, needles)) in secrets.into_iter().enumerate() {
            matcher.names.push(name);
            for needle in needles.into_iter().filter(|n| !n.is_empty()) {
                matcher.max_len = matcher.max_len.max(needle.len());
                matcher
                    .by_first
                    .entry(needle[0])
                    .or_default()
                    .push(matcher.needles.len());
                matcher.needles.push((needle, index));
            }
        }
        matcher
    }
}

impl PlainMatcher {
    /// Every needle with its secret index (the server turns them into fingerprints).
    pub fn needles(&self) -> impl Iterator<Item = (&[u8], usize)> {
        self.needles.iter().map(|(n, s)| (n.as_slice(), *s))
    }
}

impl Matcher for PlainMatcher {
    fn find(&self, data: &[u8]) -> Vec<Match> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < data.len() {
            let mut best: Option<Match> = None;
            if let Some(candidates) = self.by_first.get(&data[i]) {
                for &n in candidates {
                    let (needle, secret) = &self.needles[n];
                    if starts_with_ct(&data[i..], needle)
                        && best.is_none_or(|b| needle.len() > b.end - b.start)
                    {
                        best = Some(Match {
                            start: i,
                            end: i + needle.len(),
                            secret: *secret,
                        });
                    }
                }
            }
            match best {
                Some(m) => {
                    i = m.end;
                    out.push(m);
                }
                None => i += 1,
            }
        }
        out
    }

    fn max_len(&self) -> usize {
        self.max_len
    }

    fn names(&self) -> &[String] {
        &self.names
    }

    fn pending_prefix(&self, data: &[u8], min: usize) -> Option<usize> {
        let window = self.max_len.saturating_sub(1).min(data.len());
        let from = data.len() - window;
        for p in from..data.len() {
            let suffix = &data[p..];
            if suffix.len() < min.max(1) {
                break;
            }
            if let Some(candidates) = self.by_first.get(&data[p]) {
                let pending = candidates.iter().any(|&n| {
                    let needle = &self.needles[n].0;
                    needle.len() > suffix.len() && needle.starts_with(suffix)
                });
                if pending {
                    return Some(suffix.len());
                }
            }
        }
        Some(0)
    }
}

/// Modulus of the rolling hash (the Mersenne prime 2^61 − 1).
const MODULUS: u64 = (1 << 61) - 1;

fn mul_mod(a: u64, b: u64) -> u64 {
    let product = u128::from(a) * u128::from(b);
    let folded = (product & u128::from(MODULUS)) + (product >> 61);
    let folded = u64::try_from(folded % u128::from(MODULUS)).unwrap_or(0);
    folded % MODULUS
}

/// Polynomial rolling hash of `bytes` with `base` (each byte counts as `byte + 1`).
#[must_use]
pub fn rolling_hash(base: u64, bytes: &[u8]) -> u64 {
    let base = base % MODULUS;
    bytes
        .iter()
        .fold(0, |h, &b| (mul_mod(h, base) + u64::from(b) + 1) % MODULUS)
}

/// One needle as a fingerprint: its length, its rolling hash and the salted digest that
/// confirms a candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    /// Index into the matcher's names.
    pub secret: usize,
    /// Needle length in bytes.
    pub len: usize,
    /// [`rolling_hash`] of the needle.
    pub rolling: u64,
    /// The caller's salted digest of the needle.
    pub digest: [u8; 32],
}

/// A matcher over fingerprints; `verify` computes the salted digest of a candidate.
pub struct HashedMatcher<V: Fn(&[u8]) -> [u8; 32]> {
    base: u64,
    groups: Vec<(usize, u64, HashMap<u64, Vec<usize>>)>,
    fingerprints: Vec<Fingerprint>,
    names: Vec<String>,
    max_len: usize,
    verify: V,
}

impl<V: Fn(&[u8]) -> [u8; 32]> HashedMatcher<V> {
    /// Builds from fingerprints made with `base`.
    pub fn new(base: u64, names: Vec<String>, fingerprints: Vec<Fingerprint>, verify: V) -> Self {
        let base = base % MODULUS;
        let mut by_len: HashMap<usize, HashMap<u64, Vec<usize>>> = HashMap::new();
        let mut max_len = 0;
        for (i, fp) in fingerprints.iter().enumerate() {
            if fp.len == 0 || fp.secret >= names.len() {
                continue;
            }
            max_len = max_len.max(fp.len);
            by_len
                .entry(fp.len)
                .or_default()
                .entry(fp.rolling)
                .or_default()
                .push(i);
        }
        let mut groups: Vec<(usize, u64, HashMap<u64, Vec<usize>>)> = by_len
            .into_iter()
            .map(|(len, map)| {
                let pow = (1..len).fold(1, |p, _| mul_mod(p, base));
                (len, pow, map)
            })
            .collect();
        groups.sort_by_key(|g| g.0);
        Self {
            base,
            groups,
            fingerprints,
            names,
            max_len,
            verify,
        }
    }
}

impl<V: Fn(&[u8]) -> [u8; 32]> Matcher for HashedMatcher<V> {
    fn find(&self, data: &[u8]) -> Vec<Match> {
        let mut candidates = Vec::new();
        for (len, pow, map) in &self.groups {
            let len = *len;
            if data.len() < len {
                continue;
            }
            let mut h = rolling_hash(self.base, &data[..len]);
            let mut i = 0;
            loop {
                if let Some(hits) = map.get(&h) {
                    let window = &data[i..i + len];
                    let digest = (self.verify)(window);
                    for &f in hits {
                        let fp = &self.fingerprints[f];
                        if fp.digest == digest {
                            candidates.push(Match {
                                start: i,
                                end: i + len,
                                secret: fp.secret,
                            });
                        }
                    }
                }
                if i + len >= data.len() {
                    break;
                }
                let out = u64::from(data[i]) + 1;
                let incoming = u64::from(data[i + len]) + 1;
                h = (h + MODULUS - mul_mod(out, *pow)) % MODULUS;
                h = (mul_mod(h, self.base) + incoming) % MODULUS;
                i += 1;
            }
        }
        select(candidates)
    }

    fn max_len(&self) -> usize {
        self.max_len
    }

    fn names(&self) -> &[String] {
        &self.names
    }

    fn pending_prefix(&self, _data: &[u8], _min: usize) -> Option<usize> {
        None
    }
}

/// Redacts a byte stream chunk by chunk: a value split across chunks is still found, because a
/// tail that may begin one is held until the next chunk or [`StreamRedactor::flush`]. When a
/// beginning was already emitted (shorter than the hold minimum, or emitted by a flush), the
/// rest of the value is still replaced: the stream remembers its last emitted bytes.
pub struct StreamRedactor<M: Matcher> {
    matcher: M,
    held: Vec<u8>,
    recent: Vec<u8>,
    min_hold: usize,
    used: BTreeSet<usize>,
    replace: Replacement,
}

impl<M: Matcher> StreamRedactor<M> {
    /// A redactor; with a prefix-aware matcher only tails of at least `min_hold` bytes that
    /// begin a needle are held (a shorter beginning shows before the rest arrives, and the rest
    /// is replaced).
    pub fn new(matcher: M, min_hold: usize) -> Self {
        Self {
            matcher,
            held: Vec::new(),
            recent: Vec::new(),
            min_hold,
            used: BTreeSet::new(),
            replace: label_replacement,
        }
    }

    /// The redactor with `replace` standing for each value instead of its [`label`].
    #[must_use]
    pub fn with_replacement(mut self, replace: Replacement) -> Self {
        self.replace = replace;
        self
    }

    /// The matcher.
    pub fn matcher(&self) -> &M {
        &self.matcher
    }

    /// Whether bytes wait for more input.
    #[must_use]
    pub fn holding(&self) -> bool {
        !self.held.is_empty()
    }

    /// Names redacted so far (each once), clearing the record.
    pub fn take_used(&mut self) -> Vec<String> {
        let used = std::mem::take(&mut self.used);
        used.into_iter()
            .filter_map(|i| self.matcher.names().get(i).cloned())
            .collect()
    }

    /// Redacts what can be decided now; returns the bytes to emit.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        if self.matcher.is_empty() {
            return chunk.to_vec();
        }
        let mut data = std::mem::take(&mut self.held);
        data.extend_from_slice(chunk);
        self.process(data, false)
    }

    /// Redacts and returns everything held (end of stream, or quiet for a while).
    pub fn flush(&mut self) -> Vec<u8> {
        let held = std::mem::take(&mut self.held);
        if held.is_empty() {
            return held;
        }
        self.process(held, true)
    }

    fn process(&mut self, data: Vec<u8>, finished: bool) -> Vec<u8> {
        let offset = self.recent.len();
        let mut combined = std::mem::take(&mut self.recent);
        combined.extend_from_slice(&data);
        // Matches in data coordinates; one that began in already emitted bytes covers the
        // start of `data`. `found` keeps each one's place in `combined`.
        let found: Vec<Match> = self
            .matcher
            .find(&combined)
            .into_iter()
            .filter(|m| m.end > offset)
            .collect();
        let matches: Vec<Match> = found
            .iter()
            .map(|m| Match {
                start: m.start.saturating_sub(offset),
                end: m.end - offset,
                secret: m.secret,
            })
            .collect();
        let hold = if finished {
            0
        } else {
            match self.matcher.pending_prefix(&data, self.min_hold) {
                Some(pending) => pending,
                None => {
                    let line_start = data
                        .iter()
                        .rposition(|b| *b == b'\n' || *b == b'\r')
                        .map_or(0, |p| p + 1);
                    (data.len() - line_start).min(self.matcher.max_len().saturating_sub(1))
                }
            }
        };
        let mut cut = data.len() - hold;
        for m in &matches {
            if m.start < cut && m.end > cut {
                cut = m.start;
            }
        }
        let emitted: Vec<Match> = matches.into_iter().filter(|m| m.end <= cut).collect();
        self.used.extend(emitted.iter().map(|m| m.secret));
        let replace = self.replace;
        let out = splice(&data[..cut], &emitted, self.matcher.names(), |m, name| {
            let at = found
                .iter()
                .find(|f| f.end == m.end + offset)
                .map_or(m.start + offset, |f| f.start);
            replace(
                name,
                &combined[at..m.end + offset],
                offset.saturating_sub(at),
            )
        });
        self.held = data[cut..].to_vec();
        // Remember the last emitted bytes (a beginning of a value may be among them).
        let keep = self.matcher.max_len().saturating_sub(1);
        let mut recent = combined[..offset].to_vec();
        recent.extend_from_slice(&out);
        let drop = recent.len().saturating_sub(keep);
        self.recent = recent.split_off(drop);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::encode::{base64, needles};
    use super::*;

    const VALUE: &str = "sk_live_51HxQ9b2eZvKYlo2C";

    fn plain() -> PlainMatcher {
        PlainMatcher::new(vec![("STRIPE_KEY".into(), needles(VALUE))])
    }

    /// A stand-in digest (tests only): the needle bytes padded, keyed by a salt byte.
    fn digest(bytes: &[u8]) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (i, b) in bytes.iter().enumerate() {
            out[i % 32] = out[i % 32].wrapping_mul(31).wrapping_add(*b ^ 0x5a);
        }
        out[31] = u8::try_from(bytes.len() % 256).unwrap_or(0);
        out
    }

    type Digest = fn(&[u8]) -> [u8; 32];

    fn hashed() -> HashedMatcher<Digest> {
        let base = 1_000_003;
        let fingerprints = needles(VALUE)
            .iter()
            .map(|n| Fingerprint {
                secret: 0,
                len: n.len(),
                rolling: rolling_hash(base, n),
                digest: digest(n),
            })
            .collect();
        HashedMatcher::new(
            base,
            vec!["STRIPE_KEY".into()],
            fingerprints,
            digest as Digest,
        )
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn whole_texts_lose_every_form_of_the_value() {
        for matcher in [&plain() as &dyn Matcher, &hashed() as &dyn Matcher] {
            let text = format!(
                "key={VALUE}\nb64={}\nhex={}\njson={{\"k\":\"{VALUE}\"}}\n",
                base64(format!("x{VALUE}y").as_bytes(), false, true),
                super::super::encode::hex(VALUE.as_bytes(), false)
            );
            let (out, used) = redact_text(matcher, &text);
            assert!(!out.contains(VALUE), "{out}");
            assert!(!out.contains("NWlrX2xpdmVf"), "base64 fragment left: {out}");
            assert_eq!(used, vec!["STRIPE_KEY".to_owned()]);
            assert!(out.starts_with("key=[secret:STRIPE_KEY]\n"), "{out}");
        }
    }

    #[test]
    fn prefixes_and_other_text_are_left_alone() {
        let (out, used) = redact_text(&plain(), "sk_live_ and sk_live_51 are not the key");
        assert_eq!(out, "sk_live_ and sk_live_51 are not the key");
        assert!(used.is_empty());
    }

    /// Every split point of every chunking: the value never leaves the stream whole, and only a
    /// beginning shorter than the hold minimum can show (plain), or nothing (hashed, one line).
    #[test]
    fn values_split_across_chunks_are_still_found() {
        let text = format!("before {VALUE} after\n");
        let bytes = text.as_bytes();
        for split in 1..bytes.len() {
            for (name, min_hold) in [("plain", 4usize), ("hashed", 1)] {
                let mut out = Vec::new();
                if name == "plain" {
                    let mut stream = StreamRedactor::new(plain(), min_hold);
                    out.extend(stream.push(&bytes[..split]));
                    out.extend(stream.push(&bytes[split..]));
                    out.extend(stream.flush());
                } else {
                    let mut stream = StreamRedactor::new(hashed(), min_hold);
                    out.extend(stream.push(&bytes[..split]));
                    out.extend(stream.push(&bytes[split..]));
                    out.extend(stream.flush());
                }
                assert!(!contains(&out, VALUE.as_bytes()), "{name} split {split}");
                // At most a beginning shorter than the hold minimum shows before the label.
                let shown_prefix = out
                    .iter()
                    .zip(bytes.iter())
                    .take_while(|(a, b)| a == b)
                    .count();
                assert!(
                    shown_prefix < "before ".len() + min_hold.max(1),
                    "{name} split {split}"
                );
                let shown = String::from_utf8_lossy(&out);
                assert!(
                    shown.contains("[secret:STRIPE_KEY]"),
                    "{name} split {split}: {shown}"
                );
                assert!(shown.ends_with(" after\n"), "{name} split {split}: {shown}");
            }
        }
    }

    #[test]
    fn byte_by_byte_streams_are_redacted() {
        let text = format!("{VALUE}{VALUE}\n");
        let mut stream = StreamRedactor::new(hashed(), 1);
        let mut out = Vec::new();
        for b in text.as_bytes() {
            out.extend(stream.push(std::slice::from_ref(b)));
        }
        out.extend(stream.flush());
        assert_eq!(
            String::from_utf8_lossy(&out),
            "[secret:STRIPE_KEY][secret:STRIPE_KEY]\n"
        );
        assert_eq!(stream.take_used(), vec!["STRIPE_KEY".to_owned()]);
    }

    /// The terminal case: ordinary output passes at once; only a tail that begins the value
    /// (at least the hold minimum) waits.
    #[test]
    fn plain_streams_hold_only_possible_beginnings() {
        let mut stream = StreamRedactor::new(plain(), 4);
        assert_eq!(stream.push(b"typing s"), b"typing s".to_vec());
        assert!(!stream.holding(), "one byte is below the minimum");
        assert_eq!(stream.push(b" sk_l"), b" ".to_vec());
        assert!(stream.holding());
        assert_eq!(stream.push(b"ive?"), b"sk_live?".to_vec());
        assert!(!stream.holding());
    }

    #[test]
    fn hashed_streams_emit_finished_lines_at_once() {
        let mut stream = StreamRedactor::new(hashed(), 1);
        assert_eq!(stream.push(b"line one\nline"), b"line one\n".to_vec());
        assert!(stream.holding());
        assert_eq!(stream.flush(), b"line".to_vec());
    }

    #[test]
    fn rolling_hash_rolls() {
        let base = 257;
        let data = b"abcdefghij";
        let len = 4;
        let pow = (1..len).fold(1, |p, _| mul_mod(p, base));
        let mut h = rolling_hash(base, &data[..len]);
        for i in 0..data.len() - len {
            let out = u64::from(data[i]) + 1;
            let incoming = u64::from(data[i + len]) + 1;
            h = (h + MODULUS - mul_mod(out, pow)) % MODULUS;
            h = (mul_mod(h, base) + incoming) % MODULUS;
            assert_eq!(h, rolling_hash(base, &data[i + 1..i + 1 + len]));
        }
    }

    #[test]
    fn fitted_labels_take_the_width_they_are_given() {
        assert_eq!(fitted_label("KEY", 16), "[secret:KEY]****");
        assert_eq!(fitted_label("KEY", 12), "[secret:KEY]");
        assert_eq!(
            fitted_label("PIPEDREAM_PROJECT_ENVIRONMENT", 10),
            "[secret:P]"
        );
        assert_eq!(fitted_label("KEY", 2), "[]");
        assert_eq!(fitted_label("KEY", 1), "*");
        assert_eq!(fitted_label("KEY", 0), "");
        // A name is ASCII today; one that is not is cut by characters, never mid-character.
        assert_eq!(fitted_label("ÉTÉ", 10), "[secret:É]");
    }

    /// A replacement sees the bytes it stands for, also when a value was split across chunks.
    #[test]
    fn streams_use_their_replacement() {
        fn stars(_name: &str, form: &[u8], shown: usize) -> Vec<u8> {
            vec![b'*'; form.len() - shown]
        }
        let text = format!("a {VALUE} b\n");
        let bytes = text.as_bytes();
        for split in 0..=bytes.len() {
            let mut stream = StreamRedactor::new(plain(), 4).with_replacement(stars);
            let mut out = stream.push(&bytes[..split]);
            out.extend(stream.push(&bytes[split..]));
            out.extend(stream.flush());
            assert_eq!(out.len(), bytes.len(), "split {split}");
            assert!(!contains(&out, VALUE.as_bytes()), "split {split}");
        }
        let (whole, used) = redact_all_with(&plain(), bytes, stars);
        assert_eq!(
            whole,
            format!("a {} b\n", "*".repeat(VALUE.len())).into_bytes()
        );
        assert_eq!(used, vec!["STRIPE_KEY".to_owned()]);
    }

    #[test]
    fn empty_matchers_pass_everything() {
        let mut stream = StreamRedactor::new(PlainMatcher::default(), 4);
        assert_eq!(stream.push(b"anything"), b"anything".to_vec());
        assert!(stream.flush().is_empty());
    }
}
