use super::*;
use crate::instance::import::Landed;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const BOOT: &str = "boot-1";

/// The boot every test's passes run in, with a clock that was never set.
fn era() -> Era {
    at(BOOT, 0)
}

/// The boot `boot`, with a clock `ahead` milliseconds ahead of the time the boot has run.
fn at(boot: &str, ahead: i64) -> Era {
    Era {
        boot: boot.into(),
        ahead: Some(ahead),
    }
}

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
    stage.begin(nonce, live, 1_000, &era())?;
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
        &era(),
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
        &era(),
    )?;

    // Sent again and no receipt earned (it changed while it was read).
    stage.begin("n2", true, 2_000, &era())?;
    stage.sending(d(1), check(0))?;
    stage.seal()?;
    let found = stage.reconcile(
        Some(&mark("s1", 2, Some(landed("n2", 1, true, true)))),
        &era(),
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
        &era(),
    )?;
    // The new host may have taken some of the next stream in before it died.
    send(&mut stage, "n2", true, &[2, 3])?;
    let found = stage.reconcile(
        Some(&mark("s1", 1, Some(landed("n1", 2, true, true)))),
        &era(),
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
        &era(),
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
        &era(),
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
        &era(),
    )?;
    send(&mut stage, "n1", true, &[1, 2, 9])?;
    let mut last = landed("n1", 2, false, true);
    last.sums = ["00".repeat(8), "11".repeat(8)];
    let found = stage.reconcile(Some(&mark("s1", 2, Some(last))), &era())?;
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
    let found = stage.reconcile(Some(&mark("s1", 1, Some(last))), &era())?;
    assert_eq!(found.adopted, 2);
    assert!(stage.held(&d(1), &d(101)) && stage.held(&d(3), &d(103)));
    assert!(!stage.held(&d(2), &d(102)));
    Ok(())
}

#[test]
fn any_other_marker_drops_the_receipts() -> TestResult {
    let whole = |nonce: &str| Some(landed(nonce, 1, true, true));
    let cases: Vec<(&str, Option<Mark>)> = vec![
        ("the copy was removed", None),
        ("another stage", Some(mark("s2", 2, whole("n2")))),
        ("another stream", Some(mark("s1", 2, whole("nX")))),
        ("two streams ahead", Some(mark("s1", 3, whole("n2")))),
        ("behind", Some(mark("s1", 0, None))),
    ];
    for (name, found) in cases {
        let tmp = tempfile::tempdir()?;
        let mut stage = new_stage(tmp.path())?;
        send(&mut stage, "n1", true, &[1])?;
        stage.reconcile(Some(&mark("s1", 1, whole("n1"))), &era())?;
        send(&mut stage, "n2", true, &[2])?;
        let out = stage.reconcile(found.as_ref(), &era())?;
        assert!(out.fresh, "{name}");
        assert!(!stage.held(&d(1), &d(101)), "{name}");
        assert_eq!((stage.id(), stage.receipts()), (None, 0), "{name}");
        assert_eq!(Stage::open(&tmp.path().join("copy")).id(), None, "{name}");
        // A stream needs a stage.
        assert!(stage.begin("n3", true, 0, &era()).is_err(), "{name}");
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
        &era(),
    )?;

    // Frozen: 1 is still there and left alone, 2 changed and is sent, 3 is gone on the old
    // host (the new host deletes it).
    stage.begin("n2", false, 2_000, &era())?;
    stage.kept(d(1), d(101))?;
    stage.sending(d(2), check(0))?;
    stage.seal()?;
    let found = stage.reconcile(
        Some(&mark("s1", 2, Some(landed("n2", 1, true, false)))),
        &era(),
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
    stage.begin("n3", false, 3_000, &era())?;
    stage.sending(d(1), check(0))?;
    stage.sending(d(5), check(1))?;
    stage.seal()?;
    stage.reconcile(
        Some(&mark("s1", 3, Some(landed("n3", 1, false, false)))),
        &era(),
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
        &era(),
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
        &era(),
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
        &era(),
    )?;
    // It replaced a directory by a file (what was below is gone there) and then broke.
    stage.begin("n2", false, 2_000, &era())?;
    stage.kept(d(1), d(101))?;
    stage.sending(d(2), check(0))?;
    stage.seal()?;
    let mut last = landed("n2", 1, false, false);
    last.unknown = true;
    let found = stage.reconcile(Some(&mark("s1", 2, Some(last))), &era())?;
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
        &era(),
    )?;
    // The next stream's receive was killed: files half written, directories' times moved, and
    // only the marker it wrote first says so.
    send(&mut stage, "n2", true, &[2])?;
    let found = stage.reconcile(Some(&mark("s1", 2, Some(opened("n2", true)))), &era())?;
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
        &era(),
    )?;
    send(&mut stage, "n2", true, &[2, 3])?;
    // The new host took the stream in; this host was restarted since, so what the journal
    // says was sent may not be all that was.
    let found = stage.reconcile(
        Some(&mark("s1", 2, Some(landed("n2", 2, true, true)))),
        &at("boot-2", 0),
    )?;
    assert_eq!((found.fresh, found.adopted), (false, 0));
    assert_eq!(
        (stage.id(), stage.acked(), stage.receipts()),
        (Some("s1"), 2, 0)
    );

    Ok(())
}

