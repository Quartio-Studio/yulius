//! The repository's own Vidoc switch (ADR-0107): `enabled` in `.config/vidoc.toml`, as Yulius
//! reports it on a sandbox's Security scan card (SPEC-359 VSB-003). The scanner reads the whole
//! file; Yulius needs only `enabled`, so this reads the top-level key and nothing else.

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

/// `enabled` of `.config/vidoc.toml` (`None`: no file): a boolean before the first table, once.
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
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut enabled = None;
    // Open `[` of a multi-line array value: its lines are not keys.
    let mut depth = 0usize;
    for (index, raw) in text.lines().enumerate() {
        let code = without_comment(raw).trim();
        if depth == 0 {
            if code.starts_with('[') {
                break;
            }
            if let Some((key, value)) = code.split_once('=')
                && key.trim() == "enabled"
            {
                let line = index + 1;
                if enabled.is_some() {
                    return RepositoryVidoc::Unreadable(format!(
                        "{VIDOC_CONFIG} sets enabled twice (line {line})"
                    ));
                }
                enabled = Some(match value.trim() {
                    "true" => true,
                    "false" => false,
                    _ => {
                        return RepositoryVidoc::Unreadable(format!(
                            "{VIDOC_CONFIG} must set enabled = true or enabled = false (line \
                             {line})"
                        ));
                    }
                });
            }
        }
        depth = brackets(code, depth);
    }
    if enabled == Some(false) {
        RepositoryVidoc::Off
    } else {
        RepositoryVidoc::On
    }
}

/// `line` up to a `#` outside a string.
fn without_comment(line: &str) -> &str {
    let mut quote = None;
    for (at, c) in line.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if c == open => quote = None,
            (None, '#') => return &line[..at],
            _ => {}
        }
    }
    line
}

/// The open-bracket depth after `code`, brackets inside strings left out.
fn brackets(code: &str, mut depth: usize) -> usize {
    let mut quote = None;
    for c in code.chars() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if c == open => quote = None,
            (None, '[') => depth += 1,
            (None, ']') => depth = depth.saturating_sub(1),
            _ => {}
        }
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
        let text =
            "exclude = [\n  \"enabled = false\",\n]\n\n[[ignore]]\npath = \"a\"\nenabled = false\n";
        assert_eq!(repository_vidoc(Some(text)), RepositoryVidoc::On);
    }

    #[test]
    fn a_value_the_scanner_refuses_is_unreadable() {
        assert!(unreadable("enabled = \"false\"\n"), "a string");
        assert!(unreadable("enabled = 0\n"), "a number");
        assert!(unreadable("enabled = true\nenabled = false\n"), "twice");
        assert!(unreadable("enabled = false trailing\n"), "junk after it");
    }
}
