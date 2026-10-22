//! Agent secret values in the project's checkout (SPEC-274 PLW-003, ADR-0204): redaction masks a
//! value wherever it appears, so one that a tracked file holds would be masked in the repository
//! as tools return it. The values stay in memory: they are never a process argument or a file.
//! One pass over at most [`MAX_TOTAL_BYTES`] finds them all, however many there are.

use std::ffi::OsStr;
use std::io::Read as _;
use std::path::Path;

use aho_corasick::AhoCorasick;

use crate::process::{LIST_OUTPUT, QUERY_TIMEOUT, git};

/// Largest tracked file looked through (bytes); larger ones are data, not configuration.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Most bytes one run reads.
const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

/// Where one value occurs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Found {
    /// Tracked files holding it.
    pub(crate) files: usize,
    /// The first of them, repository-relative.
    pub(crate) first: String,
}

impl Found {
    /// "occurs in 3 tracked files of the repository (src/config.ts, …)".
    pub(crate) fn describe(&self) -> String {
        let (noun, more) = if self.files == 1 {
            ("file", "")
        } else {
            ("files", ", …")
        };
        format!(
            "occurs in {} tracked {noun} of the repository ({}{more})",
            self.files, self.first
        )
    }
}

/// What a search found.
#[derive(Debug, Default)]
pub(crate) struct Search {
    /// Per value, in order: where it occurs, if anywhere.
    pub(crate) found: Vec<Option<Found>>,
    /// Whether it stopped at its byte budget before the last file.
    pub(crate) partial: bool,
}

/// Looks for each of `values` (empty ones are never found) in the tracked text files of the
/// checkout at `root`: regular files up to [`MAX_FILE_BYTES`], [`MAX_TOTAL_BYTES`] in all; links,
/// binary and larger files are skipped.
///
/// # Errors
/// Why nothing was looked for: `git ls-files` failed, or the values make no matcher.
pub(crate) fn find_in_checkout(root: &Path, values: &[&str]) -> Result<Search, String> {
    find_within(root, values, MAX_FILE_BYTES, MAX_TOTAL_BYTES)
}

fn find_within(
    root: &Path,
    values: &[&str],
    max_file: u64,
    max_total: u64,
) -> Result<Search, String> {
    let mut search = Search {
        found: vec![None; values.len()],
        partial: false,
    };
    let wanted: Vec<(usize, &str)> = values
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, v)| !v.is_empty())
        .collect();
    if wanted.is_empty() {
        return Ok(search);
    }
    // Its error names a limit, never a value.
    let matcher = AhoCorasick::new(wanted.iter().map(|(_, v)| v)).map_err(|e| e.to_string())?;
    let listed = git(
        root,
        &[OsStr::new("ls-files"), OsStr::new("-z")],
        LIST_OUTPUT,
        QUERY_TIMEOUT,
    )
    .map_err(|e| e.to_string())?;
    let mut total: u64 = 0;
    let mut seen = vec![false; wanted.len()];
    for raw in listed.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let relative = String::from_utf8_lossy(raw);
        let path = root.join(relative.as_ref());
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.is_file() || meta.len() > max_file {
            continue;
        }
        if total + meta.len() > max_total {
            search.partial = true;
            break;
        }
        // Read at most one byte past the bound: a file that grew since is skipped, not read whole.
        let mut bytes = Vec::new();
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        if file.take(max_file + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 > max_file
        {
            continue;
        }
        total += bytes.len() as u64;
        if std::str::from_utf8(&bytes).is_err() {
            continue;
        }
        seen.fill(false);
        for hit in matcher.find_overlapping_iter(&bytes) {
            seen[hit.pattern().as_usize()] = true;
        }
        for (pattern, _) in seen.iter().enumerate().filter(|(_, hit)| **hit) {
            let slot = &mut search.found[wanted[pattern].0];
            match slot {
                Some(found) => found.files += 1,
                None => {
                    *slot = Some(Found {
                        files: 1,
                        first: relative.to_string(),
                    });
                }
            }
        }
    }
    Ok(search)
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn found_names_its_files() {
        let one = Found {
            files: 1,
            first: "a.yml".into(),
        };
        assert_eq!(
            one.describe(),
            "occurs in 1 tracked file of the repository (a.yml)"
        );
        let many = Found {
            files: 3,
            first: "a.yml".into(),
        };
        assert_eq!(
            many.describe(),
            "occurs in 3 tracked files of the repository (a.yml, …)"
        );
    }

    /// A repository with `files` tracked (`(path, content)`) and `links` (`(path, target)`).
    fn repo(
        files: &[(&str, &[u8])],
        links: &[(&str, &Path)],
    ) -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        for (name, content) in files {
            std::fs::write(dir.path().join(name), content)?;
        }
        for (name, target) in links {
            std::os::unix::fs::symlink(target, dir.path().join(name))?;
        }
        for args in [&["init", "-q"][..], &["add", "-A"][..]] {
            let status = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .status()?;
            assert!(status.success(), "git {args:?}");
        }
        Ok(dir)
    }

    #[test]
    fn every_value_is_found_in_one_pass_and_counted_per_file() -> TestResult {
        let dir = repo(
            &[
                ("a.yml", b"env: production-like-1\nbucket: bucket-0042\n"),
                ("b.yml", b"bucket: bucket-0042 bucket-0042\n"),
            ],
            &[],
        )?;
        let search = find_within(
            dir.path(),
            &["bucket-0042", "", "nowhere-0001", "production-like-1"],
            1024,
            1024,
        )?;
        assert_eq!(
            search.found,
            vec![
                Some(Found {
                    files: 2,
                    first: "a.yml".into()
                }),
                None,
                None,
                Some(Found {
                    files: 1,
                    first: "a.yml".into()
                }),
            ]
        );
        assert!(!search.partial);
        Ok(())
    }

    /// Review of SPEC-274: links, larger and binary files are skipped; the budget stops the search.
    #[test]
    fn links_large_and_binary_files_are_skipped_and_the_budget_holds() -> TestResult {
        let outside = tempfile::tempdir()?;
        let target = outside.path().join("held.txt");
        std::fs::write(&target, "bucket-0042")?;
        let large = format!("{}bucket-0042", "x".repeat(100));
        let dir = repo(
            &[
                ("binary.bin", b"\xff\xfebucket-0042"),
                ("large.txt", large.as_bytes()),
            ],
            &[("link.txt", &target)],
        )?;
        let search = find_within(dir.path(), &["bucket-0042"], 64, 1024)?;
        assert_eq!(search.found, vec![None]);
        assert!(!search.partial);

        let dir = repo(&[("a.txt", b"0123456789"), ("b.txt", b"bucket-0042")], &[])?;
        let search = find_within(dir.path(), &["bucket-0042"], 64, 15)?;
        assert_eq!(search.found, vec![None], "b.txt is past the budget");
        assert!(search.partial);
        Ok(())
    }
}
