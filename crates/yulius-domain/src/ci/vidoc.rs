//! The repository's own Vidoc switch (ADR-0107): `enabled` in `.config/vidoc.toml`, as Yulius
//! reports it on a sandbox's Security scan card (SPEC-359 VSB-003). It reads the file as TOML,
//! as the scanner's `parse_config` does (`tomllib`), and checks its shape in the scanner's order
//! (the keys, `enabled`, `exclude`, each `[[ignore]]` entry), so a file the scanner refuses reads
//! as unreadable, never as on or off; TOML 1.0's limits are checked where the `toml` crate reads
//! 1.1 (a byte-order mark, the escapes `\e` and `\xHH`, multi-line inline tables), and blank and
//! numbers as Python reads them.

use toml::de::{DeTable, DeValue};

/// Where the repository's Vidoc switch lives.
pub const VIDOC_CONFIG: &str = ".config/vidoc.toml";

/// What `.config/vidoc.toml` says about the scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryVidoc {
    /// No file, or `enabled = true`: the scan runs as Yulius's switches say.
    On,
    /// `enabled = false`: the scanner says "disabled" whatever Yulius's switches say.
    Off,
    /// The scanner would stop "not run" on it: the problem, in words.
    Unreadable(String),
}

/// The keys an `[[ignore]]` entry may set.
const ENTRY_KEYS: [&str; 5] = ["path", "title", "reason", "function", "line"];
const IGNORE_PROBLEM: &str = "each [[ignore]] needs path, title and reason, may name its function \
                              and line (a number from 1), and nothing else";

/// `enabled` of `.config/vidoc.toml` (`None`: no file), once the file is one the scanner reads.
///
/// # Examples
/// ```
/// use yulius_domain::ci::vidoc::{RepositoryVidoc, repository_vidoc};
/// assert_eq!(repository_vidoc(Some("enabled = false # paused\n")), RepositoryVidoc::Off);
/// assert_eq!(repository_vidoc(None), RepositoryVidoc::On);
/// ```
#[must_use]
pub fn repository_vidoc(text: Option<&str>) -> RepositoryVidoc {
    let Some(text) = text else {
        return RepositoryVidoc::On;
    };
    match read(text) {
        Ok(true) => RepositoryVidoc::On,
        Ok(false) => RepositoryVidoc::Off,
        Err(problem) if problem.starts_with(':') => {
            RepositoryVidoc::Unreadable(format!("{VIDOC_CONFIG}{problem}"))
        }
        Err(problem) => RepositoryVidoc::Unreadable(format!("{VIDOC_CONFIG} {problem}")),
    }
}

/// Whether the scan is on (`enabled`, true when absent), or why the scanner would refuse `text`.
fn read(text: &str) -> Result<bool, String> {
    let line = |offset: usize| {
        let before = &text.as_bytes()[..offset.min(text.len())];
        before.iter().filter(|&&b| b == b'\n').count() + 1
    };
    // `tomllib` reads none of TOML 1.1's: a byte-order mark is no statement, `\e` and `\xHH`
    // are no escapes.
    if text.starts_with('\u{feff}') {
        return Err("cannot be read: a byte-order mark starts it (line 1)".to_owned());
    }
    if let Some(offset) = toml_1_1_escape(text) {
        return Err(format!(
            "cannot be read: a string uses an escape of TOML 1.1 (line {})",
            line(offset)
        ));
    }
    let root = DeTable::parse(text).map_err(|error| {
        let at = error.span().map_or(1, |span| line(span.start));
        let message = error
            .message()
            .lines()
            .next()
            .unwrap_or("invalid TOML")
            .trim();
        format!("cannot be read: {message} (line {at})")
    })?;
    let root = root.get_ref();
    let mut unknown: Vec<String> = root
        .keys()
        .map(|key| key.get_ref().to_string())
        .filter(|key| !["enabled", "exclude", "ignore"].contains(&key.as_str()))
        .map(|key| format!("'{key}'"))
        .collect();
    if !unknown.is_empty() {
        unknown.sort();
        return Err(format!("has unexpected keys [{}]", unknown.join(", ")));
    }
    let enabled = match root.get("enabled") {
        None => true,
        Some(value) => match value.get_ref() {
            DeValue::Boolean(flag) => *flag,
            _ => {
                return Err(format!(
                    "must set enabled = true or enabled = false (line {})",
                    line(value.span().start)
                ));
            }
        },
    };
    if let Some(exclude) = root.get("exclude") {
        let globs = matches!(exclude.get_ref(), DeValue::Array(items) if items.iter().all(|item| {
            matches!(item.get_ref(), DeValue::String(glob) if !glob.is_empty())
        }));
        if !globs {
            return Err(": exclude must be a list of glob strings".to_owned());
        }
    }
    if let Some(ignore) = root.get("ignore") {
        let DeValue::Array(entries) = ignore.get_ref() else {
            return Err(format!(": {IGNORE_PROBLEM}"));
        };
        let kept = |entry: &toml::Spanned<DeValue<'_>>| {
            ignore_entry(entry.get_ref()) && text.get(entry.span()).is_some_and(toml_1_0)
        };
        if !entries.iter().all(kept) {
            return Err(format!(": {IGNORE_PROBLEM}"));
        }
    }
    Ok(enabled)
}

