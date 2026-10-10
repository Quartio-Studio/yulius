//! Secret redaction for session transcripts (SPEC-001 IMP-008, SES-004).
//!
//! Deterministic and pure: the same text always redacts to the same text, so a redacted
//! transcript is content addressed like the original (IMP-006) and redacting twice changes
//! nothing. A secret is replaced by `[REDACTED:<rule>]`; assignments keep their name
//! (`DATABASE_PASSWORD=[REDACTED:secret-assignment]`). A [`Finding`] names the rule and a label
//! that is never the secret (a key name or a well-known token prefix).
//!
//! The rules follow the default rule set of gitleaks (rule ids reused where they exist): tokens of
//! well-known providers by their documented prefixes, private key blocks, JSON Web Tokens,
//! credentials in URLs, `Authorization` headers, secret-named assignments with a literal value,
//! and every value of a dotenv file ([`redact_env`]) or of a run of three or more dotenv lines.
//! Placeholders (`${VAR}`, `<token>`, `changeme`, `xxxx`), references (`process.env.X`) and type
//! annotations (`token: string`) are left alone.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Prefix of every replacement.
pub const REDACTED_PREFIX: &str = "[REDACTED:";
/// Longest value examined for one assignment or quoted literal.
const MAX_VALUE_CHARS: usize = 512;
/// Longest token run examined after a well-known prefix.
const MAX_TOKEN_CHARS: usize = 1024;
/// Longest private key block redacted when its END line is missing.
const MAX_KEY_BLOCK: usize = 16 * 1024;

/// The detection rule behind a redaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rule {
    /// `-----BEGIN … PRIVATE KEY-----` blocks.
    PrivateKey,
    /// AWS access key ids (`AKIA…`, `ASIA…`).
    AwsAccessToken,
    /// GitHub tokens (`ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`, `github_pat_`).
    GithubToken,
    /// GitLab personal access tokens (`glpat-`).
    GitlabToken,
    /// Anthropic API keys (`sk-ant-`).
    AnthropicApiKey,
    /// OpenAI API keys (`sk-proj-`, `sk-svcacct-`, `sk-admin-`, long `sk-`).
    OpenaiApiKey,
    /// Slack tokens (`xoxb-`, `xoxp-`, `xapp-`, …) and incoming webhooks.
    SlackToken,
    /// Stripe keys and webhook secrets (`sk_live_`, `rk_live_`, `whsec_`, …).
    StripeAccessToken,
    /// Google API keys (`AIza…`) and OAuth client secrets (`GOCSPX-`).
    GcpApiKey,
    /// JSON Web Tokens (`eyJ….eyJ….…`).
    Jwt,
    /// Package registry tokens (npm `npm_`, PyPI `pypi-`).
    RegistryToken,
    /// Other provider tokens with documented prefixes (Hugging Face, SendGrid, DigitalOcean,
    /// Shopify, Docker Hub, Linear, Supabase, Notion, Figma, Sentry, age keys).
    ProviderToken,
    /// Yulius CLI tokens (`yul_`).
    YuliusToken,
    /// The password of a `scheme://user:password@host` URL.
    UrlCredentials,
    /// `Bearer` / `Basic` authorization values.
    AuthorizationHeader,
    /// A literal assigned to a secret-named key (`password = "…"`, `API_KEY: …`).
    SecretAssignment,
    /// A dotenv value.
    EnvValue,
}

impl Rule {
    /// Stable rule id (gitleaks-style).
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::PrivateKey => "private-key",
            Self::AwsAccessToken => "aws-access-token",
            Self::GithubToken => "github-token",
            Self::GitlabToken => "gitlab-pat",
            Self::AnthropicApiKey => "anthropic-api-key",
            Self::OpenaiApiKey => "openai-api-key",
            Self::SlackToken => "slack-token",
            Self::StripeAccessToken => "stripe-access-token",
            Self::GcpApiKey => "gcp-api-key",
            Self::Jwt => "jwt",
            Self::RegistryToken => "registry-token",
            Self::ProviderToken => "provider-token",
            Self::YuliusToken => "yulius-token",
            Self::UrlCredentials => "url-credentials",
            Self::AuthorizationHeader => "authorization-header",
            Self::SecretAssignment => "secret-assignment",
            Self::EnvValue => "env-value",
        }
    }

    /// Preference when two detections overlap (lower wins).
    fn priority(self) -> u8 {
        match self {
            Self::SecretAssignment => 4,
            Self::EnvValue => 5,
            Self::AuthorizationHeader => 3,
            Self::UrlCredentials => 2,
            Self::Jwt => 1,
            _ => 0,
        }
    }
}

/// One redacted secret: the rule and a label that never contains the secret.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Finding {
    /// The rule that matched.
    pub rule: Rule,
    /// A key name (`DATABASE_PASSWORD`) or a well-known prefix (`ghp_`).
    pub label: String,
}

/// Redacted text and what was redacted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redacted {
    /// The text with every secret replaced.
    pub text: String,
    /// The redactions, in text order.
    pub findings: Vec<Finding>,
}

/// Findings counted by rule id and by (rule id, label).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RedactionTally {
    /// Redactions per rule id.
    pub by_rule: BTreeMap<&'static str, u64>,
    /// Redactions per (rule id, label).
    pub by_label: BTreeMap<(&'static str, String), u64>,
}

impl RedactionTally {
    /// Adds findings.
    pub fn add(&mut self, findings: &[Finding]) {
        for finding in findings {
            *self.by_rule.entry(finding.rule.id()).or_default() += 1;
            *self
                .by_label
                .entry((finding.rule.id(), finding.label.clone()))
                .or_default() += 1;
        }
    }

