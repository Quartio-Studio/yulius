//! Dotenv files for agent secrets copied by Yulius (SPEC-129): one `NAME=value` line per key in
//! the git-ignored `.env`, and `NAME=` with no value in the tracked `.env.example`.
//!
//! Values are quoted the way every common reader (Node `dotenv`, `python-dotenv`, Docker Compose,
//! `sh`) reads them the same: bare when plain, in single quotes (literal) otherwise, in double
//! quotes only for a value with `'` and nothing those readers escape differently. A value with
//! `\` or `${` is refused: `python-dotenv` unescapes the one and expands the other even inside
//! single quotes, and Docker Compose reads `\'` as a quote. The exception is a key block stored
//! with literal `\n` (a PEM or OpenSSH key; an agent secret never spans lines): it is written as
//! the key file it stands for, in single quotes across lines, which those readers take literally
//! (SPEC-186 SAV-005).

use super::VaultError;

/// Characters a bare value may hold.
fn plain(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_-./:+=@,%^".contains(c)
}

/// The label of a key's armour line `-----<edge> <label>-----` (PEM, OpenSSH).
fn armour<'a>(line: &'a str, edge: &str) -> Option<&'a str> {
    line.strip_prefix("-----")
        .and_then(|rest| rest.strip_prefix(edge))
        .and_then(|rest| rest.strip_prefix(' '))
        .and_then(|rest| rest.strip_suffix("-----"))
        .filter(|label| !label.is_empty() && !label.contains('-'))
}

