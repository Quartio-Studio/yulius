//! What the new host holds of a move's staged copy, as the old host knows it (SPEC-465 WCP-003,
//! WCP-006, WCP-011, ADR-0303). A receipt maps a path to the state the new host holds there: a
//! digest of the entry's archive headers, its inode's change time and its inode number. A pass
//! skips an entry only when its path's receipt is its current state, so sending a path revokes
//! what was known of it.
//!
//! The receipts live in the move's state directory: `stage.json` (the stage id, the count of
//! acknowledged streams, the valid length of the log), a log of 32-byte records (path, state; a
//! zero state revokes) and, while a stream is in flight, its journal `pending`: 40 bytes per sent
//! entry in stream order (the stream's digest before the entry, its path, its state once
//! earned). The new host's marker says how much of a stream it installed and synced;
//! [`Stage::reconcile`] adopts exactly that much.
//!
//! A receipt is believed only where it was earned ([`Era`]): in the same boot, and while the
//! clock was not set back. An inode's times are not written with its data, so after a crash a
//! file may hold newer content than its change time says; and a clock that goes back can stamp
//! a later change with the time of an earlier one. In both cases the receipts are dropped and
//! the staged copy is filled again.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufWriter, Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, FileExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::import::Mark;
use crate::wiring::update_files::write_atomic;

/// A path's or a state's digest.
pub type Digest = [u8; 16];
/// A journal record: the stream's digest before the entry, its path, its state.
const SENT: usize = 40;
/// A log record: a path and its state.
const HELD: usize = 32;
/// No state: the path is not known to be held.
const NONE: Digest = [0; 16];

/// The state of an entry: its archive headers (path, kind, size, owner, mode, time, attributes),
/// its inode's change time and its inode number. A rewrite in place, a time set back, a rename
/// or a new file at the same path each give another one.
#[must_use]
pub fn state(headers: &[u8], ino: u64, ctime: (i64, i64)) -> Digest {
    let mut hash = Sha256::new();
    hash.update(headers);
    hash.update(ino.to_le_bytes());
    hash.update(ctime.0.to_le_bytes());
    hash.update(ctime.1.to_le_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&hash.finalize()[..16]);
    out
}

/// Where and when a host is, as far as receipts go (SPEC-465 WCP-016).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Era {
    /// This boot's id; empty where the kernel names none.
    pub boot: String,
    /// How far the clock is ahead of the time this boot has run, in milliseconds; `None` where
    /// that is not known. Setting the clock changes it, the clock's own pace does not.
    pub ahead: Option<i64>,
}

impl Era {
    /// This host, now.
    #[must_use]
    pub fn here() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|now| i64::try_from(now.as_millis()).ok());
        Self {
            boot: boot_id(),
            ahead: now.zip(up_millis()).map(|(now, up)| now - up),
        }
    }
}

/// How long this boot has run, in milliseconds.
fn up_millis() -> Option<i64> {
    let text = std::fs::read_to_string("/proc/uptime").ok()?;
    let secs: f64 = text.split_whitespace().next()?.parse().ok()?;
    Some((secs * 1000.0) as i64)
}

/// A clock that fell this far behind where it was when a pass began was set back: far enough
/// to notice every step that could give a later change an earlier entry's time (two seconds,
/// [`super::export::RACY_SECS`]), and more than a leap second.
const SET_BACK_MS: i64 = 1_500;

/// Why receipts are not believed any more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outdated {
    /// The host was restarted since they were earned.
    Restarted,
    /// The host's clock was set back since a pass began.
    ClockBack,
}

