//! Dotenv files for agent secrets copied by Yulius (SPEC-129): one `NAME=value` line per key in
//! the git-ignored `.env`, and `NAME=` with no value in the tracked `.env.example`.
//!
//! Values are quoted the way every common reader (Node `dotenv`, `python-dotenv`, Docker Compose,
//! `sh`) reads them the same: bare when plain, in single quotes (literal) otherwise, in double
//! quotes only for a value with `'` and nothing those readers escape differently. A value with
//! `\` or `${` is refused: `python-dotenv` unescapes the one and expands the other even inside
//! single quotes, and Docker Compose reads `\'` as a quote.

use super::VaultError;

/// Characters a bare value may hold.
fn plain(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_-./:+=@,%^".contains(c)
}

/// The `NAME=value` line for a value (no line break).
///
/// # Errors
/// [`VaultError::Value`] for a value with a line break, `\` or `${`, or with `'` and also `"`,
/// `$` or a backtick, which no quoting writes portably.
pub fn line(name: &str, value: &str) -> Result<String, VaultError> {
    if value.chars().any(char::is_control) {
        return Err(VaultError::Value(format!(
            "{name} holds a control character a .env line cannot carry"
        )));
    }
    if value.contains('\\') || value.contains("${") {
        return Err(VaultError::Value(format!(
            "{name} holds \\ or ${{, which .env readers change (they unescape or expand it)"
        )));
    }
    if !value.is_empty() && value.chars().all(plain) {
        return Ok(format!("{name}={value}"));
    }
    if !value.contains('\'') {
        return Ok(format!("{name}='{value}'"));
    }
    if value.contains(['"', '$', '`']) {
        return Err(VaultError::Value(format!(
            "{name} holds both ' and one of \" $ `, which .env readers do not quote alike"
        )));
    }
    Ok(format!("{name}=\"{value}\""))
}

