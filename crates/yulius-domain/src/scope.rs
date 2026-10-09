//! Advisory scope claims over repository path globs (SPEC-001 COM-003).
//!
//! Tasks claim the parts of the repository they are about to change; overlapping claims of
//! different tasks are reported to both so they can negotiate. Claims are advisory: they never
//! block file access.
//!
//! Pattern syntax (repository-relative, `/`-separated):
//!
//! - `*` matches any characters within one path segment, `?` exactly one character;
//! - `**` as a whole segment matches zero or more segments (`src/**/test.rs`, `docs/**`);
//!   `dir/**` also matches `dir` itself;
//! - a trailing `/` claims a directory and is normalized to `/**`;
//! - every other character is literal, including `[`, `]` and `{}` (Next.js-style `[id].tsx`).
//!
//! Absolute paths, `.`/`..` segments, empty segments, backslashes and control characters are
//! rejected.

use std::fmt;

use crate::ids::TaskId;

/// Longest accepted pattern.
pub const MAX_PATTERN_BYTES: usize = 512;
/// Longest path considered by [`pattern_matches`]; longer paths never match.
pub const MAX_PATH_BYTES: usize = 4_096;
/// Longest claim note.
pub const MAX_NOTE_BYTES: usize = 1_000;

/// A rejected scope pattern or claim.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScopeError {
    /// Empty pattern.
    #[error("scope pattern is empty")]
    Empty,
    /// Longer than [`MAX_PATTERN_BYTES`].
    #[error("scope pattern is longer than {MAX_PATTERN_BYTES} bytes")]
    TooLong,
    /// Starts with `/` or a drive letter.
    #[error("scope pattern '{0}' must be repository-relative, not absolute")]
    Absolute(String),
    /// Contains a `.` or `..` segment.
    #[error("scope pattern '{0}' must not contain '.' or '..' segments")]
    DotSegment(String),
    /// Contains `\`.
    #[error("scope pattern '{0}' must use '/' as separator, not '\\'")]
    Backslash(String),
    /// Contains `//`.
    #[error("scope pattern '{0}' has an empty path segment")]
    EmptySegment(String),
    /// `**` mixed with other characters in one segment.
    #[error("scope pattern '{0}': '**' must be a whole path segment, as in 'src/**/x.rs'")]
    DoubleStarInSegment(String),
    /// Contains a control character.
    #[error("scope pattern contains a control character")]
    ControlCharacter,
    /// Note longer than [`MAX_NOTE_BYTES`].
    #[error("scope claim note is longer than {MAX_NOTE_BYTES} bytes")]
    NoteTooLong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
    Literal(char),
    One,
    Many,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    AnyDepth,
    Glob(Vec<Token>),
}

/// A validated, normalized repository path glob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    text: String,
    segments: Vec<Segment>,
}

impl Pattern {
    /// Validates and normalizes a pattern.
    ///
    /// # Errors
    /// Returns [`ScopeError`] for empty, oversized, absolute or malformed patterns.
    ///
    /// # Examples
    /// ```
    /// use yulius_domain::scope::{Pattern, pattern_matches};
    /// let pattern = Pattern::parse("src/api/")?;
    /// assert_eq!(pattern.as_str(), "src/api/**");
    /// assert!(pattern_matches(&pattern, "src/api/routes/cart.rs"));
    /// assert!(Pattern::parse("../secrets").is_err());
    /// # Ok::<(), yulius_domain::scope::ScopeError>(())
    /// ```
    pub fn parse(text: &str) -> Result<Self, ScopeError> {
        if text.is_empty() {
            return Err(ScopeError::Empty);
        }
        if text.len() > MAX_PATTERN_BYTES {
            return Err(ScopeError::TooLong);
        }
        if text.chars().any(char::is_control) {
            return Err(ScopeError::ControlCharacter);
        }
        if text.contains('\\') {
            return Err(ScopeError::Backslash(text.to_owned()));
        }
        let bytes = text.as_bytes();
        let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
        if text.starts_with('/') || drive {
            return Err(ScopeError::Absolute(text.to_owned()));
        }
        let body = match text.strip_suffix('/') {
            Some(dir) => format!("{dir}/**"),
            None => text.to_owned(),
        };
        let mut segments = Vec::new();
        for part in body.split('/') {
            match part {
                "" => return Err(ScopeError::EmptySegment(text.to_owned())),
                "." | ".." => return Err(ScopeError::DotSegment(text.to_owned())),
                "**" => {
                    if segments.last() != Some(&Segment::AnyDepth) {
                        segments.push(Segment::AnyDepth);
                    }
                }
                glob if glob.contains("**") => {
                    return Err(ScopeError::DoubleStarInSegment(text.to_owned()));
                }
                glob => segments.push(Segment::Glob(
                    glob.chars()
                        .map(|c| match c {
                            '*' => Token::Many,
                            '?' => Token::One,
                            other => Token::Literal(other),
                        })
                        .collect(),
                )),
            }
        }
        let text = segments
            .iter()
            .map(|segment| match segment {
                Segment::AnyDepth => "**".to_owned(),
                Segment::Glob(tokens) => tokens
                    .iter()
                    .map(|token| match token {
                        Token::Literal(c) => *c,
                        Token::One => '?',
                        Token::Many => '*',
                    })
                    .collect(),
            })
            .collect::<Vec<_>>()
            .join("/");
        Ok(Self { text, segments })
    }

