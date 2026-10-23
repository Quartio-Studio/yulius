//! Values that cannot be told apart from ordinary text (SPEC-274 PLW-001, ADR-0204): redaction
//! masks every occurrence of an agent secret's value, so a configuration word, a host name, an
//! e-mail address or a URL without credentials held as a secret masks ordinary text everywhere.
//! The import refuses them; they belong in configuration or the hand-over.

/// Which kind of ordinary text a value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrdinaryText {
    /// A configuration or dictionary word, or words of them (`production`, `true`, `read-only`).
    Word,
    /// A host name or an IPv4 address, optionally with a port.
    Host,
    /// An e-mail address.
    Email,
    /// A URL that carries no credential.
    Url,
    /// A cloud region, a time zone, a locale or a file path.
    Setting,
}

impl OrdinaryText {
    /// What the value is, for the person ("skipped: a configuration word: …").
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::Word => "a configuration word",
            Self::Host => "a host name or address",
            Self::Email => "an e-mail address",
            Self::Url => "a URL without credentials",
            Self::Setting => "a region, time zone, locale or path",
        }
    }
}

/// Words that configuration files hold as values (environments, branches, switches, levels,
/// roles, plans, regions): never secrets, often everywhere in a repository.
const WORDS: &[&str] = &[
    "true",
    "false",
    "yes",
    "no",
    "on",
    "off",
    "none",
    "null",
    "nil",
    "undefined",
    "empty",
    "default",
    "defaults",
    "enabled",
    "disabled",
    "enable",
    "disable",
    "active",
    "inactive",
    "required",
    "optional",
    "automatic",
    "auto",
    "manual",
    "always",
    "never",
    "inherit",
    "unknown",
    "unset",
    "production",
    "prod",
    "development",
    "develop",
    "dev",
    "staging",
    "stage",
    "testing",
    "test",
    "tests",
    "preview",
    "previews",
    "sandbox",
    "local",
    "localhost",
    "remote",
    "live",
    "demo",
    "qa",
    "uat",
    "integration",
    "nightly",
    "canary",
    "stable",
    "beta",
    "alpha",
    "latest",
    "release",
    "releases",
    "main",
    "master",
    "trunk",
    "debug",
    "verbose",
    "info",
    "notice",
    "warn",
    "warning",
    "error",
    "critical",
    "fatal",
    "trace",
    "silent",
    "quiet",
    "internal",
    "external",
    "public",
    "private",
    "protected",
    "primary",
    "secondary",
    "replica",
    "readonly",
    "read",
    "write",
    "readwrite",
    "only",
    "admin",
    "administrator",
    "user",
    "users",
    "guest",
    "owner",
    "member",
    "viewer",
    "editor",
    "service",
    "server",
    "client",
    "worker",
    "standard",
    "premium",
    "enterprise",
    "business",
    "starter",
    "basic",
    "free",
    "pro",
    "team",
    "personal",
    "global",
    "regional",
    "europe",
    "america",
    "asia",
    "east",
    "west",
    "north",
    "south",
    "central",
    "password",
    "secret",
    "changeme",
    "example",
    "placeholder",
    "sample",
    "dummy",
    "todo",
    "customer",
    "project",
    "environment",
    "application",
    "database",
    "postgres",
    "postgresql",
    "mysql",
    "sqlite",
    "redis",
    "memory",
    "disk",
    "file",
    "files",
    "json",
    "text",
    "plain",
    "pretty",
    "compact",
    "utc",
    "english",
    "polish",
    "german",
    "french",
    "spanish",
];

/// Separators between the words of a configuration value.
fn is_separator(c: char) -> bool {
    matches!(c, '-' | '_' | '.' | ' ')
}

/// Which kind of ordinary text `value` (trimmed) is, `None` when it may be a secret.
#[must_use]
pub fn ordinary_text(value: &str) -> Option<OrdinaryText> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if is_words(value) {
        Some(OrdinaryText::Word)
    } else if is_email(value) {
        Some(OrdinaryText::Email)
    } else if is_host(value) {
        Some(OrdinaryText::Host)
    } else if is_plain_url(value) {
        Some(OrdinaryText::Url)
    } else if is_region(value) || is_time_zone(value) || is_locale(value) || is_path(value) {
        Some(OrdinaryText::Setting)
    } else {
        None
    }
}

