//! `yulius instance import` (SPEC-329 IMP-001..006): read the manifest, refuse what cannot work
//! before writing anything, make the service user and its ids, extract as the stream arrives,
//! check the checksum and length, and say what comes next.

use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use super::archive::{Item, Reader};
use super::extract::{Extractor, IdMap, Report};
use super::{FORMAT, MANIFEST, MANIFEST_LEN, Manifest, Progress, REFUSED};

/// The marker of an import that has not finished, at the data directory's top.
pub const INCOMPLETE: &str = ".yulius-import-incomplete";

/// Options of `yulius instance import`.
#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// The archive; standard input when `None` or `-`.
    pub file: Option<PathBuf>,
    /// The data directory, chosen here (never by the copy) and refused unless it is the one the
    /// copy was exported from.
    pub data_dir: PathBuf,
    /// The service user when the data directory does not exist yet (default: the exported one).
    pub user: Option<String>,
}

/// Why an import did not happen or did not finish.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// Refused before anything was written.
    Refused(String),
    /// Failed after writing began: the data directory holds an incomplete copy.
    Failed(String),
}

/// Free space, and free inodes when the filesystem counts them (btrfs reports none), of the
/// filesystem that will hold `dir` (its nearest existing ancestor).
fn free(dir: &Path) -> Option<(u64, Option<u64>)> {
    let mut at = Some(dir);
    while let Some(path) = at {
        if let Ok(st) = rustix::fs::statvfs(path) {
            let inodes = (st.f_files > 0).then_some(st.f_favail);
            return Some((st.f_bavail.saturating_mul(st.f_frsize), inodes));
        }
        at = path.parent();
    }
    None
}

/// Whether `name` at the top of an existing target may stay: the filesystem's `lost+found` and
/// the dotfiles `useradd --create-home` copies from `/etc/skel`.
fn may_be_there(name: &std::ffi::OsStr, meta: &std::fs::Metadata) -> bool {
    let name = name.to_string_lossy();
    (name == "lost+found" && meta.is_dir()) || (name.starts_with('.') && meta.is_file())
}

/// What an existing target must be (IMP-001): a directory, not a link, holding nothing but
/// [`may_be_there`], owned by `user` (or by root when empty, which the person fixes first).
fn existing_target(target: &Path, user: &str) -> Result<(), String> {
    let Ok(meta) = std::fs::symlink_metadata(target) else {
        return Ok(());
    };
    let shown = target.display();
    if !meta.is_dir() {
        return Err(format!("{shown} exists and is not a directory"));
    }
    let entries = std::fs::read_dir(target).map_err(|e| format!("{shown}: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{shown}: {e}"))?;
        let name = entry.file_name();
        let kept = entry.metadata().is_ok_and(|m| may_be_there(&name, &m));
        if kept {
            continue;
        }
        if name == "yulius.db" || name == "sandboxes" {
            return Err(format!(
                "{shown} already holds an instance ({}); an import never overwrites one",
                name.to_string_lossy()
            ));
        }
        return Err(format!(
            "{shown} holds {:?}: an import goes only into a new or empty data directory",
            name.to_string_lossy()
        ));
    }
    use std::os::unix::fs::MetadataExt as _;
    let passwd = super::read_text(Path::new("/etc/passwd")).unwrap_or_default();
    let owner = super::user_by_uid(&passwd, meta.uid())
        .map_or_else(|| meta.uid().to_string(), |(name, _)| name);
    if meta.uid() == 0 {
        return Err(format!(
            "{shown} belongs to root (a new disk?); give it to the service user first: `sudo \
             useradd --system --home-dir {shown} --shell /usr/sbin/nologin {user}; sudo chown \
             {user}: {shown}; sudo chmod 0700 {shown}`"
        ));
    }
    if owner != user {
        return Err(format!(
            "{shown} belongs to {owner}, not to the copy's service user {user} (--user names \
             another)"
        ));
    }
    Ok(())
}

