//! `secret_verify` (SPEC-178 VAU-002, ADR-0144): whether a value someone holds is an agent
//! secret's value, told by the candidate's SHA-256 so the value reaches neither the model nor the
//! screen. Yulius never receives the candidate, so near matches are variants of the stored value.

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

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

/// The 32 bytes of a SHA-256 written as 64 hexadecimal digits (either case), alone or as the
/// first word of the line `sha256sum` prints (`<hash>  <file>`).
fn parse_sha256(text: &str) -> Option<[u8; 32]> {
    let text = text.split_whitespace().next()?;
    if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// The SHA-256 of `pieces` joined by `glue`, then `end`: the hasher reads the stored value in
/// place, so no copy of it is made, and wipes its state on drop.
fn digest_of(pieces: &[&str], glue: &str, end: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for (i, piece) in pieces.iter().enumerate() {
        if i > 0 {
            hasher.update(glue.as_bytes());
        }
        hasher.update(piece.as_bytes());
    }
    hasher.update(end.as_bytes());
    hasher.finalize().into()
}

/// How the candidate whose SHA-256 is `candidate` compares with `value`. Every variant is
/// hashed and compared with `same` (constant time: the caller chooses the bytes compared), with
/// no early exit, so the answer's timing says nothing about the stored value's hash.
fn likeness(value: &str, candidate: &[u8; 32], same: &dyn Fn(&[u8], &[u8]) -> bool) -> Likeness {
    let whole = [value];
    let lines: Vec<&str> = value.split("\\n").collect();
    let variants = [
        (Likeness::Exact, &whole[..], "", ""),
        (Likeness::TrailingNewline, &whole[..], "", "\n"),
        (Likeness::TrailingNewline, &whole[..], "", "\r\n"),
        (Likeness::EscapedNewlines, &lines[..], "\n", ""),
        (Likeness::EscapedNewlines, &lines[..], "\n", "\n"),
        (Likeness::EscapedNewlines, &lines[..], "\r\n", ""),
        (Likeness::EscapedNewlines, &lines[..], "\r\n", "\r\n"),
    ];
    let mut found = Likeness::Different;
    for (kind, pieces, glue, end) in variants {
        let matched = same(&digest_of(pieces, glue, end), candidate);
        // Without a literal `\n` the multi-line forms are the value itself and its trailing
        // forms, already told apart above.
        let applies = kind != Likeness::EscapedNewlines || pieces.len() > 1;
        if matched && applies && found == Likeness::Different {
            found = kind;
        }
    }
    found
}

/// The lines `value` stands for, its literal `\n` read as line breaks (a final one ends the last
/// line).
fn lines(value: &str) -> usize {
    let breaks = value.matches("\\n").count();
    if value.ends_with("\\n") {
        breaks
    } else {
        breaks + 1
    }
}

/// The key that audits a check once: the vault generation (a rotation audits it again), the
/// sandbox (task numbers and instance-wide secrets repeat across projects), the caller, the secret
/// and a digest of the candidate, never the candidate itself, which on a match is the value's
/// hash.
fn audit_key(
    generation: u64,
    sandbox: &str,
    actor: &str,
    record: &str,
    candidate: &[u8; 32],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"yulius secret_verify audit key|");
    hasher.update(candidate);
    let candidate: [u8; 32] = hasher.finalize().into();
    format!(
        "vault.agent.verify|{generation}|{sandbox}|{actor}|{record}|{}",
        yulius_domain::vault::encode::hex(&candidate, false)
    )
}