/// The key file a value stands for, when it is a key block stored with literal `\n` (an agent
/// secret never spans lines, SPEC-186 SAV-005): its armour opens the first line and closes the
/// last with the same label, a body is between them, and no other `\` remains. The file ends with one line break, as
/// PEM and OpenSSH files do (OpenSSH refuses a key without it), whether or not the stored value
/// ends with a literal `\n`. `None` for any other value: one that is not a key keeps its escapes.
#[must_use]
pub fn key_block(value: &str) -> Option<String> {
    let text = value
        .strip_suffix("\\n")
        .unwrap_or(value)
        .replace("\\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    let (first, last) = (lines.first()?, lines.last()?);
    let block = lines.len() >= 3
        && armour(first, "BEGIN").is_some_and(|label| armour(last, "END") == Some(label))
        && !text.contains(['\\', '\'', '\r'])
        && !text.contains("${");
    block.then(|| format!("{text}\n"))
}

/// Whether `value` is a key flattened onto one line (SPEC-193 MLK-002): it opens with
/// `-----BEGIN <label>-----` and closes with `-----END <label>-----`, something is between them
/// and no literal `\n` marks its line breaks, which a single-line field replaced with spaces or
/// dropped. A one-line key block is never a usable key file.
#[must_use]
pub fn flattened_key(value: &str) -> bool {
    let text = value.trim();
    if text.contains("\\n") {
        return false;
    }
    let Some(rest) = text.strip_prefix("-----BEGIN ") else {
        return false;
    };
    let Some((label, rest)) = rest.split_once("-----") else {
        return false;
    };
    let end = format!("-----END {label}-----");
    armour(&format!("-----BEGIN {label}-----"), "BEGIN").is_some()
        && rest
            .strip_suffix(end.as_str())
            .is_some_and(|body| !body.trim().is_empty())
}

/// The lines of a key block's file that hold key material (its armour excluded): a copied key
/// file shows them one per line, so redaction looks for each (SPEC-186 SAV-005). Empty for a
/// value that is not a key block.
#[must_use]
pub fn key_lines(value: &str) -> Vec<String> {
    key_block(value).map_or_else(Vec::new, |file| {
        file.lines()
            .filter(|line| !line.starts_with("-----"))
            .map(str::to_owned)
            .collect()
    })
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
    // A key: its file, in single quotes across lines, which every reader takes literally.
    if let Some(file) = key_block(value) {
        return Ok(format!("{name}='{file}'"));
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

/// The quote a value opens and leaves open on its line (`"` or `'`), if any.
fn opens(value: &str) -> Option<char> {
    let value = value.trim_start();
    let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    still_open(&value[1..], quote).then_some(quote)
}

/// Whether `rest` leaves a value quoted with `quote` open (`\"` does not close a double quote).
fn still_open(rest: &str, quote: char) -> bool {
    let mut escaped = false;
    for c in rest.chars() {
        if c == quote && !escaped {
            return false;
        }
        escaped = quote == '"' && c == '\\' && !escaped;
    }
    true
}

/// `text` with each `(name, line)` set: the first line for a name is replaced in place (keeping
/// `export`), later ones are dropped, missing names are appended. A name's multi-line quoted
/// value is replaced or dropped whole (SPEC-186 SAV-005), never cut in half. Every other line is
/// kept, including the lines of another name's multi-line quoted value.
///
/// # Errors
/// [`VaultError::Value`] when a quoted value is still open at the end of the file: it would
/// swallow the lines appended after it (and where a value being replaced ends is unknown).
pub fn upsert(text: &str, entries: &[(String, String)]) -> Result<String, VaultError> {
    let eol = line_end(text);
    let mut seen = vec![false; entries.len()];
    let mut out = String::with_capacity(text.len() + entries.len() * 64);
    let mut open: Option<char> = None;
    // The open value belongs to a name being set: its lines go with its first one.
    let mut dropping = false;
    // The line number and name of the value the open quote belongs to.
    let mut opened_at = (0, "");
    for (number, raw) in text.lines().enumerate() {
        if let Some(quote) = open {
            if !still_open(raw, quote) {
                open = None;
            }
            if !dropping {
                out.push_str(raw);
                out.push_str(eol);
            }
            continue;
        }
        let key = key_of(raw);
        let value = raw.split_once('=').map_or("", |(_, v)| v);
        let found = key.and_then(|k| entries.iter().position(|(n, _)| n == k));
        if let Some(key) = key {
            open = opens(value);
            opened_at = (number + 1, key);
        }
        dropping = found.is_some();
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
    if open.is_some() {
        let (number, key) = opened_at;
        return Err(VaultError::Value(format!(
            "the quote that opens {key}'s value on line {number} of the file never closes: close \
             it first"
        )));
    }
    for (i, (_, line)) in entries.iter().enumerate() {
        if !seen[i] {
            out.push_str(line);
            out.push_str(eol);
        }
    }
    Ok(out)
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
        )?;
        assert_eq!(
            out,
            "# local settings\nOTHER=1\nexport API_KEY=new-value-123\n\nMORE='x'\nNEW=n-v-456\n"
        );
        assert_eq!(upsert("", &[entry("A", "abcdefgh")?])?, "A=abcdefgh\n");
        assert_eq!(
            upsert("# API_KEY=commented\n", &[entry("API_KEY", "abcdefgh")?])?,
            "# API_KEY=commented\nAPI_KEY=abcdefgh\n",
            "a comment is not the key's line"
        );
        Ok(())
    }

    /// The armour of a synthetic key: `-----BEGIN <label>-----`, built at run time so no scanner
    /// takes the source for a key.
    fn armour(edge: &str, label: &str) -> String {
        format!("-----{edge} {label}-----")
    }

    /// A synthetic key file of `body` lines between its armour, with its final line break.
    fn key_file(label: &str, body: &[&str]) -> String {
        let mut lines = vec![armour("BEGIN", label)];
        lines.extend(body.iter().map(|l| (*l).to_owned()));
        lines.push(armour("END", label));
        format!("{}\n", lines.join("\n"))
    }

    /// SPEC-193 MLK-002: a key whose line breaks a single-line field turned into spaces, or
    /// dropped, is told apart from a key stored with literal `\n` and from ordinary values.
    #[test]
    fn a_key_flattened_onto_one_line_is_recognised() {
        let file = key_file(
            &format!("{} PRIVATE KEY", "OPENSSH"),
            &["c3ludGhldGljLWZsYXQtMDE=", "c3ludGhldGljLWZsYXQtMDI="],
        );
        let file = file.trim_end();
        for flat in [
            file.replace('\n', " "),
            file.replace('\n', ""),
            format!(" {} ", file.replace('\n', " ")),
            file.replace('\n', "  "),
        ] {
            assert!(flattened_key(&flat), "{flat}");
        }
        let pem = key_file("SYNTHETIC KEY", &["c3ludGhldGljLXBlbS1ib2R5"]);
        assert!(flattened_key(&pem.trim_end().replace('\n', " ")));
        for kept in [
            file.replace('\n', "\\n"),
            format!("{}\\n", file.replace('\n', "\\n")),
            // A body with spaces is still a key once one break is kept.
            format!(
                "{} body with spaces\\n{}",
                armour("BEGIN", "KEY"),
                armour("END", "KEY")
            ),
            "sk_synthetic_value_7781".to_owned(),
            format!(
                "{} c3ludGhldGlj {}",
                armour("BEGIN", "KEY"),
                armour("END", "CERTIFICATE")
            ),
            format!("{} {}", armour("BEGIN", "KEY"), armour("END", "KEY")),
            format!(
                "prefix {} c3ludGhldGlj {}",
                armour("BEGIN", "KEY"),
                armour("END", "KEY")
            ),
            armour("BEGIN", "KEY"),
        ] {
            assert!(!flattened_key(&kept), "{kept}");
        }
    }

    /// SPEC-186 SAV-005: a key stored with literal `\n` (an agent secret never spans lines) is
    /// written as the key file it stands for, with its final line break, whether or not the
    /// stored value ends with a literal `\n`; `sh` and dotenv readers read single quotes
    /// literally, across lines.
    #[test]
    fn a_key_stored_with_escaped_line_breaks_is_written_as_its_file() -> Result<(), VaultError> {
        let pem = key_file("SYNTHETIC KEY", &["c3ludGhldGljLXBlbS1ib2R5LTAx"]);
        let openssh = key_file(
            &format!("{} PRIVATE KEY", "OPENSSH"),
            &[
                "b3BlbnNzaC1rZXktdjEAAAAAc3ludGhldGljLTAx",
                "c3ludGhldGljLW9wZW5zc2gta2V5LWxpbmUtMDIA",
                "c3ludGhldGljLW9wZW5zc2gta2V5LWxpbmUtMDMA",
                "c3ludGhldGljLW9wZW5zc2gta2V5LWxpbmUtMDQA",
                "c3ludGhldGljLTA1==",
            ],
        );
        assert_eq!(pem.lines().count(), 3);
        assert_eq!(openssh.lines().count(), 7);
        for file in [&pem, &openssh] {
            let escaped = file.replace('\n', "\\n");
            let bare = escaped.trim_end_matches("\\n").to_owned();
            for stored in [escaped, bare] {
                assert_eq!(line("KEY", &stored)?, format!("KEY='{file}'"), "{stored}");
                assert_eq!(key_block(&stored).as_deref(), Some(file.as_str()));
            }
        }
        // Not a key: a JSON file kept on one line keeps its escapes, so it is still refused.
        let json = r#"{"private_key":"x\ny","client":"synthetic-1234"}"#;
        assert_eq!(key_block(json), None);
        assert!(line("K", json).is_err());
        // Armour without an end, another escape, or a quote inside: refused as before.
        for odd in [
            format!("{}\\nYWJj", armour("BEGIN", "KEY")),
            format!(
                "{}\\nYW\\tJj\\n{}",
                armour("BEGIN", "KEY"),
                armour("END", "KEY")
            ),
            format!(
                "{}\\nit's\\n{}",
                armour("BEGIN", "KEY"),
                armour("END", "KEY")
            ),
            format!("{}\\n{}", armour("BEGIN", "KEY"), armour("END", "KEY")),
            // Review round 1: the armour's labels differ.
            format!(
                "{}\\nYWJj\\n{}",
                armour("BEGIN", "KEY"),
                armour("END", "OTHER KEY")
            ),
        ] {
            assert_eq!(key_block(&odd), None, "{odd}");
            assert!(line("K", &odd).is_err(), "{odd}");
        }
        Ok(())
    }

    /// SPEC-186 SAV-005: copying a key again (after a rotation) replaces its whole multi-line
    /// value; a multi-line value is never cut in half (its other lines would become stray
    /// assignments, review of SPEC-129 round 2).
    #[test]
    fn a_multi_line_value_being_replaced_is_replaced_whole() -> Result<(), VaultError> {
        let pem = "KEY=\"-----BEGIN KEY-----\nabc\nOTHER=x\n-----END KEY-----\"\nB=1\n";
        assert_eq!(
            upsert(pem, &[entry("KEY", "new-value-1")?])?,
            "KEY=new-value-1\nB=1\n"
        );
        let new_key = key_file("KEY", &["bmV3LWtleS1ib2R5"]).replace('\n', "\\n");
        let copied = upsert("A=1\n", &[entry("KEY", &new_key)?])?;
        assert_eq!(
            upsert(
                &copied,
                &[entry("KEY", "new-value-1")?, entry("A", "a-value-12")?]
            )?,
            "A=a-value-12\nKEY=new-value-1\n"
        );
        assert_eq!(
            upsert(&copied, &[entry("KEY", &new_key)?])?,
            copied,
            "the same key copied again"
        );
        assert_eq!(
            upsert(
                "KEY='a\nb'\nKEY='c\nd'\nB=1\n",
                &[entry("KEY", "new-value-1")?]
            )?,
            "KEY=new-value-1\nB=1\n",
            "a later line of the same name goes, all of it"
        );
        assert!(upsert("KEY='open\nB=1\n", &[entry("KEY", "new-value-1")?]).is_err());
        assert_eq!(
            upsert(pem, &[entry("B", "b-value-12")?])?,
            pem.replace("B=1", "B=b-value-12"),
            "another name's multi-line value is kept as it is"
        );
        assert_eq!(
            upsert("KEY=\"a \\\" b\"\n", &[entry("KEY", "new-value-1")?])?,
            "KEY=new-value-1\n",
            "an escaped quote does not close the value"
        );
        Ok(())
    }

    /// #475: a quote that never closes swallows the rest of the file, so the names appended
    /// after it would land inside that value. The file is refused, naming the line.
    #[test]
    fn a_quote_left_open_at_the_end_is_refused() -> Result<(), VaultError> {
        let text = "A=1\nBROKEN='never closed\nKEY=old\n";
        for entries in [
            vec![entry("NEW", "n-v-456")?],
            vec![entry("KEY", "new-value-1")?],
        ] {
            let Err(VaultError::Value(why)) = upsert(text, &entries) else {
                panic!("an unclosed quote was written after: {entries:?}");
            };
            assert!(why.contains("line 2") && why.contains("BROKEN"), "{why}");
        }
        assert_eq!(
            upsert("A='x\ny'\n", &[entry("NEW", "n-v-456")?])?,
            "A='x\ny'\nNEW=n-v-456\n",
            "a quote closed on a later line is fine"
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
            )?,
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