/// Letters and separators only, every part a known word.
fn is_words(value: &str) -> bool {
    if !value
        .chars()
        .all(|c| c.is_ascii_alphabetic() || is_separator(c))
    {
        return false;
    }
    let mut parts = value
        .split(is_separator)
        .filter(|p| !p.is_empty())
        .peekable();
    parts.peek().is_some()
        && parts.all(|part| {
            let lower = part.to_ascii_lowercase();
            WORDS.contains(&lower.as_str())
        })
}

/// A lowercase host name (two or more labels, an alphabetic last one) or an IPv4 address, with
/// an optional port.
fn is_host(value: &str) -> bool {
    let host = match value.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => host,
        Some(_) => return false,
        None => value,
    };
    // A host written in capitals is still one; mixed case is not how hosts are written.
    let lower = host.to_ascii_lowercase();
    let host = if host.to_ascii_uppercase() == host {
        lower.as_str()
    } else {
        host
    };
    is_ipv4(host) || is_host_name(host)
}

fn is_ipv4(host: &str) -> bool {
    host.parse::<std::net::Ipv4Addr>().is_ok()
}

fn is_host_name(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    if host.len() > 253 || labels.len() < 2 {
        return false;
    }
    let label_ok = |label: &str| {
        (1..=63).contains(&label.len())
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    let last = labels[labels.len() - 1];
    labels.iter().all(|l| label_ok(l) && !short_token(l))
        && (2..=24).contains(&last.len())
        && last.chars().all(|c| c.is_ascii_lowercase())
}

/// `local@host.name`.
fn is_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && local.len() <= 64
        && local
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-'))
        && is_host_name(&domain.to_ascii_lowercase())
}

/// Query parameter names that carry a credential.
const CREDENTIAL_PARAMS: &[&str] = &[
    "key",
    "token",
    "secret",
    "password",
    "passwd",
    "pwd",
    "pass",
    "sig",
    "signature",
    "auth",
    "credential",
    "credentials",
    "access",
    "session",
    "code",
    "apikey",
    "api_key",
    "client_secret",
];

/// `scheme://…` with no user information, no credential-named parameter and no random-looking
/// path segment or parameter value (a webhook's token).
fn is_plain_url(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once("://") else {
        return false;
    };
    let scheme_ok = scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'));
    if !scheme_ok || rest.is_empty() || rest.chars().any(char::is_whitespace) {
        return false;
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    if authority.contains('@') || authority.is_empty() {
        return false;
    }
    // A webhook's token can be a host label (`<token>.m.pipedream.net`).
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, _)| host);
    if host.split('.').any(short_token) {
        return false;
    }
    let (path, query) = match tail.split_once('?') {
        Some((path, query)) => (path, query.split('#').next().unwrap_or("")),
        None => (tail.split('#').next().unwrap_or(""), ""),
    };
    if path.split('/').any(|p| looks_random(p) || short_token(p)) {
        return false;
    }
    query.split('&').filter(|p| !p.is_empty()).all(|pair| {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let name = name.to_ascii_lowercase();
        let credential_name = CREDENTIAL_PARAMS
            .iter()
            .any(|c| name == *c || name.ends_with(&format!("_{c}")) || name.ends_with(c));
        !credential_name && !looks_random(value) && !short_token(value)
    })
}

/// A short token: 6 or more letters and digits only, mixing both (`abcd1ef`, `AbCdEf1234`), or
/// 6 or more digits (an account or hook id).
fn short_token(part: &str) -> bool {
    part.len() >= 6
        && part.chars().all(|c| c.is_ascii_alphanumeric())
        && part.chars().any(|c| c.is_ascii_digit())
        && (part.chars().any(|c| c.is_ascii_alphabetic())
            || part.chars().all(|c| c.is_ascii_digit()))
}

