use std::fs;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;
use crate::instance::archive::{Item, Reader};

fn tree() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    fs::create_dir_all(root.join("sandboxes/sb/home/.claude"))?;
    fs::create_dir_all(root.join("system/base-1"))?;
    fs::write(root.join("yulius.db"), vec![7u8; 70_000])?;
    fs::write(root.join("master.key"), b"k")?;
    fs::write(root.join("sandboxes/sb/home/.claude/s.jsonl"), b"{}\n")?;
    fs::write(root.join("system/base-1/big"), vec![1u8; 10_000])?;
    Ok(dir)
}

fn manifest() -> Manifest {
    Manifest {
        format: crate::instance::FORMAT,
        data_dir: "/var/lib/yulius".into(),
        ..Manifest::default()
    }
}

#[test]
fn the_stream_is_exactly_the_announced_length_and_reads_back() -> TestResult {
    let dir = tree()?;
    let plan = plan(dir.path())?;
    let mut out = Vec::new();
    let mut ticks = Vec::new();
    let sent = send(
        dir.path(),
        &plan,
        &manifest(),
        &mut out,
        &mut |d| ticks.push(d),
        &mut || Ok(()),
    )?;
    assert_eq!(sent, plan.total);
    assert_eq!(out.len() as u64, plan.total);
    assert!(ticks.windows(2).all(|w| w[0] <= w[1]) && !ticks.is_empty());

    let mut r = Reader::new(out.as_slice());
    let Item::Entry(first) = r.next_item()? else {
        panic!("no manifest")
    };
    assert_eq!(first.path, MANIFEST);
    let mut json = vec![0u8; MANIFEST_LEN as usize];
    let mut at = 0;
    while at < json.len() {
        at += r.data(&mut json[at..])?;
    }
    let m: Manifest = serde_json::from_slice(&json)?;
    assert_eq!(m.total_bytes, plan.total);
    assert_eq!(m.entries, plan.entries);
    let mut paths = Vec::new();
    while let Item::Entry(e) = r.next_item()? {
        paths.push(String::from_utf8(e.path)?);
    }
    assert_eq!(paths.len() as u64, plan.entries);
    assert!(paths.contains(&"data/sandboxes/sb/home/.claude/s.jsonl".to_owned()));
    assert!(
        !paths.iter().any(|p| p.starts_with("data/system")),
        "{paths:?}"
    );
    Ok(())
}

#[test]
fn a_file_that_changed_after_the_plan_aborts_the_stream() -> TestResult {
    let dir = tree()?;
    let plan = plan(dir.path())?;
    fs::write(dir.path().join("master.key"), b"longer key")?;
    let err = send(
        dir.path(),
        &plan,
        &manifest(),
        Vec::new(),
        &mut |_| {},
        &mut || Ok(()),
    )
    .err()
    .ok_or("expected an error")?;
    assert!(err.to_string().contains("during the export"), "{err}");
    Ok(())
}

#[test]
fn a_new_file_after_the_plan_aborts_the_stream() -> TestResult {
    let dir = tree()?;
    let plan = plan(dir.path())?;
    fs::write(dir.path().join("zzz"), vec![0u8; 5000])?;
    let err = send(
        dir.path(),
        &plan,
        &manifest(),
        Vec::new(),
        &mut |_| {},
        &mut || Ok(()),
    )
    .err()
    .ok_or("expected an error")?;
    assert!(
        err.to_string().contains("changed during the export"),
        "{err}"
    );
    Ok(())
}

#[test]
fn a_reader_that_goes_away_fails_the_stream() -> TestResult {
    struct Gone;
    impl Write for Gone {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let dir = tree()?;
    let p = plan(dir.path())?;
    let err = send(dir.path(), &p, &manifest(), Gone, &mut |_| {}, &mut || {
        Ok(())
    })
    .err()
    .ok_or("expected an error")?;
    assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    Ok(())
}

#[test]
fn an_update_found_before_the_end_leaves_the_copy_without_its_checksum() -> TestResult {
    let dir = tree()?;
    let p = plan(dir.path())?;
    let mut out = Vec::new();
    let err = send(
        dir.path(),
        &p,
        &manifest(),
        &mut out,
        &mut |_| {},
        &mut || Err(io::Error::other("an update is running")),
    )
    .err()
    .ok_or("expected an error")?;
    assert!(err.to_string().contains("an update is running"), "{err}");
    let mut r = Reader::new(out.as_slice());
    let ended = loop {
        match r.next_item() {
            Ok(Item::Entry(_)) => {}
            Ok(Item::End) => break None,
            Err(e) => break Some(e),
        }
    };
    let ended = ended.ok_or("the copy read as complete")?;
    assert!(ended.to_string().contains("ended early"), "{ended}");
    Ok(())
}

#[test]
fn an_entry_the_import_would_refuse_fails_the_plan() -> TestResult {
    let dir = tree()?;
    let file = dir.path().join("master.key");
    // Many attributes whose pax records pass the reader's 1 MiB bound.
    let value = vec![b'v'; 60_000];
    for i in 0..20 {
        if rustix::fs::lsetxattr(
            &file,
            format!("user.big{i}").as_str(),
            &value[..],
            rustix::fs::XattrFlags::empty(),
        )
        .is_err()
        {
            eprintln!("skipped: this filesystem keeps no large user attributes");
            return Ok(());
        }
    }
    let err = plan(dir.path()).err().ok_or("expected an error")?;
    assert!(
        err.to_string().contains("more than an export carries"),
        "{err}"
    );
    Ok(())
}