/// The checks before anything is written (IMP-001): the format, the schema, the target the
/// person chose (never the manifest's alone), what is already there, and room for the copy.
///
/// # Errors
/// Each refusal, saying why and what to do.
pub fn check(m: &Manifest, target: &Path, user: &str, own_schema: u32) -> Result<(), String> {
    if m.format != FORMAT {
        return Err(format!(
            "the copy is in format {}, this Yulius reads format {FORMAT}",
            m.format
        ));
    }
    if m.schema > own_schema {
        return Err(format!(
            "the copy's database is newer than this Yulius knows (schema {} > {own_schema}): \
             install a Yulius at least as new as the exported one (build {})",
            m.schema,
            m.yulius_build.as_deref().unwrap_or("unknown")
        ));
    }
    let plain = target.is_absolute()
        && target
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
        && target.parent().is_some();
    if !plain {
        return Err(format!(
            "{} is not a plain absolute path below /",
            target.display()
        ));
    }
    if Path::new(&m.data_dir) != target {
        return Err(format!(
            "the copy was exported from {}; import it into the same path (`--data-dir {}`): Git \
             worktrees, the database and sandbox files hold absolute paths under it",
            m.data_dir, m.data_dir
        ));
    }
    if std::fs::symlink_metadata(target.join(INCOMPLETE)).is_ok() {
        return Err(format!(
            "an earlier import into {} did not finish: remove that directory (`sudo rm -rf {}`) \
             and import again",
            target.display(),
            target.display()
        ));
    }
    existing_target(target, user)?;
    if let Some((bytes, inodes)) = free(target) {
        if bytes < m.total_bytes {
            return Err(format!(
                "the copy needs {}, {} is free where {} goes",
                yulius_import::human_bytes(m.total_bytes),
                yulius_import::human_bytes(bytes),
                target.display()
            ));
        }
        if let Some(inodes) = inodes
            && inodes < m.entries
        {
            return Err(format!(
                "the copy has {} files and directories, the filesystem of {} has {inodes} inodes \
                 free",
                m.entries,
                target.display()
            ));
        }
    }
    Ok(())
}

/// Reads the manifest, the archive's first entry.
fn manifest<R: Read>(r: &mut Reader<R>) -> Result<Manifest, String> {
    let first = r.next_item().map_err(|e| e.to_string())?;
    let Item::Entry(e) = first else {
        return Err("not a Yulius export: it has no manifest".into());
    };
    if e.path != MANIFEST || e.size() != MANIFEST_LEN {
        return Err("not a Yulius export: it does not start with its manifest".into());
    }
    let mut bytes = Vec::new();
    r.data_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| format!("not a Yulius export: its manifest: {e}"))
}

/// Installs the archive read from `input`: manifest checks, `prepare` (the host's user and ids,
/// then the id map), the incomplete marker, every entry, the checksum and length, the marker
/// removed. `tick` gets the bytes read.
///
/// # Errors
/// [`Failure::Refused`] before anything was written, else [`Failure::Failed`].
pub fn install(
    input: impl Read,
    target: &Path,
    user: Option<&str>,
    prepare: &mut dyn FnMut(&Manifest, &Path) -> Result<IdMap, String>,
    chown: bool,
    tick: &mut dyn FnMut(&Manifest, u64),
) -> Result<(Manifest, Report), Failure> {
    let mut r = Reader::new(input);
    let m = manifest(&mut r).map_err(Failure::Refused)?;
    let user = user.unwrap_or(&m.service.user);
    check(&m, target, user, yulius_store::schema_version()).map_err(Failure::Refused)?;
    let map = prepare(&m, target).map_err(Failure::Refused)?;
    let failed = |e: io::Error| {
        Failure::Failed(format!(
            "{e}; {} holds an incomplete copy: remove it (`sudo rm -rf {}`) before importing \
             again",
            target.display(),
            target.display()
        ))
    };
    std::fs::create_dir_all(target).map_err(|e| Failure::Refused(e.to_string()))?;
    // Created new: a link planted at the marker's name is never followed.
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target.join(INCOMPLETE))
        .map_err(|e| Failure::Refused(format!("cannot mark the import as unfinished: {e}")))?;
    let mut ex = Extractor::new(target, map, chown).map_err(failed)?;
    loop {
        match r.next_item().map_err(failed)? {
            Item::End => break,
            Item::Entry(e) => {
                ex.put(&e, &mut r.data_reader()).map_err(failed)?;
                tick(&m, r.read_bytes());
            }
        }
    }
    let report = ex.finish().map_err(failed)?;
    if r.read_bytes() != m.total_bytes {
        return Err(failed(io::Error::other(format!(
            "the copy is {} bytes long, its manifest says {}",
            r.read_bytes(),
            m.total_bytes
        ))));
    }
    std::fs::remove_file(target.join(INCOMPLETE)).map_err(failed)?;
    Ok((m, report))
}