#[test]
fn receipts_earned_before_a_restart_are_not_believed_after_it() -> TestResult {
    // An inode's times are not written with its data: after a crash a file may hold newer
    // content than the change time its receipt was earned with.
    let tmp = tempfile::tempdir()?;
    let mut stage = new_stage(tmp.path())?;
    send(&mut stage, "n1", true, &[1, 2])?;
    let whole = mark("s1", 1, Some(landed("n1", 2, true, true)));
    stage.reconcile(Some(&whole), &era())?;
    assert!(stage.held(&d(1), &d(101)) && stage.whole());
    assert_eq!(stage.outdated(&era()), None);

    // The same marker after a restart: the staged copy is this move's, and nothing of it is
    // known to be as the files are now.
    let mut stage = Stage::open(&tmp.path().join("copy"));
    assert_eq!(stage.receipts(), 2);
    assert_eq!(stage.outdated(&at("boot-2", 0)), Some(Outdated::Restarted));
    let found = stage.reconcile(Some(&whole), &at("boot-2", 0))?;
    assert_eq!(
        (found.fresh, found.adopted, found.revoked, found.outdated),
        (false, 0, 2, Some(Outdated::Restarted))
    );
    assert_eq!(
        (stage.id(), stage.acked(), stage.receipts(), stage.whole()),
        (Some("s1"), 1, 0, false)
    );
    assert!(!stage.held(&d(1), &d(101)));
    // Dropped for good, also for a run that finds the first boot's id again.
    let again = Stage::open(&tmp.path().join("copy"));
    assert_eq!((again.id(), again.receipts()), (Some("s1"), 0));
    assert_eq!(again.outdated(&era()), None);

    // A stream that never reached the new host, and then the restart: the same.
    let mut stage = new_stage(&tmp.path().join("b"))?;
    send(&mut stage, "n1", true, &[1, 2])?;
    stage.reconcile(Some(&whole), &era())?;
    send(&mut stage, "n2", true, &[9])?;
    let found = stage.reconcile(Some(&whole), &at("boot-2", 0))?;
    assert_eq!(
        (found.fresh, found.outdated),
        (false, Some(Outdated::Restarted))
    );
    assert_eq!(
        (stage.id(), stage.acked(), stage.receipts()),
        (Some("s1"), 1, 0)
    );
    // The next pass of the new boot earns receipts as any other.
    stage.begin("n3", true, 3_000, &at("boot-2", 0))?;
    stage.sending(d(1), check(0))?;
    stage.earned(d(101))?;
    stage.seal()?;
    let found = stage.reconcile(
        Some(&mark("s1", 2, Some(landed("n3", 1, true, true)))),
        &at("boot-2", 0),
    )?;
    assert_eq!((found.adopted, found.outdated), (1, None));
    assert!(stage.held(&d(1), &d(101)));
    Ok(())
}

#[test]
fn a_pass_does_not_begin_on_receipts_from_before_a_restart() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut stage = new_stage(tmp.path())?;
    send(&mut stage, "n1", true, &[1, 2])?;
    stage.reconcile(
        Some(&mark("s1", 1, Some(landed("n1", 2, true, true)))),
        &era(),
    )?;
    // What was planned on the receipts must not be sent: the pass is refused, and the
    // receipts are gone for the next one.
    let refused = stage.begin("n2", false, 2_000, &at("boot-2", 0));
    assert!(
        refused.as_ref().is_err_and(|why| why.contains("restarted")),
        "{refused:?}"
    );
    assert_eq!((stage.receipts(), stage.whole()), (0, false));
    stage.begin("n2", true, 2_000, &at("boot-2", 0))?;
    Ok(())
}

