//! Exact-value redaction (SPEC-002 ADR-20, VLT-007): every occurrence of an agent secret's value
//! or of one of its encodings ([`super::encode::needles`]) becomes `[secret:NAME]`.
//!
//! Two matchers share one streaming redactor:
//! - [`PlainMatcher`] holds the needles themselves (the server: proxy responses, MCP results,
//!   the terminal, served transcripts). It knows prefixes, so a stream holds back only a tail
//!   that could begin a value.
//! - [`HashedMatcher`] holds fingerprints only — a few bits of a hash of a needle's beginning
//!   ([`needle_tag`]) to find candidates and a salted SHA-256 digest (computed by the caller's
//!   injected function) to confirm them — so code
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

/// How many bytes at a needle's beginning its tag is made from: the shortest needle.
pub const TAG_WINDOW: usize = super::encode::MIN_NEEDLE_LEN;

/// Bits of a [`TAG_WINDOW`] hash.
const HASH_BITS: u32 = 16;

/// How many bits of its beginning's hash a needle of `len` bytes publishes as its tag: 3 for the
/// shortest, one more each time the length doubles, at most 16. A tag is all a fingerprint says
/// about a needle besides its length and its salted digest, so few bits keep what a holder of
/// the fingerprints learns small (at most these bits about the first [`TAG_WINDOW`] bytes of each
/// form); a longer needle costs more to confirm, so it gets more bits and each needle costs the
/// redactor less than two hashed bytes per byte of output.
#[must_use]
pub fn tag_bits(len: usize) -> u32 {
    (3 + (len / TAG_WINDOW).max(1).ilog2()).min(HASH_BITS)
}

/// A 16-bit hash of the first [`TAG_WINDOW`] bytes of `window` (0 for a shorter one). Letters
/// count without their case and `-` and `_` as `+` and `/`, so the two hexadecimal forms of a
/// value, and its base64 and base64url forms, share a tag instead of each saying its own bits.
fn window_hash(window: &[u8]) -> u16 {
    let Some(mut word) = window
        .get(..TAG_WINDOW)
        .and_then(|w| <[u8; 8]>::try_from(w).ok())
    else {
        return 0;
    };
    for byte in &mut word {
        *byte = match *byte {
            b'-' => b'+',
            b'_' => b'/',
            other => other.to_ascii_lowercase(),
        };
    }
    let mixed = u64::from_le_bytes(word).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mixed = (mixed ^ (mixed >> 32)).wrapping_mul(0xD6E8_FEB8_6659_FD93);
    u16::try_from(mixed >> (u64::BITS - HASH_BITS)).unwrap_or(0)
}

/// The tag of `needle`: the top [`tag_bits`] bits of the hash of its first [`TAG_WINDOW`] bytes;
/// `None` for a needle shorter than that.
#[must_use]
pub fn needle_tag(needle: &[u8]) -> Option<u16> {
    (needle.len() >= TAG_WINDOW)
        .then(|| window_hash(needle) >> (HASH_BITS - tag_bits(needle.len())))
}

/// One needle as a fingerprint: its length, the tag that finds candidates and the salted digest
/// that confirms one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    /// Index into the matcher's names.
    pub secret: usize,
    /// Needle length in bytes.
    pub len: usize,
    /// [`needle_tag`] of the needle.
    pub tag: u16,
    /// The caller's salted digest of the needle.
    pub digest: [u8; 32],
}

/// A matcher over fingerprints; `verify` computes the salted digest of a candidate.
pub struct HashedMatcher<V: Fn(&[u8]) -> [u8; 32]> {
    /// Per tag width: the fingerprints of each tag.
    groups: Vec<(u32, HashMap<u16, Vec<usize>>)>,
    fingerprints: Vec<Fingerprint>,
    names: Vec<String>,
    max_len: usize,
    verify: V,
}