/// An entry's source as TOML 1.0 has it, which the scanner's `tomllib` reads: an inline table
/// on one line and without a comma before its `}` (TOML 1.1 allows both; the `toml` crate reads
/// 1.1). A newline inside one of its strings is refused too, the safer side.
fn toml_1_0(source: &str) -> bool {
    let Some(inline) = source.trim().strip_prefix('{') else {
        return true;
    };
    let inner = inline.strip_suffix('}').unwrap_or(inline);
    !inline.contains(['\n', '\r']) && !inner.trim_end().ends_with(',')
}

/// Where a basic string of `text` uses `\e` or `\xHH`, escapes TOML 1.1 added: comments and
/// literal strings (`'…'`, `'''…'''`) are skipped, and `\\` is an escaped backslash.
fn toml_1_1_escape(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        let rest = &bytes[at..];
        if rest.starts_with(b"#") {
            at += rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len());
        } else if rest.starts_with(b"'''") || rest.starts_with(b"'") {
            let quote: &[u8] = if rest.starts_with(b"'''") {
                b"'''"
            } else {
                b"'"
            };
            let body = &rest[quote.len()..];
            let end = body
                .windows(quote.len())
                .position(|w| w == quote)
                .map_or(body.len(), |p| p + quote.len());
            at += quote.len() + end;
        } else if rest.starts_with(b"\"") {
            let quote: &[u8] = if rest.starts_with(b"\"\"\"") {
                b"\"\"\""
            } else {
                b"\""
            };
            let mut i = at + quote.len();
            while i < bytes.len() && !bytes[i..].starts_with(quote) {
                if bytes[i] == b'\\' {
                    if matches!(bytes.get(i + 1), Some(b'e' | b'x')) {
                        return Some(i);
                    }
                    i += 2;
                } else {
                    i += 1;
                }
            }
            at = i + quote.len();
        } else {
            at += 1;
        }
    }
    None
}

