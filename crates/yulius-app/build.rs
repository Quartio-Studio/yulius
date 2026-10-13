//! Embeds the default workflows of `catalog/workflows/*.md` (SPEC-002 §7.10 "Workflow catalog
//! format") and the ready-made automations of `catalog/automations/*.md` (SPEC-042): every
//! Markdown file of those directories except `README.md`, so adding one needs no code change. The
//! files are parsed on first use (and by unit tests); this script only lists them.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Writes `$OUT_DIR/<out>` with `pub static <name>: &[(&str, &str)]`, the (stem, contents) of
/// every catalog file of `dir`, sorted by stem.
fn embed(dir: &Path, name: &str, doc: &str, out: &str) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "md"))
                .filter_map(|p| {
                    let stem = p.file_stem()?.to_str()?.to_owned();
                    (stem != "README").then_some((stem, p))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    let mut code = format!(
        "/// {doc}: (file stem, contents), sorted by stem.\npub static {name}: &[(&str, &str)] = &[\n"
    );
    for (stem, path) in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        let _ = writeln!(
            code,
            "    ({stem:?}, include_str!({:?})),",
            path.display().to_string()
        );
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default()).join(out);
    if let Err(err) = std::fs::write(&out, code) {
        println!("cargo:warning=failed to write {}: {err}", out.display());
    }
}

fn main() {
    let catalog = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default())
        .join("../../catalog");
    embed(
        &catalog.join("workflows"),
        "WORKFLOW_FILES",
        "Embedded default workflow files",
        "workflows.rs",
    );
    embed(
        &catalog.join("automations"),
        "AUTOMATION_FILES",
        "Embedded ready-made automation files",
        "automations.rs",
    );
}
