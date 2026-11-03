//! Test fixtures hold no credential in a provider's shape (SPEC-406 TCF-001, ADR-0258): no file
//! under `crates/yulius/tests` (the Rust sources and the stream probe's JavaScript and HTML) has
//! a provider's token prefix followed by a token body. Secret scanners and push protection match
//! that shape, and a real token pasted into a fixture would look like the synthetic ones. A
//! fixture that needs the shape (the product detects credentials by prefix) puts it together
//! from the prefix and a synthetic body, as in `concat!("sk_test_", "51NzQ9vaultValue8f3a")`, so
//! a pasted token fails here.

use std::path::{Path, PathBuf};

/// The characters a token body is made of.
#[derive(Clone, Copy)]
enum Body {
    /// Letters, digits, `_` and `-`.
    Token,
    /// The same and `.` (SendGrid, Sentry).
    TokenDot,
    /// Upper-case letters and digits (AWS key ids).
    Upper,
}

/// A provider's token prefix, the prefix rules of `yulius_domain::redact`, and how its body looks.
struct Shape {
    prefix: &'static str,
    /// The fewest body characters that make it a credential's shape: 8 where a fixture of the
    /// product's detection would use the prefix, else the redaction rule's own minimum, so that
    /// words such as `npm_config_cache` or `secret_name` are not taken for tokens.
    min: usize,
    body: Body,
}

const fn shape(prefix: &'static str, min: usize, body: Body) -> Shape {
    Shape { prefix, min, body }
}

const SHAPES: &[Shape] = &[
    shape("github_pat_", 8, Body::Token),
    shape("ghp_", 8, Body::Token),
    shape("gho_", 8, Body::Token),
    shape("ghu_", 8, Body::Token),
    shape("ghs_", 8, Body::Token),
    shape("ghr_", 8, Body::Token),
    shape("glpat-", 8, Body::Token),
    shape("sk-ant-", 8, Body::Token),
    shape("sk-proj-", 8, Body::Token),
    shape("sk-svcacct-", 8, Body::Token),
    shape("sk-admin-", 8, Body::Token),
    shape("sk-", 40, Body::Token),
    shape("xoxb-", 8, Body::Token),
    shape("xoxp-", 8, Body::Token),
    shape("xoxa-", 8, Body::Token),
    shape("xoxr-", 8, Body::Token),
    shape("xoxs-", 8, Body::Token),
    shape("xoxe-", 8, Body::Token),
    shape("xapp-", 8, Body::Token),
    shape("sk_live_", 8, Body::Token),
    shape("sk_test_", 8, Body::Token),
    shape("rk_live_", 8, Body::Token),
    shape("rk_test_", 8, Body::Token),
    shape("whsec_", 8, Body::Token),
    shape("AIza", 8, Body::Token),
    shape("GOCSPX-", 8, Body::Token),
    shape("npm_", 36, Body::Token),
    shape("pypi-AgEIcHlwaS5vcmc", 8, Body::Token),
    shape("hf_", 30, Body::Token),
    shape("SG.", 60, Body::TokenDot),
    shape("dop_v1_", 8, Body::Token),
    shape("doo_v1_", 8, Body::Token),
    shape("dor_v1_", 8, Body::Token),
    shape("shpat_", 8, Body::Token),
    shape("shpss_", 8, Body::Token),
    shape("shpca_", 8, Body::Token),
    shape("shppa_", 8, Body::Token),
    shape("dckr_pat_", 8, Body::Token),
    shape("lin_api_", 8, Body::Token),
    shape("sbp_", 40, Body::Token),
    shape("ntn_", 40, Body::Token),
    shape("secret_", 43, Body::Token),
    shape("figd_", 8, Body::Token),
    shape("sntrys_", 8, Body::TokenDot),
    shape("AGE-SECRET-KEY-1", 8, Body::Token),
    shape("yul_", 32, Body::Token),
    shape("AKIA", 16, Body::Upper),
    shape("ASIA", 16, Body::Upper),
    shape("ABIA", 16, Body::Upper),
    shape("ACCA", 16, Body::Upper),
];

fn in_body(body: Body, byte: u8) -> bool {
    match body {
        Body::Token => byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-',
        Body::TokenDot => byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'),
        Body::Upper => byte.is_ascii_uppercase() || byte.is_ascii_digit(),
    }
}

/// Each credential-shaped run in `text` that starts a word, as the product's redaction reads
/// them: its line and prefix and its body's length, never the body itself (a real token must
/// not reach the test output too).
fn shaped(text: &str) -> Vec<(usize, &'static str, usize)> {
    let mut found = Vec::new();
    for (index, line) in text.lines().enumerate() {
        for shape in SHAPES {
            for (at, _) in line.match_indices(shape.prefix) {
                let before = line.as_bytes()[..at].last().copied();
                if before.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_') {
                    continue;
                }
                let body = line.as_bytes()[at + shape.prefix.len()..]
                    .iter()
                    .take_while(|&&b| in_body(shape.body, b))
                    .count();
                if body >= shape.min {
                    found.push((index + 1, shape.prefix, body));
                }
            }
        }
    }
    found
}

fn sources(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            sources(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

#[test]
fn the_scan_finds_a_provider_shape_and_leaves_prefixes_alone() {
    let token = format!("let t = \"{}{}\";", "ghp_", "A1b2".repeat(9));
    assert_eq!(shaped(&token), vec![(1, "ghp_", 36)]);
    let aws = format!("\n{}{}", "AKIA", "ABCDEFGH12345678");
    assert_eq!(shaped(&aws), vec![(2, "AKIA", 16)]);
    // A prefix alone, a format string, a mask and a fixture put together from parts.
    let parts = concat!(
        "\"ghp_\" \"ghp_{}\" \"sk-ant-oat01-[REDACTED]\" ",
        "concat!(\"sk_test_\", \"51NzQ9vaultValue8f3a\") ASIA_PACIFIC"
    );
    assert_eq!(shaped(parts), vec![]);
    // A prefix inside a word is no token, and a rule's own minimum keeps words apart.
    let words = "fn network_test_fixtures_are_kept() {} task_live_value_long npm_config_cache";
    assert_eq!(shaped(words), vec![]);
    let npm = format!("({}{})", "npm_", "a1".repeat(18));
    assert_eq!(shaped(&npm), vec![(1, "npm_", 36)]);
}

#[test]
fn test_fixtures_hold_no_provider_shaped_credential() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&root.join("tests"), &mut files)?;
    files.sort();
    assert!(files.len() > 50, "the tests folder was not read: {files:?}");
    let mut hits = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file)?;
        let name = file.strip_prefix(root)?.display();
        for (line, prefix, body) in shaped(&text) {
            hits.push(format!(
                "{name}:{line}: {prefix} and {body} token characters"
            ));
        }
    }
    assert!(
        hits.is_empty(),
        "credential-shaped literals in test code; put each fixture together from its prefix and \
         a synthetic body (concat!), and never commit a real one:\n{}",
        hits.join("\n")
    );
    Ok(())
}