/// Blank as Python's `str.strip` sees it: Unicode white space and the separators U+001C to
/// U+001F.
fn python_blank(text: &str) -> bool {
    text.chars()
        .all(|c| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// An `[[ignore]]` entry the scanner keeps: path, title and reason, maybe function and line, the
/// strings not blank and the line a number from 1.
fn ignore_entry(entry: &DeValue<'_>) -> bool {
    let DeValue::Table(entry) = entry else {
        return false;
    };
    let named = ["path", "title", "reason"]
        .iter()
        .all(|key| entry.contains_key(*key));
    named
        && entry.iter().all(
            |(key, value)| match (key.get_ref().as_ref(), value.get_ref()) {
                // Python's integers have no bound: any number from 1, however large.
                ("line", DeValue::Integer(number)) => {
                    let digits = number.as_str().trim_start_matches('+').replace('_', "");
                    !digits.starts_with('-')
                        && u128::from_str_radix(&digits, number.radix())
                            .map_or(digits.chars().all(|c| c.is_digit(number.radix())), |n| {
                                n >= 1
                            })
                }
                ("line", _) => false,
                (key, DeValue::String(text)) => ENTRY_KEYS.contains(&key) && !python_blank(text),
                _ => false,
            },
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unreadable(text: &str) -> bool {
        matches!(repository_vidoc(Some(text)), RepositoryVidoc::Unreadable(_))
    }

    /// Whether the scanner's own `parse_config` (`scripts/vidoc_scan.py`, Python's `tomllib`)
    /// accepts `text`, as it reads the committed file.
    fn scanner_accepts(text: &str) -> bool {
        use std::io::Write as _;
        let scripts = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts");
        let code = "import sys; sys.path.insert(0, sys.argv[1]); import vidoc_scan\n\
                    try:\n    vidoc_scan.parse_config(sys.stdin.buffer.read().decode(), 'x')\n\
                    except vidoc_scan.Usage:\n    sys.exit(3)";
        let mut child = std::process::Command::new("python3")
            .args(["-I", "-c", code, scripts])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("python3: {e}"));
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .unwrap_or_else(|e| panic!("stdin: {e}"));
        }
        let status = child.wait().unwrap_or_else(|e| panic!("wait: {e}"));
        assert!(matches!(status.code(), Some(0 | 3)), "{status:?}");
        status.success()
    }

    /// #1811: the card reads each of these as the scanner does: a byte-order mark, the TOML 1.1
    /// escapes `\e` and `\xHH`, a string of only the separators Python counts as blank, and a
    /// line number past `u64::MAX`.
    #[test]
    fn the_card_and_the_scanner_agree_on_the_corner_cases() {
        let entry = |title: &str, line: &str| {
            format!("[[ignore]]\npath = \"a\"\ntitle = {title}\nreason = \"r\"\n{line}")
        };
        for text in [
            "\u{feff}enabled = false\n".to_owned(),
            "\u{feff}# why\nenabled = true\n".to_owned(),
            entry("\"x\\e\"", ""),
            entry("\"\\x41\"", ""),
            entry("'\\e literal'", ""),
            entry("\"\\\\e escaped backslash\"", ""),
            entry("\"\\u001C\\u001F\"", ""),
            entry("\"\\u001Cx\"", ""),
            entry("\"t\"", "line = 99999999999999999999999\n"),
            entry("\"t\"", "line = 0\n"),
            entry("\"t\"", "line = 7\n"),
        ] {
            let card = !unreadable(&text);
            assert_eq!(card, scanner_accepts(&text), "{text:?}");
        }
    }

    #[test]
    fn no_file_or_no_key_is_on() {
        assert_eq!(repository_vidoc(None), RepositoryVidoc::On);
        assert_eq!(repository_vidoc(Some("")), RepositoryVidoc::On);
        assert_eq!(
            repository_vidoc(Some("# comments only\nexclude = [\"a\"]\n")),
            RepositoryVidoc::On
        );
    }

    #[test]
    fn enabled_reads_true_or_false_with_comments_and_spaces() {
        assert_eq!(
            repository_vidoc(Some("enabled = true\n")),
            RepositoryVidoc::On
        );
        assert_eq!(
            repository_vidoc(Some("# why\n  enabled=false   # quota\n")),
            RepositoryVidoc::Off
        );
    }

    #[test]
    fn this_repositorys_file_is_on() {
        let text = include_str!("../../../../.config/vidoc.toml");
        assert_eq!(repository_vidoc(Some(text)), RepositoryVidoc::On);
    }

    #[test]
    fn keys_of_tables_and_arrays_are_not_the_switch() {
        let text = "exclude = [\n  \"enabled = false\",\n]\n\n[[ignore]]\npath = \"a\"\n\
                    title = \"t # x\"\nreason = \"say \\\"no\\\" [\"\n";
        assert_eq!(repository_vidoc(Some(text)), RepositoryVidoc::On);
        assert!(
            unreadable(
                "[[ignore]]\npath = \"a\"\ntitle = \"t\"\nreason = \"r\"\nenabled = false\n"
            ),
            "an entry's own enabled is no switch, and no key of an entry"
        );
    }

    #[test]
    fn a_value_the_scanner_refuses_is_unreadable() {
        assert!(unreadable("enabled = \"false\"\n"), "a string");
        assert!(unreadable("enabled = 0\n"), "a number");
        assert!(unreadable("enabled = true\nenabled = false\n"), "twice");
        assert!(unreadable("enabled = false trailing\n"), "junk after it");
    }

    /// Review round 1: what the scanner's `parse_config` refuses before it reads `enabled` is
    /// unreadable too, so the card never says "on" or "off" for a scan that ends "not run".
    #[test]
    fn a_file_the_scanner_refuses_elsewhere_is_unreadable() {
        let ignore = "[[ignore]]\npath = \"a.rs\"\ntitle = \"t\"\nreason = \"r\"\n";
        assert_eq!(
            repository_vidoc(Some(&format!(
                "enabled = true\n{ignore}function = \"f\"\nline = 3\n"
            ))),
            RepositoryVidoc::On,
            "a complete entry"
        );
        assert!(unreadable("enabled = false\ntypo = 1\n"), "an unknown key");
        assert!(
            unreadable("enabled = true\n[other]\nx = 1\n"),
            "an unknown table"
        );
        assert!(
            unreadable("[ignore]\npath = \"a\"\n"),
            "ignore must be an array of tables"
        );
        assert!(
            unreadable("[[ignore]]\npath = \"a.rs\"\ntitle = \"t\"\n"),
            "an entry without a reason"
        );
        assert!(
            unreadable(&format!("{ignore}owner = \"x\"\n")),
            "an unknown entry key"
        );
        assert!(unreadable(&format!("{ignore}line = 0\n")), "a line from 1");
        assert!(
            unreadable(&format!("{ignore}line = \"3\"\n")),
            "a line is a number"
        );
        assert!(
            unreadable(&format!("{ignore}reason = \"again\"\n")),
            "a key twice"
        );
        assert!(
            unreadable("[[ignore]]\npath = \"\"\ntitle = \"t\"\nreason = \"r\"\n"),
            "empty"
        );
        assert!(unreadable("exclude = \"a\"\n"), "exclude is a list");
        assert!(unreadable("exclude = [\n  \"a\",\n"), "an array left open");
        assert!(
            unreadable("enabled = true\nx = \"open\n"),
            "a string left open"
        );
    }

    /// Review round 2: `exclude` is a list of non-empty strings and nothing after it.
    #[test]
    fn exclude_is_a_list_of_glob_strings() {
        for good in [
            "exclude = []\n",
            "exclude = [\"a\", 'b/**',]\n",
            "exclude = [\n  \"a\", # why\n  \"b\",\n]\n",
        ] {
            assert_eq!(repository_vidoc(Some(good)), RepositoryVidoc::On, "{good}");
        }
        for bad in [
            "exclude = [1]\n",
            "exclude = [\"\"]\n",
            "exclude = [\"a\", 2]\n",
            "exclude = [src/**]\n",
            "exclude = [\"a\"] x\n",
            "exclude = [\"a\" \"b\"]\n",
            "exclude = [\n  \"a\",\n  3,\n]\n",
        ] {
            assert!(unreadable(bad), "{bad}");
        }
        let RepositoryVidoc::Unreadable(problem) = repository_vidoc(Some("exclude = [1]\n")) else {
            return;
        };
        assert_eq!(
            problem,
            ".config/vidoc.toml: exclude must be a list of glob strings"
        );
    }

    /// The file is read as TOML, as the scanner reads it, so forms the earlier line reader called
    /// unreadable read as the scanner reads them, and unknown keys are named as the scanner names
    /// them; an inline table only TOML 1.1 allows (the scanner's `tomllib` reads 1.0) stays
    /// unreadable.
    #[test]
    fn forms_the_scanner_reads_are_read_the_same() {
        assert_eq!(
            repository_vidoc(Some("ignore = []\nenabled = false\n")),
            RepositoryVidoc::Off
        );
        let entry = "[[ignore]]\npath = \"a\"\ntitle = \"t\"\nreason = \"\"\"two\nlines\"\"\"\nline = 1_0\n";
        assert_eq!(repository_vidoc(Some(entry)), RepositoryVidoc::On);
        assert_eq!(
            repository_vidoc(Some("enabled = false\ntypo = 1\nother = 2\n")),
            RepositoryVidoc::Unreadable(
                ".config/vidoc.toml has unexpected keys ['other', 'typo']".to_owned()
            )
        );

        for toml_1_1 in [
            "ignore = [{path = \"a\", title = \"t\", reason = \"r\",}]\n",
            "ignore = [{path = \"a\",\n  title = \"t\", reason = \"r\"}]\n",
        ] {
            assert!(unreadable(toml_1_1), "{toml_1_1}");
        }
        assert_eq!(
            repository_vidoc(Some(
                "ignore = [{path = \"a\", title = \"t\", reason = \"r\"}]\n"
            )),
            RepositoryVidoc::On,
            "a TOML 1.0 inline table"
        );
    }

    #[test]
    fn a_quoted_key_is_the_same_key() {
        assert_eq!(
            repository_vidoc(Some("\"enabled\" = false\n")),
            RepositoryVidoc::Off
        );
        assert_eq!(
            repository_vidoc(Some("'enabled' = false\n")),
            RepositoryVidoc::Off
        );
    }
}
