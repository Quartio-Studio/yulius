//! Embeds the default workflows of `catalog/workflows/*.md` (SPEC-002 §7.10 "Workflow catalog
//! format"): every Markdown file of that directory except `README.md`, so adding a workflow needs
//! no code change. The files are parsed on first use (and by a unit test); this script only lists
//! them.

use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let dir = manifest.join("../../catalog/workflows");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<(String, PathBuf)> = std::fs::read_dir(&dir)
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
    let mut code = String::from(
        "/// Embedded default workflow files: (file stem, contents), sorted by stem.\n\
         pub static WORKFLOW_FILES: &[(&str, &str)] = &[\n",
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
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default()).join("workflows.rs");
    if let Err(err) = std::fs::write(&out, code) {
        println!("cargo:warning=failed to write the workflow catalog: {err}");
    }
}
