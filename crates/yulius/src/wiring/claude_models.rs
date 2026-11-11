//! Which models a Claude Code build knows (SPEC-461 CCM-002, ADR-0293).
//!
//! A Claude Code binary names the models it knows as plain text (its model tables are bundled
//! JavaScript): 2.1.322 holds `claude-fable-5-2` 26 times, 2.1.311 not once. So a build is read
//! for the ids the composer offers, in chunks, and never run: a downloaded release in the pass
//! that checks its version, and the version sandboxes get when the server looks at it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use aho_corasick::AhoCorasick;
use yulius_domain::agent_cli::{Version, ends_model_id, missing_models};
use yulius_domain::harness::OFFERED_CLAUDE_MODELS;

/// Texts looked for in a stream read in chunks, each found once.
pub struct Scan {
    matcher: AhoCorasick,
    /// Whether each text must end where a model id ends ([`ends_model_id`]).
    whole: Vec<bool>,
    found: Vec<bool>,
    /// The end of what was read, as long as the longest text: one may straddle two chunks, or
    /// end a chunk with the byte after it still to come.
    carry: Vec<u8>,
    longest: usize,
    left: usize,
}

impl Scan {
    /// A scan for `texts`, each with whether it is a model id (and must end like one).
    ///
    /// # Errors
    /// When the matcher cannot be built.
    pub fn new(texts: &[(String, bool)]) -> Result<Self, String> {
        let matcher = AhoCorasick::new(texts.iter().map(|(text, _)| text.as_bytes()))
            .map_err(|e| format!("cannot look for the texts: {e}"))?;
        Ok(Self {
            matcher,
            whole: texts.iter().map(|(_, whole)| *whole).collect(),
            found: vec![false; texts.len()],
            carry: Vec::new(),
            longest: texts.iter().map(|(text, _)| text.len()).max().unwrap_or(0),
            left: texts.len(),
        })
    }

    /// Whether every text was found: nothing more needs reading.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.left == 0
    }

    /// Reads the next chunk.
    pub fn feed(&mut self, chunk: &[u8]) {
        if self.complete() || chunk.is_empty() {
            return;
        }
        let mut buffer = std::mem::take(&mut self.carry);
        buffer.extend_from_slice(chunk);
        self.search(&buffer, false);
        let keep = self.longest.min(buffer.len());
        buffer.drain(..buffer.len() - keep);
        self.carry = buffer;
    }

    /// Which texts were found, in the order given; the end of the stream ends a model id.
    #[must_use]
    pub fn finish(mut self) -> Vec<bool> {
        let rest = std::mem::take(&mut self.carry);
        self.search(&rest, true);
        self.found
    }

    fn search(&mut self, buffer: &[u8], at_end: bool) {
        if self.complete() {
            return;
        }
        // Every occurrence, also of a text inside a longer one.
        for found in self.matcher.find_overlapping_iter(buffer) {
            let index = found.pattern().as_usize();
            if self.found[index] {
                continue;
            }
            let ends = match buffer.get(found.end()) {
                Some(next) => !self.whole[index] || ends_model_id(Some(*next)),
                // The byte after it comes with the next chunk, or never.
                None => !self.whole[index] || at_end,
            };
            if ends {
                self.found[index] = true;
                self.left -= 1;
                if self.complete() {
                    return;
                }
            }
        }
    }
}

/// A scan of one build: its embedded version, when one is expected, and the offered models.
pub struct BuildScan {
    scan: Scan,
    version: bool,
}

impl BuildScan {
    /// A scan for the offered models and, with `version`, for `VERSION:"<version>"` as Claude
    /// Code's builds embed it.
    ///
    /// # Errors
    /// When the matcher cannot be built.
    pub fn new(version: Option<Version>) -> Result<Self, String> {
        let mut texts: Vec<(String, bool)> = version
            .map(|version| (format!("VERSION:\"{version}\""), false))
            .into_iter()
            .collect();
        texts.extend(
            OFFERED_CLAUDE_MODELS
                .iter()
                .map(|model| ((*model).to_owned(), true)),
        );
        Ok(Self {
            scan: Scan::new(&texts)?,
            version: version.is_some(),
        })
    }

    /// Reads the next chunk.
    pub fn feed(&mut self, chunk: &[u8]) {
        self.scan.feed(chunk);
    }

    /// Whether everything was found: the rest of the build need not be searched.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.scan.complete()
    }

    /// Whether the expected version was named (true when none was expected), and the offered
    /// models the build does not know, the default first.
    #[must_use]
    pub fn finish(self) -> (bool, Vec<String>) {
        let found = self.scan.finish();
        let (named, models) = if self.version {
            (found.first().copied().unwrap_or(false), &found[1..])
        } else {
            (true, &found[..])
        };
        let known: Vec<&str> = OFFERED_CLAUDE_MODELS
            .iter()
            .zip(models)
            .filter(|(_, found)| **found)
            .map(|(model, _)| *model)
            .collect();
        (named, missing_models(&known))
    }
}

/// The offered models the build at `binary` does not know; `None` when it cannot be read.
/// Blocking: a Claude Code binary is about 340 MB.
#[must_use]
pub fn missing_in(binary: &Path) -> Option<Vec<String>> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(binary).ok()?;
    let mut scan = BuildScan::new(None).ok()?;
    let mut buffer = vec![0u8; 1024 * 1024];
    while !scan.complete() {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => scan.feed(&buffer[..n]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(scan.finish().1)
}

/// The file a reading was taken of: read again once any of these changes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
}

