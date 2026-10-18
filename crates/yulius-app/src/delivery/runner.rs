//! The output and end of a delivery environment command (SPEC-001 DLV-003). The command itself
//! runs through the process host like a CD run (SPEC-219, `crate::ci`'s `environment` module).

use std::collections::VecDeque;

/// How a command ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandEnd {
    /// The leader exited with this status code.
    Exited(i32),
    /// The leader was terminated by this signal.
    Signaled(i32),
    /// The deadline elapsed; the process group was killed and the leader reaped.
    TimedOut,
    /// The command could not be started (or waiting for it failed).
    SpawnFailed(String),
}

impl CommandEnd {
    /// Exit status 0.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        matches!(self, Self::Exited(0))
    }

    /// The exit code, when the leader exited normally.
    #[must_use]
    pub fn exit_code(&self) -> Option<i32> {
        match self {
            Self::Exited(code) => Some(*code),
            _ => None,
        }
    }
}

/// Keeps the last `limit` bytes written to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputRing {
    bytes: VecDeque<u8>,
    limit: usize,
    dropped: u64,
}

impl OutputRing {
    /// An empty ring holding at most `limit` bytes (at least 1).
    #[must_use]
    pub fn new(limit: usize) -> Self {
        let limit = limit.max(1);
        Self {
            bytes: VecDeque::with_capacity(limit.min(64 * 1024)),
            limit,
            dropped: 0,
        }
    }

    /// Appends bytes, discarding the oldest ones beyond the limit.
    pub fn push(&mut self, data: &[u8]) {
        if data.len() >= self.limit {
            self.dropped = self.dropped.saturating_add(
                u64::try_from(self.bytes.len() + data.len() - self.limit).unwrap_or(u64::MAX),
            );
            self.bytes.clear();
            self.bytes.extend(&data[data.len() - self.limit..]);
            return;
        }
        let overflow = (self.bytes.len() + data.len()).saturating_sub(self.limit);
        if overflow > 0 {
            self.bytes.drain(..overflow);
            self.dropped = self
                .dropped
                .saturating_add(u64::try_from(overflow).unwrap_or(u64::MAX));
        }
        self.bytes.extend(data);
    }

    /// Appends text.
    pub fn push_str(&mut self, text: &str) {
        self.push(text.as_bytes());
    }

    /// Bytes discarded so far.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Bytes currently held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Nothing held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// The held bytes as text (invalid UTF-8 replaced), prefixed by a note when earlier
    /// output was discarded.
    #[must_use]
    pub fn text(&self) -> String {
        let (front, back) = self.bytes.as_slices();
        let mut raw = Vec::with_capacity(self.bytes.len());
        raw.extend_from_slice(front);
        raw.extend_from_slice(back);
        let body = String::from_utf8_lossy(&raw);
        if self.dropped == 0 {
            body.into_owned()
        } else {
            format!(
                "[yulius] {} earlier bytes of output were discarded\n{body}",
                self.dropped
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_the_tail_and_counts_discarded_bytes() {
        let mut ring = OutputRing::new(8);
        ring.push(b"abcd");
        ring.push(b"efgh");
        assert_eq!(ring.text(), "abcdefgh");
        ring.push(b"ij");
        assert_eq!(ring.dropped(), 2);
        assert!(ring.text().ends_with("cdefghij"));
        assert!(ring.text().starts_with("[yulius] 2 earlier bytes"));
        ring.push(b"0123456789");
        assert_eq!(ring.len(), 8);
        assert_eq!(ring.dropped(), 12);
        assert!(ring.text().ends_with("23456789"));
    }
}
