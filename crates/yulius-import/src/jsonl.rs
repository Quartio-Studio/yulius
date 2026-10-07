//! Bounded JSON Lines reading (SPEC-002 §9: transcript line 4 MiB; at most 64 MiB scanned).

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// Bytes scanned per transcript before reading stops.
pub(crate) const MAX_SCAN_BYTES: u64 = 64 * 1024 * 1024;
/// Lines longer than this are skipped.
pub(crate) const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

/// One read line.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Line {
    /// A complete line (without the newline) is in the buffer.
    Data,
    /// The line exceeded the bound and was skipped.
    TooLong,
}

/// Reads newline-terminated lines with a per-line bound and a total byte budget.
pub(crate) struct JsonlReader<R> {
    inner: BufReader<R>,
    consumed: u64,
    budget: u64,
    max_line: usize,
    /// `true` once the byte budget stopped reading before the end of input.
    pub(crate) truncated: bool,
}

impl JsonlReader<File> {
    /// Opens `path` with the default bounds.
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        Ok(Self::new(File::open(path)?, MAX_SCAN_BYTES, MAX_LINE_BYTES))
    }
}

impl<R: Read> JsonlReader<R> {
    /// Wraps `reader` with explicit bounds.
    pub(crate) fn new(reader: R, budget: u64, max_line: usize) -> Self {
        Self {
            inner: BufReader::with_capacity(64 * 1024, reader),
            consumed: 0,
            budget,
            max_line,
            truncated: false,
        }
    }

    /// Reads the next line into `buf`. Returns `None` at end of input or when the budget is
    /// exhausted (an incomplete line cut by the budget is dropped). A final line without a
    /// trailing newline is returned.
    pub(crate) fn next_line(&mut self, buf: &mut Vec<u8>) -> io::Result<Option<Line>> {
        buf.clear();
        let mut too_long = false;
        let mut started = false;
        loop {
            if self.consumed >= self.budget {
                if !self.inner.fill_buf()?.is_empty() {
                    self.truncated = true;
                }
                // Whatever was gathered is an incomplete line cut by the budget.
                buf.clear();
                return Ok(None);
            }
            let available = self.inner.fill_buf()?;
            if available.is_empty() {
                return Ok(match (started, too_long) {
                    (false, _) => None,
                    (true, true) => Some(Line::TooLong),
                    (true, false) => Some(Line::Data),
                });
            }
            started = true;
            let remaining = usize::try_from(self.budget - self.consumed).unwrap_or(usize::MAX);
            let window = &available[..available.len().min(remaining)];
            let (chunk, consumed, complete) = match window.iter().position(|b| *b == b'\n') {
                Some(index) => (&window[..index], index + 1, true),
                None => (window, window.len(), false),
            };
            if !too_long {
                if buf.len() + chunk.len() > self.max_line {
                    too_long = true;
                    buf.clear();
                } else {
                    buf.extend_from_slice(chunk);
                }
            }
            self.inner.consume(consumed);
            self.consumed += consumed as u64;
            if complete {
                return Ok(Some(if too_long { Line::TooLong } else { Line::Data }));
            }
        }
    }
}

/// Complete lines from the last `window` bytes of `path` (the first, partial line is dropped
/// unless the window covers the whole file). Used to find a large file's final timestamps.
pub(crate) fn tail_lines(path: &Path, window: u64) -> io::Result<Vec<Vec<u8>>> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let start = size.saturating_sub(window);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(window).read_to_end(&mut bytes)?;
    let mut lines: Vec<Vec<u8>> = bytes.split(|b| *b == b'\n').map(<[u8]>::to_vec).collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    lines.retain(|line| !line.is_empty());
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_all(input: &[u8], budget: u64, max_line: usize) -> io::Result<(Vec<String>, bool)> {
        let mut reader = JsonlReader::new(input, budget, max_line);
        let mut buf = Vec::new();
        let mut out = Vec::new();
        while let Some(line) = reader.next_line(&mut buf)? {
            out.push(match line {
                Line::Data => String::from_utf8_lossy(&buf).into_owned(),
                Line::TooLong => "<too long>".to_owned(),
            });
        }
        Ok((out, reader.truncated))
    }

    #[test]
    fn splits_lines_and_keeps_final_unterminated_line() -> io::Result<()> {
        let (lines, truncated) = read_all(b"a\nbb\n\nccc", 1000, 10)?;
        assert_eq!(lines, ["a", "bb", "", "ccc"]);
        assert!(!truncated);
        Ok(())
    }

    #[test]
    fn skips_overlong_lines_and_continues() -> io::Result<()> {
        let mut input = b"ok\n".to_vec();
        input.extend(std::iter::repeat_n(b'x', 50));
        input.extend(b"\nafter\n");
        let (lines, _) = read_all(&input, 1000, 10)?;
        assert_eq!(lines, ["ok", "<too long>", "after"]);
        Ok(())
    }

    #[test]
    fn stops_at_the_byte_budget_and_drops_the_cut_line() -> io::Result<()> {
        let (lines, truncated) = read_all(b"one\ntwo\nthree\n", 6, 100)?;
        assert_eq!(lines, ["one"]);
        assert!(truncated);
        let (lines, truncated) = read_all(b"one\ntwo\n", 8, 100)?;
        assert_eq!(lines, ["one", "two"]);
        assert!(!truncated);
        Ok(())
    }

    #[test]
    fn tail_drops_the_partial_first_line() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("t.jsonl");
        std::fs::write(&path, b"first line\nsecond\nthird\n")?;
        let lines = tail_lines(&path, 13)?;
        assert_eq!(lines, [b"third".to_vec()]);
        let lines = tail_lines(&path, 1000)?;
        assert_eq!(lines.len(), 3);
        Ok(())
    }
}