/// Makes the service user and its ids with `host setup`'s items and maps the copy's ids to them.
fn prepare_host(m: &Manifest, target: &Path, user: Option<&str>) -> Result<IdMap, String> {
    let opts = crate::host_setup::Options {
        data_dir: target.to_path_buf(),
        user: user.unwrap_or(&m.service.user).to_owned(),
        public_url: None,
        exe: std::env::current_exe().unwrap_or_default(),
        smoke: false,
        service: m.service.unit.clone(),
        update_state: None,
    };
    let mut host = crate::host_setup::system::RealHost { quiet: false };
    let mut print = |line: &yulius_contracts::update::HostLine| {
        eprintln!("{}", crate::host_setup::format_line(line));
    };
    let lines = crate::host_setup::Run::new(
        &mut host,
        crate::host_setup::Mode::Setup,
        opts.clone(),
        &mut print,
    )
    .user_and_ids();
    if !crate::host_setup::passed(&lines) {
        return Err("the service user or its subordinate ids could not be made (see above)".into());
    }
    let passwd = super::read_text(Path::new("/etc/passwd")).unwrap_or_default();
    let (uid, gid) = super::user_by_name(&passwd, &opts.user)
        .ok_or_else(|| format!("no user {} after the setup", opts.user))?;
    let range = |file: &str, old: Option<super::IdRange>| -> Result<_, String> {
        let Some(old) = old else { return Ok(None) };
        let text = super::read_text(Path::new(file)).unwrap_or_default();
        let (start, count) = yulius_sandbox::subid::find_range(&text, &opts.user, uid)
            .ok_or_else(|| format!("{} has no range in {file}", opts.user))?;
        if count < old.count {
            return Err(format!(
                "{}'s range in {file} has {count} ids, the copy's had {}",
                opts.user, old.count
            ));
        }
        Ok(Some((old.start, start, old.count)))
    };
    Ok(IdMap {
        uid: (m.service.uid, uid),
        gid: (m.service.gid, gid),
        subuid: range("/etc/subuid", m.subuid)?,
        subgid: range("/etc/subgid", m.subgid)?,
    })
}

/// `yulius instance import`.
#[must_use]
pub fn main(opts: &ImportOptions) -> ExitCode {
    let refused = |why: &str| {
        eprintln!("yulius instance import: {why}");
        ExitCode::from(REFUSED)
    };
    if !super::is_root() {
        return refused(
            "run it as root (sudo): it makes the service user and owns files by its ids",
        );
    }
    let input: Box<dyn Read> = match opts.file.as_deref() {
        None => Box::new(io::stdin().lock()),
        Some(p) if p == Path::new("-") => Box::new(io::stdin().lock()),
        Some(p) => match File::open(p) {
            Ok(f) => Box::new(f),
            Err(e) => return refused(&format!("{}: {e}", p.display())),
        },
    };
    let input = BufReader::with_capacity(1 << 20, input);
    let mut progress: Option<Progress> = None;
    let mut prepare = |m: &Manifest, target: &Path| {
        eprintln!(
            "Importing {} ({} entries) into {}, exported from {} by build {}.",
            yulius_import::human_bytes(m.total_bytes),
            m.entries,
            target.display(),
            m.os_release.as_deref().unwrap_or("an unknown host"),
            m.yulius_build.as_deref().unwrap_or("unknown")
        );
        let here =
            super::read_text(Path::new("/etc/os-release")).and_then(|t| super::os_release(&t));
        if m.os_release.is_some() && here != m.os_release {
            eprintln!(
                "Warning: this host runs {}, the copy comes from {}: packages installed in \
                 sandboxes sit on this host's system and may need reinstalling.",
                here.as_deref().unwrap_or("an unknown system"),
                m.os_release.as_deref().unwrap_or_default()
            );
        }
        prepare_host(m, target, opts.user.as_deref())
    };
    let result = install(
        input,
        &opts.data_dir,
        opts.user.as_deref(),
        &mut prepare,
        true,
        &mut |m, done| {
            progress
                .get_or_insert_with(|| Progress::new("Imported", m.total_bytes))
                .tick(done);
        },
    );
    if let Some(p) = &progress {
        p.end();
    }
    match result {
        Ok((m, report)) => {
            let secs = progress.as_ref().map_or(0, |p| p.elapsed().as_secs());
            eprintln!(
                "Imported {} ({} entries) into {} in {secs} s.",
                yulius_import::human_bytes(m.total_bytes),
                report.entries,
                m.data_dir
            );
            if report.foreign_owners > 0 {
                eprintln!(
                    "{} entries belonged to neither the service user nor its ids and now belong \
                     to the service user, without setuid or setgid.",
                    report.foreign_owners
                );
            }
            if report.skipped_xattrs > 0 {
                eprintln!(
                    "{} extended attributes were not restored (file capabilities, ACLs, others \
                     than user.* and trusted.overlay.*).",
                    report.skipped_xattrs
                );
            }
            let url = m
                .public_url
                .as_deref()
                .unwrap_or("https://yulius.example.com");
            eprintln!(
                "Next:\n  1. sudo yulius host setup --public-url {url}\n     (writes {unit}, \
                 builds the sandbox base for this host, checks Docker, starts Yulius)\n  2. Copy \
                 other Environment= lines you changed in the old {unit} (keys, limits), then \
                 `sudo systemctl restart {unit}`.\n  3. Point the reverse proxy and DNS here.\n  \
                 4. Keep the old instance frozen or stopped (`yulius instance export --move` \
                 does that): both would run the same agents on the same repositories.",
                unit = m.service.unit
            );
            ExitCode::SUCCESS
        }
        Err(Failure::Refused(why)) => refused(&why),
        Err(Failure::Failed(why)) => {
            eprintln!("yulius instance import: {why}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests;