    /// Adds another tally.
    pub fn merge(&mut self, other: &Self) {
        for (rule, count) in &other.by_rule {
            *self.by_rule.entry(rule).or_default() += count;
        }
        for (key, count) in &other.by_label {
            *self.by_label.entry(key.clone()).or_default() += count;
        }
    }

    /// Total redactions.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.by_rule.values().sum()
    }

    /// `true` when nothing was redacted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_rule.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Charset {
    /// `[A-Za-z0-9]`.
    Alnum,
    /// `[A-Za-z0-9_]`.
    AlnumUnderscore,
    /// `[A-Za-z0-9_-]`.
    Base64Url,
    /// `[A-Za-z0-9_.-]`.
    Base64UrlDot,
    /// `[A-Za-z]`.
    Alpha,
    /// `[a-fA-F0-9]`.
    Hex,
    /// `[A-Z2-7]` (AWS key ids).
    UpperBase32,
}

impl Charset {
    fn contains(self, byte: u8) -> bool {
        match self {
            Self::Alnum => byte.is_ascii_alphanumeric(),
            Self::AlnumUnderscore => byte.is_ascii_alphanumeric() || byte == b'_',
            Self::Base64Url => byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'),
            Self::Base64UrlDot => {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
            }
            Self::Alpha => byte.is_ascii_alphabetic(),
            Self::Hex => byte.is_ascii_hexdigit(),
            Self::UpperBase32 => byte.is_ascii_uppercase() || (b'2'..=b'7').contains(&byte),
        }
    }
}

/// A token recognized by its documented prefix and the characters that follow it.
struct TokenRule {
    rule: Rule,
    prefix: &'static str,
    charset: Charset,
    /// Fewest characters after the prefix.
    min: usize,
}

const fn token(rule: Rule, prefix: &'static str, charset: Charset, min: usize) -> TokenRule {
    TokenRule {
        rule,
        prefix,
        charset,
        min,
    }
}

/// Prefix rules (gitleaks default rules where one exists). Longer prefixes first, so `sk-ant-`
/// wins over `sk-` at the same position.
const TOKEN_RULES: &[TokenRule] = &[
    token(
        Rule::GithubToken,
        "github_pat_",
        Charset::AlnumUnderscore,
        60,
    ),
    token(Rule::GithubToken, "ghp_", Charset::Alnum, 36),
    token(Rule::GithubToken, "gho_", Charset::Alnum, 36),
    token(Rule::GithubToken, "ghu_", Charset::Alnum, 36),
    token(Rule::GithubToken, "ghs_", Charset::Alnum, 36),
    token(Rule::GithubToken, "ghr_", Charset::Alnum, 36),
    token(Rule::GitlabToken, "glpat-", Charset::Base64Url, 20),
    token(Rule::AnthropicApiKey, "sk-ant-", Charset::Base64Url, 32),
    token(Rule::OpenaiApiKey, "sk-proj-", Charset::Base64Url, 32),
    token(Rule::OpenaiApiKey, "sk-svcacct-", Charset::Base64Url, 32),
    token(Rule::OpenaiApiKey, "sk-admin-", Charset::Base64Url, 32),
    token(Rule::OpenaiApiKey, "sk-", Charset::Base64Url, 40),
    token(Rule::SlackToken, "xoxb-", Charset::Base64Url, 10),
    token(Rule::SlackToken, "xoxp-", Charset::Base64Url, 10),
    token(Rule::SlackToken, "xoxa-", Charset::Base64Url, 10),
    token(Rule::SlackToken, "xoxr-", Charset::Base64Url, 10),
    token(Rule::SlackToken, "xoxs-", Charset::Base64Url, 10),
    token(Rule::SlackToken, "xoxe-", Charset::Base64Url, 10),
    token(Rule::SlackToken, "xapp-", Charset::Base64Url, 20),
    token(Rule::StripeAccessToken, "sk_live_", Charset::Alnum, 16),
    token(Rule::StripeAccessToken, "sk_test_", Charset::Alnum, 16),
    token(Rule::StripeAccessToken, "rk_live_", Charset::Alnum, 16),
    token(Rule::StripeAccessToken, "rk_test_", Charset::Alnum, 16),
    token(Rule::StripeAccessToken, "whsec_", Charset::Alnum, 24),
    token(Rule::GcpApiKey, "AIza", Charset::Base64Url, 35),
    token(Rule::GcpApiKey, "GOCSPX-", Charset::Base64Url, 24),
    token(Rule::RegistryToken, "npm_", Charset::Alnum, 36),
    token(
        Rule::RegistryToken,
        "pypi-AgEIcHlwaS5vcmc",
        Charset::Base64Url,
        40,
    ),
    token(Rule::ProviderToken, "hf_", Charset::Alpha, 30),
    token(Rule::ProviderToken, "SG.", Charset::Base64UrlDot, 60),
    token(Rule::ProviderToken, "dop_v1_", Charset::Hex, 64),
    token(Rule::ProviderToken, "doo_v1_", Charset::Hex, 64),
    token(Rule::ProviderToken, "dor_v1_", Charset::Hex, 64),
    token(Rule::ProviderToken, "shpat_", Charset::Hex, 32),
    token(Rule::ProviderToken, "shpss_", Charset::Hex, 32),
    token(Rule::ProviderToken, "shpca_", Charset::Hex, 32),
    token(Rule::ProviderToken, "shppa_", Charset::Hex, 32),
    token(Rule::ProviderToken, "dckr_pat_", Charset::Base64Url, 24),
    token(Rule::ProviderToken, "lin_api_", Charset::Alnum, 32),
    token(Rule::ProviderToken, "sbp_", Charset::Hex, 40),
    token(Rule::ProviderToken, "ntn_", Charset::Alnum, 40),
    token(Rule::ProviderToken, "secret_", Charset::Alnum, 43),
    token(Rule::ProviderToken, "figd_", Charset::Base64Url, 32),
    token(Rule::ProviderToken, "sntrys_", Charset::Base64UrlDot, 40),
    token(Rule::ProviderToken, "AGE-SECRET-KEY-1", Charset::Alnum, 40),
    token(Rule::YuliusToken, "yul_", Charset::Hex, 32),
    token(Rule::AwsAccessToken, "AKIA", Charset::UpperBase32, 16),
    token(Rule::AwsAccessToken, "ASIA", Charset::UpperBase32, 16),
    token(Rule::AwsAccessToken, "ABIA", Charset::UpperBase32, 16),
    token(Rule::AwsAccessToken, "ACCA", Charset::UpperBase32, 16),
];

