use super::*;
use crate::instance::import::Landed;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const BOOT: &str = "boot-1";

fn d(n: u8) -> Digest {
    [n; 16]
}

/// The stream's digest before entry `n`, as a test's sender records it.
fn check(n: u8) -> [u8; 8] {
    [0xc0 | n; 8]
}

fn mark(stage: &str, acked: u64, last: Option<Landed>) -> Mark {
    Mark {
        version: 1,
        stage: stage.into(),
        acked,
        last,
    }
}

fn landed(nonce: &str, entries: u64, complete: bool, live: bool) -> Landed {
    let sum = |at: u64| {
        u8::try_from(at)
            .map(|n| hex::encode(check(n)))
            .unwrap_or_default()
    };
    Landed {
        nonce: nonce.into(),
        entries,
        sums: [sum(entries), sum(entries.saturating_sub(1))],
        complete,
        live,
        deferred: Vec::new(),
        unknown: false,
    }
}

/// Sends `paths` in a stream called `nonce`; each earns the state of its own number.
fn send(stage: &mut Stage, nonce: &str, live: bool, paths: &[u8]) -> TestResult {
    stage.begin(nonce, live, 1_000, BOOT)?;
    for (at, path) in paths.iter().enumerate() {
        stage.sending(d(*path), check(u8::try_from(at)?))?;
        stage.earned(d(*path + 100))?;
    }
    stage.seal()?;
    Ok(())
}

fn new_stage(dir: &Path) -> Result<Stage, String> {
    let mut stage = Stage::open(&dir.join("copy"));
    assert_eq!(stage.id(), None);
    stage.renew("s1")?;
    Ok(stage)
}

#[test]
fn a_state_is_the_headers_the_inode_and_its_change_time() {
    let base = state(b"headers", 7, (100, 5));
    assert_eq!(base, state(b"headers", 7, (100, 5)));
    // Same size and time, another content: the change time moved.
    assert_ne!(base, state(b"headers", 7, (100, 6)));
    assert_ne!(base, state(b"headers", 7, (101, 5)));
    // Another file at the same path.
    assert_ne!(base, state(b"headers", 8, (100, 5)));
    assert_ne!(base, state(b"headerz", 7, (100, 5)));
}

#[test]
fn a_whole_stream_that_was_acknowledged_is_held_and_survives_a_reload() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut stage = new_stage(tmp.path())?;
    assert!(!stage.held(&d(1), &d(101)) && !stage.whole());
    send(&mut stage, "n1", true, &[1, 2, 3])?;
    // Nothing is held on the old host's own word.
    assert!(!stage.held(&d(1), &d(101)));
    let found = stage.reconcile(
        Some(&mark("s1", 1, Some(landed("n1", 3, true, true)))),
        BOOT,
    )?;
    assert_eq!((found.fresh, found.adopted, found.revoked), (false, 3, 0));
    assert!(stage.held(&d(1), &d(101)) && stage.held(&d(3), &d(103)));
    // Another state of the same path is not held.
    assert!(!stage.held(&d(1), &d(102)));
    assert_eq!(
        (stage.acked(), stage.whole(), stage.began()),
        (1, true, Some(1_000))
    );

    let again = Stage::open(&tmp.path().join("copy"));
    assert_eq!(
        (again.id(), again.acked(), again.receipts()),
        (Some("s1"), 1, 3)
    );
    assert!(again.held(&d(2), &d(102)) && again.whole());
    Ok(())
}

#[test]
fn sending_a_path_revokes_what_was_known_of_it() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut stage = new_stage(tmp.path())?;
    send(&mut stage, "n1", true, &[1, 2])?;
    stage.reconcile(
        Some(&mark("s1", 1, Some(landed("n1", 2, true, true)))),
        BOOT,
    )?;

    // Sent again and no receipt earned (it changed while it was read).
    stage.begin("n2", true, 2_000, BOOT)?;
    stage.sending(d(1), check(0))?;
    stage.seal()?;
    let found = stage.reconcile(
        Some(&mark("s1", 2, Some(landed("n2", 1, true, true)))),
        BOOT,
    )?;
    assert_eq!((found.adopted, found.revoked), (0, 1));
    assert!(!stage.held(&d(1), &d(101)) && stage.held(&d(2), &d(102)));
    let again = Stage::open(&tmp.path().join("copy"));
    assert!(!again.held(&d(1), &d(101)) && again.held(&d(2), &d(102)));
    Ok(())
}