/// What a pass asks and records while it streams.
pub trait Ledger {
    /// Whether the new host holds `path` in `state`.
    fn held(&self, path: &Digest, state: &Digest) -> bool;
    /// An entry is about to be sent: its path and the stream's digest before it.
    ///
    /// # Errors
    /// The record cannot be kept; the entry must not be sent then.
    fn sending(&mut self, path: Digest, check: [u8; 8]) -> io::Result<()>;
    /// The entry just sent earned a receipt for `state`.
    ///
    /// # Errors
    /// The record cannot be kept.
    fn earned(&mut self, state: Digest) -> io::Result<()>;
    /// An entry was left alone because the new host holds it (exact passes keep the list).
    ///
    /// # Errors
    /// The record cannot be kept.
    fn kept(&mut self, path: Digest, state: Digest) -> io::Result<()>;
    /// Makes the records durable: called before a stream's checksum is sent, so a stream the
    /// new host acknowledges whole always has its whole record here.
    ///
    /// # Errors
    /// The records cannot be written.
    fn seal(&mut self) -> io::Result<()>;
}

/// A stream sent and not yet reconciled with the new host's marker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Pending {
    /// The count of acknowledged streams it builds on.
    seq: u64,
    /// Its name in the manifest.
    nonce: String,
    /// Read while the instance ran.
    live: bool,
    /// When it began, seconds since the epoch.
    began: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Meta {
    /// 1.
    version: u32,
    /// The staged copy's id.
    stage: String,
    /// Streams the new host acknowledged.
    acked: u64,
    /// The log's file name.
    log: String,
    /// Its valid length.
    log_len: u64,
    /// A whole pass was acknowledged: the staged copy holds every path as of some moment.
    whole: bool,
    /// When the last acknowledged stream began.
    began: Option<u64>,
    /// The stream in flight.
    pending: Option<Pending>,
    /// The boot the receipts and the journal were written in ([`Era`]).
    #[serde(default)]
    boot: String,
    /// The furthest the clock was ahead of the boot's own time when a pass began, since there
    /// were no receipts.
    #[serde(default)]
    ahead: Option<i64>,
}

/// What [`Stage::reconcile`] found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reconciled {
    /// The hosts agree on no staged copy: the receipts are gone and the next pass is a full one
    /// into a new stage.
    pub fresh: bool,
    /// Receipts adopted from the pending stream.
    pub adopted: u64,
    /// Paths of the pending stream whose receipts were revoked (sent beyond what was installed,
    /// or not installed there).
    pub revoked: u64,
    /// Every receipt was dropped, and why: the stage goes on, and the next pass sends everything
    /// into it again.
    pub outdated: Option<Outdated>,
}

/// The receipts of one staged copy.
pub struct Stage {
    dir: PathBuf,
    meta: Meta,
    held: HashMap<Digest, Digest>,
    journal: Option<Journal>,
}

struct Journal {
    file: File,
    count: u64,
    kept: Option<BufWriter<File>>,
}

impl std::fmt::Debug for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stage")
            .field("meta", &self.meta)
            .field("held", &self.held.len())
            .finish_non_exhaustive()
    }
}

fn digest(bytes: &[u8]) -> Digest {
    let mut out = [0u8; 16];
    out.copy_from_slice(&bytes[..16]);
    out
}

/// A random name for a stage or a stream.
#[must_use]
pub fn token() -> String {
    let mut seed = [0u8; 16];
    let read = File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut seed));
    if read.is_err() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let mixed = Sha256::digest(format!("{now}{}", std::process::id()).as_bytes());
        seed.copy_from_slice(&mixed[..16]);
    }
    hex::encode(&seed[..8])
}

/// This boot's id; empty where the kernel names none.
#[must_use]
pub fn boot_id() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|id| id.trim().to_owned())
        .unwrap_or_default()
}

fn private(path: &Path, truncate: bool) -> io::Result<File> {
    File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(truncate)
        .mode(0o600)
        .open(path)
}