/// Name fragments that make an assignment's literal value a secret (case-insensitive).
const SECRET_WORDS: &[&str] = &[
    "password",
    "passwd",
    "passphrase",
    "pwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "api-key",
    "access_key",
    "accesskey",
    "private_key",
    "privatekey",
    "credential",
    "auth_key",
    "signing_key",
    "encryption_key",
    "session_key",
    "master_key",
    "client_key",
];

/// Words whose assignments are secret whatever their value looks like.
const STRONG_WORDS: &[&str] = &["password", "passwd", "passphrase", "pwd", "secret"];

/// Values that are placeholders or type names, never secrets (compared case-insensitively).
const NOT_SECRETS: &[&str] = &[
    "null",
    "none",
    "nil",
    "undefined",
    "true",
    "false",
    "string",
    "str",
    "bool",
    "boolean",
    "number",
    "int",
    "integer",
    "object",
    "any",
    "unknown",
    "required",
    "optional",
    "password",
    "secret",
    "token",
    "changeme",
    "change_me",
    "example",
    "redacted",
    "placeholder",
    "default",
    "empty",
    "hidden",
    "masked",
];

#[derive(Debug, Clone)]
struct Span {
    start: usize,
    end: usize,
    rule: Rule,
    label: String,
}

/// Redacts `text`; `None` when nothing was found (the common case, no allocation).
#[must_use]
pub fn redact(text: &str) -> Option<Redacted> {
    let mut spans = Vec::new();
    detect_tokens(text, &mut spans);
    detect_jwts(text, &mut spans);
    detect_private_keys(text, &mut spans);
    detect_url_credentials(text, &mut spans);
    detect_assignments(text, &mut spans);
    detect_env_blocks(text, &mut spans);
    apply(text, spans)
}

/// Redacts the contents of a dotenv file: every non-trivial `KEY=value` value (numbers,
/// booleans and `${…}` references stay), plus everything [`redact`] finds. Lines may carry the
/// `cat -n` prefix Claude Code's Read tool adds (`   12→KEY=value`).
#[must_use]
pub fn redact_env(text: &str) -> Option<Redacted> {
    let mut spans = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if let Some(span) = env_line_value(line, offset) {
            spans.push(span);
        }
        offset += line.len();
    }
    detect_tokens(text, &mut spans);
    detect_jwts(text, &mut spans);
    detect_private_keys(text, &mut spans);
    detect_url_credentials(text, &mut spans);
    detect_assignments(text, &mut spans);
    apply(text, spans)
}

/// Redacts a value stored under a JSON key (`{"password": "…"}` in a tool call): the value is
/// replaced when the key is secret-named and the value is a literal secret, else [`redact`]
/// applies to the value.
#[must_use]
pub fn redact_named(key: &str, value: &str) -> Option<Redacted> {
    if is_secret_name(key) && value_is_secret(key, value) {
        return Some(Redacted {
            text: placeholder(Rule::SecretAssignment),
            findings: vec![Finding {
                rule: Rule::SecretAssignment,
                label: key.to_owned(),
            }],
        });
    }
    redact(value)
}

/// `true` for dotenv file names: `.env`, `.env.local`, `prod.env`, `.envrc`; not the shared
/// templates `.env.example`, `.env.sample`, `.env.template`, `.env.dist`, `.env.defaults`.
#[must_use]
pub fn is_env_file(path: &str) -> bool {
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .trim_matches(|c: char| c == '"' || c == '\'' || c == '`');
    let lower = name.to_ascii_lowercase();
    let template = [".example", ".sample", ".template", ".dist", ".defaults"]
        .iter()
        .any(|suffix| lower.ends_with(suffix));
    if template {
        return false;
    }
    lower == ".env" || lower == ".envrc" || lower.starts_with(".env.") || lower.ends_with(".env")
}

/// `true` when a shell command reads or writes a dotenv file (`cat .env`, `source .env.local`).
#[must_use]
pub fn mentions_env_file(command: &str) -> bool {
    command
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '<' | '>' | '(' | ')'))
        .any(|word| !word.is_empty() && is_env_file(word))
}