    /// The normalized pattern text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// Whether `path` (repository-relative, `/`-separated; one trailing `/` allowed) matches.
/// Invalid paths (absolute, `..`, empty segments, oversized) never match.
#[must_use]
pub fn pattern_matches(pattern: &Pattern, path: &str) -> bool {
    let Some(segments) = path_segments(path) else {
        return false;
    };
    let pat = &pattern.segments;
    // matched[i][j]: pat[i..] matches segments[j..]
    let width = segments.len() + 1;
    let mut matched = vec![false; (pat.len() + 1) * width];
    matched[pat.len() * width + segments.len()] = true;
    for i in (0..pat.len()).rev() {
        for j in (0..=segments.len()).rev() {
            matched[i * width + j] = match &pat[i] {
                Segment::AnyDepth => {
                    matched[(i + 1) * width + j]
                        || (j < segments.len() && matched[i * width + j + 1])
                }
                Segment::Glob(tokens) => {
                    j < segments.len()
                        && glob_matches(tokens, segments[j])
                        && matched[(i + 1) * width + j + 1]
                }
            };
        }
    }
    matched[0]
}

/// Whether some valid path could match both patterns. Exact for this glob language, hence
/// conservative: it never reports "no overlap" when an overlapping path exists.
#[must_use]
pub fn patterns_overlap(a: &Pattern, b: &Pattern) -> bool {
    let (a, b) = (&a.segments, &b.segments);
    let width = b.len() + 1;
    // overlap[i][j]: a[i..] and b[j..] share a match
    let mut overlap = vec![false; (a.len() + 1) * width];
    for i in (0..=a.len()).rev() {
        for j in (0..=b.len()).rev() {
            let value = if i == a.len() && j == b.len() {
                true
            } else if i < a.len() && a[i] == Segment::AnyDepth {
                overlap[(i + 1) * width + j] || (j < b.len() && overlap[i * width + j + 1])
            } else if j < b.len() && b[j] == Segment::AnyDepth {
                overlap[i * width + j + 1] || (i < a.len() && overlap[(i + 1) * width + j])
            } else if let (Some(Segment::Glob(x)), Some(Segment::Glob(y))) = (a.get(i), b.get(j)) {
                globs_overlap(x, y) && overlap[(i + 1) * width + j + 1]
            } else {
                false
            };
            overlap[i * width + j] = value;
        }
    }
    overlap[0]
}

/// One task's advisory claim on part of the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeClaim {
    /// The claiming task.
    pub task: TaskId,
    /// The claimed paths.
    pub pattern: Pattern,
    /// Why the task needs these paths (may be empty).
    pub note: String,
}

impl ScopeClaim {
    /// Builds a claim, trimming the note.
    ///
    /// # Errors
    /// Returns [`ScopeError::NoteTooLong`] when the trimmed note exceeds [`MAX_NOTE_BYTES`].
    pub fn new(task: TaskId, pattern: Pattern, note: &str) -> Result<Self, ScopeError> {
        let note = note.trim();
        if note.len() > MAX_NOTE_BYTES {
            return Err(ScopeError::NoteTooLong);
        }
        Ok(Self {
            task,
            pattern,
            note: note.to_owned(),
        })
    }
}