impl Stage {
    /// The receipts kept in `dir`; none when there are none or they cannot be read (the next
    /// pass is then a full one).
    #[must_use]
    pub fn open(dir: &Path) -> Self {
        let meta = std::fs::read_to_string(dir.join("stage.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Meta>(&text).ok())
            .filter(|meta| meta.version == 1 && !meta.stage.is_empty());
        let mut stage = Self {
            dir: dir.to_path_buf(),
            meta: meta.unwrap_or_default(),
            held: HashMap::new(),
            journal: None,
        };
        if stage.load().is_err() {
            stage.meta = Meta::default();
            stage.held.clear();
        }
        stage
    }

    /// Replays the log up to its valid length.
    fn load(&mut self) -> io::Result<()> {
        if self.meta.stage.is_empty() || self.meta.log_len == 0 {
            return Ok(());
        }
        let file = File::open(self.dir.join(&self.meta.log))?;
        if file.metadata()?.len() < self.meta.log_len {
            return Err(io::Error::other("the receipts are shorter than recorded"));
        }
        let records = usize::try_from(self.meta.log_len).unwrap_or(0) / HELD;
        self.held.reserve(records);
        let mut reader = io::BufReader::with_capacity(1 << 20, file.take(self.meta.log_len));
        let mut record = [0u8; HELD];
        for _ in 0..records {
            reader.read_exact(&mut record)?;
            self.apply(digest(&record), digest(&record[16..]));
        }
        Ok(())
    }

    fn apply(&mut self, path: Digest, state: Digest) {
        if state == NONE {
            self.held.remove(&path);
        } else {
            self.held.insert(path, state);
        }
    }

    /// The staged copy's id, when there is one.
    #[must_use]
    pub fn id(&self) -> Option<&str> {
        (!self.meta.stage.is_empty()).then_some(self.meta.stage.as_str())
    }

    /// Streams the new host acknowledged.
    #[must_use]
    pub fn acked(&self) -> u64 {
        self.meta.acked
    }

    /// Whether a whole pass was acknowledged: the next pass sends changes only.
    #[must_use]
    pub fn whole(&self) -> bool {
        self.meta.whole
    }

    /// When the last acknowledged stream began, seconds since the epoch.
    #[must_use]
    pub fn began(&self) -> Option<u64> {
        self.meta.began
    }

    /// Paths with a receipt.
    #[must_use]
    pub fn receipts(&self) -> usize {
        self.held.len()
    }

    fn save(&self) -> Result<(), String> {
        let json = serde_json::to_vec(&self.meta).map_err(|e| e.to_string())?;
        write_atomic(&self.dir.join("stage.json"), &json, 0o600).map_err(|e| {
            format!(
                "cannot write {}: {e}",
                self.dir.join("stage.json").display()
            )
        })
    }

    /// Forgets the staged copy: no receipts, no stage.
    pub fn forget(&mut self) {
        self.journal = None;
        self.held.clear();
        self.meta = Meta::default();
        let _ = std::fs::remove_dir_all(&self.dir);
    }

    /// Starts a new stage called `id`, with no receipts.
    ///
    /// # Errors
    /// The state directory cannot be written.
    pub fn renew(&mut self, id: &str) -> Result<(), String> {
        self.forget();
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)
            .map_err(|e| format!("cannot make {}: {e}", self.dir.display()))?;
        self.meta = Meta {
            version: 1,
            stage: id.to_owned(),
            log: "receipts-0".into(),
            ..Meta::default()
        };
        self.save()
    }

    /// Whether what this host knows of the staged copy is not to be believed at `at`: receipts
    /// and a journal written before a restart, or before the clock was set back.
    #[must_use]
    pub fn outdated(&self, at: &Era) -> Option<Outdated> {
        if self.held.is_empty() && self.meta.pending.is_none() {
            return None;
        }
        if self.meta.boot != at.boot {
            return Some(Outdated::Restarted);
        }
        match (self.meta.ahead, at.ahead) {
            (Some(was), Some(now)) if now < was - SET_BACK_MS => Some(Outdated::ClockBack),
            _ => None,
        }
    }

    /// Begins the journal of a stream that builds on what is acknowledged, at `at`
    /// ([`Era::here`]).
    ///
    /// # Errors
    /// There is no stage, the receipts are not to be believed at `at` ([`Stage::outdated`]; they
    /// are dropped, and what was planned on them must not be sent), or the state directory
    /// cannot be written.
    pub fn begin(&mut self, nonce: &str, live: bool, began: u64, at: &Era) -> Result<(), String> {
        if self.meta.stage.is_empty() {
            return Err("there is no staged copy to send a pass into".into());
        }
        if let Some(why) = self.outdated(at) {
            self.unknown(None)?;
            return Err(format!(
                "{}: what this host knew of the staged copy was dropped",
                match why {
                    Outdated::Restarted => "this host was restarted",
                    Outdated::ClockBack => "this host's clock was set back",
                }
            ));
        }
        self.meta.boot.clone_from(&at.boot);
        // Without receipts there is nothing an earlier clock could speak for.
        self.meta.ahead = match (self.meta.ahead, at.ahead) {
            (Some(was), Some(now)) if !self.held.is_empty() => Some(was.max(now)),
            (was, None) if !self.held.is_empty() => was,
            (_, now) => now,
        };
        let io = |e: io::Error| format!("cannot write in {}: {e}", self.dir.display());
        let file = private(&self.dir.join("pending"), true).map_err(io)?;
        let kept = if live {
            let _ = std::fs::remove_file(self.dir.join("pending.kept"));
            None
        } else {
            let kept = private(&self.dir.join("pending.kept"), true).map_err(io)?;
            Some(BufWriter::with_capacity(1 << 20, kept))
        };
        self.meta.pending = Some(Pending {
            seq: self.meta.acked,
            nonce: nonce.to_owned(),
            live,
            began,
        });
        self.save()?;
        self.journal = Some(Journal {
            file,
            count: 0,
            kept,
        });
        Ok(())
    }

    /// How many entries the pending stream's journal holds, and the stream's digest before
    /// its entry number `at`.
    fn journal(&self) -> (u64, impl Fn(u64) -> Option<String> + use<>) {
        let file = File::open(self.dir.join("pending")).ok();
        let count = file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map_or(0, |m| m.len() / SENT as u64);
        let sum = move |at: u64| {
            let mut check = [0u8; 8];
            (at < count)
                .then(|| {
                    file.as_ref()?
                        .read_exact_at(&mut check, at * SENT as u64)
                        .ok()
                })
                .flatten()
                .map(|()| hex::encode(check))
        };
        (count, sum)
    }

    /// Makes the receipts agree with the new host's marker `mark` (SPEC-465 WCP-006, WCP-011).
    /// Equal stage and count: the new host took nothing of a pending stream in (it marks a
    /// stream before its first entry), and the paths that stream sent are revoked all the same.
    /// One count ahead with the pending stream's name: the receipts of exactly the entries the
    /// marker counts are adopted, when the stream's digest there is the recorded one; the rest
    /// of what was sent is revoked. A stream the new host never closed and one that removed
    /// paths and broke leave nothing known, and so does everything this host wrote before it
    /// was restarted or before its clock was set back ([`Stage::outdated`] at `at`): the stage
    /// goes on without receipts. Anything else: no staged copy is agreed on, and the receipts
    /// are dropped with the stage.
    ///
    /// # Errors
    /// The state directory cannot be written; nothing may be skipped on these receipts then.
    pub fn reconcile(&mut self, mark: Option<&Mark>, at: &Era) -> Result<Reconciled, String> {
        self.journal = None;
        let fresh = Reconciled {
            fresh: true,
            ..Reconciled::default()
        };
        let Some(mark) = mark.filter(|m| !self.meta.stage.is_empty() && m.stage == self.meta.stage)
        else {
            self.forget();
            return Ok(fresh);
        };
        let pending = self.meta.pending.clone();
        let outdated = self.outdated(at);
        let (count, sum) = self.journal();
        // The entries of the pending stream whose receipts count, and those among them that
        // were not installed.
        let (take, deferred) = if mark.acked == self.meta.acked {
            if outdated.is_some() {
                let mut out = self.unknown(None)?;
                out.outdated = outdated;
                return Ok(out);
            }
            (0, Vec::new())
        } else if let (Some(p), Some(last)) = (&pending, &mark.last)
            && mark.acked == self.meta.acked + 1
            && p.seq == self.meta.acked
            && p.nonce == last.nonce
        {
            if last.unknown || outdated.is_some() {
                // The receive ended without closing its stream (a kill, a power cut), or it
                // removed paths this host never sent and broke; or this host was restarted
                // since (what its journal says was sent may not be all that was, and no
                // receipt speaks for a file after a crash), or its clock was set back.
                let mut out = self.unknown(Some(p))?;
                out.outdated = outdated;
                return Ok(out);
            }
            let take = if last.complete {
                // Its checksum matched there, so every record here is of what arrived.
                if last.entries == count { count } else { 0 }
            } else if sum(last.entries).as_ref() == Some(&last.sums[0]) {
                last.entries
            } else if last.entries > 0 && sum(last.entries - 1).as_ref() == Some(&last.sums[1]) {
                last.entries - 1
            } else {
                0
            };
            if last.complete && !p.live {
                // After a frozen pass's deletions the new host holds what was left alone,
                // nothing else; a pass that is not the journal's leaves nothing known.
                return if take == count {
                    self.replace(p)
                } else {
                    self.unknown(Some(p))
                };
            }
            self.meta.whole |= last.complete && take == count;
            (take, last.deferred.clone())
        } else {
            self.forget();
            return Ok(fresh);
        };
        let out = self.adopt(take, &deferred)?;
        if mark.acked > self.meta.acked {
            self.meta.acked = mark.acked;
            self.meta.began = pending.as_ref().map(|p| p.began);
        }
        self.settle()?;
        Ok(out)
    }

    /// Appends the pending stream's paths to the log, durably, and applies them: the first
    /// `take` with the state they earned unless `deferred`, the others revoked.
    fn adopt(&mut self, take: u64, deferred: &[u64]) -> Result<Reconciled, String> {
        let mut out = Reconciled::default();
        let Ok(journal) = File::open(self.dir.join("pending")) else {
            return Ok(out);
        };
        let deferred: std::collections::HashSet<u64> = deferred.iter().copied().collect();
        let path = self.dir.join(&self.meta.log);
        let io = |e: io::Error| format!("cannot write {}: {e}", path.display());
        let log = private(&path, false).map_err(io)?;
        // What a run that died left beyond the valid length is overwritten.
        log.set_len(self.meta.log_len).map_err(io)?;
        let mut log = BufWriter::with_capacity(1 << 20, log);
        io::Seek::seek(&mut log, io::SeekFrom::Start(self.meta.log_len)).map_err(io)?;
        let mut journal = io::BufReader::with_capacity(1 << 20, journal);
        let mut record = [0u8; SENT];
        let mut at = 0u64;
        while journal.read_exact(&mut record).is_ok() {
            let path = digest(&record[8..]);
            let state = if at < take && !deferred.contains(&at) {
                digest(&record[24..])
            } else {
                NONE
            };
            if state == NONE {
                out.revoked += u64::from(self.held.contains_key(&path));
            } else {
                out.adopted += 1;
            }
            log.write_all(&path).map_err(io)?;
            log.write_all(&state).map_err(io)?;
            self.apply(path, state);
            self.meta.log_len += HELD as u64;
            at += 1;
        }
        let log = log.into_inner().map_err(|e| io(e.into_error()))?;
        log.sync_all().map_err(io)?;
        Ok(out)
    }

    /// The stage goes on with no receipts: the next pass sends everything again. `pending` is
    /// the stream the new host counted, when it counted one.
    fn unknown(&mut self, pending: Option<&Pending>) -> Result<Reconciled, String> {
        let revoked = self.held.len() as u64;
        self.held.clear();
        self.meta.log_len = 0;
        self.meta.whole = false;
        self.meta.ahead = None;
        if let Some(pending) = pending {
            self.meta.acked += 1;
            self.meta.began = Some(pending.began);
        }
        self.settle()?;
        // The log's valid length is 0 now: what it held is only freed.
        if let Ok(log) = File::options()
            .write(true)
            .open(self.dir.join(&self.meta.log))
        {
            let _ = log.set_len(0);
        }
        Ok(Reconciled {
            revoked,
            ..Reconciled::default()
        })
    }

    /// The receipts become what a completed frozen pass left alone (`pending.kept`).
    fn replace(&mut self, pending: &Pending) -> Result<Reconciled, String> {
        let log = format!("receipts-{}", self.meta.acked + 1);
        let path = self.dir.join(&log);
        let io = |e: io::Error| format!("cannot write {}: {e}", path.display());
        let before = self.held.len() as u64;
        self.held.clear();
        let mut len = 0u64;
        // The kept list is in the log's own form.
        if let Ok(kept) = File::open(self.dir.join("pending.kept")) {
            len = kept.metadata().map_err(io)?.len() / HELD as u64 * HELD as u64;
            let mut out = private(&path, true).map_err(io)?;
            io::copy(&mut (&kept).take(len), &mut out).map_err(io)?;
            out.sync_all().map_err(io)?;
        }
        let old = std::mem::replace(&mut self.meta.log, log);
        self.meta.log_len = len;
        self.load().map_err(io)?;
        self.meta.acked += 1;
        self.meta.whole = true;
        self.meta.began = Some(pending.began);
        self.settle()?;
        if old != self.meta.log {
            let _ = std::fs::remove_file(self.dir.join(old));
        }
        Ok(Reconciled {
            revoked: before.saturating_sub(self.held.len() as u64),
            ..Reconciled::default()
        })
    }

    /// The pending stream is accounted for.
    fn settle(&mut self) -> Result<(), String> {
        self.meta.pending = None;
        self.save()?;
        for name in ["pending", "pending.kept"] {
            let _ = std::fs::remove_file(self.dir.join(name));
        }
        Ok(())
    }
}

impl Ledger for Stage {
    fn held(&self, path: &Digest, state: &Digest) -> bool {
        self.held.get(path) == Some(state)
    }

