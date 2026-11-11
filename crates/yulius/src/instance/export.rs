//! `yulius instance export` (SPEC-329 EXP-001..007): freeze, plan the exact length, announce it,
//! stream the archive to stdout, thaw (or keep frozen for a move).

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufWriter, IsTerminal as _, Write};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

use sha2::{Digest as _, Sha256};

use super::archive::{
    Entry, Kind, Writer, check_entry, ending_len, headers, padded, path_digest, plain_file_len,
};
use super::freeze::{self, Freeze};
use super::receipts::{self, Digest, Ledger};
use super::walk::{self, Seen, Skipped};
use super::{IdRange, KEPT, MANIFEST, MANIFEST_LEN, Manifest, Progress, REFUSED, Service};

/// Options of `yulius instance export`.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// The data directory.
    pub data_dir: PathBuf,
    /// The server's unit.
    pub service: String,
    /// The update helper's unit.
    pub update_service: String,
    /// Keep the instance frozen and disable its unit after a successful export.
    pub move_out: bool,
}

/// The planned archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// Entries under `data` in the stream.
    pub entries: u64,
    /// The archive's exact length.
    pub total: u64,
    /// What is not archived.
    pub skipped: Skipped,
    /// SHA-256 of every entry's headers (path, kind, size, owners, mode, time, attributes): the
    /// stream must find the same, or the directory changed after the plan.
    pub fingerprint: [u8; 32],
    /// Every path the directory holds, sent or not (a pass of a move; 0 for a whole copy).
    pub kept: u64,
}

/// Walks `root` once and computes the archive's exact length.
///
/// # Errors
/// A directory or file that cannot be read, or an entry the reader would refuse.
pub fn plan(root: &Path) -> io::Result<Plan> {
    plan_observed(root, &mut |_| {})
}

/// [`plan`], showing every planned entry to `observe` (a move counts its inventory from them,
/// SPEC-460 MIG-031, without another walk).
///
/// # Errors
/// As [`plan`].
pub fn plan_observed(root: &Path, observe: &mut dyn FnMut(&Entry)) -> io::Result<Plan> {
    plan_pass(root, None, observe)
}

/// Which links of a file must travel with it: when a file with several links is sent, the new
/// host makes a new file of it, and its other links there would keep the old one.
#[derive(Default)]
struct Relinked(HashSet<(u64, u64)>);

impl Relinked {
    /// Whether the new host holds `seen` as it is, by `ledger`: its path and state.
    fn held(
        &self,
        ledger: &dyn Ledger,
        seen: &Seen<'_>,
        headers: &[u8],
    ) -> Option<(Digest, Digest)> {
        let path = path_digest(&seen.entry.path);
        let state = receipts::state(headers, seen.ino, seen.ctime);
        let relinked = matches!(seen.entry.kind, Kind::HardLink { .. })
            && self.0.contains(&(seen.dev, seen.ino));
        (!relinked && ledger.held(&path, &state)).then_some((path, state))
    }

    /// `seen` is sent as a file.
    fn sent(&mut self, seen: &Seen<'_>) {
        if seen.nlink > 1 && matches!(seen.entry.kind, Kind::File { .. }) {
            self.0.insert((seen.dev, seen.ino));
        }
    }
}

/// The entry that lists the paths an exact pass keeps.
fn kept_entry(paths: u64) -> Entry {
    Entry {
        path: KEPT.to_vec(),
        kind: Kind::File { size: paths * 16 },
        mode: 0o600,
        uid: 0,
        gid: 0,
        mtime: 0,
        xattrs: Vec::new(),
    }
}

/// [`plan_observed`] for an exact pass of a move (SPEC-465 WCP-007): with `ledger`, only the
/// entries the new host does not hold are planned into the stream, followed by the digests of
/// every path. `observe` still sees every entry.
///
/// # Errors
/// As [`plan`].
pub fn plan_pass(
    root: &Path,
    ledger: Option<&dyn Ledger>,
    observe: &mut dyn FnMut(&Entry),
) -> io::Result<Plan> {
    let mut entries = 0u64;
    let mut kept = 0u64;
    let mut bytes = 0u64;
    let mut fingerprint = Sha256::new();
    let mut relinked = Relinked::default();
    let skipped = walk::walk_seen(root, false, &walk::copied, &mut |seen| {
        let e = seen.entry;
        check_entry(e)?;
        observe(e);
        let h = headers(e);
        fingerprint.update(&h);
        if let Some(ledger) = ledger {
            kept += 1;
            let held = relinked.held(ledger, seen, &h).is_some();
            fingerprint.update([u8::from(held)]);
            if held {
                return Ok(true);
            }
            relinked.sent(seen);
        }
        entries += 1;
        bytes += h.len() as u64 + padded(e.size());
        Ok(true)
    })?;
    let list = ledger.map_or(0, |_| kept_entry(kept).archived_len());
    Ok(Plan {
        entries,
        total: plain_file_len(MANIFEST_LEN) + bytes + list + ending_len(),
        skipped,
        fingerprint: fingerprint.finalize().into(),
        kept: ledger.map_or(0, |_| kept),
    })
}