/// `true` when a key or variable name holds secrets (`DB_PASSWORD`, `apiKey`, `GITHUB_TOKEN`,
/// `STRIPE_KEY`). Plural and counter names (`max_tokens`, `token_count`) are not secrets.
#[must_use]
pub fn is_secret_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.len() > 128 || lower.is_empty() {
        return false;
    }
    let counter = [
        "tokens", "_count", "_limit", "_length", "_len", "_type", "_name", "_id",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
        || lower.starts_with("max_")
        || lower.starts_with("num_");
    if counter {
        return false;
    }
    if SECRET_WORDS.iter().any(|word| lower.contains(word)) {
        return true;
    }
    // Environment-style names ending in KEY: STRIPE_KEY, OPENAI_KEY (not `key`, `primary_key`).
    let env_style = name.len() > 4
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
    env_style && name.ends_with("_KEY")
}

fn placeholder(rule: Rule) -> String {
    format!("{REDACTED_PREFIX}{}]", rule.id())
}

/// Builds the output from detected spans: overlapping spans keep the earliest (then the
/// longest, then the most specific rule).
fn apply(text: &str, mut spans: Vec<Span>) -> Option<Redacted> {
    spans.retain(|span| span.end > span.start && span.end <= text.len());
    if spans.is_empty() {
        return None;
    }
    spans.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then(b.end.cmp(&a.end))
            .then(a.rule.priority().cmp(&b.rule.priority()))
    });
    let mut out = String::with_capacity(text.len());
    let mut findings = Vec::new();
    let mut cursor = 0;
    for span in spans {
        if span.start < cursor
            || !text.is_char_boundary(span.start)
            || !text.is_char_boundary(span.end)
        {
            continue;
        }
        out.push_str(&text[cursor..span.start]);
        let _ = write!(out, "{REDACTED_PREFIX}{}]", span.rule.id());
        findings.push(Finding {
            rule: span.rule,
            label: span.label,
        });
        cursor = span.end;
    }
    if findings.is_empty() {
        return None;
    }
    out.push_str(&text[cursor..]);
    Some(Redacted {
        text: out,
        findings,
    })
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Length of the run of `charset` bytes at `from`, at most [`MAX_TOKEN_CHARS`].
fn run_len(bytes: &[u8], from: usize, charset: Charset) -> usize {
    bytes[from.min(bytes.len())..]
        .iter()
        .take(MAX_TOKEN_CHARS)
        .take_while(|b| charset.contains(**b))
        .count()
}

/// One pass over word starts: provider tokens by prefix (dispatched on the first byte) and
/// `Bearer`/`Basic` authorization values.
fn detect_tokens(text: &str, spans: &mut Vec<Span>) {
    let bytes = text.as_bytes();
    for start in 0..bytes.len() {
        let first = bytes[start];
        if !first.is_ascii_alphabetic() || (start > 0 && is_word_byte(bytes[start - 1])) {
            continue;
        }
        let rest = &text[start..];
        if matches!(first, b'b' | b'B') {
            authorization_at(text, start, spans);
        }
        for rule in TOKEN_RULES {
            if rule.prefix.as_bytes()[0] != first || !rest.starts_with(rule.prefix) {
                continue;
            }
            let body = start + rule.prefix.len();
            let len = run_len(bytes, body, rule.charset);
            if len < rule.min {
                continue;
            }
            let end = body + len;
            // AWS key ids are exactly 20 characters long.
            if rule.rule == Rule::AwsAccessToken
                && (len != rule.min || bytes.get(end).is_some_and(|b| is_word_byte(*b)))
            {
                continue;
            }
            spans.push(Span {
                start,
                end,
                rule: rule.rule,
                label: rule.prefix.to_owned(),
            });
            break;
        }
    }
    for (start, _) in text.match_indices(SLACK_WEBHOOK) {
        let body = start + SLACK_WEBHOOK.len();
        let len = run_len(bytes, body, Charset::Base64UrlDot);
        if len >= 20 {
            spans.push(Span {
                start: body,
                end: body + len,
                rule: Rule::SlackToken,
                label: "hooks.slack.com".to_owned(),
            });
        }
    }
}

/// Slack incoming webhook path; the part after it is the secret.
const SLACK_WEBHOOK: &str = "hooks.slack.com/services/";

/// `Bearer <token>` anywhere, `Basic <credentials>` after an `Authorization:` header.
fn authorization_at(text: &str, start: usize, spans: &mut Vec<Span>) {
    let bytes = text.as_bytes();
    let head = &bytes[start..(start + 7).min(bytes.len())];
    let (word_len, minimum, label) = if head.eq_ignore_ascii_case(b"bearer ") {
        (7, 16, "Bearer")
    } else if head.len() >= 6 && head[..6].eq_ignore_ascii_case(b"basic ") {
        let before = text[..start].trim_end();
        let header = before.len() >= 14
            && before.is_char_boundary(before.len() - 14)
            && before[before.len() - 14..].eq_ignore_ascii_case("authorization:");
        if !header {
            return;
        }
        (6, 12, "Basic")
    } else {
        return;
    };
    let value = start + word_len;
    let len = bytes[value..]
        .iter()
        .take(MAX_TOKEN_CHARS)
        .take_while(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'+' | b'/' | b'=' | b'-')
        })
        .count();
    if len < minimum {
        return;
    }
    let candidate = &text[value..value + len];
    if is_placeholder(candidate) || !has_digit_or_mixed_case(candidate) {
        return;
    }
    spans.push(Span {
        start: value,
        end: value + len,
        rule: Rule::AuthorizationHeader,
        label: label.to_owned(),
    });
}

