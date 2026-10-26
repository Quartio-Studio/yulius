use std::fs;
use std::sync::atomic::AtomicBool;

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
    let stop = AtomicBool::new(false);
    let plan = plan(dir.path(), &stop)?;
    let mut out = Vec::new();
    let mut ticks = Vec::new();
    let sent = send(dir.path(), &plan, &manifest(), &mut out, &stop, &mut |d| {
        ticks.push(d)
    })?;
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
    let stop = AtomicBool::new(false);
    let plan = plan(dir.path(), &stop)?;
    fs::write(dir.path().join("master.key"), b"longer key")?;
    let err = send(
        dir.path(),
        &plan,
        &manifest(),
        Vec::new(),
        &stop,
        &mut |_| {},
    )
    .err()
    .ok_or("expected an error")?;
    assert!(err.to_string().contains("during the export"), "{err}");
    Ok(())
}

#[test]
fn a_new_file_after_the_plan_aborts_the_stream() -> TestResult {
    let dir = tree()?;
    let stop = AtomicBool::new(false);
    let plan = plan(dir.path(), &stop)?;
    fs::write(dir.path().join("zzz"), vec![0u8; 5000])?;
    let err = send(
        dir.path(),
        &plan,
        &manifest(),
        Vec::new(),
        &stop,
        &mut |_| {},
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
fn a_signal_stops_the_plan_and_the_stream() -> TestResult {
    let dir = tree()?;
    let stop = AtomicBool::new(true);
    assert_eq!(
        plan(dir.path(), &stop)
            .err()
            .ok_or("expected an error")?
            .kind(),
        io::ErrorKind::Interrupted
    );
    let go = AtomicBool::new(false);
    let p = plan(dir.path(), &go)?;
    let err = send(dir.path(), &p, &manifest(), Vec::new(), &stop, &mut |_| {})
        .err()
        .ok_or("expected an error")?;
    assert_eq!(err.kind(), io::ErrorKind::Interrupted);
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
    let stop = AtomicBool::new(false);
    let p = plan(dir.path(), &stop)?;
    let err = send(dir.path(), &p, &manifest(), Gone, &stop, &mut |_| {})
        .err()
        .ok_or("expected an error")?;
    assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    Ok(())
}
