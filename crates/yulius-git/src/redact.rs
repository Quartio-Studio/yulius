//! Credential redaction for text that may reach logs or error messages (SEC-002).

const TOKEN_PREFIXES: &[&str] = &["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"];
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
        while let Some(start) = word.find(prefix) {
            let tail = &word[start + prefix.len()..];
            let len = tail
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(tail.len());
            if len == 0 {
                break;
            }
            word.replace_range(start..start + prefix.len() + len, REDACTED);
        }
    }
    word
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

    #[test]
    fn leaves_ordinary_text_alone() {
        let text = "error: pathspec 'x' did not match any file(s) known to git";
        assert_eq!(redact(text), text);
    }
}