fn detect_jwts(text: &str, spans: &mut Vec<Span>) {
    let bytes = text.as_bytes();
    for (start, _) in text.match_indices("eyJ") {
        if start > 0 && (is_word_byte(bytes[start - 1]) || bytes[start - 1] == b'-') {
            continue;
        }
        let mut cursor = start;
        let mut parts = 0;
        let mut ok = true;
        while parts < 3 {
            let len = run_len(bytes, cursor, Charset::Base64Url);
            let minimum = if parts == 2 { 10 } else { 12 };
            if len < minimum || (parts == 1 && !text[cursor..].starts_with("eyJ")) {
                ok = false;
                break;
            }
            cursor += len;
            parts += 1;
            if parts < 3 {
                if bytes.get(cursor) != Some(&b'.') {
                    ok = false;
                    break;
                }
                cursor += 1;
            }
        }
        if ok {
            spans.push(Span {
                start,
                end: cursor,
                rule: Rule::Jwt,
                label: "eyJ…".to_owned(),
            });
        }
    }
}

fn detect_private_keys(text: &str, spans: &mut Vec<Span>) {
    for (start, _) in text.match_indices("-----BEGIN ") {
        let header_start = start + "-----BEGIN ".len();
        let Some(close) = text[header_start..].find("-----") else {
            continue;
        };
        let header = &text[header_start..header_start + close];
        if header.len() > 64 || !header.contains("PRIVATE KEY") {
            continue;
        }
        let after_header = header_start + close + 5;
        let end = text[after_header..]
            .find("-----END ")
            .and_then(|end| {
                let end_start = after_header + end + "-----END ".len();
                text[end_start..]
                    .find("-----")
                    .map(|close| end_start + close + 5)
            })
            .filter(|end| end - start <= MAX_KEY_BLOCK)
            .unwrap_or_else(|| {
                let mut end = (start + MAX_KEY_BLOCK).min(text.len());
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                end
            });
        spans.push(Span {
            start,
            end,
            rule: Rule::PrivateKey,
            label: header.to_owned(),
        });
    }
}

fn detect_url_credentials(text: &str, spans: &mut Vec<Span>) {
    let bytes = text.as_bytes();
    for (sep, _) in text.match_indices("://") {
        let scheme_start = bytes[..sep]
            .iter()
            .rposition(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-')))
            .map_or(0, |i| i + 1);
        if scheme_start == sep || !bytes[scheme_start].is_ascii_alphabetic() {
            continue;
        }
        let authority = sep + 3;
        let end = bytes[authority..]
            .iter()
            .take(512)
            .position(|b| {
                b.is_ascii_whitespace()
                    || matches!(b, b'/' | b'?' | b'#' | b'"' | b'\'' | b'`' | b'<' | b'>')
            })
            .map_or_else(|| (authority + 512).min(bytes.len()), |i| authority + i);
        let Some(at) = bytes[authority..end].iter().rposition(|b| *b == b'@') else {
            continue;
        };
        let at = authority + at;
        let Some(colon) = bytes[authority..at].iter().position(|b| *b == b':') else {
            continue;
        };
        let password_start = authority + colon + 1;
        if password_start >= at || is_placeholder(&text[password_start..at]) {
            continue;
        }
        spans.push(Span {
            start: password_start,
            end: at,
            rule: Rule::UrlCredentials,
            label: format!("{}://", &text[scheme_start..sep]),
        });
    }
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
}

/// `name = value`, `name: value`, `"name": "value"`, `name := value`, `name => value` and
/// `--name value` where the name is secret-named and the value a literal secret. One pass over
/// the name-like runs of the text.
fn detect_assignments(text: &str, spans: &mut Vec<Span>) {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if !is_name_byte(bytes[index]) {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && is_name_byte(bytes[index]) {
            index += 1;
        }
        let name = text[start..index].trim_end_matches(['.', '-']);
        if name.len() < 3 {
            continue;
        }
        let flag = name.starts_with("--");
        let bare_name = name.trim_start_matches('-');
        if !is_secret_name(bare_name) {
            continue;
        }
        if let Some(span) = assignment_value(text, start + name.len(), bare_name, flag) {
            spans.push(span);
        }
    }
}

/// The value after a name ending at `after`, when it is a literal secret.
fn assignment_value(text: &str, after: usize, name: &str, flag: bool) -> Option<Span> {
    let bytes = text.as_bytes();
    let mut cursor = after;
    // A quoted key: `"password": "…"`.
    if matches!(bytes.get(cursor), Some(b'"' | b'\'')) {
        cursor += 1;
    }
    let skip_spaces = |mut at: usize| {
        while matches!(bytes.get(at), Some(b' ' | b'\t')) {
            at += 1;
        }
        at
    };
    let spaced = skip_spaces(cursor);
    let separator = &text[spaced..(spaced + 2).min(text.len())];
    if separator.starts_with("==") || separator.starts_with("=~") {
        return None;
    }
    cursor = if separator.starts_with(":=") || separator.starts_with("=>") {
        spaced + 2
    } else if separator.starts_with('=') || separator.starts_with(':') {
        spaced + 1
    } else if flag && spaced > cursor && spaced == cursor + 1 {
        spaced
    } else {
        return None;
    };
    cursor = skip_spaces(cursor);
    let (start, end) = match bytes.get(cursor) {
        Some(quote @ (b'"' | b'\'' | b'`')) => {
            let open = cursor + 1;
            let close = bytes[open..]
                .iter()
                .take(MAX_VALUE_CHARS + 1)
                .position(|b| b == quote || *b == b'\n')?;
            if bytes[open + close] != *quote {
                return None;
            }
            (open, open + close)
        }
        Some(_) => {
            let len = bytes[cursor..]
                .iter()
                .take(MAX_VALUE_CHARS + 1)
                .take_while(|b| {
                    !b.is_ascii_whitespace()
                        && !matches!(
                            b,
                            b',' | b';' | b'"' | b'\'' | b'`' | b')' | b'}' | b']' | b'\\'
                        )
                })
                .count();
            if len > MAX_VALUE_CHARS {
                return None;
            }
            let value = &text[cursor..cursor + len];
            // Bare words and dotted paths are code (`token = newToken`, `os.environ`).
            if !value.bytes().any(|b| b.is_ascii_digit())
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphabetic() || matches!(b, b'_' | b'.'))
            {
                return None;
            }
            if value.contains('(') || value.contains('[') || value.contains('{') {
                return None;
            }
            (cursor, cursor + len)
        }
        None => return None,
    };
    let value = &text[start..end];
    if !value_is_secret(name, value) {
        return None;
    }
    Some(Span {
        start,
        end,
        rule: Rule::SecretAssignment,
        label: name.to_owned(),
    })
}