#[test]
fn a_stream_that_was_not_acknowledged_revokes_every_path_it_sent() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut stage = new_stage(tmp.path())?;
    send(&mut stage, "n1", true, &[1, 2])?;
    stage.reconcile(
        Some(&mark("s1", 1, Some(landed("n1", 2, true, true)))),
        BOOT,
    )?;
    // The new host may have taken some of the next stream in before it died.
    send(&mut stage, "n2", true, &[2, 3])?;
    let found = stage.reconcile(
        Some(&mark("s1", 1, Some(landed("n1", 2, true, true)))),
        BOOT,
    )?;
    assert_eq!((found.fresh, found.adopted, found.revoked), (false, 0, 1));
    assert!(stage.held(&d(1), &d(101)));
    assert!(!stage.held(&d(2), &d(102)) && !stage.held(&d(3), &d(103)));
    assert_eq!(stage.acked(), 1);
    Ok(())
}

#[test]
fn a_cut_stream_is_adopted_up_to_the_entry_whose_digest_both_hosts_recorded() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut stage = new_stage(tmp.path())?;
    send(&mut stage, "n1", true, &[1, 2, 3, 4])?;
    // Two entries were installed there; the digest before the third is the recorded one.
    let found = stage.reconcile(
        Some(&mark("s1", 1, Some(landed("n1", 2, false, true)))),
        BOOT,
    )?;
    assert_eq!((found.fresh, found.adopted), (false, 2));
    assert!(stage.held(&d(1), &d(101)) && stage.held(&d(2), &d(102)));
    assert!(!stage.held(&d(3), &d(103)) && !stage.held(&d(4), &d(104)));
    // A cut stream is not a whole pass.
    assert_eq!((stage.acked(), stage.whole()), (1, false));
    Ok(())
}

#[test]
fn a_cut_after_the_last_recorded_entry_adopts_up_to_the_entry_before() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut stage = new_stage(tmp.path())?;
    send(&mut stage, "n1", true, &[1, 2, 3])?;
    // All three arrived, and no record says what the stream was after the third.
    let found = stage.reconcile(
        Some(&mark("s1", 1, Some(landed("n1", 3, false, true)))),
        BOOT,
    )?;
    assert_eq!(found.adopted, 2);
    assert!(stage.held(&d(2), &d(102)) && !stage.held(&d(3), &d(103)));
    Ok(())
}

#[test]
fn a_prefix_whose_digest_is_not_the_recorded_one_is_not_adopted() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut stage = new_stage(tmp.path())?;
    send(&mut stage, "n0", true, &[9])?;
    stage.reconcile(
        Some(&mark("s1", 1, Some(landed("n0", 1, true, true)))),
        BOOT,
    )?;
    send(&mut stage, "n1", true, &[1, 2, 9])?;
    let mut last = landed("n1", 2, false, true);
    last.sums = ["00".repeat(8), "11".repeat(8)];
    let found = stage.reconcile(Some(&mark("s1", 2, Some(last))), BOOT)?;
    assert_eq!((found.fresh, found.adopted, found.revoked), (false, 0, 1));
    assert!(!stage.held(&d(1), &d(101)) && !stage.held(&d(9), &d(109)));
    assert_eq!(stage.acked(), 2);
    Ok(())
}

#[test]
fn an_entry_the_new_host_did_not_install_gets_no_receipt() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut stage = new_stage(tmp.path())?;
    send(&mut stage, "n1", true, &[1, 2, 3])?;
    let mut last = landed("n1", 3, true, true);
    last.deferred = vec![1];
    let found = stage.reconcile(Some(&mark("s1", 1, Some(last))), BOOT)?;
    assert_eq!(found.adopted, 2);
    assert!(stage.held(&d(1), &d(101)) && stage.held(&d(3), &d(103)));
    assert!(!stage.held(&d(2), &d(102)));
    Ok(())
}

#[test]
fn any_other_marker_drops_the_receipts() -> TestResult {
    let whole = |nonce: &str| Some(landed(nonce, 1, true, true));
    let cases: Vec<(&str, Option<Mark>, &str)> = vec![
        ("the copy was removed", None, BOOT),
        ("another stage", Some(mark("s2", 2, whole("n2"))), BOOT),
        ("another stream", Some(mark("s1", 2, whole("nX"))), BOOT),
        ("two streams ahead", Some(mark("s1", 3, whole("n2"))), BOOT),
        ("behind", Some(mark("s1", 0, None)), BOOT),
    ];
    for (name, found, boot) in cases {
        let tmp = tempfile::tempdir()?;
        let mut stage = new_stage(tmp.path())?;
        send(&mut stage, "n1", true, &[1])?;
        stage.reconcile(Some(&mark("s1", 1, whole("n1"))), BOOT)?;
        send(&mut stage, "n2", true, &[2])?;
        let out = stage.reconcile(found.as_ref(), boot)?;
        assert!(out.fresh, "{name}");
        assert!(!stage.held(&d(1), &d(101)), "{name}");
        assert_eq!((stage.id(), stage.receipts()), (None, 0), "{name}");
        assert_eq!(Stage::open(&tmp.path().join("copy")).id(), None, "{name}");
        // A stream needs a stage.
        assert!(stage.begin("n3", true, 0, BOOT).is_err(), "{name}");
    }
    Ok(())
}