/// The line ending a file uses: Windows' when it has any, else `\n`.
fn line_end(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// The key a line assigns (`NAME=…`, `export NAME=…`), if it assigns one.
fn key_of(line: &str) -> Option<&str> {
    let text = line.trim_start();
    let text = text.strip_prefix("export ").map_or(text, str::trim_start);
    let (key, _) = text.split_once('=')?;
    let key = key.trim_end();
    (!key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).then_some(key)
}

/// `text` with each `(name, line)` set: the first line for a name is replaced in place (keeping
/// `export`), later ones are dropped, missing names are appended. Every other line is kept.
#[must_use]
pub fn upsert(text: &str, entries: &[(String, String)]) -> String {
    let eol = line_end(text);
    let mut seen = vec![false; entries.len()];
    let mut out = String::with_capacity(text.len() + entries.len() * 64);
    for raw in text.lines() {
        let found = key_of(raw).and_then(|k| entries.iter().position(|(n, _)| n == k));
        match found {
            Some(i) if seen[i] => {}
            Some(i) => {
                seen[i] = true;
                if raw.trim_start().starts_with("export ") {
                    out.push_str("export ");
                }
                out.push_str(&entries[i].1);
                out.push_str(eol);
            }
            None => {
                out.push_str(raw);
                out.push_str(eol);
            }
        }
    }
    for (i, (_, line)) in entries.iter().enumerate() {
        if !seen[i] {
            out.push_str(line);
            out.push_str(eol);
        }
    }
    out
}

/// `text` of a `.env.example` with `NAME=` appended for each of `names` it does not mention (a
/// commented `# NAME=` counts), and the names added.
#[must_use]
pub fn example(text: &str, names: &[String]) -> (String, Vec<String>) {
    let present = |name: &str| {
        text.lines()
            .any(|l| key_of(l.trim_start().trim_start_matches('#')) == Some(name))
    };
    let added: Vec<String> = names.iter().filter(|n| !present(n)).cloned().collect();
    let eol = line_end(text);
    let mut out = text.to_owned();
    if !added.is_empty() && !out.is_empty() && !out.ends_with('\n') {
        out.push_str(eol);
    }
    for name in &added {
        out.push_str(name);
        out.push('=');
        out.push_str(eol);
    }
    (out, added)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, value: &str) -> Result<(String, String), VaultError> {
        Ok((name.to_owned(), line(name, value)?))
    }

    #[test]
    fn values_are_quoted_the_way_every_reader_agrees_on() -> Result<(), VaultError> {
        assert_eq!(
            line("K", "sk_live_a1b2.c3/d4+e5=")?,
            "K=sk_live_a1b2.c3/d4+e5="
        );
        assert_eq!(line("K", "has space #and hash")?, "K='has space #and hash'");
        assert_eq!(line("K", "dollar$and\"quote")?, "K='dollar$and\"quote'");
        // Review of SPEC-129, M1: python-dotenv unescapes `\` and expands `${…}` even in single
        // quotes, Docker Compose reads `\'` as a quote, and `sh` expands a bare `~`.
        assert_eq!(line("K", "~/key-1234")?, "K='~/key-1234'");
        assert_eq!(line("K", "x:~root-12")?, "K='x:~root-12'");
        assert!(line("K", "dollar$and\"quote\\").is_err());
        assert!(line("K", "a\\b-12345678").is_err());
        assert!(line("K", "x${HOME}y-1234").is_err());
        assert_eq!(line("K", "it's-a-key")?, "K=\"it's-a-key\"");
        assert!(line("K", "it's $HOME").is_err());
        assert!(line("K", "it's \"q\"").is_err());
        assert!(line("K", "a`b'c").is_err());
        assert!(line("K", "two\nlines").is_err());
        Ok(())
    }

    #[test]
    fn the_names_lines_are_replaced_and_everything_else_kept() -> Result<(), VaultError> {
        let text = "# local settings\nOTHER=1\nexport API_KEY=old\n\nAPI_KEY = older\nMORE='x'";
        let out = upsert(
            text,
            &[entry("API_KEY", "new-value-123")?, entry("NEW", "n-v-456")?],
        );
        assert_eq!(
            out,
            "# local settings\nOTHER=1\nexport API_KEY=new-value-123\n\nMORE='x'\nNEW=n-v-456\n"
        );
        assert_eq!(upsert("", &[entry("A", "abcdefgh")?]), "A=abcdefgh\n");
        assert_eq!(
            upsert("# API_KEY=commented\n", &[entry("API_KEY", "abcdefgh")?]),
            "# API_KEY=commented\nAPI_KEY=abcdefgh\n",
            "a comment is not the key's line"
        );
        Ok(())
    }

    /// Review of SPEC-129, L6: a file with Windows line endings keeps them.
    #[test]
    fn windows_line_endings_are_kept() -> Result<(), VaultError> {
        assert_eq!(
            upsert(
                "A=1\r\nKEY=old\r\n",
                &[entry("KEY", "new-value-1")?, entry("B", "b-value-12")?]
            ),
            "A=1\r\nKEY=new-value-1\r\nB=b-value-12\r\n"
        );
        let names = vec!["KEY".to_owned()];
        assert_eq!(
            example("# keys\r\nA=", &names).0,
            "# keys\r\nA=\r\nKEY=\r\n"
        );
        Ok(())
    }

    #[test]
    fn the_example_gains_each_missing_name_without_a_value() {
        let names = vec!["API_KEY".to_owned(), "DOCS".to_owned(), "SEEN".to_owned()];
        let (out, added) = example("# keys\nSEEN=\n# DOCS=your docs token", &names);
        assert_eq!(out, "# keys\nSEEN=\n# DOCS=your docs token\nAPI_KEY=\n");
        assert_eq!(added, vec!["API_KEY".to_owned()]);
        let (same, none) = example(&out, &names);
        assert_eq!(same, out);
        assert!(none.is_empty());
        assert_eq!(example("", &names[..1]).0, "API_KEY=\n");
    }
}