/// [`missing_in`], remembered per file while the file does not change (CCM-005): the version
/// sandboxes get is asked about at every check, every page view and every turn's end.
#[derive(Default)]
pub struct Known {
    last: Mutex<Option<(Identity, Vec<String>)>>,
}

impl Known {
    /// The offered models the build at `binary` does not know; `None` when it cannot be read.
    /// Blocking the first time for a file.
    #[must_use]
    pub fn missing(&self, binary: &Path) -> Option<Vec<String>> {
        let meta = std::fs::metadata(binary).ok()?;
        let identity = Identity {
            path: binary.to_path_buf(),
            len: meta.len(),
            modified: meta.modified().ok(),
        };
        if let Ok(last) = self.last.lock()
            && let Some((known, missing)) = last.as_ref()
            && *known == identity
        {
            return Some(missing.clone());
        }
        let missing = missing_in(binary)?;
        if let Ok(mut last) = self.last.lock() {
            *last = Some((identity, missing.clone()));
        }
        Some(missing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: &str = OFFERED_CLAUDE_MODELS[0];

    fn scan_in(chunks: &[&[u8]], version: Option<&str>) -> (bool, Vec<String>) {
        let version = version.and_then(Version::parse);
        let mut scan = BuildScan::new(version).unwrap_or_else(|e| panic!("{e}"));
        for chunk in chunks {
            scan.feed(chunk);
        }
        scan.finish()
    }

    fn all_models() -> String {
        OFFERED_CLAUDE_MODELS
            .iter()
            .map(|model| format!("{{id:\"{model}\"}},"))
            .collect()
    }

    #[test]
    fn a_build_naming_every_offered_model_misses_none() {
        let text = format!("....VERSION:\"2.1.322\"....{}", all_models());
        let (named, missing) = scan_in(&[text.as_bytes()], Some("2.1.322"));
        assert!(named);
        assert!(missing.is_empty(), "{missing:?}");
        // Another version is not named, whatever the models.
        assert!(!scan_in(&[text.as_bytes()], Some("2.1.323")).0);
    }

    #[test]
    fn a_model_id_and_the_version_are_found_across_chunks() {
        let text = format!("xx{}VERSION:\"2.1.322\"yy", all_models());
        let bytes = text.as_bytes();
        // Every split point, and one byte at a time.
        for cut in 0..bytes.len() {
            let (named, missing) = scan_in(&[&bytes[..cut], &bytes[cut..]], Some("2.1.322"));
            assert!(named && missing.is_empty(), "cut at {cut}: {missing:?}");
        }
        let single: Vec<&[u8]> = bytes.chunks(1).collect();
        let (named, missing) = scan_in(&single, Some("2.1.322"));
        assert!(named && missing.is_empty(), "{missing:?}");
    }

    #[test]
    fn an_id_that_goes_on_is_another_model() {
        // A longer id, a dated id and a letter after it are not the offered model.
        for other in [
            format!("\"{DEFAULT}0\""),
            format!("\"{DEFAULT}-20261101\""),
            format!("\"{DEFAULT}x\""),
        ] {
            let (_, missing) = scan_in(&[other.as_bytes()], None);
            assert!(missing.iter().any(|m| m == DEFAULT), "{other}");
            // Also when the byte that goes on arrives with the next chunk.
            let (id, rest) = other.split_at(1 + DEFAULT.len());
            let (_, missing) = scan_in(&[id.as_bytes(), rest.as_bytes()], None);
            assert!(
                missing.iter().any(|m| m == DEFAULT),
                "{other} in two chunks"
            );
        }
        // Followed by a quote, a bracket or the end of the file, it is.
        for known in [
            format!("\"{DEFAULT}\""),
            format!("{DEFAULT}[1m]"),
            DEFAULT.to_owned(),
        ] {
            let (_, missing) = scan_in(&[known.as_bytes()], None);
            assert!(!missing.iter().any(|m| m == DEFAULT), "{known}");
        }
        // The longer id does not hide a plain one later on.
        let both = format!("\"{DEFAULT}-20261101\",\"{DEFAULT}\"");
        let (_, missing) = scan_in(&[both.as_bytes()], None);
        assert!(!missing.iter().any(|m| m == DEFAULT));
    }

    #[test]
    fn missing_models_are_named_the_default_first() {
        let text = "\"claude-opus-5-5\",\"claude-sonnet-5-5\"";
        let (named, missing) = scan_in(&[text.as_bytes()], None);
        assert!(named, "no version was expected");
        assert_eq!(missing, [DEFAULT, "claude-fable-5-1", "claude-haiku-4-5"]);
        assert_eq!(scan_in(&[], None).1.len(), OFFERED_CLAUDE_MODELS.len());
    }

    #[test]
    fn a_file_is_read_once_until_it_changes() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let binary = dir.path().join("claude.exe");
        std::fs::write(&binary, all_models())?;
        let known = Known::default();
        assert_eq!(known.missing(&binary), Some(Vec::new()));
        // Same size and time: the remembered reading, though the bytes differ.
        let before = std::fs::metadata(&binary)?.modified()?;
        std::fs::write(&binary, "x".repeat(all_models().len()))?;
        std::fs::File::options()
            .write(true)
            .open(&binary)?
            .set_modified(before)?;
        assert_eq!(known.missing(&binary), Some(Vec::new()));
        // Another size: read again.
        std::fs::write(&binary, "\"claude-opus-5-5\"")?;
        assert_eq!(known.missing(&binary).map(|m| m.len()), Some(4));
        assert_eq!(known.missing(&dir.path().join("none")), None);
        assert_eq!(missing_in(&dir.path().join("none")), None);
        Ok(())
    }
}