#[test]
fn a_completed_frozen_pass_leaves_exactly_what_it_left_alone() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut stage = new_stage(tmp.path())?;
    send(&mut stage, "n1", true, &[1, 2, 3])?;
    stage.reconcile(
        Some(&mark("s1", 1, Some(landed("n1", 3, true, true)))),
        BOOT,
    )?;

    // Frozen: 1 is still there and left alone, 2 changed and is sent, 3 is gone on the old
    // host (the new host deletes it).
    stage.begin("n2", false, 2_000, BOOT)?;
    stage.kept(d(1), d(101))?;
    stage.sending(d(2), check(0))?;
    stage.seal()?;
    let found = stage.reconcile(
        Some(&mark("s1", 2, Some(landed("n2", 1, true, false)))),
        BOOT,
    )?;
    assert_eq!((found.fresh, found.adopted, found.revoked), (false, 0, 2));
    assert!(stage.held(&d(1), &d(101)));
    assert!(!stage.held(&d(2), &d(102)) && !stage.held(&d(3), &d(103)));
    assert_eq!(
        (stage.acked(), stage.receipts(), stage.began()),
        (2, 1, Some(2_000))
    );
    let again = Stage::open(&tmp.path().join("copy"));
    assert!(again.held(&d(1), &d(101)) && !again.held(&d(3), &d(103)));
    assert_eq!(again.receipts(), 1);

    // A frozen pass that was cut deleted nothing: the receipts stay, what it sent is revoked.
    let mut stage = again;
    stage.begin("n3", false, 3_000, BOOT)?;
    stage.sending(d(1), check(0))?;
    stage.sending(d(5), check(1))?;
    stage.seal()?;
    stage.reconcile(
        Some(&mark("s1", 3, Some(landed("n3", 1, false, false)))),
        BOOT,
    )?;
    assert!(!stage.held(&d(1), &d(101)));
    assert_eq!(stage.acked(), 3);
    Ok(())
}

#[test]
fn what_a_run_that_died_left_beyond_the_valid_log_is_not_a_receipt() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut stage = new_stage(tmp.path())?;
    send(&mut stage, "n1", true, &[1])?;
    stage.reconcile(
        Some(&mark("s1", 1, Some(landed("n1", 1, true, true)))),
        BOOT,
    )?;
    drop(stage);
    // Appended, and the count never saved.
    let log = tmp.path().join("copy/receipts-0");
    let mut bytes = std::fs::read(&log)?;
    bytes.extend_from_slice(&[d(7), d(107)].concat());
    std::fs::write(&log, bytes)?;
    let mut stage = Stage::open(&tmp.path().join("copy"));
    assert!(stage.held(&d(1), &d(101)) && !stage.held(&d(7), &d(107)));
    // The next adoption writes over it.
    send(&mut stage, "n2", true, &[2])?;
    stage.reconcile(
        Some(&mark("s1", 2, Some(landed("n2", 1, true, true)))),
        BOOT,
    )?;
    let again = Stage::open(&tmp.path().join("copy"));
    assert!(again.held(&d(2), &d(102)) && !again.held(&d(7), &d(107)));

    // A log that lost its end cannot be trusted at all.
    std::fs::write(&log, [0u8; 8])?;
    let broken = Stage::open(&tmp.path().join("copy"));
    assert_eq!((broken.id(), broken.receipts()), (None, 0));
    Ok(())
}

#[test]
fn the_state_directory_is_private() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;
    let tmp = tempfile::tempdir()?;
    let mut stage = new_stage(tmp.path())?;
    send(&mut stage, "n1", true, &[1])?;
    let mode = |name: &str| -> std::io::Result<u32> {
        Ok(std::fs::metadata(tmp.path().join("copy").join(name))?
            .permissions()
            .mode()
            & 0o777)
    };
    assert_eq!(mode("")?, 0o700);
    assert_eq!(mode("pending")?, 0o600);
    assert_eq!(mode("stage.json")?, 0o600);
    assert_ne!(token(), token());
    assert_eq!(token().len(), 16);
    Ok(())
}