impl App {
    /// `secret_verify` (the Superagent's only, ADR-0144): whether the candidate whose SHA-256 the
    /// caller gives is the value of an agent secret of its project (not a production one), or
    /// close to it; audited once per vault generation, sandbox, caller, secret and candidate.
    /// Never answers a value or anything computed from one but the comparison and, on a match,
    /// its line count.
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
        let crypto = self.deps.crypto.as_ref();
        let found = likeness(&payload.value, &candidate, &|a, b| {
            crypto.constant_time_eq(a, b)
        });
        let count = lines(&payload.value);
        drop(payload);
        self.vault_audit_once(
            audit_key(
                self.vault_generation(),
                caller.sandbox.as_str(),
                &caller.actor,
                &record.id,
                &candidate,
            ),
            &caller.actor,
            "vault.agent.verify",
            Some(&record.id),
            &format!("{} → {}", record.name, found.as_str()),
        )
        .await;
        let mut answer = json!({
            "secret": record.name,
            "match": found.as_str(),
            "meaning": found.meaning(&record.name),
        });
        // Only a caller who holds the value learns its shape (review round 1).
        if found != Likeness::Different {
            answer["lines"] = json!(count);
        }
        Ok(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::{Likeness, audit_key, likeness, lines, parse_sha256};
    use sha2::{Digest, Sha256};

    fn digest(text: &str) -> [u8; 32] {
        Sha256::digest(text.as_bytes()).into()
    }

    fn same(a: &[u8], b: &[u8]) -> bool {
        a == b
    }

    const STORED: &str = "-----BEGIN K-----\\nbody-4417\\n-----END K-----";
    const PEM: &str = "-----BEGIN K-----\nbody-4417\n-----END K-----";

    fn of(candidate: &str) -> Likeness {
        likeness(STORED, &digest(candidate), &same)
    }

    #[test]
    fn each_kind_of_match_is_told_apart() {
        assert_eq!(of(STORED), Likeness::Exact);
        assert_eq!(of(&format!("{STORED}\n")), Likeness::TrailingNewline);
        assert_eq!(of(&format!("{STORED}\r\n")), Likeness::TrailingNewline);
        assert_eq!(of(PEM), Likeness::EscapedNewlines);
        assert_eq!(of(&format!("{PEM}\n")), Likeness::EscapedNewlines);
        assert_eq!(of(&PEM.replace('\n', "\r\n")), Likeness::EscapedNewlines);
        assert_eq!(
            of(&format!("{}\r\n", PEM.replace('\n', "\r\n"))),
            Likeness::EscapedNewlines
        );
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
        assert_eq!(likeness(value, &digest(value), &same), Likeness::Exact);
        assert_eq!(
            likeness(value, &digest(&format!("{value}\n")), &same),
            Likeness::TrailingNewline
        );
        assert_eq!(lines(value), 1);
        assert_eq!(lines(STORED), 3);
    }

    #[test]
    fn a_final_escape_ends_the_last_line_without_adding_one() {
        let stored = format!("{STORED}\\n");
        assert_eq!(lines(&stored), 3);
        assert_eq!(
            likeness(&stored, &digest(&format!("{PEM}\n")), &same),
            Likeness::EscapedNewlines
        );
    }

    #[test]
    fn the_audit_key_names_the_sandbox_and_hides_the_candidate() {
        let candidate = digest(STORED);
        let here = audit_key(7, "s1", "cockpit:alice", "as_1", &candidate);
        assert_ne!(
            here,
            audit_key(7, "s2", "cockpit:alice", "as_1", &candidate)
        );
        assert_ne!(
            here,
            audit_key(8, "s1", "cockpit:alice", "as_1", &candidate)
        );
        let hex = yulius_domain::vault::encode::hex(&candidate, false);
        assert!(!here.contains(&hex), "{here}");
    }

    #[test]
    fn sha256sum_lines_are_read_as_their_hash() {
        let hex = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(parse_sha256(&format!("{hex}  key.pem\n")), Some(digest("")));
        assert_eq!(parse_sha256(&format!("{hex}  -")), Some(digest("")));
        assert_eq!(parse_sha256(&format!("{hex}x  key.pem")), None);
    }

    #[test]
    fn hashes_are_64_hex_digits_in_either_case() {
        let hex = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(parse_sha256(hex), Some(digest("")));
        assert_eq!(
            parse_sha256(&format!(" {} ", hex.to_uppercase())),
            Some(digest(""))
        );
        for bad in [
            "",
            "abc",
            &hex[1..],
            &format!("{hex}0"),
            &hex.replace('e', "g"),
        ] {
            assert_eq!(parse_sha256(bad), None, "{bad}");
        }
    }
}
