//! What a follow-up after an approval changed (SPEC-493 APR-001 to APR-004): the lines of product
//! code the commits after the reviewed one added and removed, counted commit by commit. Up to
//! [`FOLLOW_UP_LINES`] of them keep the approval; tests and documents do not count.

use crate::scope::{Pattern, pattern_matches};
use crate::spec_format::is_documentation_path;

/// Most product lines a follow-up may change and keep its approval.
pub const FOLLOW_UP_LINES: u32 = 20;

/// Longest changed line counted as one line; a longer one, or one with a NUL byte, makes its
/// file opaque (review round 1 of SPEC-493 fix 6).
pub const MAX_COUNTED_LINE_BYTES: usize = 200;

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
    /// Their text, as `added` (empty when unknown).
    pub added_text: Vec<String>,
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

    /// The text of the product lines it added (outside test modules).
    fn product_text(&self) -> impl Iterator<Item = &str> {
        self.added
            .iter()
            .zip(&self.added_text)
            .filter(|&(line, _)| {
                !self
                    .tests_new
                    .iter()
                    .any(|&(from, to)| (from..=to).contains(line))
            })
            .map(|(_, text)| text.as_str())
    }
}

/// Outer attributes no macro stands behind: a module that has only these is compiled as written
/// (`cfg_attr` is not one: it can apply any attribute).
const BUILT_IN_ATTRIBUTES: [&str; 11] = [
    "cfg",
    "allow",
    "warn",
    "deny",
    "forbid",
    "expect",
    "doc",
    "path",
    "rustfmt",
    "clippy",
    "macro_use",
];

