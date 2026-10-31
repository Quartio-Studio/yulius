//! The repository's own Vidoc switch (ADR-0107): `enabled` in `.config/vidoc.toml`, as Yulius
//! reports it on a sandbox's Security scan card (SPEC-359 VSB-003). It checks the file's shape as
//! the scanner's `parse_config` does (the keys, the tables, `enabled`, `exclude` and each
//! `[[ignore]]` entry), so a file the scanner refuses reads as unreadable, never as on or off; it
//! is no full TOML parser, and a TOML form beyond that shape (a multi-line string, a number
//! written `1_0`, an inline `ignore = []`) reads as unreadable too: the safer side, a warning.

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
    match read(text.strip_prefix('\u{feff}').unwrap_or(text)) {
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
    let mut enabled = None;
    let mut top: Vec<String> = Vec::new();
    // The `[[ignore]]` entry being read: the keys it set so far.
    let mut entry: Option<Vec<String>> = None;
    // Open `[` of a multi-line array value: its lines are not keys.
    let mut depth = 0usize;
    // `exclude`'s value while its lines are read, checked once its array closes.
    let mut exclude: Option<String> = None;
    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let Some(code) = without_comment(raw) else {
            return Err(format!(
                "cannot be read: a string is not closed (line {line})"
            ));
        };
        let code = code.trim();
        if depth > 0 {
            depth = brackets(code, depth);
            if let Some(value) = exclude.as_mut() {
                value.push(' ');
                value.push_str(code);
            }
            if depth == 0 {
                globs(exclude.take())?;
            }
            continue;
        }
        if code.is_empty() {
            continue;
        }
        if let Some(name) = code.strip_prefix("[[").and_then(|r| r.strip_suffix("]]")) {
            let name = unquote(name.trim());
            if name != "ignore" {
                return Err(format!("has unexpected keys ['{name}']"));
            }
            finish(entry.take())?;
            entry = Some(Vec::new());
            continue;
        }
        if let Some(name) = code.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            let name = unquote(name.trim());
            return Err(if name == "ignore" {
                format!(": {IGNORE_PROBLEM}")
            } else {
                format!("has unexpected keys ['{name}']")
            });
        }
        let Some((key, value)) = code.split_once('=') else {
            return Err(format!("cannot be read: no key = value (line {line})"));
        };
        let key = unquote(key.trim());
        let value = value.trim();
        let seen = entry.as_mut().unwrap_or(&mut top);
        if seen.iter().any(|known| known == key) {
            return Err(format!("sets {key} twice (line {line})"));
        }
        seen.push(key.to_owned());
        if entry.is_some() {
            let valid = match key {
                "line" => value.parse::<u64>().is_ok_and(|n| n >= 1),
                _ if ENTRY_KEYS.contains(&key) => filled_string(value),
                _ => false,
            };
            if !valid {
                return Err(format!(": {IGNORE_PROBLEM}"));
            }
        } else {
            match key {
                "enabled" => {
                    enabled = Some(match value {
                        "true" => true,
                        "false" => false,
                        _ => {
                            return Err(format!(
                                "must set enabled = true or enabled = false (line {line})"
                            ));
                        }
                    });
                }
                "exclude" => exclude = Some(value.to_owned()),
                other => return Err(format!("has unexpected keys ['{other}']")),
            }
        }
        depth = brackets(value, 0);
        if depth == 0 {
            globs(exclude.take())?;
        }
    }
    if depth > 0 {
        return Err("cannot be read: an array is not closed".to_owned());
    }
    finish(entry)?;
    Ok(enabled != Some(false))
}

/// `exclude`'s whole value, if it was set: a list of non-empty strings, nothing after it.
fn globs(value: Option<String>) -> Result<(), String> {
    match value {
        Some(value) if !glob_list(&value) => {
            Err(": exclude must be a list of glob strings".to_owned())
        }
        _ => Ok(()),
    }
}

/// `[ "a", 'b', ]`: quoted non-empty strings between commas (one may trail), and nothing after.
fn glob_list(value: &str) -> bool {
    let Some(mut rest) = value.trim().strip_prefix('[') else {
        return false;
    };
    let mut item_next = true;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix(']') {
            return after.trim().is_empty();
        }
        if !item_next {
            let Some(after) = rest.strip_prefix(',') else {
                return false;
            };
            rest = after;
            item_next = true;
            continue;
        }
        let Some((inner, after)) = quoted(rest) else {
            return false;
        };
        if inner.is_empty() {
            return false;
        }
        rest = after;
        item_next = false;
    }
}

/// The string `text` starts with and what follows it.
fn quoted(text: &str) -> Option<(&str, &str)> {
    let quote = text.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let mut escaped = false;
    for (at, c) in text.char_indices().skip(1) {
        if quote == '"' && c == '\\' && !escaped {
            escaped = true;
            continue;
        }
        if c == quote && !escaped {
            return Some((&text[1..at], &text[at + 1..]));
        }
        escaped = false;
    }
    None
}

/// An `[[ignore]]` entry's end: it named its path, title and reason.
fn finish(entry: Option<Vec<String>>) -> Result<(), String> {
    let named = |keys: &[String]| {
        ["path", "title", "reason"]
            .iter()
            .all(|key| keys.iter().any(|set| set == key))
    };
    match entry {
        Some(keys) if !named(&keys) => Err(format!(": {IGNORE_PROBLEM}")),
        _ => Ok(()),
    }
}

/// `key` without the quotes of a quoted key.
fn unquote(key: &str) -> &str {
    ['"', '\'']
        .iter()
        .find_map(|q| key.strip_prefix(*q).and_then(|k| k.strip_suffix(*q)))
        .unwrap_or(key)
}

/// A one-line string value with more than blanks in it.
fn filled_string(value: &str) -> bool {
    ['"', '\'']
        .iter()
        .find_map(|q| value.strip_prefix(*q).and_then(|v| v.strip_suffix(*q)))
        .is_some_and(|inner| !inner.trim().is_empty())
}

/// `line` up to a `#` outside a string; `None` when a string is left open.
fn without_comment(line: &str) -> Option<&str> {
    let mut quote = None;
    let mut escaped = false;
    for (at, c) in line.char_indices() {
        match (quote, c) {
            (Some('"'), '\\') if !escaped => {
                escaped = true;
                continue;
            }
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if c == open && !escaped => quote = None,
            (None, '#') => return Some(&line[..at]),
            _ => {}
        }
        escaped = false;
    }
    quote.is_none().then_some(line)
}

/// The open-bracket depth after `code` (comment-free), brackets inside strings left out.
fn brackets(code: &str, mut depth: usize) -> usize {
    let mut quote = None;
    let mut escaped = false;
    for c in code.chars() {
        match (quote, c) {
            (Some('"'), '\\') if !escaped => {
                escaped = true;
                continue;
            }
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if c == open && !escaped => quote = None,
            (None, '[') => depth += 1,
            (None, ']') => depth = depth.saturating_sub(1),
            _ => {}
        }
        escaped = false;
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unreadable(text: &str) -> bool {
        matches!(repository_vidoc(Some(text)), RepositoryVidoc::Unreadable(_))
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
            repository_vidoc(Some("\u{feff}# why\n  enabled=false   # quota\n")),
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
