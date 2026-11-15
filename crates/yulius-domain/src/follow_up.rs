//! What a follow-up after an approval changed (SPEC-493 APR-001 to APR-004): the lines of product
//! code the commits after the reviewed one added and removed, counted commit by commit. Up to
//! [`FOLLOW_UP_LINES`] of them keep the approval; tests and documents do not count.

use crate::scope::{Pattern, pattern_matches};
use crate::spec_format::is_documentation_path;

/// Most product lines a follow-up may change and keep its approval.
pub const FOLLOW_UP_LINES: u32 = 20;

/// Longest changed line counted as one line; a longer one, or one with a NUL byte, makes its
/// file opaque (review round 1 of SPEC-493 fix 6).
pub const MAX_COUNTED_LINE_BYTES: usize = 1_000;

/// Folders whose files are tests.
const TEST_FOLDERS: [&str; 5] = ["tests", "test", "e2e", "__tests__", "fixtures"];

/// What counts as product code in a project (`[review]` in `yulius.toml`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProductRule {
    /// Only these count (`product`), documents and tests never; empty: every file but documents
    /// and tests.
    pub product: Vec<Pattern>,
    /// These never count (`not_product`).
    pub not_product: Vec<Pattern>,
}

impl ProductRule {
    /// Whether a change to `path` counts as product code.
    #[must_use]
    pub fn counts(&self, path: &str) -> bool {
        if self.not_product.iter().any(|p| pattern_matches(p, path)) {
            return false;
        }
        if is_documentation_path(path) || is_test_path(path) {
            return false;
        }
        self.product.is_empty() || self.product.iter().any(|p| pattern_matches(p, path))
    }
}

/// Whether `path` is a test file: in a `tests/`, `test/`, `e2e/`, `__tests__/` or `fixtures/`
/// folder, or named `*_test.*`, `test_*.*`, `*.test.*`, `*.spec.*` or `*.e2e.*`.
#[must_use]
pub fn is_test_path(path: &str) -> bool {
    let mut parts: Vec<&str> = path.split('/').collect();
    let name = parts.pop().unwrap_or_default();
    if parts.iter().any(|folder| TEST_FOLDERS.contains(folder)) {
        return true;
    }
    let Some((stem, _ext)) = name.rsplit_once('.') else {
        return false;
    };
    stem.ends_with("_test")
        || stem.starts_with("test_")
        || [".test", ".spec", ".e2e"]
            .iter()
            .any(|mark| stem.ends_with(mark) || stem.contains(&format!("{mark}.")))
}

/// The lines one commit added and removed in one file, by line number (1-based): the new file's
/// for added lines, the old file's for removed ones.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileLines {
    /// The file (its new path; a deletion's old one).
    pub path: String,
    /// Added lines.
    pub added: Vec<u32>,
    /// Removed lines.
    pub removed: Vec<u32>,
    /// In a Rust file, the test modules of the new file ([`test_module_spans`]).
    pub tests_new: Vec<(u32, u32)>,
    /// The same in the old file.
    pub tests_old: Vec<(u32, u32)>,
    /// A changed line holds a NUL byte or is very long: the count does not say what changed.
    pub opaque: bool,
}

impl FileLines {
    /// The product lines of this change: in a Rust file, lines inside a test module do not
    /// count.
    #[must_use]
    pub fn product_lines(&self) -> u32 {
        let outside = |lines: &[u32], spans: &[(u32, u32)]| {
            lines
                .iter()
                .filter(|&&line| !spans.iter().any(|&(from, to)| (from..=to).contains(&line)))
                .count()
        };
        let n = outside(&self.added, &self.tests_new) + outside(&self.removed, &self.tests_old);
        u32::try_from(n).unwrap_or(u32::MAX)
    }
}

