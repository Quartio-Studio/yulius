//! Uncommitted-state snapshot contents, exclusions and symlink handling (IMP-001).

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use yulius_import::{MAX_SNAPSHOT_FILE_BYTES, SkipReason, SnapshotError, snapshot_uncommitted};

use crate::support::{RepoFixture, TestResult, git, write};

#[derive(Debug, PartialEq, Eq)]
enum Archived {
    File { content: Vec<u8>, mode: u32 },
    Link(PathBuf),
}

fn read_archive(path: &Path) -> TestResult<BTreeMap<String, Archived>> {
    let mut archive = tar::Archive::new(GzDecoder::new(File::open(path)?));
    let mut entries = BTreeMap::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let name = entry.path()?.to_string_lossy().into_owned();
        let header = entry.header();
        assert_eq!(header.mtime()?, 0, "{name}");
        assert_eq!(header.uid()?, 0, "{name}");
        let archived = if header.entry_type().is_symlink() {
            Archived::Link(entry.link_name()?.ok_or("link without target")?.into_owned())
        } else {
            let mode = header.mode()?;
            let mut content = Vec::new();
            entry.read_to_end(&mut content)?;
            Archived::File { content, mode }
        };
        entries.insert(name, archived);
    }
    Ok(entries)
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(unix)]
#[test]
fn captures_changes_and_excludes_ignored_large_and_escaping_paths() -> TestResult {
    let fixture = RepoFixture::new()?;
    let repo = &fixture.repo;
    let outside = fixture.temp.path().join("outside");
    write(&outside.join("inner.txt"), "OUTSIDE SECRET\n")?;
    write(&outside.join("passwd"), "OUTSIDE SECRET\n")?;

    write(&repo.join(".gitignore"), "*.log\n")?;
    write(&repo.join("README.md"), "# demo, edited\n")?; // modified tracked
    fs::remove_file(repo.join("b.txt"))?; // deleted tracked
    write(&repo.join("staged.txt"), "staged only\n")?;
    git(repo, &["add", "staged.txt"])?; // staged new file
    write(&repo.join("notes/new file ü.txt"), "untracked\n")?; // untracked, unusual name
    write(&repo.join("debug.log"), "ignored\n")?; // ignored
    write(&repo.join("run.sh"), "#!/bin/sh\n")?;
    make_executable(&repo.join("run.sh"))?;
    File::create(repo.join("huge.bin"))?.set_len(MAX_SNAPSHOT_FILE_BYTES + 1)?; // sparse, too large
    symlink(Path::new("README.md"), &repo.join("link-inside"))?;
    symlink(&outside.join("passwd"), &repo.join("link-outside"))?;
    // Replace a tracked directory with a symlink to a directory outside the repository.
    fs::remove_dir_all(repo.join("dir"))?;
    symlink(&outside, &repo.join("dir"))?;

    let out = fixture.temp.path().join("snapshot.tar.gz");
    let snapshot = snapshot_uncommitted(repo, &out, 1024 * 1024 * 1024)?;
    let Some(file) = &snapshot.file else {
        return Err("expected an archive".into());
    };
    assert_eq!(file.name, "snapshot.tar.gz");
    assert_eq!(file.size, fs::metadata(&out)?.len());

    let entries = read_archive(&out)?;
    let names: Vec<&str> = entries.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "DELETED",
            "files/.gitignore",
            "files/README.md",
            "files/dir",
            "files/link-inside",
            "files/link-outside",
            "files/notes/new file ü.txt",
            "files/run.sh",
            "files/staged.txt",
        ]
    );
    assert_eq!(
        entries.get("files/README.md"),
        Some(&Archived::File {
            content: b"# demo, edited\n".to_vec(),
            mode: 0o644
        })
    );
    assert!(matches!(entries.get("files/run.sh"), Some(Archived::File { mode: 0o755, .. })));
    assert_eq!(entries.get("files/link-inside"), Some(&Archived::Link(PathBuf::from("README.md"))));
    // Links are recorded, never followed: the outside content is not archived.
    assert_eq!(entries.get("files/link-outside"), Some(&Archived::Link(outside.join("passwd"))));
    assert_eq!(entries.get("files/dir"), Some(&Archived::Link(outside.clone())));
    for archived in entries.values() {
        if let Archived::File { content, .. } = archived {
            assert!(!String::from_utf8_lossy(content).contains("OUTSIDE SECRET"));
        }
    }
    assert_eq!(
        entries.get("DELETED"),
        Some(&Archived::File {
            content: b"b.txt\0".to_vec(),
            mode: 0o644
        })
    );

    assert_eq!(snapshot.files, 5);
    assert_eq!(snapshot.links, 3);
    assert_eq!(snapshot.deleted, 1);
    let mut skipped: Vec<(String, SkipReason)> = snapshot
        .skipped
        .iter()
        .map(|skip| (skip.path.to_string_lossy().into_owned(), skip.reason.clone()))
        .collect();
    skipped.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        skipped,
        [
            ("dir/inner.txt".to_owned(), SkipReason::OutsideRepository),
            (
                "huge.bin".to_owned(),
                SkipReason::TooLarge {
                    size: MAX_SNAPSHOT_FILE_BYTES + 1
                }
            ),
        ]
    );

    // Identical state, identical archive digest (IMP-006).
    let again = fixture.temp.path().join("again.tar.gz");
    let second = snapshot_uncommitted(&repo.join("notes"), &again, 1024 * 1024 * 1024)?;
    assert_eq!(second.file.map(|file| file.sha256), Some(file.sha256.clone()));
    Ok(())
}

#[test]
fn clean_checkout_produces_no_archive() -> TestResult {
    let fixture = RepoFixture::new()?;
    let out = fixture.temp.path().join("snapshot.tar.gz");
    let snapshot = snapshot_uncommitted(&fixture.repo, &out, 1024)?;
    assert_eq!(snapshot.file, None);
    assert!(snapshot.skipped.is_empty());
    assert!(!out.exists());
    Ok(())
}

#[test]
fn refuses_snapshots_over_the_budget() -> TestResult {
    let fixture = RepoFixture::new()?;
    write(&fixture.repo.join("big.txt"), &"x".repeat(4096))?;
    let out = fixture.temp.path().join("snapshot.tar.gz");
    let result = snapshot_uncommitted(&fixture.repo, &out, 1000);
    assert!(matches!(result, Err(SnapshotError::TooLarge { limit: 1000 })));
    Ok(())
}