    fn sending(&mut self, path: Digest, check: [u8; 8]) -> io::Result<()> {
        let journal = self
            .journal
            .as_mut()
            .ok_or_else(|| io::Error::other("no stream was begun"))?;
        let mut record = [0u8; SENT];
        record[..8].copy_from_slice(&check);
        record[8..24].copy_from_slice(&path);
        // Unbuffered: the record is in the file before the entry can reach the new host.
        journal
            .file
            .write_all_at(&record, journal.count * SENT as u64)?;
        journal.count += 1;
        Ok(())
    }

    fn earned(&mut self, state: Digest) -> io::Result<()> {
        let journal = self
            .journal
            .as_mut()
            .filter(|journal| journal.count > 0)
            .ok_or_else(|| io::Error::other("no entry was sent"))?;
        journal
            .file
            .write_all_at(&state, (journal.count - 1) * SENT as u64 + 24)
    }

    fn kept(&mut self, path: Digest, state: Digest) -> io::Result<()> {
        let Some(kept) = self.journal.as_mut().and_then(|j| j.kept.as_mut()) else {
            return Ok(());
        };
        kept.write_all(&path)?;
        kept.write_all(&state)
    }

    fn seal(&mut self) -> io::Result<()> {
        let Some(journal) = &mut self.journal else {
            return Ok(());
        };
        journal.file.sync_all()?;
        if let Some(kept) = &mut journal.kept {
            kept.flush()?;
            kept.get_ref().sync_all()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
