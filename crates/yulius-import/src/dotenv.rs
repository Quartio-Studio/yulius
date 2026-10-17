//! Reading a dotenv file for `yulius secrets import` (SPEC-205 SIM-003): names, values and the
//! comments above them. Errors name line numbers and never quote a value.

use crate::secret::Secret;

/// Largest file read (SPEC-205 SIM-002).
pub const MAX_DOTENV_BYTES: usize = 1024 * 1024;
/// Most secrets in one file (SPEC-205 SIM-002).
pub const MAX_DOTENV_ENTRIES: usize = 500;

/// One `NAME=value` of a dotenv file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DotenvEntry {
    /// The name.
    pub name: String,
    /// The value, unquoted.
    pub value: Secret,
    /// The comment lines directly above, joined with spaces (empty when none).
    pub comment: String,
    /// The 1-based line the entry starts on.
    pub line: usize,
}

/// A file that cannot be read as dotenv; the message never holds a value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DotenvError(pub String);

/// Reads `text` as a dotenv file.
///
/// # Errors
/// [`DotenvError`] for a file over [`MAX_DOTENV_BYTES`] or [`MAX_DOTENV_ENTRIES`], a line that is
/// not `NAME=value`, an unterminated quote or a name defined twice.
pub fn parse_dotenv(text: &str) -> Result<Vec<DotenvEntry>, DotenvError> {
    if text.len() > MAX_DOTENV_BYTES {
        return Err(DotenvError("the file is larger than 1 MiB".into()));
    }
    let text = text.replace("\r\n", "\n");
    let mut entries: Vec<DotenvEntry> = Vec::new();
    let mut comment: Vec<&str> = Vec::new();
    let (mut pos, mut line) = (0, 1);
    while pos < text.len() {
        let end = line_end(&text, pos);
        let trimmed = text[pos..end].trim();
        let (next, next_line) = if trimmed.is_empty() {
            comment.clear();
            (end + 1, line + 1)
        } else if let Some(note) = trimmed.strip_prefix('#') {
            comment.push(note.trim());
            (end + 1, line + 1)
        } else {
            let body = trimmed
                .strip_prefix("export ")
                .map_or(trimmed, str::trim_start);
            let (name, rest) = body
                .split_once('=')
                .ok_or_else(|| DotenvError(format!("line {line} is not NAME=value")))?;
            let name = name.trim();
            if !is_variable_name(name) {
                return Err(DotenvError(format!(
                    "line {line}: the name must be letters, digits and _, not starting with a digit"
                )));
            }
            let rest = rest.trim_start();
            let (value, next, next_line) = match rest.chars().next() {
                Some(quote @ ('\'' | '"')) => {
                    let open = offset(&text, rest);
                    quoted(&text, open, quote, line)?
                }
                _ => (Secret::new(bare(rest)), end + 1, line + 1),
            };
            if let Some(first) = entries.iter().find(|e| e.name == name) {
                return Err(DotenvError(format!(
                    "{name} is defined on lines {} and {line}; keep one",
                    first.line
                )));
            }
            if entries.len() == MAX_DOTENV_ENTRIES {
                return Err(DotenvError(format!(
                    "the file holds more than {MAX_DOTENV_ENTRIES} secrets"
                )));
            }
            entries.push(DotenvEntry {
                name: name.to_owned(),
                value,
                comment: comment.join(" ").trim().to_owned(),
                line,
            });
            comment.clear();
            (next, next_line)
        };
        pos = next;
        line = next_line;
    }
    Ok(entries)
}

/// Where the line starting at `pos` ends (its `\n`, or the end of the text).
fn line_end(text: &str, pos: usize) -> usize {
    text[pos..].find('\n').map_or(text.len(), |at| pos + at)
}

/// The byte offset of `part`, a slice of `text`, in `text`.
fn offset(text: &str, part: &str) -> usize {
    part.as_ptr() as usize - text.as_ptr() as usize
}

fn is_variable_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// An unquoted value: up to a ` #` comment, trimmed.
fn bare(rest: &str) -> String {
    if rest.starts_with('#') {
        return String::new();
    }
    let end = rest
        .char_indices()
        .find(|&(at, c)| c == '#' && rest[..at].ends_with([' ', '\t']))
        .map_or(rest.len(), |(at, _)| at);
    rest[..end].trim().to_owned()
}

