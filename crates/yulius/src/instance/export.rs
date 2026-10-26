//! `yulius instance export` (SPEC-329 EXP-001..007): freeze, plan the exact length, announce it,
//! stream the archive to stdout, thaw (or keep frozen for a move).

use std::fs::File;
use std::io::{self, BufWriter, IsTerminal as _, Write};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

use sha2::{Digest as _, Sha256};

use super::archive::{
    Entry, Kind, Writer, check_entry, ending_len, headers, padded, plain_file_len,
};
use super::freeze::{self, Freeze};
use super::walk::{self, Skipped};
use super::{IdRange, MANIFEST, MANIFEST_LEN, Manifest, Progress, REFUSED, Service};

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
    /// Entries under `data`.
    pub entries: u64,
    /// The archive's exact length.
    pub total: u64,
    /// What is not archived.
    pub skipped: Skipped,
    /// SHA-256 of every entry's headers (path, kind, size, owners, mode, time, attributes): the
    /// stream must find the same, or the directory changed after the plan.
    pub fingerprint: [u8; 32],
}

/// Walks `root` once and computes the archive's exact length.
///
/// # Errors
/// A directory or file that cannot be read, or an entry the reader would refuse.
pub fn plan(root: &Path) -> io::Result<Plan> {
    let mut entries = 0u64;
    let mut bytes = 0u64;
    let mut fingerprint = Sha256::new();
    let skipped = walk::walk(root, &mut |e, _| {
        check_entry(e)?;
        let h = headers(e);
        fingerprint.update(&h);
        entries += 1;
        bytes += h.len() as u64 + padded(e.size());
        Ok(())
    })?;
    Ok(Plan {
        entries,
        total: plain_file_len(MANIFEST_LEN) + bytes + ending_len(),
        skipped,
        fingerprint: fingerprint.finalize().into(),
    })
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
    let mut manifest = manifest.clone();
    manifest.total_bytes = plan.total;
    manifest.entries = plan.entries;
    let content = manifest.to_bytes().map_err(io::Error::other)?;
    let mut w = Writer::new(out);
    let head = Entry {
        path: MANIFEST.to_vec(),
        kind: Kind::File { size: MANIFEST_LEN },
        mode: 0o600,
        uid: 0,
        gid: 0,
        mtime: i64::try_from(manifest.created_at).unwrap_or(0),
        xattrs: Vec::new(),
    };
    w.entry(&head, Some(&mut content.as_slice()), |_| {})?;
    let limit = plan.total - ending_len();
    let mut fingerprint = Sha256::new();
    walk::walk(root, &mut |e, disk| {
        fingerprint.update(headers(e));
        if w.written() + e.archived_len() > limit {
            return Err(drifted());
        }
        if let Kind::File { .. } = e.kind {
            let mut file = File::options()
                .read(true)
                .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
                .open(disk)?;
            w.entry(e, Some(&mut file), &mut *tick)
        } else {
            w.entry(e, None, &mut *tick)
        }
    })?;
    let same: [u8; 32] = fingerprint.finalize().into();
    if w.written() != limit || same != plan.fingerprint {
        // No trailer: the reader sees an incomplete copy.
        return Err(drifted());
    }
    before_end()?;
    w.finish()
}

fn drifted() -> io::Error {
    io::Error::other(
        "the data directory changed during the export; nothing should write while the instance \
         is frozen",
    )
}

/// The facts of the instance at `data_dir` for its manifest.
fn manifest_of(data_dir: &Path, unit: &str) -> Result<Manifest, String> {
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
            if let Some(why) = freeze::on_signal(&watched, &mut freeze::System, &unit) {
                eprintln!("yulius instance export: {why}");
            }
            std::process::exit(1);
        }
    });
    let mut held = match freeze::freeze(
        Box::new(freeze::System),
        &opts.service,
        &opts.update_service,
    ) {
        Ok(held) => held,
        Err(why) => {
            state.store(freeze::LEAVE, Ordering::SeqCst);
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
    // From here the export thaws (or keeps the freeze for a move) itself.
    state.store(freeze::LEAVE, Ordering::SeqCst);
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
