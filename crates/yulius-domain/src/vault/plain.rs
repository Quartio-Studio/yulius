//! Which values of a `.env` are a project's variables rather than its secrets (SPEC-275 VAR-006,
//! ADR-0203): a region, an environment name, a base URL or a contact address belongs in project
//! variables, shown in plain, not in agent secrets, where it would be refused (shorter than 8
//! characters) or as ordinary text (SPEC-274's rule, [`ordinary_text`]).
//!
//! The rule errs towards "secret": a name that says it is a credential keeps its value a secret
//! whatever the value looks like, because a variable is shown to every agent in plain.

use super::{MIN_AGENT_VALUE_CHARS, OrdinaryText, ordinary_text};

/// Why a value is not a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotASecret {
    /// Shorter than an agent secret may be, and plain-shaped.
    Short,
    /// Ordinary text (SPEC-274): a configuration word, a host, an e-mail address, a URL without
    /// credentials, a region, time zone, locale or path.
    Ordinary(OrdinaryText),
}

impl NotASecret {
    /// What it is, for a person reading the plan.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::Short => "shorter than 8 characters",
            Self::Ordinary(kind) => kind.describe(),
        }
    }
}

/// Parts of a name that say its value is a credential, wherever they occur in it.
const CREDENTIAL_PARTS: &[&str] = &[
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "PASSPHRASE",
    "CREDENTIAL",
    "PRIVATE",
    "MNEMONIC",
];

/// What a word of a name (between `_`) ends with when its value is a credential: `API_KEY` and
/// `APIKEY`, `DB_PASS` and `DBPASS`, `OAUTH`; never `KEYCLOAK`, `AUTHOR` or `TOKENIZER`.
const CREDENTIAL_ENDINGS: &[&str] = &[
    "KEY",
    "KEYS",
    "TOKEN",
    "TOKENS",
    "PASS",
    "PW",
    "PWD",
    "PHRASE",
    "CRED",
    "CREDS",
    "CREDENTIALS",
    "AUTH",
    "COOKIE",
    "SIG",
    "SIGNATURE",
    "SALT",
    "CERT",
    "PIN",
    "OTP",
    "DSN",
    "SEED",
    "HOOK",
    "LICENSE",
    "CODE",
    "BEARER",
    "JWT",
    "HMAC",
];

/// Whether `name` says its value is a credential (`API_KEY`, `DBPASS`, `BORG_PASSPHRASE`, …).
#[must_use]
pub fn credential_name(name: &str) -> bool {
    let name = name.trim().to_ascii_uppercase();
    CREDENTIAL_PARTS.iter().any(|part| name.contains(part))
        || name
            .split('_')
            // A numbered one (`DB_PASS2`, `ADMIN_PW1`) is one too.
            .map(|word| word.trim_end_matches(|c: char| c.is_ascii_digit()))
            .any(|word| CREDENTIAL_ENDINGS.iter().any(|end| word.ends_with(end)))
}

/// Why the value of `name` is a plain configuration value rather than a secret, or `None` when
/// it may be a secret.
#[must_use]
pub fn not_a_secret(name: &str, value: &str) -> Option<NotASecret> {
    if credential_name(name) || value.chars().any(char::is_control) || value.contains("-----BEGIN ")
    {
        return None;
    }
    if value.chars().count() < MIN_AGENT_VALUE_CHARS {
        short_plain(value).then_some(NotASecret::Short)
    } else {
        ordinary_text(value)
            .filter(|kind| !carries_in_plain_view(*kind, value))
            .map(NotASecret::Ordinary)
    }
}

/// Whether a value carries a credential however it is named: a URL with user info, or a key.
#[must_use]
pub fn carries_credential(value: &str) -> bool {
    value.contains("-----BEGIN ")
        || value.split_once("://").is_some_and(|(_, rest)| {
            rest.split(['/', '?', '#'])
                .next()
                .is_some_and(|authority| authority.contains('@'))
        })
}

/// What ordinary text may still carry once shown to every agent (review round 5): a URL's
/// fragment (a share link's decryption key, an `#access_token=`), or an address whose local part
/// is a token (an inbound address that triggers a webhook). Such a value is not refused as a
/// secret, but it is not moved into plain view either.
fn carries_in_plain_view(kind: OrdinaryText, value: &str) -> bool {
    match kind {
        OrdinaryText::Url => value.contains('#'),
        OrdinaryText::Email => value
            .split_once('@')
            .is_some_and(|(local, _)| token_like(local)),
        _ => false,
    }
}

/// A run of 12 or more letters and digits holding a digit (`a1b2c3d4e5f6…`, an id-like token).
fn token_like(text: &str) -> bool {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|run| run.len() >= 12 && run.bytes().any(|b| b.is_ascii_digit()))
}