/// A quoted value opening at `open`, on line `line`: single quotes literal, double quotes with
/// `\n`, `\r`, `\t`, `\"` and `\\` read. Returns it with the position and number of the line
/// after its closing quote's line.
fn quoted(
    text: &str,
    open: usize,
    quote: char,
    line: usize,
) -> Result<(Secret, usize, usize), DotenvError> {
    let mut value = String::new();
    let mut lines = 0;
    let mut chars = text[open + 1..].char_indices();
    let mut close = None;
    while let Some((at, c)) = chars.next() {
        match c {
            c if c == quote => {
                close = Some(open + 1 + at);
                break;
            }
            '\\' if quote == '"' => match chars.next() {
                Some((_, 'n')) => value.push('\n'),
                Some((_, 'r')) => value.push('\r'),
                Some((_, 't')) => value.push('\t'),
                Some((_, '"')) => value.push('"'),
                Some((_, '\\')) => value.push('\\'),
                Some((_, other)) => {
                    if other == '\n' {
                        lines += 1;
                    }
                    value.push('\\');
                    value.push(other);
                }
                None => value.push('\\'),
            },
            c => {
                if c == '\n' {
                    lines += 1;
                }
                value.push(c);
            }
        }
    }
    let value = Secret::new(value);
    let close = close.ok_or_else(|| DotenvError(format!("line {line}: the quote is never closed")))?;
    let close_line = line + lines;
    let end = line_end(text, close + 1);
    let trailing = text[close + 1..end].trim();
    if !trailing.is_empty() && !trailing.starts_with('#') {
        return Err(DotenvError(format!(
            "line {close_line}: only a # comment may follow the closing quote"
        )));
    }
    Ok((value, end + 1, close_line + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(text: &str) -> Vec<(String, String, String, usize)> {
        parse_dotenv(text)
            .unwrap_or_else(|e| panic!("{e}"))
            .into_iter()
            .map(|e| (e.name, e.value.expose().to_owned(), e.comment, e.line))
            .collect()
    }

    fn error(text: &str) -> String {
        match parse_dotenv(text) {
            Ok(_) => panic!("accepted {text:?}"),
            Err(e) => e.0,
        }
    }

    fn row(name: &str, value: &str, comment: &str, line: usize) -> (String, String, String, usize) {
        (name.into(), value.into(), comment.into(), line)
    }

    #[test]
    fn reads_bare_quoted_exported_and_commented_entries() {
        let text = "\
# Stripe, live mode
# from the dashboard
STRIPE_KEY=sk_live_abc123 # inline comment

export  OPENAI_KEY = sk-proj-xyz
SINGLE='lit $HOME \\n # not a comment'
DOUBLE=\"a\\tb\\n\\\"c\\\" \\\\ ${KEPT}\"
HASH=abc#def
EMPTY=
";
        assert_eq!(
            entries(text),
            vec![
                row(
                    "STRIPE_KEY",
                    "sk_live_abc123",
                    "Stripe, live mode from the dashboard",
                    3
                ),
                row("OPENAI_KEY", "sk-proj-xyz", "", 5),
                row("SINGLE", "lit $HOME \\n # not a comment", "", 6),
                row("DOUBLE", "a\tb\n\"c\" \\ ${KEPT}", "", 7),
                row("HASH", "abc#def", "", 8),
                row("EMPTY", "", "", 9),
            ]
        );
    }

    #[test]
    fn a_blank_line_detaches_a_comment_and_crlf_is_read() {
        assert_eq!(
            entries("# orphan\r\n\r\nA=1\r\nB='x'\r\n"),
            vec![row("A", "1", "", 3), row("B", "x", "", 4)]
        );
    }

    #[test]
    fn quoted_values_span_lines() {
        let text = "KEY='-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----'\nNEXT=\"one\ntwo\"\nLAST=x\n";
        assert_eq!(
            entries(text),
            vec![
                row(
                    "KEY",
                    "-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----",
                    "",
                    1
                ),
                row("NEXT", "one\ntwo", "", 4),
                row("LAST", "x", "", 6),
            ]
        );
    }

    #[test]
    fn malformed_files_are_refused_by_line_without_their_values() {
        let cases = [
            ("A=1\njust-a-secret-value-123\n", "line 2"),
            ("A=1\nB='unterminated-secret-value\nC=2\n", "line 2"),
            ("A=\"unterminated-secret-value\n", "line 1"),
            ("A=1\n1BAD=secret-value-123\n", "line 2"),
            ("A=first-secret-value\nB=2\nA=second-secret-value\n", "lines 1 and 3"),
            ("A='closed-secret-value' trailing-secret\n", "line 1"),
            ("=secret-value-123\n", "line 1"),
        ];
        for (text, place) in cases {
            let message = error(text);
            assert!(message.contains(place), "{message:?} for {text:?}");
            for word in ["secret-value", "secret\n", "trailing-secret"] {
                assert!(!message.contains(word), "{message:?} leaks a value");
            }
        }
    }

    #[test]
    fn size_and_count_are_bounded() {
        assert!(error(&"A".repeat(MAX_DOTENV_BYTES + 1)).contains("1 MiB"));
        let many: String = (0..=MAX_DOTENV_ENTRIES)
            .map(|i| format!("K{i}=v\n"))
            .collect();
        assert!(error(&many).contains(&MAX_DOTENV_ENTRIES.to_string()));
    }
}
