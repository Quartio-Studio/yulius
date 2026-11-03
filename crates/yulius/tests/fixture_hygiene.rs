//! Test fixtures hold no credential in a provider's shape (SPEC-406 TCF-001, ADR-0258): no file
//! under `crates/yulius/tests` has a provider's token prefix followed by a token body. Secret
//! scanners and push protection match that shape, and a real token pasted into a fixture would
//! look like the synthetic ones. A fixture that needs the shape (the product detects credentials
//! by prefix) puts it together from the prefix and a synthetic body, as in
//! `concat!("sk_test_", "51NzQ9vaultValue8f3a")`, so a pasted token fails here.

use std::path::{Path, PathBuf};

/// A provider's token prefix (the prefix rules of `yulius_domain::redact`) and how its body looks.
struct Shape {
    prefix: &'static str,
    /// The fewest body characters that make it a credential's shape.
    min: usize,
    /// AWS key ids: upper-case letters and digits only.
    upper: bool,
}

const fn shape(prefix: &'static str) -> Shape {
    Shape {
        prefix,
        min: 8,
        upper: false,
    }
}

const SHAPES: &[Shape] = &[
    shape("sk-ant-"),
    shape("sk-proj-"),
    shape("sk-svcacct-"),
    shape("sk-admin-"),
    shape("sk_live_"),
    shape("sk_test_"),
    shape("rk_live_"),
    shape("rk_test_"),
    shape("whsec_"),
    shape("ghp_"),
    shape("gho_"),
    shape("ghu_"),
    shape("ghs_"),
    shape("ghr_"),
    shape("github_pat_"),
    shape("glpat-"),
    shape("xoxb-"),
    shape("xoxp-"),
    shape("xoxa-"),
    shape("xapp-"),
    shape("AIza"),
    shape("GOCSPX-"),
    Shape {
        prefix: "AKIA",
        min: 16,
        upper: true,
    },
    Shape {
        prefix: "ASIA",
        min: 16,
        upper: true,
    },
];

/// Each credential-shaped run in `text`: its line and prefix and its body's length, never the
/// body itself (a real token must not reach the test output too).
fn shaped(text: &str) -> Vec<(usize, &'static str, usize)> {
    let mut found = Vec::new();
    for (index, line) in text.lines().enumerate() {
        for shape in SHAPES {
            for (at, _) in line.match_indices(shape.prefix) {
                let body = line[at + shape.prefix.len()..]
                    .bytes()
                    .take_while(|&b| {
                        if shape.upper {
                            b.is_ascii_uppercase() || b.is_ascii_digit()
                        } else {
                            b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
                        }
                    })
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
        } else if path.extension().is_some_and(|ext| ext == "rs") {
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
