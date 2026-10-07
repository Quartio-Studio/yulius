//! Generates the embedded web asset table from `web/` (SPEC-002 ADR-01).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "txt" | "md" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let web = manifest.join("web");
    println!("cargo:rerun-if-changed={}", web.display());
    let mut files = Vec::new();
    walk(&web, &mut files);
    let mut code = String::from(
        "/// Embedded web assets: (path, bytes, content type).\npub static ASSETS: &[(&str, &[u8], &str)] = &[\n",
    );
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
        let rel = file
            .strip_prefix(&web)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        if rel.ends_with(".md") {
            continue;
        }
        let _ = writeln!(
            code,
            "    ({rel:?}, include_bytes!({:?}), {:?}),",
            file.display().to_string(),
            mime(file)
        );
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default()).join("assets.rs");
    if let Err(err) = std::fs::write(&out, code) {
        println!("cargo:warning=failed to write asset table: {err}");
    }
}