/// Existing claims of *other* tasks that overlap `new`, in their original order.
#[must_use]
pub fn conflicts<'a>(new: &ScopeClaim, existing: &'a [ScopeClaim]) -> Vec<&'a ScopeClaim> {
    existing
        .iter()
        .filter(|claim| claim.task != new.task && patterns_overlap(&claim.pattern, &new.pattern))
        .collect()
}

fn path_segments(path: &str) -> Option<Vec<&str>> {
    let path = path.strip_suffix('/').unwrap_or(path);
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || path.starts_with('/')
        || path.contains('\\')
        || path.chars().any(char::is_control)
    {
        return None;
    }
    let segments: Vec<&str> = path.split('/').collect();
    if segments
        .iter()
        .any(|segment| matches!(*segment, "" | "." | ".."))
    {
        return None;
    }
    Some(segments)
}

fn glob_matches(tokens: &[Token], text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let (mut t, mut c) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while c < chars.len() {
        match tokens.get(t) {
            Some(Token::Many) => {
                backtrack = Some((t, c));
                t += 1;
            }
            Some(Token::One) => {
                t += 1;
                c += 1;
            }
            Some(Token::Literal(expected)) if *expected == chars[c] => {
                t += 1;
                c += 1;
            }
            _ => match backtrack {
                Some((star, consumed)) => {
                    t = star + 1;
                    c = consumed + 1;
                    backtrack = Some((star, consumed + 1));
                }
                None => return false,
            },
        }
    }
    tokens[t.min(tokens.len())..]
        .iter()
        .all(|token| *token == Token::Many)
}

/// Whether two single-segment globs share a non-empty match.
fn globs_overlap(a: &[Token], b: &[Token]) -> bool {
    let width = b.len() + 1;
    let mut overlap = vec![false; (a.len() + 1) * width];
    for i in (0..=a.len()).rev() {
        for j in (0..=b.len()).rev() {
            let value = match (a.get(i), b.get(j)) {
                (None, None) => true,
                (Some(Token::Many), _) => {
                    overlap[(i + 1) * width + j] || (j < b.len() && overlap[i * width + j + 1])
                }
                (_, Some(Token::Many)) => {
                    overlap[i * width + j + 1] || (i < a.len() && overlap[(i + 1) * width + j])
                }
                (Some(x), Some(y)) => tokens_compatible(*x, *y) && overlap[(i + 1) * width + j + 1],
                _ => false,
            };
            overlap[i * width + j] = value;
        }
    }
    overlap[0]
}

