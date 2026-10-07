//! Incremental JSONL transcript tailing (SPEC-002 §7.2 "Transcript tailing").
//!
//! The caller polls (SPEC-002: every second). Each poll returns only complete lines appended
//! since the previous poll. Partial trailing lines wait for their newline. A file that shrank
//! is treated as truncated and read again from the start. Lines longer than the bound are
//! skipped and counted. A missing file yields no lines until it appears.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Default maximum line length (SPEC-002 §9: 4 MiB).
pub const DEFAULT_MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

/// Maximum bytes read from the file by one [`TranscriptTail::poll`]; the rest is read by later
/// polls. Bounds the work and memory of a single poll.
pub const MAX_POLL_BYTES: u64 = 16 * 1024 * 1024;

const READ_CHUNK: usize = 64 * 1024;

/// Follows one append-only JSONL file.
#[derive(Debug)]
pub struct TranscriptTail {
    path: PathBuf,
    max_line_bytes: usize,
    read_pos: u64,
    committed: u64,
    partial: Vec<u8>,
    skipping: bool,
    skipped_lines: u64,
    truncations: u64,
}

impl TranscriptTail {
    /// Starts tailing `path` from its beginning. The file need not exist yet.
    ///
    /// # Errors
    /// Returns `InvalidInput` when `max_line_bytes` is zero.
    pub fn open(path: impl Into<PathBuf>, max_line_bytes: usize) -> io::Result<Self> {
        Self::open_at(path, max_line_bytes, 0)
    }

    /// Starts tailing `path` from byte `offset` (a cursor saved from [`Self::offset`]). The
    /// offset should be at a line boundary; if the file is shorter, tailing restarts at 0.
    ///
    /// # Errors
    /// Returns `InvalidInput` when `max_line_bytes` is zero.
    pub fn open_at(
        path: impl Into<PathBuf>,
        max_line_bytes: usize,
        offset: u64,
    ) -> io::Result<Self> {
        if max_line_bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "max_line_bytes must be positive",
            ));
        }
        Ok(Self {
            path: path.into(),
            max_line_bytes,
            read_pos: offset,
            committed: offset,
            partial: Vec::new(),
            skipping: false,
            skipped_lines: 0,
            truncations: 0,
        })
    }

    /// Starts tailing `path` at its current end, so only lines appended later are returned.
    ///
    /// # Errors
    /// Returns `InvalidInput` when `max_line_bytes` is zero, or an I/O error other than
    /// "not found" while reading the file size.
    pub fn open_at_end(path: impl Into<PathBuf>, max_line_bytes: usize) -> io::Result<Self> {
        let path = path.into();
        let len = match std::fs::metadata(&path) {
            Ok(meta) => meta.len(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error),
        };
        Self::open_at(path, max_line_bytes, len)
    }

    /// The tailed path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Byte offset just after the last complete line consumed: a resume cursor for
    /// [`Self::open_at`]. A buffered partial line is not included.
    #[must_use]
    pub fn offset(&self) -> u64 {
        self.committed
    }

    /// `true` while an incomplete trailing line is buffered.
    #[must_use]
    pub fn has_partial_line(&self) -> bool {
        !self.partial.is_empty() || self.skipping
    }

    /// Number of lines skipped because they exceeded the line bound.
    #[must_use]
    pub fn skipped_lines(&self) -> u64 {
        self.skipped_lines
    }

    /// Number of times the file shrank and tailing restarted from the beginning.
    #[must_use]
    pub fn truncations(&self) -> u64 {
        self.truncations
    }

    /// Returns the complete, non-empty lines appended since the last poll (without line
    /// terminators; invalid UTF-8 is replaced).
    ///
    /// # Errors
    /// Returns I/O errors other than "not found" from opening, seeking or reading the file.
    pub fn poll(&mut self) -> io::Result<Vec<String>> {
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let len = file.metadata()?.len();
        if len < self.read_pos {
            self.read_pos = 0;
            self.committed = 0;
            self.partial.clear();
            self.skipping = false;
            self.truncations += 1;
        }
        if len == self.read_pos {
            return Ok(Vec::new());
        }
        file.seek(SeekFrom::Start(self.read_pos))?;
        let mut reader = file.take(MAX_POLL_BYTES.min(len - self.read_pos));
        let mut lines = Vec::new();
        let mut chunk = vec![0u8; READ_CHUNK];
        loop {
            let read = reader.read(&mut chunk)?;
            if read == 0 {
                break;
            }
            self.consume(&chunk[..read], &mut lines);
        }
        Ok(lines)
    }

    fn consume(&mut self, mut data: &[u8], lines: &mut Vec<String>) {
        while !data.is_empty() {
            let newline = data.iter().position(|&b| b == b'\n');
            let (piece, rest, complete) = match newline {
                Some(at) => (&data[..at], &data[at + 1..], true),
                None => (data, &data[data.len()..], false),
            };
            data = rest;
            self.read_pos += piece.len() as u64 + u64::from(complete);
            if complete {
                self.committed = self.read_pos;
            }
            if self.skipping {
                if complete {
                    self.skipping = false;
                    self.skipped_lines += 1;
                }
                continue;
            }
            if self.partial.len() + piece.len() > self.max_line_bytes {
                self.partial.clear();
                if complete {
                    self.skipped_lines += 1;
                } else {
                    self.skipping = true;
                }
                continue;
            }
            self.partial.extend_from_slice(piece);
            if complete {
                let mut line = std::mem::take(&mut self.partial);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                if !line.iter().all(u8::is_ascii_whitespace) {
                    lines.push(String::from_utf8_lossy(&line).into_owned());
                }
            }
        }
    }
}