fn open(disk: &Path) -> io::Result<File> {
    // Never through a link, and never waiting on what took a file's place (a FIFO).
    File::options()
        .read(true)
        .custom_flags((nix::fcntl::OFlag::O_NOFOLLOW | nix::fcntl::OFlag::O_NONBLOCK).bits())
        .open(disk)
}

/// Streams the archive of `root` to `out`: `manifest` (with `plan`'s totals) first, then the
/// entries, then the checksum; `tick` gets the bytes written so far, and `before_end` may
/// still refuse the copy before its checksum is written (the reader then sees an incomplete
/// copy).
///
/// # Errors
/// A read or write error, the directory no longer matching `plan`, or `before_end`'s error.
pub fn send(
    root: &Path,
    plan: &Plan,
    manifest: &Manifest,
    out: impl Write,
    tick: &mut dyn FnMut(u64),
    before_end: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<u64> {
    send_pass(root, plan, manifest, out, None, tick, &mut |_| before_end())
}

fn head(manifest: &Manifest, w: &mut Writer<impl Write>) -> io::Result<()> {
    let content = manifest.to_bytes().map_err(io::Error::other)?;
    let head = Entry {
        path: MANIFEST.to_vec(),
        kind: Kind::File { size: MANIFEST_LEN },
        mode: 0o600,
        uid: 0,
        gid: 0,
        mtime: i64::try_from(manifest.created_at).unwrap_or(0),
        xattrs: Vec::new(),
    };
    w.entry(&head, Some(&mut content.as_slice()), |_| {})
}

/// [`send`] for an exact pass of a move planned with [`plan_pass`] (SPEC-465 WCP-007): with
/// `ledger`, the entries the new host holds are left out and recorded as kept, each sent entry
/// is recorded before it leaves, and the stream ends with the digests of every path.
/// `before_end` gets the ledger back, to make its records durable before the checksum.
///
/// # Errors
/// As [`send`].
pub fn send_pass(
    root: &Path,
    plan: &Plan,
    manifest: &Manifest,
    out: impl Write,
    mut ledger: Option<&mut dyn Ledger>,
    tick: &mut dyn FnMut(u64),
    before_end: &mut dyn FnMut(Option<&mut dyn Ledger>) -> io::Result<()>,
) -> io::Result<u64> {
    let mut manifest = manifest.clone();
    manifest.total_bytes = plan.total;
    manifest.entries = plan.entries;
    if let Some(pass) = &mut manifest.pass {
        pass.kept = plan.kept;
    }
    let mut w = Writer::new(out);
    head(&manifest, &mut w)?;
    let list = ledger
        .as_ref()
        .map_or(0, |_| kept_entry(plan.kept).archived_len());
    let limit = plan.total - ending_len() - list;
    let mut fingerprint = Sha256::new();
    let mut relinked = Relinked::default();
    let mut paths: Vec<u8> = Vec::new();
    walk::walk_seen(root, false, &walk::copied, &mut |seen| {
        let (e, disk) = (seen.entry, seen.disk);
        let h = headers(e);
        fingerprint.update(&h);
        if let Some(ledger) = ledger.as_deref_mut() {
            paths.extend_from_slice(&path_digest(&e.path));
            let held = relinked.held(ledger, seen, &h);
            fingerprint.update([u8::from(held.is_some())]);
            if let Some((path, state)) = held {
                ledger.kept(path, state)?;
                return Ok(true);
            }
            relinked.sent(seen);
        }
        if w.written() + e.archived_len() > limit {
            return Err(drifted());
        }
        if let Some(ledger) = ledger.as_deref_mut() {
            ledger.sending(path_digest(&e.path), w.sum())?;
        }
        if let Kind::File { .. } = e.kind {
            let mut file = open(disk)?;
            w.entry(e, Some(&mut file), &mut *tick)?;
        } else {
            w.entry(e, None, &mut *tick)?;
        }
        Ok(true)
    })?;
    let same: [u8; 32] = fingerprint.finalize().into();
    if w.written() != limit || same != plan.fingerprint || paths.len() as u64 != plan.kept * 16 {
        // No trailer: the reader sees an incomplete copy.
        return Err(drifted());
    }
    if ledger.is_some() {
        w.entry(&kept_entry(plan.kept), Some(&mut paths.as_slice()), |_| {})?;
    }
    before_end(ledger)?;
    w.finish()
}

/// What a live pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Sent {
    /// Entries the walk found.
    pub walked: u64,
    /// Entries sent.
    pub sent: u64,
    /// The stream's length.
    pub bytes: u64,
    /// Paths that were gone when they were looked at or opened.
    pub vanished: u64,
    /// Files that ended early while they were read (sent padded, to be sent again).
    pub torn: u64,
    /// Sockets, FIFOs and devices left out.
    pub special: u64,
    /// The time spent sending entries; the rest of the pass was the walk.
    pub sending: Duration,
}