/// The first word of a cloud region (AWS, GCP, Azure-style names).
const REGION_AREAS: &[&str] = &[
    "us",
    "eu",
    "ap",
    "sa",
    "ca",
    "me",
    "af",
    "il",
    "mx",
    "cn",
    "europe",
    "asia",
    "australia",
    "northamerica",
    "southamerica",
    "africa",
];

/// The other words of a cloud region.
const REGION_WORDS: &[&str] = &[
    "east",
    "west",
    "north",
    "south",
    "central",
    "northeast",
    "northwest",
    "southeast",
    "southwest",
    "gov",
];

/// A cloud region or zone: an area and directions joined by `-`, ending in a number and maybe a
/// zone letter (`us-east-1`, `europe-west4`, `ap-southeast-2a`).
fn is_region(value: &str) -> bool {
    let parts: Vec<&str> = value.split('-').collect();
    let Some((last, words)) = parts.split_last() else {
        return false;
    };
    let Some((area, directions)) = words.split_first() else {
        return false;
    };
    let digits_on = last.trim_start_matches(|c: char| c.is_ascii_lowercase());
    let word = &last[..last.len() - digits_on.len()];
    let number = digits_on.trim_end_matches(|c: char| c.is_ascii_lowercase());
    REGION_AREAS.contains(area)
        && directions.iter().all(|d| REGION_WORDS.contains(d))
        && (word.is_empty() || REGION_WORDS.contains(&word))
        && (1..=2).contains(&number.len())
        && number.chars().all(|c| c.is_ascii_digit())
        && digits_on.len() - number.len() <= 1
}

/// Areas of the time zone database.
const TZ_AREAS: &[&str] = &[
    "Africa",
    "America",
    "Antarctica",
    "Arctic",
    "Asia",
    "Atlantic",
    "Australia",
    "Europe",
    "Indian",
    "Pacific",
    "Etc",
];

/// `Area/Location` of the time zone database (`Europe/Warsaw`).
fn is_time_zone(value: &str) -> bool {
    let parts: Vec<&str> = value.split('/').collect();
    (2..=3).contains(&parts.len())
        && TZ_AREAS.contains(&parts[0])
        && parts[1..].iter().all(|p| {
            p.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                && p.chars()
                    .all(|c| c.is_ascii_alphabetic() || matches!(c, '_' | '-'))
        })
}