/// The test modules of a Rust file's `text`, as 1-based inclusive line spans: a `#[cfg(test)]`
/// whose item (on its line or after further attributes) opens `mod name {`, to the first line
/// after it that starts with `}` at the attribute's indentation. Anything else, such as
/// `#[cfg(test)] mod tests;` or a single test-only item, is no span, and a module whose end is
/// not found is none either: when in doubt, a line counts (review round 1 of SPEC-493 fix 6).
#[must_use]
pub fn test_module_spans(text: &str) -> Vec<(u32, u32)> {
    let lines: Vec<&str> = text.lines().collect();
    let indent_of = |line: &str| line.len() - line.trim_start().len();
    let mut spans = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        let line = lines[at];
        let Some(rest) = line.trim_start().strip_prefix("#[cfg(test)]") else {
            at += 1;
            continue;
        };
        let indent = indent_of(line);
        let mut item = rest.trim();
        let mut item_at = at;
        while item.is_empty() || (item.starts_with("#[") && item.ends_with(']')) {
            item_at += 1;
            let Some(next) = lines.get(item_at) else {
                break;
            };
            item = next.trim();
        }
        let item = ["pub(crate) ", "pub(super) ", "pub "]
            .iter()
            .find_map(|vis| item.strip_prefix(vis))
            .unwrap_or(item);
        let opens = item
            .strip_prefix("mod ")
            .and_then(|rest| rest.strip_suffix('{'))
            .is_some_and(|name| {
                let name = name.trim();
                !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')
            });
        let end = opens
            .then(|| {
                (item_at + 1..lines.len()).find(|&k| {
                    indent_of(lines[k]) == indent && lines[k].trim_start().starts_with('}')
                })
            })
            .flatten();
        match end {
            Some(end) => {
                let span = |n: usize| u32::try_from(n + 1).unwrap_or(u32::MAX);
                spans.push((span(at), span(end)));
                at = end + 1;
            }
            None => at += 1,
        }
    }
    spans
}

/// What a follow-up changed: its product lines in total and per file, most first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FollowUp {
    /// Product lines, commit by commit.
    pub lines: u32,
    /// Per file.
    pub files: Vec<(String, u32)>,
}

impl FollowUp {
    /// Counts `changes` (every commit's files) under `rule`.
    #[must_use]
    pub fn count(changes: &[FileLines], rule: &ProductRule) -> Self {
        let mut files: Vec<(String, u32)> = Vec::new();
        for change in changes.iter().filter(|c| rule.counts(&c.path)) {
            let n = change.product_lines();
            if n == 0 {
                continue;
            }
            match files.iter_mut().find(|(path, _)| *path == change.path) {
                Some((_, total)) => *total += n,
                None => files.push((change.path.clone(), n)),
            }
        }
        files.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        Self {
            lines: files.iter().map(|(_, n)| n).sum(),
            files,
        }
    }

    /// Whether it keeps the approval.
    #[must_use]
    pub fn within_bound(&self) -> bool {
        self.lines <= FOLLOW_UP_LINES
    }