/// Whether an entry last changed at `ctime` is old enough at a pass that began at `began` to
/// earn a receipt (SPEC-465 WCP-003): any later change gets a later change time, also on a
/// filesystem that keeps whole seconds.
fn settled(ctime: (i64, i64), began: (i64, i64)) -> bool {
    (ctime.0 + RACY_SECS, ctime.1) <= began
}

/// How long before a pass an entry must have changed last to earn a receipt.
pub const RACY_SECS: i64 = 2;

fn same_inode(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    (
        a.dev(),
        a.ino(),
        a.ctime(),
        a.ctime_nsec(),
        a.len(),
        a.mtime(),
    ) == (
        b.dev(),
        b.ino(),
        b.ctime(),
        b.ctime_nsec(),
        b.len(),
        b.mtime(),
    )
}

/// Streams a warm pass of a move: `root` as it is while the instance runs (SPEC-465 WCP-002,
/// WCP-003). One walk: the entries `ledger` says the new host holds are left out; a path that is
/// gone is left out; a file's header is its open file's stat and exactly that many bytes
/// follow. An entry earns a receipt when its stat did not move while it was read, it lies on
/// `root`'s filesystem and it last changed [`RACY_SECS`] before `began`: the caller syncs that
/// filesystem and then reads the clock, before the call. `manifest` names the pass; its totals
/// stay estimates.
///
/// # Errors
/// A read or write error that is not a path being gone, a record `ledger` cannot keep, or
/// `before_end`'s error.
pub fn send_live(
    root: &Path,
    manifest: &Manifest,
    out: impl Write,
    ledger: &mut dyn Ledger,
    began: (i64, i64),
    tick: &mut dyn FnMut(u64),
    before_end: &mut dyn FnMut(&mut dyn Ledger) -> io::Result<()>,
) -> io::Result<Sent> {
    let mut w = Writer::new(out);
    head(manifest, &mut w)?;
    let mut sent = Sent::default();
    let mut relinked = Relinked::default();
    let mut root_dev = None;
    let skipped = walk::walk_seen(root, true, &walk::copied, &mut |seen| {
        let e = seen.entry;
        sent.walked += 1;
        let here = *root_dev.get_or_insert(seen.dev) == seen.dev;
        if relinked.held(ledger, seen, &headers(e)).is_some() {
            return Ok(true);
        }
        let started = Instant::now();
        let path = path_digest(&e.path);
        let earned = if let Kind::File { .. } = e.kind {
            let opened = open(seen.disk).and_then(|file| Ok((file.metadata()?, file)));
            let (meta, mut file) = match opened {
                Ok((meta, file)) if meta.is_file() => (meta, file),
                // Gone, or something else took its place since the walk looked.
                Ok(_) => {
                    sent.vanished += 1;
                    return Ok(false);
                }
                Err(err) if walk::gone(&err) => {
                    sent.vanished += 1;
                    return Ok(false);
                }
                Err(err) => return Err(err),
            };
            // The header is the open file's own: what is read is that file's content.
            let kind = Kind::File { size: meta.len() };
            let e = walk::described(e.path.clone(), kind, &meta, walk::xattrs_of(&file)?);
            check_entry(&e)?;
            ledger.sending(path, w.sum())?;
            let torn = w.entry_torn(&e, &mut file, &mut *tick)?;
            sent.torn += u64::from(torn);
            if meta.nlink() > 1 {
                relinked.0.insert((meta.dev(), meta.ino()));
            }
            let still = !torn
                && (meta.dev(), meta.ino()) == (seen.dev, seen.ino)
                && same_inode(&meta, &file.metadata()?);
            let ctime = (meta.ctime(), meta.ctime_nsec());
            (still && settled(ctime, began))
                .then(|| receipts::state(&headers(&e), meta.ino(), ctime))
        } else {
            check_entry(e)?;
            ledger.sending(path, w.sum())?;
            w.entry(e, None, &mut *tick)?;
            let still = std::fs::symlink_metadata(seen.disk).is_ok_and(|now| {
                (now.dev(), now.ino(), now.ctime(), now.ctime_nsec())
                    == (seen.dev, seen.ino, seen.ctime.0, seen.ctime.1)
            });
            (still && settled(seen.ctime, began))
                .then(|| receipts::state(&headers(e), seen.ino, seen.ctime))
        };
        if let Some(state) = earned.filter(|_| here) {
            ledger.earned(state)?;
        }
        sent.sent += 1;
        sent.sending += started.elapsed();
        Ok(true)
    })?;
    sent.vanished += skipped.vanished;
    sent.special = skipped.special;
    before_end(ledger)?;
    sent.bytes = w.finish()?;
    Ok(sent)
}