#[test]
fn a_clock_that_was_set_back_leaves_nothing_known() -> TestResult {
    // A change is told by its inode's change time: a clock that goes back by two seconds or
    // more can stamp a later change with the time of what was copied.
    let tmp = tempfile::tempdir()?;
    let whole = mark("s1", 1, Some(landed("n1", 2, true, true)));
    let begun = |dir: &Path| -> Result<Stage, Box<dyn std::error::Error>> {
        let mut stage = new_stage(dir)?;
        stage.begin("n1", true, 1_000, &at(BOOT, 50_000))?;
        stage.sending(d(1), check(0))?;
        stage.earned(d(101))?;
        stage.sending(d(2), check(1))?;
        stage.earned(d(102))?;
        stage.seal()?;
        Ok(stage)
    };
    // The clock's own pace and a step forward change nothing, nor does a second back (a leap
    // second).
    let mut stage = begun(&tmp.path().join("a"))?;
    for ahead in [50_000, 50_004, 49_000, 3_600_000] {
        assert_eq!(stage.outdated(&at(BOOT, ahead)), None, "{ahead}");
    }
    let found = stage.reconcile(Some(&whole), &at(BOOT, 49_000))?;
    assert_eq!((found.adopted, found.outdated), (2, None));

    // Two seconds back while the pass ran: what it sent earned nothing.
    let mut stage = begun(&tmp.path().join("b"))?;
    assert_eq!(stage.outdated(&at(BOOT, 48_000)), Some(Outdated::ClockBack));
    let found = stage.reconcile(Some(&whole), &at(BOOT, 48_000))?;
    assert_eq!(
        (found.fresh, found.adopted, found.outdated),
        (false, 0, Some(Outdated::ClockBack))
    );
    assert_eq!(
        (stage.id(), stage.acked(), stage.receipts(), stage.whole()),
        (Some("s1"), 1, 0, false)
    );
    // From there on the clock is where it is.
    assert_eq!(stage.outdated(&at(BOOT, 48_000)), None);
    stage.begin("n2", true, 2_000, &at(BOOT, 48_000))?;

    // Forward and, passes later, back again by as much: every pass's receipts are measured
    // against the furthest the clock was ahead when one of them began.
    let mut stage = begun(&tmp.path().join("c"))?;
    stage.reconcile(Some(&whole), &at(BOOT, 50_000))?;
    stage.begin("n2", true, 2_000, &at(BOOT, 80_000))?;
    stage.sending(d(3), check(0))?;
    stage.earned(d(103))?;
    stage.seal()?;
    let second = mark("s1", 2, Some(landed("n2", 1, true, true)));
    assert_eq!(
        stage.reconcile(Some(&second), &at(BOOT, 80_000))?.adopted,
        1
    );
    let reopened = Stage::open(&tmp.path().join("c/copy"));
    assert_eq!(reopened.outdated(&at(BOOT, 79_000)), None);
    assert_eq!(
        reopened.outdated(&at(BOOT, 50_000)),
        Some(Outdated::ClockBack)
    );
    // Where the time a boot has run is not known, only the boot counts.
    let unknown = Era {
        boot: BOOT.into(),
        ahead: None,
    };
    assert_eq!(reopened.outdated(&unknown), None);
    Ok(())
}

#[test]
fn a_completed_frozen_pass_that_is_not_the_journal_s_leaves_nothing_known() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut stage = new_stage(tmp.path())?;
    send(&mut stage, "n1", true, &[1, 2, 3])?;
    stage.reconcile(
        Some(&mark("s1", 1, Some(landed("n1", 3, true, true)))),
        &era(),
    )?;
    stage.begin("n2", false, 2_000, &era())?;
    stage.kept(d(1), d(101))?;
    stage.sending(d(2), check(0))?;
    stage.seal()?;
    // It says it took in two entries and removed the rest; the journal knows of one.
    let found = stage.reconcile(
        Some(&mark("s1", 2, Some(landed("n2", 2, true, false)))),
        &era(),
    )?;
    assert_eq!(
        (found.fresh, stage.receipts(), stage.acked()),
        (false, 0, 2)
    );
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn this_host_names_its_boot_and_its_clock_s_lead() -> TestResult {
    // What every other test injects, read for real: a boot id that stays, and a lead that
    // does not move between two looks (a lead that moved would drop the receipts each time).
    let first = Era::here();
    let second = Era::here();
    assert!(!first.boot.is_empty());
    assert_eq!(first.boot, second.boot);
    let (was, now) = (
        first.ahead.ok_or("no lead")?,
        second.ahead.ok_or("no lead")?,
    );
    assert!((now - was).abs() < SET_BACK_MS, "{was} then {now}");
    // The lead is the time this boot began, as the kernel says it.
    let stat = std::fs::read_to_string("/proc/stat")?;
    let began: i64 = stat
        .lines()
        .find_map(|line| line.strip_prefix("btime "))
        .ok_or("no btime")?
        .trim()
        .parse()?;
    assert!(
        (was / 1000 - began).abs() <= 2,
        "{was} ms against {began} s"
    );
    // A stage that began a pass here still believes its receipts here.
    let tmp = tempfile::tempdir()?;
    let mut stage = new_stage(tmp.path())?;
    stage.begin("n1", true, 1_000, &first)?;
    stage.sending(d(1), check(0))?;
    stage.earned(d(101))?;
    stage.seal()?;
    assert_eq!(stage.outdated(&second), None);
    Ok(())
}