/// The test modules of a Rust file's `text`, as 1-based inclusive line spans: a `#[cfg(test)]`
/// attribute, first of its item, whose item after any further attributes is `mod name {`, to
/// that module's closing brace, at the top of the file or inside modules with built-in
/// attributes only (not in a macro's input, nor in a module an attribute macro rewrites, where
/// the macro may drop the attribute and keep the body). The text is read as Rust tokens: comments, strings (raw ones too) and
/// character literals are skipped, so nothing in them opens or closes a span (review round 2 of
/// SPEC-493 fix 6). Anything else, such as `#[cfg(test)] mod tests;` or a single test-only item,
/// is no span, and a text that does not lex (an unclosed comment or string) has none: when in
/// doubt, a line counts.
#[must_use]
pub fn test_module_spans(text: &str) -> Vec<(u32, u32)> {
    let Some(code) = code_only(text) else {
        return Vec::new();
    };
    let line_of = |at: usize| {
        let line = code[..at].iter().filter(|&&(c, _)| c == '\n').count() + 1;
        u32::try_from(line).unwrap_or(u32::MAX)
    };
    let tokens: Vec<usize> = (0..code.len())
        .filter(|&k| !code[k].0.is_whitespace())
        .collect();
    let char_at = |t: usize| tokens.get(t).map(|&k| code[k].0);
    // The token index after a balanced `[ … ]` starting at `t` (an attribute's body).
    let after_brackets = |mut t: usize| {
        let mut depth = 0usize;
        while let Some(c) = char_at(t) {
            t += 1;
            match c {
                '[' => depth += 1,
                ']' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(t);
                    }
                }
                _ => {}
            }
        }
        None
    };
    let word_at = |t: usize| -> Option<(String, usize)> {
        let start = *tokens.get(t)?;
        let mut end = start;
        while code
            .get(end)
            .is_some_and(|&(c, _)| c.is_alphanumeric() || c == '_')
        {
            end += 1;
        }
        let word: String = code[start..end].iter().map(|&(c, _)| c).collect();
        let next = tokens.partition_point(|&k| k < end);
        (!word.is_empty()).then_some((word, next))
    };
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    // The word whose last character is token `u`, and the token of its first character.
    let word_ending_at = |u: usize| -> Option<(String, usize)> {
        let end = *tokens.get(u)?;
        if !ident(code[end].0) {
            return None;
        }
        let mut start = end;
        while start > 0 && ident(code[start - 1].0) {
            start -= 1;
        }
        let word: String = code[start..=end].iter().map(|&(c, _)| c).collect();
        Some((word, tokens.partition_point(|&k| k < start)))
    };
    // Whether the `{` at token `t` opens a module (`mod name {`).
    let opens_module = |t: usize| {
        t.checked_sub(1)
            .and_then(word_ending_at)
            .and_then(|(_, first)| first.checked_sub(1))
            .and_then(word_ending_at)
            .is_some_and(|(word, _)| word == "mod")
    };
    let mut spans = Vec::new();
    // The open delimiters around the scan, each whether it is a module's brace: a test module
    // counts only among modules, never inside a macro's input or a function (review round 3).
    let mut enclosing: Vec<bool> = Vec::new();
    // Where the last outer attribute ended: `#[cfg(test)]` must be the item's first one, so no
    // attribute macro before it sees the module (review round 3).
    let mut attribute_end = None;
    let mut t = 0;
    // Whether an outer attribute other than a built-in one came since the last item boundary:
    // a module it applies to may be rewritten by a macro, test modules and all (review round 4).
    let mut foreign_attribute = false;
    while let Some(c) = char_at(t) {
        if c != '#' || char_at(t + 1) != Some('[') {
            match c {
                '{' => enclosing.push(opens_module(t) && !foreign_attribute),
                '(' | '[' => enclosing.push(false),
                '}' | ')' | ']' => {
                    enclosing.pop();
                }
                _ => {}
            }
            if matches!(c, '{' | '}' | ';') {
                foreign_attribute = false;
            }
            t += 1;
            continue;
        }
        let Some(after) = after_brackets(t + 1) else {
            break;
        };
        let first = attribute_end != Some(t);
        attribute_end = Some(after);
        let body: String = tokens[t + 2..after - 1]
            .iter()
            .map(|&k| code[k].0)
            .collect();
        let name = body
            .split(['(', '=', ':'])
            .next()
            .unwrap_or_default()
            .to_owned();
        if !BUILT_IN_ATTRIBUTES.contains(&name.as_str()) {
            foreign_attribute = true;
        }
        if body != "cfg(test)" || !first || !enclosing.iter().all(|&module| module) {
            t = after;
            continue;
        }
        // Further attributes, a visibility, then `mod name {`.
        let mut item = after;
        while char_at(item) == Some('#') && char_at(item + 1) == Some('[') {
            match after_brackets(item + 1) {
                Some(next) => item = next,
                None => return spans,
            }
        }
        if let Some((word, next)) = word_at(item)
            && word == "pub"
        {
            item = next;
            if char_at(item) == Some('(') {
                while char_at(item).is_some_and(|c| c != ')') {
                    item += 1;
                }
                item += 1;
            }
        }
        let open = match word_at(item) {
            Some((word, next)) if word == "mod" => word_at(next)
                .filter(|(name, _)| !name.chars().next().is_some_and(char::is_numeric))
                .and_then(|(_, next)| (char_at(next) == Some('{')).then_some(next)),
            _ => None,
        };
        let Some(open) = open else {
            t = after;
            continue;
        };
        // The module's closing brace.
        let mut depth = 0usize;
        let mut close = None;
        for (u, &k) in tokens.iter().enumerate().skip(open) {
            match code[k].0 {
                '{' => depth += 1,
                '}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        close = Some(u);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else {
            return spans;
        };
        spans.push((line_of(tokens[t]), line_of(tokens[close])));
        t = close + 1;
    }
    spans
}

