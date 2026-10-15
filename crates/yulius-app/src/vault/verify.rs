//! `secret_verify` (SPEC-178 VAU-002, ADR-0144): whether a value someone holds is an agent
//! secret's value, told by the candidate's SHA-256 so the value reaches neither the model nor the
//! screen. Yulius never receives the candidate, so near matches are variants of the stored value.

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::AgentPayload;
use crate::context::App;
use crate::coordination::ProjectCaller;
use crate::error::{AppError, AppResult};

/// Arguments of `secret_verify`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifyArgs {
    secret: String,
    sha256: String,
}

/// How a candidate compares with a stored value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Likeness {
    /// The very value.
    Exact,
    /// The value followed by a line break (a file's last line, `echo`).
    TrailingNewline,
    /// The value with its literal `\n` as line breaks (LF or CRLF, with or without a final one):
    /// a multi-line key, which the vault holds on one line.
    EscapedNewlines,
    Different,
}

impl Likeness {
    fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::TrailingNewline => "trailing_newline",
            Self::EscapedNewlines => "escaped_newlines",
            Self::Different => "different",
        }
    }

    fn meaning(self, name: &str) -> String {
        match self {
            Self::Exact => format!("The candidate is exactly {name}'s value."),
            Self::TrailingNewline => format!(
                "The candidate is {name}'s value followed by a line break (a file's final newline, echo): the value matches, without that line break."
            ),
            Self::EscapedNewlines => format!(
                "The candidate is {name}'s value with line breaks where the vault holds literal \\n (an agent secret never spans lines, so a multi-line key is stored that way): the key matches; code reading the secret turns each \\n into a line break."
            ),
            Self::Different => format!(
                "The candidate is not {name}'s value, not even with a final line break or with its line breaks written as \\n."
            ),
        }
    }
}

/// The 32 bytes of a SHA-256 written as 64 hexadecimal digits (either case, spaces around).
fn parse_sha256(text: &str) -> Option<[u8; 32]> {
    let text = text.trim();
    if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

fn digest(text: &str) -> [u8; 32] {
    Sha256::digest(text.as_bytes()).into()
}

/// How the candidate whose SHA-256 is `candidate` compares with `value`.
fn likeness(value: &str, candidate: &[u8; 32]) -> Likeness {
    let is = |variant: &str| digest(variant) == *candidate;
    if is(value) {
        return Likeness::Exact;
    }
    for end in ["\n", "\r\n"] {
        if is(&Zeroizing::new(format!("{value}{end}"))) {
            return Likeness::TrailingNewline;
        }
    }
    if value.contains("\\n") {
        for line_break in ["\n", "\r\n"] {
            let lines = Zeroizing::new(value.replace("\\n", line_break));
            if is(&lines) || is(&Zeroizing::new(format!("{}{line_break}", *lines))) {
                return Likeness::EscapedNewlines;
            }
        }
    }
    Likeness::Different
}

/// The lines `value` stands for, its literal `\n` read as line breaks.
fn lines(value: &str) -> usize {
    value.matches("\\n").count() + 1
}

impl App {
    /// `secret_verify`: whether the candidate whose SHA-256 the caller gives is the value of an
    /// agent secret of its project (not a production one), or close to it; audited once per
    /// caller, secret and candidate. Never answers a value or anything computed from one but the
    /// comparison and its line count.
    pub(crate) async fn tool_secret_verify(
        &self,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<Value> {
        let args: VerifyArgs = serde_json::from_value(args.clone())
            .map_err(|e| AppError::Invalid(format!("invalid arguments: {e}")))?;
        let candidate = parse_sha256(&args.sha256).ok_or_else(|| {
            AppError::Invalid(
                "sha256 must be the candidate's SHA-256 as 64 hexadecimal digits (sha256sum prints it)"
                    .into(),
            )
        })?;
        let record = self
            .deps
            .store
            .vault_agent(Some(&caller.sandbox))
            .await?
            .into_iter()
            .find(|r| r.name == args.secret.trim())
            .ok_or_else(|| {
                AppError::Invalid(format!(
                    "no agent secret named {:?} applies to this project (see secrets_list)",
                    args.secret
                ))
            })?;
        if record.cd_prod {
            return Err(AppError::Forbidden(super::PRODUCTION_SECRET.into()));
        }
        let payload: AgentPayload = self.open_payload(&record)?;
        let found = likeness(&payload.value, &candidate);
        let count = lines(&payload.value);
        drop(payload);
        self.vault_audit_once(
            format!(
                "vault.agent.verify|{}|{}|{}",
                caller.actor,
                record.id,
                yulius_domain::vault::encode::hex(&candidate, false)
            ),
            &caller.actor,
            "vault.agent.verify",
            Some(&record.id),
            &format!("{} → {}", record.name, found.as_str()),
        )
        .await;
        Ok(json!({
            "secret": record.name,
            "match": found.as_str(),
            "lines": count,
            "meaning": found.meaning(&record.name),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{Likeness, digest, likeness, lines, parse_sha256};

    const STORED: &str = "-----BEGIN K-----\\nbody-4417\\n-----END K-----";
    const PEM: &str = "-----BEGIN K-----\nbody-4417\n-----END K-----";

    fn of(candidate: &str) -> Likeness {
        likeness(STORED, &digest(candidate))
    }

    #[test]
    fn each_kind_of_match_is_told_apart() {
        assert_eq!(of(STORED), Likeness::Exact);
        assert_eq!(of(&format!("{STORED}\n")), Likeness::TrailingNewline);
        assert_eq!(of(&format!("{STORED}\r\n")), Likeness::TrailingNewline);
        assert_eq!(of(PEM), Likeness::EscapedNewlines);
        assert_eq!(of(&format!("{PEM}\n")), Likeness::EscapedNewlines);
        assert_eq!(of(&PEM.replace('\n', "\r\n")), Likeness::EscapedNewlines);
        assert_eq!(of(&format!("{}\r\n", PEM.replace('\n', "\r\n"))), Likeness::EscapedNewlines);
    }

    #[test]
    fn a_close_candidate_is_different() {
        assert_eq!(of(&PEM.replace("4417", "4418")), Likeness::Different);
        assert_eq!(of(&format!("{STORED}\n\n")), Likeness::Different);
        assert_eq!(of(&format!(" {STORED}")), Likeness::Different);
        // Mixed line breaks are not one of the variants.
        assert_eq!(of(&PEM.replacen('\n', "\r\n", 1)), Likeness::Different);
    }

    #[test]
    fn a_value_without_escapes_has_no_multi_line_form() {
        let value = "sk_synthetic_value_7781";
        assert_eq!(likeness(value, &digest(value)), Likeness::Exact);
        assert_eq!(likeness(value, &digest(&format!("{value}\n"))), Likeness::TrailingNewline);
        assert_eq!(lines(value), 1);
        assert_eq!(lines(STORED), 3);
    }

    #[test]
    fn hashes_are_64_hex_digits_in_either_case() {
        let hex = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(parse_sha256(hex), Some(digest("")));
        assert_eq!(parse_sha256(&format!(" {} ", hex.to_uppercase())), Some(digest("")));
        for bad in ["", "abc", &hex[1..], &format!("{hex}0"), &hex.replace('e', "g")] {
            assert_eq!(parse_sha256(bad), None, "{bad}");
        }
    }
}