/// Syncs the filesystem that holds `root`: every dirty page is written and write-protected, so
/// a later write through a mapping stamps its file again (SPEC-465 WCP-003).
///
/// # Errors
/// The directory cannot be opened or the filesystem cannot be synced.
pub fn sync(root: &Path) -> io::Result<()> {
    let dir = File::open(root)?;
    #[cfg(target_os = "linux")]
    rustix::fs::syncfs(&dir)?;
    #[cfg(not(target_os = "linux"))]
    {
        dir.sync_all()?;
        rustix::fs::sync();
    }
    Ok(())
}

fn drifted() -> io::Error {
    io::Error::other(
        "the data directory changed during the export; nothing should write while the instance \
         is frozen",
    )
}

/// The facts of the instance at `data_dir` for its manifest.
pub(crate) fn manifest_of(data_dir: &Path, unit: &str) -> Result<Manifest, String> {
    let meta = std::fs::metadata(data_dir)
        .map_err(|e| format!("cannot read {}: {e}", data_dir.display()))?;
    let passwd = super::read_text(Path::new("/etc/passwd")).unwrap_or_default();
    let user = super::user_by_uid(&passwd, meta.uid())
        .map_or_else(|| meta.uid().to_string(), |(name, _)| name);
    let range = |file: &str| {
        let text = super::read_text(Path::new(file)).unwrap_or_default();
        yulius_sandbox::subid::find_range(&text, &user, meta.uid())
            .map(|(start, count)| IdRange { start, count })
    };
    let public_url = std::process::Command::new("systemctl")
        .args(["show", "--property=Environment", "--value", unit])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .find_map(|kv| kv.strip_prefix("YULIUS_PUBLIC_URL=").map(str::to_owned))
        });
    Ok(Manifest {
        format: super::FORMAT,
        total_bytes: 0,
        entries: 0,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        yulius_build: crate::serve::build_commit(),
        schema: yulius_store::schema_version(),
        data_dir: data_dir.display().to_string(),
        public_url,
        os_release: super::read_text(Path::new("/etc/os-release"))
            .and_then(|t| super::os_release(&t)),
        service: Service {
            unit: unit.to_owned(),
            user: user.clone(),
            uid: meta.uid(),
            gid: meta.gid(),
        },
        subuid: range("/etc/subuid"),
        subgid: range("/etc/subgid"),
        pass: None,
    })
}

fn refused(why: &str) -> ExitCode {
    eprintln!("yulius instance export: {why}");
    ExitCode::from(REFUSED)
}