impl<V: Fn(&[u8]) -> [u8; 32]> HashedMatcher<V> {
    /// Builds from fingerprints; one shorter than [`TAG_WINDOW`] or of an unknown secret is
    /// left out.
    pub fn new(names: Vec<String>, fingerprints: Vec<Fingerprint>, verify: V) -> Self {
        let mut by_bits: HashMap<u32, HashMap<u16, Vec<usize>>> = HashMap::new();
        let mut max_len = 0;
        for (i, fp) in fingerprints.iter().enumerate() {
            if fp.len < TAG_WINDOW || fp.secret >= names.len() {
                continue;
            }
            max_len = max_len.max(fp.len);
            by_bits
                .entry(tag_bits(fp.len))
                .or_default()
                .entry(fp.tag)
                .or_default()
                .push(i);
        }
        let mut groups: Vec<_> = by_bits.into_iter().collect();
        groups.sort_by_key(|g| g.0);
        // By length, so a window is hashed once for the needles of one length.
        for (_, by_tag) in &mut groups {
            for bucket in by_tag.values_mut() {
                bucket.sort_by_key(|&i| fingerprints[i].len);
            }
        }
        Self {
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
        for (start, window) in data.windows(TAG_WINDOW).enumerate() {
            let hash = window_hash(window);
            for (bits, by_tag) in &self.groups {
                let Some(hits) = by_tag.get(&(hash >> (HASH_BITS - bits))) else {
                    continue;
                };
                let mut hashed: Option<(usize, [u8; 32])> = None;
                for &f in hits {
                    let fp = &self.fingerprints[f];
                    let Some(candidate) = start
                        .checked_add(fp.len)
                        .and_then(|end| data.get(start..end))
                    else {
                        continue;
                    };
                    let digest = match hashed {
                        Some((len, digest)) if len == fp.len => digest,
                        _ => (self.verify)(candidate),
                    };
                    hashed = Some((fp.len, digest));
                    if digest == fp.digest {
                        let end = start + fp.len;
                        candidates.push(Match {
                            start,
                            end,
                            secret: fp.secret,
                        });
                    }
                }
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
        let fingerprints = needles(VALUE)
            .iter()
            .filter_map(|n| {
                Some(Fingerprint {
                    secret: 0,
                    len: n.len(),
                    tag: needle_tag(n)?,
                    digest: digest(n),
                })
            })
            .collect();
        HashedMatcher::new(vec!["STRIPE_KEY".into()], fingerprints, digest as Digest)
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

    /// A tag is a few bits of the hash of a needle's first eight bytes: needles that begin
    /// alike share it whatever follows, the two cases of a hexadecimal form and the two base64
    /// alphabets share it, and no tag is wider than its length allows.
    #[test]
    fn a_tag_says_a_few_bits_about_a_needles_beginning_only() {
        assert_eq!(
            [8, 15, 16, 31, 32, 64, 1024, 16 * 1024, 1 << 20].map(tag_bits),
            [3, 3, 4, 4, 5, 6, 10, 14, 16]
        );
        assert_eq!(needle_tag(b"short"), None);
        assert_eq!(
            needle_tag(b"sk_live_AAAAAAAA"),
            needle_tag(b"sk_live_BBBBBBBB"),
            "only the first eight bytes count"
        );
        assert_eq!(
            needle_tag(b"736b5f6c6976655f"),
            needle_tag(b"736B5F6C6976655F"),
            "the two hexadecimal forms say the same"
        );
        assert_eq!(
            needle_tag(b"a+b/c+d/e+f/"),
            needle_tag(b"a-b_c-d_e-f_"),
            "base64 and base64url say the same"
        );
        for needle in needles(VALUE) {
            let tag = needle_tag(&needle).unwrap_or(u16::MAX);
            assert!(u32::from(tag) < 1 << tag_bits(needle.len()), "{tag}");
        }
        // Every tag value occurs: the hash spreads over its bits.
        let seen: BTreeSet<u16> = (0..4096u32)
            .filter_map(|i| needle_tag(format!("{i:08}").as_bytes()))
            .collect();
        assert_eq!(seen.len(), 8);
    }

    /// The corners of the search: a needle of exactly eight bytes that ends the data, data
    /// shorter than a tag's window, two secrets with one value, and two needles that begin at
    /// the same place (the longest wins).
    #[test]
    fn hashed_matchers_find_needles_at_the_corners() {
        let fingerprint = |secret: usize, needle: &[u8]| Fingerprint {
            secret,
            len: needle.len(),
            tag: needle_tag(needle).unwrap_or(0),
            digest: digest(needle),
        };
        let matcher = HashedMatcher::new(
            vec!["SHORT".into(), "TWIN".into(), "LONG".into()],
            vec![
                fingerprint(0, b"12345678"),
                fingerprint(1, b"12345678"),
                fingerprint(2, b"12345678ABCDEFGH"),
            ],
            digest as Digest,
        );
        assert!(matcher.find(b"1234567").is_empty());
        let at_the_end = matcher.find(b"key=12345678");
        assert_eq!(at_the_end.len(), 1, "{at_the_end:?}");
        assert_eq!((at_the_end[0].start, at_the_end[0].end), (4, 12));
        let longest = matcher.find(b"12345678ABCDEFGH and 12345678ABCDEFG");
        assert_eq!(
            longest
                .iter()
                .map(|m| (m.start, m.end, m.secret))
                .collect::<Vec<_>>(),
            [(0, 16, 2), (21, 29, 0)]
        );
    }

    /// A text that shares a needle's tag but not its bytes is left alone, and a fingerprint of
    /// an unknown secret or too short to tag is ignored.
    #[test]
    fn hashed_matchers_confirm_candidates_and_skip_unusable_fingerprints() {
        let matcher = hashed();
        let twin = format!("{}XXXXXXXXXXXXXXXXX", &VALUE[..TAG_WINDOW]);
        assert_eq!(twin.len(), VALUE.len());
        assert_eq!(needle_tag(twin.as_bytes()), needle_tag(VALUE.as_bytes()));
        assert!(matcher.find(twin.as_bytes()).is_empty());
        let unusable = HashedMatcher::new(
            vec!["A".into()],
            vec![
                Fingerprint {
                    secret: 1,
                    len: 9,
                    tag: 0,
                    digest: digest(b"123456789"),
                },
                Fingerprint {
                    secret: 0,
                    len: 4,
                    tag: 0,
                    digest: digest(b"1234"),
                },
            ],
            digest as Digest,
        );
        assert!(unusable.is_empty());
        assert!(unusable.find(b"1234 123456789").is_empty());
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