/// `text` as `(char, true)` code with every comment, string and character literal blanked to
/// spaces (line breaks kept), or `None` when one is not closed.
fn code_only(text: &str) -> Option<Vec<(char, bool)>> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<(char, bool)> = Vec::with_capacity(chars.len());
    let blank = |c: char| {
        if c == '\n' {
            ('\n', false)
        } else {
            (' ', false)
        }
    };
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        let next = chars.get(at + 1).copied();
        let before_ident = at > 0 && ident(chars[at - 1]);
        if c == '/' && next == Some('/') {
            while at < chars.len() && chars[at] != '\n' {
                out.push(blank(chars[at]));
                at += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 0usize;
            loop {
                match (chars.get(at), chars.get(at + 1)) {
                    (Some('/'), Some('*')) => {
                        depth += 1;
                        out.extend([blank('/'), blank('*')]);
                        at += 2;
                    }
                    (Some('*'), Some('/')) => {
                        depth -= 1;
                        out.extend([blank('*'), blank('/')]);
                        at += 2;
                        if depth == 0 {
                            break;
                        }
                    }
                    (Some(&ch), _) => {
                        out.push(blank(ch));
                        at += 1;
                    }
                    (None, _) => return None,
                }
            }
        } else if !before_ident
            && (c == 'r' || (c == 'b' && next == Some('r')) || (c == 'c' && next == Some('r')))
            && {
                let mut k = at + if c == 'r' { 1 } else { 2 };
                while chars.get(k) == Some(&'#') {
                    k += 1;
                }
                chars.get(k) == Some(&'"')
            }
        {
            // A raw string: `r#"…"#`, closed by `"` and as many `#`.
            let mut k = at + if c == 'r' { 1 } else { 2 };
            let mut hashes = 0;
            while chars.get(k) == Some(&'#') {
                hashes += 1;
                k += 1;
            }
            k += 1;
            loop {
                match chars.get(k) {
                    None => return None,
                    Some('"') if (1..=hashes).all(|h| chars.get(k + h) == Some(&'#')) => {
                        k += 1 + hashes;
                        break;
                    }
                    _ => k += 1,
                }
            }
            out.extend(chars[at..k].iter().map(|&ch| blank(ch)));
            at = k;
        } else if c == '"' {
            let mut k = at + 1;
            loop {
                match chars.get(k) {
                    None => return None,
                    Some('\\') => k += 2,
                    Some('"') => {
                        k += 1;
                        break;
                    }
                    _ => k += 1,
                }
            }
            out.extend(chars[at..k.min(chars.len())].iter().map(|&ch| blank(ch)));
            at = k;
        } else if c == '\'' {
            // A character literal (`'a'`, `'\n'`, `'\u{1F600}'`), or else a lifetime's quote.
            let close = match next {
                Some('\\') => (at + 3..chars.len().min(at + 12)).find(|&k| chars[k] == '\''),
                Some(_) if chars.get(at + 2) == Some(&'\'') => Some(at + 2),
                _ => None,
            };
            match close {
                Some(close) => {
                    out.extend(chars[at..=close].iter().map(|&ch| blank(ch)));
                    at = close + 1;
                }
                None => {
                    out.push((c, true));
                    at += 1;
                }
            }
        } else {
            out.push((c, true));
            at += 1;
        }
    }
    Some(out)
}

/// What a follow-up changed: its product lines in total and per file, most first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FollowUp {
    /// Product lines, commit by commit.
    pub lines: u32,
    /// Per file.
    pub files: Vec<(String, u32)>,
    /// Rust files whose added product lines compile a file in by its path (`#[path]`,
    /// `include!`, `include_str!`, `include_bytes!`): no count says what that brings in.
    pub includes: Vec<String>,
}

/// How a product line names a file: its folder and its name without the extension
/// (`fixtures/evil` for `crates/a/fixtures/evil.rs`, as `#[path]`, `include!` or an import
/// writes it); a file at the top by its name alone.
fn reference_key(path: &str) -> Option<String> {
    let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
    let stem = name.split('.').next().unwrap_or(name);
    if stem.is_empty() {
        return None;
    }
    match folder.rsplit('/').next().filter(|f| !f.is_empty()) {
        Some(parent) => Some(format!("{parent}/{stem}")),
        None => (stem.len() >= 3).then(|| stem.to_owned()),
    }
}