    /// "N product lines in M files: a.rs 12, b.rs 3" (at most 8 files named).
    #[must_use]
    pub fn describe(&self) -> String {
        let named: Vec<String> = self
            .files
            .iter()
            .take(8)
            .map(|(path, n)| format!("{path} {n}"))
            .collect();
        let more = self.files.len().saturating_sub(named.len());
        let files = if self.files.len() == 1 {
            "file"
        } else {
            "files"
        };
        let mut text = format!(
            "{} product {} in {} {files}",
            self.lines,
            if self.lines == 1 { "line" } else { "lines" },
            self.files.len()
        );
        if !named.is_empty() {
            text.push_str(&format!(": {}", named.join(", ")));
        }
        if more > 0 {
            text.push_str(&format!(" and {more} more"));
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(path: &str, added: &[u32], removed: &[u32]) -> FileLines {
        FileLines {
            path: path.to_owned(),
            added: added.to_vec(),
            removed: removed.to_vec(),
            ..FileLines::default()
        }
    }

    /// SPEC-493 APR-002: tests and documents never count; a project's `product` list counts
    /// only those, its `not_product` list never.
    #[test]
    fn product_code_is_every_file_but_tests_and_documents() -> Result<(), crate::scope::ScopeError>
    {
        let rule = ProductRule::default();
        for path in [
            "crates/a/src/lib.rs",
            "web/app.js",
            "Cargo.toml",
            "scripts/dev",
        ] {
            assert!(rule.counts(path), "{path}");
        }
        for path in [
            "crates/a/tests/it.rs",
            "e2e/tests/x.e2e.ts",
            "web/app.test.js",
            "web/app.spec.ts",
            "src/a_test.go",
            "test_a.py",
            "fixtures/x.json",
            "docs/x.md",
            "README.md",
            "web/__tests__/a.js",
        ] {
            assert!(!rule.counts(path), "{path}");
        }
        let only = ProductRule {
            product: vec![Pattern::parse("crates/")?],
            not_product: vec![Pattern::parse("crates/gen/")?],
        };
        assert!(only.counts("crates/a/src/lib.rs"));
        assert!(!only.counts("web/app.js"));
        assert!(!only.counts("crates/gen/x.rs"));
        // Tests and documents never count, a `product` list or not.
        assert!(!only.counts("crates/a/tests/it.rs"));
        assert!(!only.counts("crates/a/README.md"));
        Ok(())
    }

    /// SPEC-493 APR-001, APR-002: lines are counted commit by commit (a line changed twice counts
    /// twice), a Rust file's lines in its test modules do not count, and the bound is 20.
    #[test]
    fn a_follow_up_counts_product_lines_commit_by_commit() {
        let mut rust = lines("crates/a/src/lib.rs", &[3, 4, 40, 41], &[3, 39]);
        rust.tests_new = vec![(30, 50)];
        rust.tests_old = vec![(29, 49)];
        assert_eq!(rust.product_lines(), 3, "lines 40, 41 and 39 are tests");
        let changes = [
            rust,
            lines("crates/a/src/lib.rs", &[5], &[5]),
            lines("web/app.js", &[1, 2, 3], &[]),
            lines("crates/a/tests/it.rs", &(1..200).collect::<Vec<_>>(), &[]),
            lines("docs/x.md", &[1, 2], &[]),
        ];
        let follow = FollowUp::count(&changes, &ProductRule::default());
        assert_eq!(follow.lines, 8);
        assert_eq!(
            follow.files,
            [
                ("crates/a/src/lib.rs".to_owned(), 5),
                ("web/app.js".to_owned(), 3)
            ]
        );
        assert!(follow.within_bound());
        assert_eq!(
            follow.describe(),
            "8 product lines in 2 files: crates/a/src/lib.rs 5, web/app.js 3"
        );
        let big = FollowUp::count(
            &[lines("src/x.rs", &(1..=21).collect::<Vec<_>>(), &[])],
            &ProductRule::default(),
        );
        assert_eq!(big.lines, 21);
        assert!(!big.within_bound());
        assert!(
            FollowUp::count(
                &[lines("src/x.rs", &(1..=20).collect::<Vec<_>>(), &[])],
                &ProductRule::default()
            )
            .within_bound()
        );
    }

    /// Review round 1 of SPEC-493 fix 6: only a `#[cfg(test)]` module, to its closing brace, is
    /// a test span; `mod tests;`, a single item and a module never closed are not.
    #[test]
    fn only_test_modules_are_test_spans() {
        let text = "#[cfg(test)]\nmod tests;\n#[cfg(test)] fn len() {}\nfn a() {}\n\
                    #[cfg(test)]\n#[allow(dead_code)]\nmod tests {\n    fn t() {\n    }\n}\n\
                    fn b() {}\n    #[cfg(test)]\n    pub(crate) mod inner {\n    } // end\n\
                    #[cfg(test)]\nmod open {\n  fn x() {}\n";
        assert_eq!(test_module_spans(text), [(5, 10), (12, 14)]);
        assert!(test_module_spans("fn a() {}\n").is_empty());
        assert!(test_module_spans("#[cfg(test)] mod t { fn a() {} }\nfn b() {}\n").is_empty());
    }
}