/// A POSIX locale (`en_US`, `en_US.UTF-8`).
fn is_locale(value: &str) -> bool {
    let (name, codeset) = value.split_once('.').unwrap_or((value, "utf8"));
    let Some((language, country)) = name.split_once('_') else {
        return false;
    };
    (2..=3).contains(&language.len())
        && language.chars().all(|c| c.is_ascii_lowercase())
        && country.len() == 2
        && country.chars().all(|c| c.is_ascii_uppercase())
        && !codeset.is_empty()
        && codeset
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// An absolute or home path whose parts are names, not tokens (`/var/lib/app`).
fn is_path(value: &str) -> bool {
    let Some(rest) = value.strip_prefix('/').or_else(|| value.strip_prefix("~/")) else {
        return false;
    };
    !rest.is_empty()
        && rest.split('/').all(|part| {
            part.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
                && !looks_random(part)
                && !short_token(part)
        })
}

/// A token rather than a word: 16 or more characters mixing letters and digits, or 24 or more
/// mixing lower and upper case.
fn looks_random(part: &str) -> bool {
    let len = part.chars().count();
    let letters = part.chars().any(|c| c.is_ascii_alphabetic());
    let digits = part.chars().any(|c| c.is_ascii_digit());
    let lower = part.chars().any(|c| c.is_ascii_lowercase());
    let upper = part.chars().any(|c| c.is_ascii_uppercase());
    (len >= 16 && letters && digits) || (len >= 24 && lower && upper)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_words_are_ordinary_text() {
        for value in [
            "production",
            "Production",
            "PRODUCTION",
            "development",
            "staging",
            "main",
            "true",
            "false",
            " production ",
            "read-only",
            "production-west",
            "changeme",
            "password",
        ] {
            assert_eq!(ordinary_text(value), Some(OrdinaryText::Word), "{value}");
        }
    }

    #[test]
    fn hosts_and_addresses_are_ordinary_text() {
        for value in [
            "api.example.com",
            "db.internal:5432",
            "10.0.0.12",
            "192.168.100.200:8080",
            "eu-west-1.amazonaws.com",
            "API.EXAMPLE.COM",
        ] {
            assert_eq!(ordinary_text(value), Some(OrdinaryText::Host), "{value}");
        }
    }

    /// Review of SPEC-274: values most company `.env` files hold.
    #[test]
    fn regions_time_zones_locales_and_paths_are_ordinary_text() {
        for value in [
            "us-east-1",
            "eu-central-1",
            "europe-west4",
            "ap-southeast-2a",
            "us-gov-west-1",
            "Europe/Warsaw",
            "America/Argentina/Buenos_Aires",
            "en_US.UTF-8",
            "/var/lib/app",
            "~/.config/app/settings.json",
        ] {
            assert_eq!(ordinary_text(value), Some(OrdinaryText::Setting), "{value}");
        }
    }

    #[test]
    fn email_addresses_are_ordinary_text() {
        for value in ["ops@example.com", "First.Last+deploy@Quartio.Studio"] {
            assert_eq!(ordinary_text(value), Some(OrdinaryText::Email), "{value}");
        }
    }

    #[test]
    fn urls_without_credentials_are_ordinary_text() {
        for value in [
            "https://api.example.com/v1",
            "https://api.pipedream.com/v1/connect/projects",
            "redis://cache.internal:6379/0",
            "postgres://db.internal/app",
            "https://example.com/callback?state=ok&lang=en",
        ] {
            assert_eq!(ordinary_text(value), Some(OrdinaryText::Url), "{value}");
        }
    }

    #[test]
    fn credentials_stay_secrets() {
        for value in [
            // Keys and tokens.
            "sk_live_51HabcDEFghiJKLmnoPQR",
            "AKIAIOSFODNN7EXAMPLE",
            "ghp_0123456789abcdefABCDEF0123456789abcd",
            "Xk9fP2qLm",
            "correcthorsebatterystaple",
            // A Google app password: four groups of random lowercase letters.
            "abcd efgh ijkl mnop",
            "production1",
            // Words joined with a number are no region.
            "after-value-4",
            "blue-horse-42",
            // URLs carrying a credential.
            "postgres://app:s3cretPass@db.internal/app",
            "https://x-access-token@github.com/o/r.git",
            // Split so that secret scanners do not take the fixture for a real webhook.
            concat!(
                "https://hooks.slack.com/services/",
                "T0AB1CD2E/B0FG3HI4J/",
                "aB3dE5fG7hI9jK1lM3nO5pQ7"
            ),
            "https://api.example.com/v1?api_key=whatever",
            "https://example.com/hook?token=abc",
            concat!(
                "https://discord.com/api/webhooks/",
                "1234/AbCdEfGhIjKlMnOpQrStUvWxYz"
            ),
            // Review of SPEC-274: webhook URLs whose token is short or in the host.
            "https://hooks.zapier.com/hooks/catch/1234567/abcd1ef/",
            // A hook id of letters only: the account id still makes it a token.
            "https://hooks.zapier.com/hooks/catch/1234567/abcdefg/",
            "https://eo1a2b3c4d5e6f7.m.pipedream.net",
            "eo1a2b3c4d5e6f7.m.pipedream.net",
            "https://status.x.com/api/push/AbCdEf1234?status=up",
            // A path holding a token, a mixed-case value that only looks like a host.
            "/hooks/AbCdEf1234567890xyz",
            "Abc.Def.Gh",
            // A JWT-shaped value is no host name.
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.Sfl_KxwRJSMeKKF2QT4fwpMeJf36POk6yJV",
            "",
            "   ",
        ] {
            assert_eq!(ordinary_text(value), None, "{value}");
        }
    }

    #[test]
    fn every_kind_is_described() {
        for kind in [
            OrdinaryText::Word,
            OrdinaryText::Host,
            OrdinaryText::Email,
            OrdinaryText::Url,
            OrdinaryText::Setting,
        ] {
            assert!(!kind.describe().is_empty());
        }
    }
}