/// `ovh-ca`, `EU`, `8080`: a short value in one case of letters, digits and `-`, `_`, `.` (a
/// short password mixes cases or symbols, and stays a secret, refused as too short).
fn short_plain(value: &str) -> bool {
    let shape = |b: u8| b.is_ascii_digit() || b"-_.".contains(&b);
    value.bytes().all(|b| b.is_ascii_lowercase() || shape(b))
        || value.bytes().all(|b| b.is_ascii_uppercase() || shape(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-275 VAR-006: the company `.env` of SPEC-256's first user.
    #[test]
    fn plain_values_are_told_from_secrets() {
        for (name, value, why) in [
            ("OVH_ENDPOINT", "ovh-ca", NotASecret::Short),
            ("APP_ENV", "", NotASecret::Short),
            ("REGION", "EU", NotASecret::Short),
            ("SESSION_TIMEOUT", "3600", NotASecret::Short),
            (
                "PIPEDREAM_PROJECT_ENVIRONMENT",
                "production",
                NotASecret::Ordinary(OrdinaryText::Word),
            ),
            (
                "AWS_REGION",
                "eu-west-1",
                NotASecret::Ordinary(OrdinaryText::Setting),
            ),
            (
                "LANGFUSE_BASE_URL",
                "https://cloud.langfuse.com",
                NotASecret::Ordinary(OrdinaryText::Url),
            ),
            (
                "KEYCLOAK_URL",
                "https://sso.example.com",
                NotASecret::Ordinary(OrdinaryText::Url),
            ),
            (
                "CONTACT_EMAIL",
                "ops@quartio.studio",
                NotASecret::Ordinary(OrdinaryText::Email),
            ),
            (
                "AUTHOR_EMAIL",
                "dev@example.com",
                NotASecret::Ordinary(OrdinaryText::Email),
            ),
        ] {
            assert_eq!(not_a_secret(name, value), Some(why), "{name}={value}");
        }
    }

    #[test]
    fn secrets_stay_secrets() {
        for (name, value) in [
            // Credential names, whatever the value.
            ("API_TOKEN", "production"),
            ("DB_PASSWORD", "correct-horse-battery"),
            ("OVH_PASS", "short"),
            ("STRIPE_KEY", "https://example.com"),
            ("APIKEY", "abc"),
            ("SLACK_WEBHOOK_URL", "https://example.com/hook"),
            ("SENTRY_DSN", "https://sentry.io/1"),
            // Review round 1: credential words inside a word, other credential words.
            ("DBPASS", "hunter2"),
            ("ADMIN_PW", "s3cr3t"),
            ("ROOTPW", "abc"),
            ("BACKUP_CODE", "482913"),
            ("BORG_PASSPHRASE", "correct-horse-battery"),
            ("WALLET_MNEMONIC", "abandon-ability-able"),
            ("APP_LICENSE", "ABC-123-XYZ"),
            (
                "ZAPIER_HOOK_URL",
                "https://hooks.zapier.com/hooks/catch/123456/abcdef1/",
            ),
            // Review round 2: a digit after a credential word.
            ("DB_PASS2", "hunter2"),
            ("ADMIN_PW1", "s3cr3t"),
            ("API_KEY_2", "abc"),
            // Short values that are not plain-shaped.
            ("UPSTREAM", "aB3$xZ9"),
            ("UPSTREAM", "hunt3r!"),
            ("UPSTREAM", "Ab1"),
            // Values SPEC-274's rule keeps secret, or that carry a credential.
            ("UPSTREAM", "aXbQrTzLmNoPqRsT"),
            ("UPSTREAM", "https://user:pass@db.example.com"),
            ("UPSTREAM", "https://api.example.com/v1?key=abc"),
            (
                "PIPEDREAM_ENDPOINT",
                "https://eo1a2b3c4d5e6f7.m.pipedream.net",
            ),
            ("UPSTREAM", "-----BEGIN OPENSSH PRIVATE KEY----- abc"),
            ("UPSTREAM", "two\nlines-here"),
            // Review round 5: a key in a URL's fragment, a token as an address's local part.
            (
                "SHARE_URL",
                "https://files.example.com/f/abc#k3yInTheFragment",
            ),
            (
                "INBOUND_ADDRESS",
                "a1b2c3d4e5f6a7b8c9d0@inbound.example.com",
            ),
        ] {
            assert_eq!(not_a_secret(name, value), None, "{name}={value}");
        }
    }

    #[test]
    fn credential_names_are_recognised_by_part_and_by_word() {
        for name in [
            "OPENAI_API_KEY",
            "GH_TOKEN",
            "OVH_APP_SECRET",
            "SMTP_PASS",
            "pin",
            "DBPASS",
            "ROOTPW",
            "APIKEY",
            "AWS_SECRET_ACCESS_KEY",
            "OAUTH",
            "DB_CREDS",
            "GOOGLE_CREDENTIALS",
            "SLACK_WEBHOOK_URL",
            "SESSION_COOKIE",
            "JWT_SIGNING_KEY",
            "TLS_CERT",
            // A number after a credential word keeps it one; `AUTH0_*` is caught too, on the safe side.
            "DB_PASS2",
            "AUTH0_DOMAIN",
        ] {
            assert!(credential_name(name), "{name}");
        }
        for name in [
            "OVH_ENDPOINT",
            "PASSAGE_URL",
            "SPINE_REGION",
            "CONTACT_EMAIL",
            "KEYCLOAK_URL",
            "AUTHOR_EMAIL",
            "TOKENIZER_MODEL",
            "SESSION_TIMEOUT",
        ] {
            assert!(!credential_name(name), "{name}");
        }
    }

    #[test]
    fn credentials_in_values_are_recognised() {
        assert!(carries_credential("postgres://u:p@db/app"));
        assert!(carries_credential("-----BEGIN RSA PRIVATE KEY-----"));
        assert!(!carries_credential("https://cloud.langfuse.com/a@b"));
        assert!(!carries_credential("ops@quartio.studio"));
    }
}