fn tokens_compatible(a: Token, b: Token) -> bool {
    match (a, b) {
        (Token::Literal(x), Token::Literal(y)) => x == y,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic xorshift generator for randomized tests (no extra dependency).
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }

        fn below(&mut self, bound: usize) -> usize {
            usize::try_from(self.next() % u64::try_from(bound.max(1)).unwrap_or(1)).unwrap_or(0)
        }

        /// Text made of `pieces`, random bytes (lossy UTF-8) and multi-byte characters.
        fn text(&mut self, pieces: &[&str], max_parts: usize) -> String {
            let mut out = String::new();
            for _ in 0..self.below(max_parts + 1) {
                match self.below(10) {
                    0 => {
                        let bytes: Vec<u8> = (0..self.below(8))
                            .map(|_| (self.next() & 0xff) as u8)
                            .collect();
                        out.push_str(&String::from_utf8_lossy(&bytes));
                    }
                    1 => out
                        .push_str(["é", "😀", "\u{0}", "\r", "\t", "ﬀ", "\u{200b}"][self.below(7)]),
                    _ => out.push_str(pieces[self.below(pieces.len())]),
                }
            }
            out
        }
    }

    /// Randomized patterns (#76): parsing never panics, overlap is symmetric, and two patterns
    /// that both match a concrete path are reported as overlapping.
    #[test]
    fn random_patterns_overlap_symmetrically_and_soundly() {
        let pieces = [
            "**", "*", "?", "/", "a", "b", "é", "[id]", ".", "..", "", "\\", "{x}", "a*", "*b",
        ];
        let paths = [
            "a", "b", "a/b", "a/a/b", "é/[id]", "a/b/c", "{x}/a", "ab", "aab/b",
        ];
        let mut rng = Rng(0x1234_5678_9abc_def1);
        for _ in 0..3_000 {
            let (ta, tb) = (rng.text(&pieces, 6), rng.text(&pieces, 6));
            let (Ok(a), Ok(b)) = (Pattern::parse(&ta), Pattern::parse(&tb)) else {
                continue;
            };
            let ab = patterns_overlap(&a, &b);
            assert_eq!(ab, patterns_overlap(&b, &a), "{ta:?} vs {tb:?}");
            for path in paths {
                if pattern_matches(&a, path) && pattern_matches(&b, path) {
                    assert!(ab, "{ta:?} and {tb:?} both match {path:?}");
                }
            }
        }
    }

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn p(text: &str) -> Result<Pattern, ScopeError> {
        Pattern::parse(text)
    }

    fn matches(pattern: &str, path: &str) -> Result<bool, ScopeError> {
        Ok(pattern_matches(&p(pattern)?, path))
    }

    fn overlap(a: &str, b: &str) -> Result<bool, ScopeError> {
        let (a, b) = (p(a)?, p(b)?);
        let forward = patterns_overlap(&a, &b);
        assert_eq!(
            forward,
            patterns_overlap(&b, &a),
            "overlap must be symmetric"
        );
        Ok(forward)
    }

    #[test]
    fn literal_star_question_and_double_star() -> TestResult {
        assert!(matches("src/main.rs", "src/main.rs")?);
        assert!(!matches("src/main.rs", "src/main.rs.bak")?);
        assert!(matches("src/*.rs", "src/lib.rs")?);
        assert!(
            !matches("src/*.rs", "src/api/lib.rs")?,
            "* stays within a segment"
        );
        assert!(matches("src/?.rs", "src/a.rs")?);
        assert!(!matches("src/?.rs", "src/ab.rs")?);
        assert!(
            matches("src/**/test.rs", "src/test.rs")?,
            "** matches zero segments"
        );
        assert!(matches("src/**/test.rs", "src/a/b/c/test.rs")?);
        assert!(matches("**/*.md", "README.md")?);
        assert!(matches("**/*.md", "docs/specs/x.md")?);
        assert!(matches("docs/**", "docs")?);
        assert!(matches("docs/**", "docs/a/b")?);
        assert!(!matches("docs/**", "documentation/a")?);
        assert!(matches("**", "anything/at/all")?);
        assert!(matches("*a*b*", "xaybz")?);
        assert!(!matches("*a*b*", "xbya")?);
        assert!(matches("app/[id]/page.tsx", "app/[id]/page.tsx")?);
        assert!(
            !matches("app/[id]/page.tsx", "app/i/page.tsx")?,
            "brackets are literal"
        );
        assert!(matches("src/żółw.rs", "src/żółw.rs")?);
        assert!(
            matches("src/?ółw.rs", "src/żółw.rs")?,
            "? is one character, not byte"
        );
        Ok(())
    }

    #[test]
    fn normalization() -> TestResult {
        assert_eq!(p("src/api/")?.as_str(), "src/api/**");
        assert_eq!(p("a/**/**/b")?.as_str(), "a/**/b");
        assert!(matches("src/api/", "src/api/handlers/cart.rs")?);
        assert!(matches("src/api/", "src/api/")?);
        Ok(())
    }

    #[test]
    fn rejects_unsafe_or_malformed_patterns() {
        assert_eq!(Pattern::parse(""), Err(ScopeError::Empty));
        assert!(matches!(
            Pattern::parse("/etc/passwd"),
            Err(ScopeError::Absolute(_))
        ));
        assert!(matches!(
            Pattern::parse("C:/x"),
            Err(ScopeError::Absolute(_))
        ));
        assert!(matches!(
            Pattern::parse("../x"),
            Err(ScopeError::DotSegment(_))
        ));
        assert!(matches!(
            Pattern::parse("a/../b"),
            Err(ScopeError::DotSegment(_))
        ));
        assert!(matches!(
            Pattern::parse("./a"),
            Err(ScopeError::DotSegment(_))
        ));
        assert!(matches!(
            Pattern::parse("a//b"),
            Err(ScopeError::EmptySegment(_))
        ));
        assert!(matches!(
            Pattern::parse("a\\b"),
            Err(ScopeError::Backslash(_))
        ));
        assert!(matches!(
            Pattern::parse("src/**.rs"),
            Err(ScopeError::DoubleStarInSegment(_))
        ));
        assert_eq!(Pattern::parse("a\nb"), Err(ScopeError::ControlCharacter));
        assert_eq!(Pattern::parse(&"a".repeat(513)), Err(ScopeError::TooLong));
    }

    #[test]
    fn invalid_paths_never_match() -> TestResult {
        let all = p("**")?;
        for path in ["", "/etc/passwd", "../x", "a//b", "a/./b", "a\\b"] {
            assert!(!pattern_matches(&all, path), "{path:?}");
        }
        assert!(!pattern_matches(&all, &"a/".repeat(3_000)));
        Ok(())
    }

    #[test]
    fn overlap_cases() -> TestResult {
        assert!(overlap("src/api/**", "src/api/cart.rs")?);
        assert!(overlap("src/**", "**/*.rs")?);
        assert!(overlap("src/*.rs", "src/lib.*")?);
        assert!(overlap("src/a*", "src/*b")?);
        assert!(overlap("**", "docs/x.md")?);
        assert!(
            overlap("src/api/", "src/api")?,
            "directory claim covers the directory"
        );
        assert!(overlap("a/**/z", "a/b/**")?);
        assert!(overlap("?", "*")?);
        assert!(!overlap("src/api/**", "src/web/**")?);
        assert!(!overlap("src/*.rs", "src/*.ts")?);
        assert!(
            !overlap("src/*.rs", "src/api/*.rs")?,
            "segment counts differ"
        );
        assert!(!overlap("docs/a.md", "docs/b.md")?);
        assert!(!overlap("a?", "abc")?);
        assert!(!overlap("x*", "y*")?);
        assert!(!overlap("src/**/a.rs", "lib/**")?);
        Ok(())
    }

    #[test]
    fn overlap_agrees_with_sampled_paths() -> TestResult {
        // When some sample path matches both patterns, overlap must be reported.
        let patterns = [
            "src/**",
            "src/*.rs",
            "**/*.md",
            "docs/**",
            "src/api/*",
            "**/test?.rs",
            "*",
            "src/api/",
            "README.md",
            "**/mod.rs",
        ];
        let paths = [
            "src/lib.rs",
            "src/api/mod.rs",
            "docs/a.md",
            "README.md",
            "src/test1.rs",
            "docs/specs/x.md",
            "Cargo.toml",
            "src/api/cart.rs",
            "src/mod.rs",
        ];
        for a in patterns {
            for b in patterns {
                let (pa, pb) = (p(a)?, p(b)?);
                let witnessed = paths
                    .iter()
                    .any(|path| pattern_matches(&pa, path) && pattern_matches(&pb, path));
                if witnessed {
                    assert!(patterns_overlap(&pa, &pb), "{a} vs {b}");
                }
            }
        }
        Ok(())
    }

    #[test]
    fn conflicts_ignore_same_task_and_keep_order() -> TestResult {
        let one = TaskId::parse("task-1")?;
        let two = TaskId::parse("task-2")?;
        let three = TaskId::parse("task-3")?;
        let existing = vec![
            ScopeClaim::new(two.clone(), p("src/api/**")?, "checkout API")?,
            ScopeClaim::new(one.clone(), p("src/**")?, "own earlier claim")?,
            ScopeClaim::new(three.clone(), p("docs/**")?, "")?,
            ScopeClaim::new(three, p("src/api/cart.rs")?, "cart")?,
        ];
        let new = ScopeClaim::new(one, p("src/api/")?, "  refactor  ")?;
        assert_eq!(new.note, "refactor");
        let found: Vec<&str> = conflicts(&new, &existing)
            .iter()
            .map(|claim| claim.note.as_str())
            .collect();
        assert_eq!(found, vec!["checkout API", "cart"]);
        let other = TaskId::parse("task-4")?;
        assert!(matches!(
            ScopeClaim::new(other, p("x")?, &"n".repeat(1_001)),
            Err(ScopeError::NoteTooLong)
        ));
        Ok(())
    }
}