/// Whether a literal assigned to `name` is a secret (not a placeholder, reference or tiny value).
fn value_is_secret(name: &str, value: &str) -> bool {
    let value = value.trim();
    let chars = value.chars().count();
    if !(6..=MAX_VALUE_CHARS).contains(&chars) || value.contains(char::is_whitespace) {
        return false;
    }
    if is_placeholder(value) || value.contains('(') || value.starts_with("process.env") {
        return false;
    }
    let lower = name.to_ascii_lowercase();
    if STRONG_WORDS.iter().any(|word| lower.contains(word)) {
        return true;
    }
    chars >= 12 && has_digit_or_mixed_case(value)
}

fn has_digit_or_mixed_case(value: &str) -> bool {
    let digit = value.bytes().any(|b| b.is_ascii_digit());
    let upper = value.bytes().any(|b| b.is_ascii_uppercase());
    let lower = value.bytes().any(|b| b.is_ascii_lowercase());
    digit || (upper && lower)
}

/// Placeholders, references and redactions, never secrets.
fn is_placeholder(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with('$')
        || lower.starts_with('<')
        || lower.starts_with('%')
        || lower.starts_with("{{")
        || lower.starts_with("[redacted")
        || lower.contains("your_")
        || lower.contains("your-")
        || lower.contains("example")
        || lower.contains("placeholder")
        || lower.contains("xxxx")
        || lower.contains("****")
        || lower.contains("…")
        || lower.contains("...")
    {
        return true;
    }
    if NOT_SECRETS.contains(&lower.as_str()) {
        return true;
    }
    let mut chars = trimmed.chars();
    let first = chars.next().unwrap_or('x');
    chars.all(|c| c == first)
}

/// The value span of one dotenv line (`[export ]KEY=value`), optionally after a Read-tool line
/// number prefix.
fn env_line_value(line: &str, offset: usize) -> Option<Span> {
    let body_end = line.trim_end_matches(['\n', '\r']).len();
    let body = &line[..body_end];
    let mut cursor = body.len() - body.trim_start().len();
    // `cat -n` style prefix: digits then a tab or `→`.
    let digits = body[cursor..]
        .bytes()
        .take_while(u8::is_ascii_digit)
        .count();
    if digits > 0 {
        let rest = &body[cursor + digits..];
        if let Some(stripped) = rest.strip_prefix('\t') {
            cursor = body.len() - stripped.len();
        } else if let Some(stripped) = rest.strip_prefix('→') {
            cursor = body.len() - stripped.len();
        }
    }
    let mut rest = &body[cursor..];
    let trimmed = rest.trim_start();
    cursor += rest.len() - trimmed.len();
    rest = trimmed;
    if let Some(stripped) = rest.strip_prefix("export ") {
        rest = stripped.trim_start();
        cursor = body.len() - rest.len();
    }
    let name_len = rest
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
        .count();
    if name_len == 0 || !rest.as_bytes()[0].is_ascii_alphabetic() && rest.as_bytes()[0] != b'_' {
        return None;
    }
    let name = &rest[..name_len];
    let after_name = &rest[name_len..];
    let spaces = after_name.len() - after_name.trim_start_matches([' ', '\t']).len();
    let after_spaces = &after_name[spaces..];
    let value_part = after_spaces.strip_prefix('=')?;
    if value_part.starts_with('=') {
        return None;
    }
    let value_offset = cursor + name_len + spaces + 1;
    let lead = value_part.len() - value_part.trim_start().len();
    let raw = value_part.trim_start();
    let (start, len) = match raw.as_bytes().first() {
        Some(quote @ (b'"' | b'\'')) => {
            let close = raw[1..].bytes().position(|b| b == *quote)?;
            (value_offset + lead + 1, close)
        }
        Some(_) => {
            let end = raw.find(" #").unwrap_or(raw.len());
            (value_offset + lead, raw[..end].trim_end().len())
        }
        None => return None,
    };
    let value = &body[start..start + len];
    if len == 0 || is_trivial_env_value(value) {
        return None;
    }
    Some(Span {
        start: offset + start,
        end: offset + start + len,
        rule: Rule::EnvValue,
        label: name.to_owned(),
    })
}

fn is_trivial_env_value(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.starts_with("${")
        || lower.starts_with("[redacted")
        || matches!(
            lower.as_str(),
            "true" | "false" | "yes" | "no" | "on" | "off" | "null" | "none"
        )
        || value.parse::<f64>().is_ok()
}