impl FollowUp {
    /// Counts `changes` (every commit's files) under `rule`. A file the rule does not count
    /// still counts when a product line the follow-up added names it (`fixtures/evil` in
    /// `#[path = "../fixtures/evil.rs"]`, an import of `__tests__/x`): product code then
    /// builds it in (review round 4 of SPEC-493 fix 6).
    #[must_use]
    pub fn count(changes: &[FileLines], rule: &ProductRule) -> Self {
        let product_text: Vec<&str> = changes
            .iter()
            .filter(|c| rule.counts(&c.path))
            .flat_map(FileLines::product_text)
            .collect();
        let named = |path: &str| {
            reference_key(path).is_some_and(|key| product_text.iter().any(|t| t.contains(&key)))
        };
        let mut includes: Vec<String> = changes
            .iter()
            .filter(|c| rule.counts(&c.path) && c.path.ends_with(".rs"))
            .filter(|c| {
                c.product_text().any(|text| {
                    let tight: String = text.chars().filter(|c| !c.is_whitespace()).collect();
                    ["#[path", "include!(", "include_str!(", "include_bytes!("]
                        .iter()
                        .any(|mark| tight.contains(mark))
                })
            })
            .map(|c| c.path.clone())
            .collect();
        includes.sort();
        includes.dedup();
        let mut files: Vec<(String, u32)> = Vec::new();
        for change in changes
            .iter()
            .filter(|c| rule.counts(&c.path) || named(&c.path))
        {
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
            includes,
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
        assert_eq!(
            test_module_spans("#[cfg(test)] mod t { fn a() {} }\nfn b() {}\n"),
            [(1, 1)]
        );
    }

    /// Review round 4 of SPEC-493 fix 6: a test or document file a new product line names
    /// counts; a Rust product line that compiles a file in by its path is reported.
    #[test]
    fn a_file_product_code_names_counts() {
        let with_text = |path: &str, text: &[&str]| FileLines {
            path: path.to_owned(),
            added: (1..=u32::try_from(text.len()).unwrap_or(0)).collect(),
            added_text: text.iter().map(|t| (*t).to_owned()).collect(),
            ..FileLines::default()
        };
        let fixture = with_text("crates/a/fixtures/evil.rs", &["fn evil() {}"; 30]);
        let test = with_text("crates/a/tests/it.rs", &["#[test] fn t() {}"; 30]);
        let named = FollowUp::count(
            &[
                with_text(
                    "crates/a/src/lib.rs",
                    &["#[path = \"../fixtures/evil.rs\"]", "mod evil;"],
                ),
                fixture.clone(),
                test.clone(),
            ],
            &ProductRule::default(),
        );
        assert_eq!(named.lines, 32, "{named:?}");
        assert_eq!(named.includes, ["crates/a/src/lib.rs"]);
        let js = FollowUp::count(
            &[
                with_text("web/app.js", &["import { x } from './__tests__/helper';"]),
                with_text("web/__tests__/helper.js", &["export const x = 1;"; 25]),
            ],
            &ProductRule::default(),
        );
        assert_eq!(js.lines, 26);
        assert!(js.includes.is_empty());
        // A product line that names nothing keeps tests out; a test module's line names nothing.
        let mut lib = with_text("crates/a/src/lib.rs", &["fn a() {}", "include_str!(\"x\")"]);
        lib.tests_new = vec![(2, 2)];
        let plain = FollowUp::count(&[lib, fixture, test], &ProductRule::default());
        assert_eq!((plain.lines, plain.includes.len()), (1, 0));
        for (path, key) in [
            ("crates/a/fixtures/evil.rs", Some("fixtures/evil")),
            ("README.md", Some("README")),
            ("x.rs", None),
        ] {
            assert_eq!(reference_key(path).as_deref(), key, "{path}");
        }
    }

    /// Real files read whole: each has one test module, from its `#[cfg(test)]` to its last
    /// line, though their tests hold the tricks above in strings (and `briefing.rs` has a
    /// single `#[cfg(test)]` constant before it).
    #[test]
    fn real_files_have_their_test_module_and_nothing_else() {
        for text in [
            include_str!("follow_up.rs"),
            include_str!("briefing.rs"),
            include_str!("spec_format/config.rs"),
            include_str!("scope.rs"),
        ] {
            let lines: Vec<&str> = text.lines().collect();
            let start = lines
                .windows(2)
                .position(|w| w == ["#[cfg(test)]", "mod tests {"])
                .and_then(|n| u32::try_from(n + 1).ok());
            let last = u32::try_from(lines.len()).unwrap_or(0);
            assert_eq!(test_module_spans(text), [(start.unwrap_or(0), last)]);
        }
    }

    /// Review round 2 of SPEC-493 fix 6: a span is read from Rust tokens. A `#[cfg(test)] mod`
    /// in a comment or a string opens none; a brace in a comment, a string or a character
    /// literal neither opens nor closes one; an unformatted closing brace ends it; an unclosed
    /// comment or string leaves no span at all.
    #[test]
    fn test_spans_are_read_from_rust_tokens() {
        let fake_in_comment = "/*\n#[cfg(test)]\nmod t {\n*/\npub fn evil() {\n}\n";
        assert!(test_module_spans(fake_in_comment).is_empty());
        let fake_in_string = "const S: &str = \"\n#[cfg(test)]\nmod t {\n\";\npub fn evil() {}\n";
        assert!(test_module_spans(fake_in_string).is_empty());
        let raw = "const S: &str = r#\"\n#[cfg(test)]\nmod t {\n\"#;\npub fn evil() {}\n";
        assert!(test_module_spans(raw).is_empty());
        let braces = "#[cfg(test)]\nmod tests {\n    const A: char = '}';\n    // }\n    \
                      const B: &str = \"}\";\n    fn t<'a>(x: &'a str) {}\n    \
                      const Q: char = '\\'';\n    const U: char = '\\u{7D}';\n}\n\
                      pub fn after() {}\n";
        assert_eq!(test_module_spans(braces), [(1, 9)]);
        let unformatted = "#[cfg(test)]\nmod tests {\n    fn t() {} }\npub fn after() {}\n}\n";
        assert_eq!(test_module_spans(unformatted), [(1, 3)]);
        let nested = "#[cfg(test)]\nmod t {\n/* /* } */ } */\n}\nfn p() {}\n";
        assert_eq!(test_module_spans(nested), [(1, 4)]);
        assert!(test_module_spans("#[cfg(test)]\nmod t {\n/* open\n}\n").is_empty());
        assert!(test_module_spans("#[cfg(test)]\nmod t {\n\"open\n}\n").is_empty());
        let spaced = "#[ cfg( test ) ]\npub mod  t\n{\n}\n";
        assert_eq!(test_module_spans(spaced), [(1, 4)]);
        // Review round 3: inside a macro's input, a function or a non-module brace, after an
        // attribute macro: no span. Inside modules, and after an inner attribute: a span.
        for hidden in [
            "macro_rules! keep { (#[cfg(test)] mod $n:ident { $($b:tt)* }) => { $($b)* }; }\n\
             keep! {\n#[cfg(test)]\nmod t {\n    pub fn evil() -> u32 { 42 }\n}\n}\n",
            "keep!(\n#[cfg(test)]\nmod t {\n    pub fn evil() {}\n}\n);\n",
            "keep![\n#[cfg(test)]\nmod t {\n}\n];\n",
            "fn f() {\n#[cfg(test)]\nmod t {\n}\n}\n",
            "impl A {\n#[cfg(test)]\nmod t {\n}\n}\n",
            "#[keep]\n#[cfg(test)]\nmod t {\n    pub fn evil() {}\n}\n",
            "#[keep]\nmod x {\n    #[cfg(test)]\n    pub mod t {\n    }\n}\n",
            "#[cfg_attr(not(test), keep)]\nmod x {\n    #[cfg(test)]\n    mod t {\n    }\n}\n",
        ] {
            assert!(test_module_spans(hidden).is_empty(), "{hidden}");
        }
        let among_modules = "#![allow(dead_code)]\n#[allow(x)]\n#[rustfmt::skip]\nmod a {\n    pub mod b {\n        #[cfg(test)]\n\
                             #[allow(x)]\n        mod t {\n        }\n    }\n}\n";
        assert_eq!(test_module_spans(among_modules), [(6, 9)]);
    }
}