/// `yulius instance export`.
#[must_use]
pub fn main(opts: &ExportOptions) -> ExitCode {
    if !super::is_root() {
        return refused(
            "run it as root (sudo): it reads every sandbox's files and freezes the service",
        );
    }
    if io::stdout().is_terminal() {
        return refused(
            "the archive goes to standard output: pipe it to a file or to ssh, for example \
             `sudo yulius instance export | ssh root@new-vm yulius instance import`",
        );
    }
    let data_dir = match std::fs::canonicalize(&opts.data_dir) {
        Ok(dir) => dir,
        Err(e) => return refused(&format!("{}: {e}", opts.data_dir.display())),
    };
    let manifest = match manifest_of(&data_dir, &opts.service) {
        Ok(m) => m,
        Err(e) => return refused(&e),
    };
    // Signals are answered on their own thread, so a thaw happens even while the export is
    // blocked writing to a stalled pipe (EXP-001).
    let state = Arc::new(AtomicU8::new(freeze::ARMING));
    let mut signals = match Signals::new([SIGINT, SIGTERM, SIGHUP]) {
        Ok(signals) => signals,
        Err(e) => return refused(&format!("cannot watch signals: {e}")),
    };
    let (watched, unit) = (Arc::clone(&state), opts.service.clone());
    std::thread::spawn(move || {
        if let Some(signal) = signals.forever().next() {
            eprintln!(
                "\nyulius instance export: stopped by signal {signal}; the copy is incomplete"
            );
            match freeze::on_signal(&watched, &mut freeze::System, &unit) {
                freeze::Signal::LeaveToMain => {}
                freeze::Signal::Exit(why) => {
                    if let Some(why) = why {
                        eprintln!("yulius instance export: {why}");
                    }
                    std::process::exit(1);
                }
            }
        }
    });
    let mut held = match freeze::freeze(
        Box::new(freeze::System),
        &opts.service,
        &opts.update_service,
    ) {
        Ok(held) => held,
        Err(why) => {
            state.store(freeze::REFUSED, Ordering::SeqCst);
            return refused(&why);
        }
    };
    let ours = if held.ours() {
        freeze::THAW
    } else {
        freeze::LEAVE
    };
    state.store(ours, Ordering::SeqCst);
    let result = run(&data_dir, &manifest, &mut held);
    // From here the export thaws (or keeps the freeze for a move) itself, unless a signal took
    // the end first: then that thread thaws and exits, and nothing here may report a move.
    if !freeze::claim_end(&state) {
        loop {
            std::thread::park();
        }
    }
    match (result, opts.move_out) {
        (Ok(()), true) => match held.keep_for_move() {
            Ok(()) => {
                eprintln!(
                    "{unit} stays frozen and is disabled, so it does not start beside the new \
                     instance. To run it here again: `sudo systemctl thaw {unit} && sudo \
                     systemctl enable {unit}`.",
                    unit = opts.service
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("yulius instance export: {e}");
                ExitCode::FAILURE
            }
        },
        (Ok(()), false) => match held.release() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("yulius instance export: {e}");
                ExitCode::FAILURE
            }
        },
        (Err(e), _) => {
            if let Err(thaw) = held.release() {
                eprintln!("yulius instance export: {thaw}");
            }
            eprintln!("yulius instance export: {e}; the copy is incomplete");
            ExitCode::FAILURE
        }
    }
}

fn run(data_dir: &Path, manifest: &Manifest, held: &mut Freeze) -> Result<(), String> {
    eprintln!("{}", held.describe());
    let plan = plan(data_dir).map_err(|e| format!("planning failed: {e}"))?;
    eprintln!(
        "Exporting {} ({} entries) of {}.",
        yulius_import::human_bytes(plan.total),
        plan.entries,
        data_dir.display()
    );
    eprintln!(
        "The copy holds master.key with the database it opens, sign-ins and homes: keep it like \
         the server itself (write a file with umask 077)."
    );
    if plan.skipped.special > 0 {
        eprintln!(
            "{} sockets, FIFOs or devices are left out (live processes' files).",
            plan.skipped.special
        );
    }
    let mut progress = Progress::new("Exported", plan.total);
    let out = BufWriter::with_capacity(1 << 20, io::stdout().lock());
    let sent = send(
        data_dir,
        &plan,
        manifest,
        out,
        &mut |done| progress.tick(done),
        &mut || held.helper_idle().map_err(io::Error::other),
    );
    progress.end();
    let sent = sent.map_err(|e| {
        if e.kind() == io::ErrorKind::BrokenPipe {
            "the reader went away (broken pipe)".to_owned()
        } else {
            e.to_string()
        }
    })?;
    eprintln!(
        "Exported {} ({} entries) in {} s.",
        yulius_import::human_bytes(sent),
        plan.entries,
        progress.elapsed().as_secs()
    );
    Ok(())
}

#[cfg(test)]
mod tests;