/// Runs of three or more `UPPER_NAME=value` lines (a pasted dotenv block) are redacted as
/// dotenv content.
fn detect_env_blocks(text: &str, spans: &mut Vec<Span>) {
    let mut run: Vec<Span> = Vec::new();
    let mut run_lines = 0;
    let mut offset = 0;
    let flush = |run: &mut Vec<Span>, run_lines: &mut usize, spans: &mut Vec<Span>| {
        if *run_lines >= 3 {
            spans.append(run);
        }
        run.clear();
        *run_lines = 0;
    };
    for line in text.split_inclusive('\n') {
        let env_like = is_env_style_line(line);
        if env_like {
            run_lines += 1;
            if let Some(span) = env_line_value(line, offset) {
                run.push(span);
            }
        } else if !line.trim().is_empty() && !line.trim_start().starts_with('#') {
            flush(&mut run, &mut run_lines, spans);
        }
        offset += line.len();
    }
    flush(&mut run, &mut run_lines, spans);
}

fn is_env_style_line(line: &str) -> bool {
    let body = line.trim();
    let body = body.strip_prefix("export ").unwrap_or(body);
    let Some((name, _)) = body.split_once('=') else {
        return false;
    };
    name.len() >= 2
        && name.as_bytes()[0].is_ascii_uppercase()
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redacted(text: &str) -> String {
        redact(text).map_or_else(|| text.to_owned(), |r| r.text)
    }

    fn rules(text: &str) -> Vec<&'static str> {
        redact(text)
            .map(|r| r.findings.iter().map(|f| f.rule.id()).collect())
            .unwrap_or_default()
    }

    /// Synthetic credentials built at run time so no literal secret sits in the source.
    fn fake(prefix: &str, body: &str, repeat: usize) -> String {
        format!("{prefix}{}", body.repeat(repeat))
    }

    #[test]
    fn provider_tokens_are_redacted_with_their_prefix_as_label() {
        let cases = [
            (fake("ghp_", "aB3dE", 8), "github-token"),
            (fake("github_pat_", "11AbCdEf_", 10), "github-token"),
            (fake("glpat-", "x1Y2z", 5), "gitlab-pat"),
            (fake("sk-ant-api03-", "Zx9-", 20), "anthropic-api-key"),
            (fake("sk-proj-", "Ab12", 12), "openai-api-key"),
            (fake("xoxb-", "1234-", 6), "slack-token"),
            (fake("sk_live_", "4eC39Hq", 4), "stripe-access-token"),
            (fake("AIza", "SyD-1a2b3", 4), "gcp-api-key"),
            (fake("npm_", "Ab1", 13), "registry-token"),
            (fake("AKIA", "IOSFODNN7EXAMPL", 1) + "Q", "aws-access-token"),
            (fake("yul_", "0f", 32), "yulius-token"),
            (fake("dop_v1_", "a1", 32), "provider-token"),
        ];
        for (secret, rule) in cases {
            let text = format!("export X={secret} # set");
            let Some(out) = redact(&text) else {
                panic!("not redacted: {secret}");
            };
            assert!(!out.text.contains(&secret), "{secret} leaked: {}", out.text);
            assert!(
                out.findings.iter().any(|f| f.rule.id() == rule),
                "{secret}: {:?}",
                out.findings
            );
            assert!(out.findings.iter().all(|f| !f.label.contains(&secret[6..])));
        }
    }

    #[test]
    fn near_misses_and_ordinary_text_are_left_alone() {
        for text in [
            "ghp_short",
            "the task-runner uses sk-learn style names",
            "AKIAIOSFODNN7EXAMPLEX is too long for an AWS key id",
            "token = newToken",
            "const token: string = getToken();",
            "password: string",
            "apiKey?: string",
            "if token == expected {",
            "max_tokens = 4096",
            "input_tokens: 1234567890",
            "password=${DB_PASSWORD}",
            "API_KEY=<your-api-key>",
            "secret: changeme",
            "password: xxxxxxxx",
            "https://github.com/Quartio-Studio/yulius",
            "postgres://user@localhost/db",
            "Bearer token",
            "api_key = os.environ[\"API_KEY\"]",
            "token = settings.github.token",
            "plain prose about passwords and secrets",
        ] {
            assert_eq!(redacted(text), text, "{text}");
        }
    }

    #[test]
    fn assignments_keep_the_name_and_redact_the_literal() {
        assert_eq!(
            redacted("DATABASE_PASSWORD=hunter2hunter"),
            "DATABASE_PASSWORD=[REDACTED:secret-assignment]"
        );
        assert_eq!(
            redacted(r#"{"client_secret": "s3cr3t-value", "x": 1}"#),
            r#"{"client_secret": "[REDACTED:secret-assignment]", "x": 1}"#
        );
        assert_eq!(
            redacted("let api_key = \"Zm9vYmFyYmF6cXV4MTIz\";"),
            "let api_key = \"[REDACTED:secret-assignment]\";"
        );
        assert_eq!(
            redacted("password: Tr0ub4dor&3"),
            "password: [REDACTED:secret-assignment]"
        );
        assert_eq!(
            redacted("mysql --password hunter2hunter -u root"),
            "mysql --password [REDACTED:secret-assignment] -u root"
        );
        assert_eq!(
            redacted("STRIPE_KEY=Zm9vYmFyYmF6cXV4MTIz"),
            "STRIPE_KEY=[REDACTED:secret-assignment]"
        );
        let found = redact("GITHUB_TOKEN=abcdEFGH12345678").map(|r| r.findings);
        assert_eq!(
            found,
            Some(vec![Finding {
                rule: Rule::SecretAssignment,
                label: "GITHUB_TOKEN".into()
            }])
        );
    }

    #[test]
    fn urls_headers_jwts_and_keys() {
        assert_eq!(
            redacted("DATABASE_URL is postgres://app:pa55word@db.internal:5432/app ok"),
            "DATABASE_URL is postgres://app:[REDACTED:url-credentials]@db.internal:5432/app ok"
        );
        assert_eq!(
            redacted("curl -H 'Authorization: Bearer abcDEF123456ghiJKL789' x"),
            "curl -H 'Authorization: Bearer [REDACTED:authorization-header]' x"
        );
        let jwt = format!(
            "eyJhbGciOiJIUzI1NiJ9.{}.{}",
            fake("eyJ", "zdWIiOiIx", 3),
            fake("", "SflKxwRJ", 4)
        );
        assert_eq!(rules(&format!("cookie={jwt};")), vec!["jwt"]);
        let key = format!(
            "-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n-----END OPENSSH PRIVATE KEY-----\nafter",
            fake("", "b3BlbnNzaC1rZXktdjEA", 5)
        );
        assert_eq!(
            redacted(&key),
            "[REDACTED:private-key]\nafter",
            "the whole block goes"
        );
        assert_eq!(
            redacted("-----BEGIN PUBLIC KEY-----\nMIIB\n-----END PUBLIC KEY-----"),
            "-----BEGIN PUBLIC KEY-----\nMIIB\n-----END PUBLIC KEY-----"
        );
    }

    #[test]
    fn env_files_lose_every_value_but_trivial_ones() {
        let file = "     1→# Local settings\n     2→export DATABASE_URL=postgres://u:p@h/db\n     3→PORT=3000\n     4→DEBUG=true\n     5→SESSION_COOKIE=abc\n     6→REF=${OTHER}\n     7→QUOTED=\"two words\"\n";
        let out = redact_env(file).map(|r| r.text).unwrap_or_default();
        assert_eq!(
            out,
            "     1→# Local settings\n     2→export DATABASE_URL=[REDACTED:env-value]\n     3→PORT=3000\n     4→DEBUG=true\n     5→SESSION_COOKIE=[REDACTED:env-value]\n     6→REF=${OTHER}\n     7→QUOTED=\"[REDACTED:env-value]\"\n"
        );
        // A pasted block of three or more dotenv lines counts as a dotenv file.
        let pasted =
            "Here is my config:\nAPP_NAME=shop\nSMTP_HOST=mail.example.org\nSMTP_PASS=abc\nThanks";
        assert_eq!(
            redacted(pasted),
            "Here is my config:\nAPP_NAME=[REDACTED:env-value]\nSMTP_HOST=[REDACTED:env-value]\nSMTP_PASS=[REDACTED:env-value]\nThanks"
        );
        assert_eq!(redacted("A=1\nB=2"), "A=1\nB=2");
    }

    #[test]
    fn env_file_names() {
        for yes in [
            ".env",
            "/repo/.env.local",
            "apps/web/.env.production",
            "prod.env",
            ".envrc",
            "\".env\"",
        ] {
            assert!(is_env_file(yes), "{yes}");
        }
        for no in [
            ".env.example",
            ".env.sample",
            "src/env.ts",
            "environment.md",
            ".envoy",
        ] {
            assert!(!is_env_file(no), "{no}");
        }
        assert!(mentions_env_file("cat .env.local | grep KEY"));
        assert!(mentions_env_file("source ./.env && npm start"));
        assert!(!mentions_env_file("cat .env.example"));
        assert!(!mentions_env_file("npm run env"));
    }

    #[test]
    fn named_values_and_secret_names() {
        assert!(is_secret_name("DB_PASSWORD"));
        assert!(is_secret_name("apiKey"));
        assert!(is_secret_name("OPENAI_KEY"));
        assert!(!is_secret_name("max_tokens"));
        assert!(!is_secret_name("input_tokens"));
        assert!(!is_secret_name("primary_key"));
        assert!(!is_secret_name("key"));
        assert_eq!(
            redact_named("password", "hunter2hunter").map(|r| r.text),
            Some("[REDACTED:secret-assignment]".into())
        );
        assert_eq!(redact_named("password", "string"), None);
        assert_eq!(redact_named("description", "a plain value"), None);
    }

    #[test]
    fn redaction_is_deterministic_and_idempotent() {
        let text = format!(
            "GITHUB_TOKEN={} and {} and postgres://a:b4dpass@h/x",
            fake("ghp_", "aB3dE", 8),
            fake("sk-ant-api03-", "Zx9-", 20)
        );
        let once = redacted(&text);
        assert_eq!(once, redacted(&text));
        assert_eq!(redacted(&once), once, "a redacted text has nothing left");
        let tally = {
            let mut tally = RedactionTally::default();
            tally.add(&redact(&text).map(|r| r.findings).unwrap_or_default());
            tally
        };
        assert_eq!(tally.total(), 3);
        assert!(!once.contains("aB3dE"));
    }

    #[test]
    fn unicode_text_around_secrets_survives() {
        let text = format!("Zażółć gęślą jaźń {} — koniec", fake("ghp_", "aB3dE", 8));
        assert_eq!(
            redacted(&text),
            "Zażółć gęślą jaźń [REDACTED:github-token] — koniec"
        );
    }
}
