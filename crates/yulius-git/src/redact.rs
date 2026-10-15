//! Credential redaction for text that may reach logs or error messages (SEC-002).

use yulius_domain::redact::{INSTALLATION_TOKEN_PREFIX, stateless_installation_token_len};

/// `ghs_` first: a stateless installation token is removed whole before another prefix found
/// inside its base64url payload or signature could cut it.
const TOKEN_PREFIXES: &[&str] = &["ghs_", "ghp_", "gho_", "ghu_", "ghr_", "github_pat_"];
const REDACTED: &str = "[redacted]";

/// Removes credential-shaped substrings from `text`: URL user information
/// (`https://user:secret@host`), GitHub token literals (`ghs_…`, `github_pat_…`),
/// `Bearer …`/`Basic …` values and `password=…` credential-protocol lines.
#[must_use]
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if line.trim_start().starts_with("password=") {
            out.push_str("password=");
            out.push_str(REDACTED);
            continue;
        }
        out.push_str(&redact_line(line));
    }
    out
}

fn redact_line(line: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut redact_next = false;
    for word in line.split(' ') {
        if redact_next && !word.is_empty() {
            words.push(REDACTED.to_owned());
            redact_next = false;
            continue;
        }
        let lower = word.to_ascii_lowercase();
        if lower == "bearer" || lower == "basic" || lower == "token" {
            redact_next = true;
        }
        words.push(redact_word(word));
    }
    words.join(" ")
}

fn redact_word(word: &str) -> String {
    let mut word = redact_userinfo(word);
    for prefix in TOKEN_PREFIXES {
        let mut from = 0;
        while let Some(found) = word[from..].find(prefix) {
            let start = from + found;
            let tail = &word[start + prefix.len()..];
            let len = token_len(prefix, tail);
            if len == 0 {
                // A bare prefix: search on after it.
                from = start + prefix.len();
                continue;
            }
            word.replace_range(start..start + prefix.len() + len, REDACTED);
            from = start + REDACTED.len();
        }
    }
    word
}

/// Length of the token after `prefix`: a stateless installation token (`ghs_<APPID>_<JWT>`)
/// through its signature, any other through its `[A-Za-z0-9_]` run.
fn token_len(prefix: &str, tail: &str) -> usize {
    let stateless = if prefix == INSTALLATION_TOKEN_PREFIX {
        stateless_installation_token_len(tail)
    } else {
        None
    };
    stateless.unwrap_or_else(|| {
        tail.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(tail.len())
    })
}

fn redact_userinfo(word: &str) -> String {
    let Some(scheme_end) = word.find("://") else {
        return word.to_owned();
    };
    let authority_start = scheme_end + 3;
    let rest = &word[authority_start..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{}{}{}", &word[..authority_start], REDACTED, &rest[at..]),
        None => word.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn removes_credentials_but_keeps_context() {
        let text = "fatal: unable to access 'https://x-access-token:ghs_abc123DEF@github.com/o/r.git/': 403\n\
                    password=hunter2\nAuthorization: Bearer abc.def.ghi\ntoken github_pat_11ABC_xyz";
        let clean = redact(text);
        assert!(!clean.contains("ghs_abc123DEF"), "{clean}");
        assert!(!clean.contains("hunter2"), "{clean}");
        assert!(!clean.contains("abc.def.ghi"), "{clean}");
        assert!(!clean.contains("github_pat_11ABC_xyz"), "{clean}");
        assert!(clean.contains("github.com/o/r.git"), "{clean}");
        assert!(clean.contains("403"), "{clean}");
    }

    /// #424: a stateless installation token (`ghs_<APPID>_<JWT>`) is removed whole, not cut at
    /// the JWT's first `.`.
    #[test]
    fn removes_a_whole_stateless_installation_token() {
        let token = format!(
            "ghs_1234567_eyJ{}.eyJ{}.{}",
            "hbGciOiJSUzI1NiJ9".repeat(2),
            "pc3MiOiIxIn0".repeat(10),
            "c2lnbmF0dXJl_-".repeat(16)
        );
        let text = format!(
            "fatal: unable to access 'https://x-access-token:{token}@github.com/o/r.git/'\n\
             remote: {token}."
        );
        assert_eq!(
            redact(&text),
            "fatal: unable to access 'https://[redacted]@github.com/o/r.git/'\nremote: [redacted]."
        );
    }

    /// Review of SPEC-138: a payload that happens to hold another prefix (`ghu_`) is not cut
    /// there, leaving the rest of the token.
    #[test]
    fn a_stateless_token_holding_another_prefix_is_removed_whole() {
        let token = format!(
            "ghs_1234567_eyJ{}.eyJpc3MiOiIxghu_AB-{}.{}",
            "hbGciOiJSUzI1NiJ9".repeat(2),
            "PAYLOADSECRET".repeat(3),
            "SIGNATURESECRET_-ghp_x".repeat(3)
        );
        assert_eq!(
            redact(&format!("remote: {token} failed")),
            "remote: [redacted] failed"
        );
    }

    /// Review of SPEC-138, round 2: a bare prefix earlier in the word does not end the search.
    #[test]
    fn a_bare_prefix_does_not_hide_a_later_token() {
        let token = format!(
            "ghs_1234567_eyJ{}.eyJpc3MiOiIx.c2lnbmF0dXJl",
            "hbGciOiJSUzI1NiJ9"
        );
        assert_eq!(
            redact(&format!("ghs_:{token} and ghp_-ghp_{}", "aB3dE".repeat(8))),
            "ghs_:[redacted] and ghp_-[redacted]"
        );
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        let text = "error: pathspec 'x' did not match any file(s) known to git";
        assert_eq!(redact(text), text);
    }
}