#[test]
fn a_frozen_pass_that_removed_and_was_cut_leaves_nothing_known() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut stage = new_stage(tmp.path())?;
    send(&mut stage, "n1", true, &[1, 2, 3])?;
    stage.reconcile(
        Some(&mark("s1", 1, Some(landed("n1", 3, true, true)))),
        BOOT,
    )?;
    // It replaced a directory by a file (what was below is gone there) and then broke.
    stage.begin("n2", false, 2_000, BOOT)?;
    stage.kept(d(1), d(101))?;
    stage.sending(d(2), check(0))?;
    stage.seal()?;
    let mut last = landed("n2", 1, false, false);
    last.unknown = true;
    let found = stage.reconcile(Some(&mark("s1", 2, Some(last))), BOOT)?;
    assert_eq!((found.fresh, found.adopted, found.revoked), (false, 0, 3));
    assert_eq!(
        (stage.id(), stage.acked(), stage.receipts(), stage.whole()),
        (Some("s1"), 2, 0, false)
    );
    let again = Stage::open(&tmp.path().join("copy"));
    assert_eq!((again.acked(), again.receipts()), (2, 0));
    Ok(())
}

/// The marker a receive writes before it takes anything in: one stream ahead, nothing known.
fn opened(nonce: &str, live: bool) -> Landed {
    Landed {
        nonce: nonce.into(),
        live,
        unknown: true,
        ..Landed::default()
    }
}

#[test]
fn a_receive_that_never_closed_its_stream_leaves_nothing_known() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut stage = new_stage(tmp.path())?;
    send(&mut stage, "n1", true, &[1, 2, 3])?;
    stage.reconcile(
        Some(&mark("s1", 1, Some(landed("n1", 3, true, true)))),
        BOOT,
    )?;
    // The next stream's receive was killed: files half written, directories' times moved, and
    // only the marker it wrote first says so.
    send(&mut stage, "n2", true, &[2])?;
    let found = stage.reconcile(Some(&mark("s1", 2, Some(opened("n2", true)))), BOOT)?;
    assert_eq!((found.fresh, found.adopted, found.revoked), (false, 0, 3));
    // The stage goes on, with no receipts: the next pass sends everything into it.
    assert_eq!(
        (stage.id(), stage.acked(), stage.receipts(), stage.whole()),
        (Some("s1"), 2, 0, false)
    );
    assert!(!stage.held(&d(1), &d(101)));
    Ok(())
}

#[test]
fn a_journal_from_before_a_restart_is_not_believed() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut stage = new_stage(tmp.path())?;
    send(&mut stage, "n1", true, &[1, 2])?;
    stage.reconcile(
        Some(&mark("s1", 1, Some(landed("n1", 2, true, true)))),
        BOOT,
    )?;
    send(&mut stage, "n2", true, &[2, 3])?;
    // The new host took the stream in; this host was restarted since, so what the journal
    // says was sent may not be all that was.
    let found = stage.reconcile(
        Some(&mark("s1", 2, Some(landed("n2", 2, true, true)))),
        "boot-2",
    )?;
    assert_eq!((found.fresh, found.adopted), (false, 0));
    assert_eq!(
        (stage.id(), stage.acked(), stage.receipts()),
        (Some("s1"), 2, 0)
    );

    // A stream that never reached the new host installed nothing there: the receipts stand,
    // whatever became of its journal.
    let mut stage = new_stage(&tmp.path().join("b"))?;
    send(&mut stage, "n1", true, &[1, 2])?;
    stage.reconcile(
        Some(&mark("s1", 1, Some(landed("n1", 2, true, true)))),
        BOOT,
    )?;
    send(&mut stage, "n2", true, &[9])?;
    let found = stage.reconcile(
        Some(&mark("s1", 1, Some(landed("n1", 2, true, true)))),
        "boot-2",
    )?;
    assert!(!found.fresh);
    assert!(stage.held(&d(1), &d(101)) && stage.held(&d(2), &d(102)));
    Ok(())
}

#[test]
fn a_completed_frozen_pass_that_is_not_the_journal_s_leaves_nothing_known() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut stage = new_stage(tmp.path())?;
    send(&mut stage, "n1", true, &[1, 2, 3])?;
    stage.reconcile(
        Some(&mark("s1", 1, Some(landed("n1", 3, true, true)))),
        BOOT,
    )?;
    stage.begin("n2", false, 2_000, BOOT)?;
    stage.kept(d(1), d(101))?;
    stage.sending(d(2), check(0))?;
    stage.seal()?;
    // It says it took in two entries and removed the rest; the journal knows of one.
    let found = stage.reconcile(
        Some(&mark("s1", 2, Some(landed("n2", 2, true, false)))),
        BOOT,
    )?;
    assert_eq!(
        (found.fresh, stage.receipts(), stage.acked()),
        (false, 0, 2)
    );
    Ok(())
}
