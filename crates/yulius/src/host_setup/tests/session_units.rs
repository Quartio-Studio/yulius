//! The sessions unit, the adopt unit and the server's second drop-in (SPEC-458 HST-001).

use std::path::{Path, PathBuf};

use yulius_contracts::update::HostOutcome;

use super::super::session_units::{adopt_unit_text, drop_in_text, sessions_unit_text};
use super::super::{Mode, Options, Run};
use super::{EXE, Fake, SERVICE_UID, line, options, outcome, passed, run};

const ITEM: &str = "sessions unit";
const SESSIONS: &str = "/etc/systemd/system/yulius-sandboxes.service";
const ADOPT: &str = "/etc/systemd/system/yulius-adopt.service";
const DROP_IN: &str = "/etc/systemd/system/yulius.service.d/20-yulius-sessions.conf";

/// A host set up before ADR-0298: everything but the three files.
fn before_the_sessions_unit() -> Fake {
    let mut fake = Fake::set_up();
    for path in [SESSIONS, ADOPT, DROP_IN] {
        fake.files.remove(Path::new(path));
    }
    fake
}

#[test]
fn a_fresh_host_gets_the_units_and_a_second_run_changes_nothing() {
    let mut fake = Fake::fresh();
    let lines = run(&mut fake, Mode::Setup);
    assert!(passed(&lines), "{lines:#?}");
    assert_eq!(outcome(&lines, ITEM), HostOutcome::Changed, "{lines:#?}");
    assert_eq!(
        fake.text(SESSIONS),
        Some(sessions_unit_text("yulius.service", "yulius", "yulius.slice").as_str())
    );
    assert_eq!(
        fake.text(ADOPT),
        Some(
            adopt_unit_text(
                "yulius.service",
                "yulius-sandboxes.service",
                Path::new("/usr/local/bin/yulius")
            )
            .as_str()
        )
    );
    assert_eq!(
        fake.text(DROP_IN),
        Some(
            drop_in_text(
                "yulius-sandboxes.service",
                "yulius-adopt.service",
                "yulius.slice"
            )
            .as_str()
        )
    );
    let position = |command: &str| fake.log.iter().position(|c| c == command);
    let written = position(&format!("write {DROP_IN}"));
    let started = position("systemctl start yulius-sandboxes.service");
    assert!(written.is_some() && written < started, "{:#?}", fake.log);
    assert!(
        started < position("systemctl start yulius.service"),
        "the sessions unit is up before the server starts: {:#?}",
        fake.log
    );

    fake.log.clear();
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(outcome(&lines, ITEM), HostOutcome::Ok, "{lines:#?}");
    assert!(
        !fake.log.iter().any(|c| c.contains("yulius-sandboxes")),
        "{:#?}",
        fake.log
    );
}

#[test]
fn the_unit_texts_say_what_the_layout_needs() {
    let sessions = sessions_unit_text("yulius.service", "yulius", "yulius.slice");
    for needed in [
        "Slice=yulius.slice\n",
        "User=yulius\n",
        "ExecStartPre=+/usr/bin/chown yulius /sys/fs/cgroup/yulius.slice/cgroup.procs\n",
        // The keeper is root's: nothing of the service user can end it, and with it every session.
        "ExecStart=+/usr/bin/sleep infinity\n",
        "Delegate=yes\n",
        "DelegateSubgroup=keeper\n",
        "OOMScoreAdjust=-1000\n",
    ] {
        assert!(sessions.contains(needed), "{needed:?} in {sessions}");
    }
    assert!(
        !sessions.contains("KillMode"),
        "stopping it ends every session: a group that keeps processes could not start again"
    );
    let adopt = adopt_unit_text(
        "yulius.service",
        "yulius-sandboxes.service",
        Path::new("/usr/local/bin/yulius"),
    );
    for needed in [
        "Requires=yulius-sandboxes.service\n",
        "After=yulius-sandboxes.service\n",
        "Type=oneshot\n",
        "ExecStart=/usr/local/bin/yulius host adopt-sessions --service yulius.service\n",
    ] {
        assert!(adopt.contains(needed), "{needed:?} in {adopt}");
    }
    assert!(
        !adopt.contains("RemainAfterExit"),
        "it must run before every start of the server"
    );
    let drop_in = drop_in_text(
        "yulius-sandboxes.service",
        "yulius-adopt.service",
        "yulius.slice",
    );
    for needed in [
        "[Unit]\nWants=yulius-sandboxes.service yulius-adopt.service\n",
        "After=yulius-sandboxes.service yulius-adopt.service\n",
        "[Service]\nSlice=yulius.slice\n",
    ] {
        assert!(drop_in.contains(needed), "{needed:?} in {drop_in}");
    }
}

#[test]
fn a_running_host_gets_them_without_a_restart() {
    let mut fake = before_the_sessions_unit();
    fake.service_active = true;
    let lines = run(&mut fake, Mode::Setup);
    let item = line(&lines, ITEM);
    assert_eq!(item.outcome, HostOutcome::Changed, "{lines:#?}");
    assert!(item.detail.contains("next restart"), "{}", item.detail);
    assert!(fake.has(SESSIONS) && fake.has(ADOPT) && fake.has(DROP_IN));
    assert!(fake.ran("systemctl daemon-reload"));
    assert!(fake.ran("systemctl start yulius-sandboxes.service"));
    assert!(!fake.ran("systemctl restart"), "{:#?}", fake.log);
    assert!(
        !fake.ran("systemctl start yulius.service"),
        "{:#?}",
        fake.log
    );
}

#[test]
fn a_check_reports_it_and_a_dry_run_says_what_it_would_write() {
    let mut fake = before_the_sessions_unit();
    let lines = run(&mut fake, Mode::Check);
    let item = line(&lines, ITEM);
    assert_eq!(item.outcome, HostOutcome::Failed, "{lines:#?}");
    assert!(
        item.detail.contains("systemd 258") && item.detail.contains("sudo yulius host setup"),
        "{}",
        item.detail
    );
    assert!(!fake.has(SESSIONS) && !fake.has(ADOPT) && !fake.has(DROP_IN));

    let lines = run(&mut fake, Mode::DryRun);
    let item = line(&lines, ITEM);
    assert_eq!(item.outcome, HostOutcome::Changed, "{lines:#?}");
    assert!(item.detail.starts_with("would write"), "{}", item.detail);
    assert!(!fake.has(SESSIONS) && !fake.has(ADOPT) && !fake.has(DROP_IN));
    assert!(!fake.ran("systemctl start"), "{:#?}", fake.log);
}

/// Review of SPEC-458 (high): the adopt unit runs the server's binary as root before every
/// start. Where the service user could replace that binary, the unit would hand it root, so
/// nothing is written and the item fails naming the directory.
#[test]
fn a_binary_the_service_user_could_replace_gets_no_root_unit() {
    let mut fake = before_the_sessions_unit();
    fake.files.insert(
        PathBuf::from("/usr/local/bin"),
        (String::new(), SERVICE_UID, 0o040_755),
    );
    fake.files
        .insert(PathBuf::from(EXE), (String::new(), SERVICE_UID, 0o100_755));
    for mode in [Mode::Setup, Mode::Check, Mode::DryRun] {
        let lines = run(&mut fake, mode);
        let item = line(&lines, ITEM);
        assert_eq!(item.outcome, HostOutcome::Failed, "{mode:?}: {lines:#?}");
        assert!(
            item.detail.contains("/usr/local/bin") && item.detail.contains("as root"),
            "{mode:?}: {}",
            item.detail
        );
        assert!(
            !fake.has(SESSIONS) && !fake.has(ADOPT) && !fake.has(DROP_IN),
            "{mode:?}"
        );
        assert!(!fake.ran("systemctl start yulius-sandboxes.service"));
    }
}

/// A directory further up counts as well: whoever may rename it may put another binary there.
#[test]
fn a_directory_above_the_binary_counts_too() {
    let mut fake = before_the_sessions_unit();
    fake.files.insert(
        PathBuf::from("/usr/local"),
        (String::new(), SERVICE_UID, 0o040_755),
    );
    let lines = run(&mut fake, Mode::Setup);
    let item = line(&lines, ITEM);
    assert_eq!(item.outcome, HostOutcome::Failed, "{lines:#?}");
    assert!(item.detail.contains("under /usr/local,"), "{}", item.detail);
    assert!(!fake.has(ADOPT) && !fake.has(DROP_IN));
}

/// Units an earlier run wrote do not make an unsafe binary safe: the item fails all the same.
#[test]
fn units_already_there_do_not_excuse_a_replaceable_binary() {
    let mut fake = Fake::set_up();
    fake.files.insert(
        PathBuf::from("/usr/local/bin"),
        (String::new(), SERVICE_UID, 0o040_755),
    );
    let lines = run(&mut fake, Mode::Check);
    assert_eq!(outcome(&lines, ITEM), HostOutcome::Failed, "{lines:#?}");
}

/// A binary that only has the wrong owner is made root's by the setup's own item first, and
/// the units follow in the same run.
#[test]
fn a_binary_made_roots_in_the_same_run_gets_the_units() {
    let mut fake = before_the_sessions_unit();
    fake.files
        .insert(PathBuf::from(EXE), (String::new(), SERVICE_UID, 0o100_755));
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        outcome(&lines, "yulius binary"),
        HostOutcome::Changed,
        "{lines:#?}"
    );
    assert_eq!(outcome(&lines, ITEM), HostOutcome::Changed, "{lines:#?}");
    let position = |command: &str| fake.log.iter().position(|c| c == command);
    let owned = position(&format!("own {EXE}"));
    assert!(
        owned.is_some() && owned < position(&format!("write {ADOPT}")),
        "{:#?}",
        fake.log
    );
}

/// Units that are there but not what this build writes (another build's, or an operator's own):
/// the check must not say that sessions live in the server's group.
#[test]
fn a_check_says_when_the_units_only_differ() {
    let mut fake = Fake::set_up();
    if let Some(unit) = fake.files.get_mut(Path::new(SESSIONS)) {
        unit.0.push_str("# edited\n");
    }
    let lines = run(&mut fake, Mode::Check);
    let item = line(&lines, ITEM);
    assert_eq!(item.outcome, HostOutcome::Failed, "{lines:#?}");
    assert!(
        item.detail.contains(SESSIONS) && item.detail.contains("differs"),
        "{}",
        item.detail
    );
    assert!(!item.detail.contains("own group"), "{}", item.detail);
    assert!(
        item.detail.contains("sudo yulius host setup"),
        "{}",
        item.detail
    );
}

/// The first restart on a host with systemd 258 or later needs the units before the new binary
/// is installed (docs/operations.md §8): `deploy/systemd` holds them as the setup writes them.
#[test]
fn the_deploy_examples_are_what_the_setup_writes() {
    assert_eq!(
        include_str!("../../../../../deploy/systemd/yulius-sandboxes.service"),
        sessions_unit_text("yulius.service", "yulius", "yulius.slice")
    );
    assert_eq!(
        include_str!("../../../../../deploy/systemd/yulius-adopt.service"),
        adopt_unit_text(
            "yulius.service",
            "yulius-sandboxes.service",
            Path::new("/usr/local/bin/yulius")
        )
    );
    assert_eq!(
        include_str!("../../../../../deploy/systemd/yulius.service.d/20-yulius-sessions.conf"),
        drop_in_text(
            "yulius-sandboxes.service",
            "yulius-adopt.service",
            "yulius.slice"
        )
    );
}

/// docs/operations.md §3 shows the same three files, for an operator who writes them by hand.
#[test]
fn the_operations_guide_shows_the_units_the_setup_writes() {
    let guide = include_str!("../../../../../docs/operations.md");
    for text in [
        sessions_unit_text("yulius.service", "yulius", "yulius.slice"),
        adopt_unit_text(
            "yulius.service",
            "yulius-sandboxes.service",
            Path::new("/usr/local/bin/yulius"),
        ),
        drop_in_text(
            "yulius-sandboxes.service",
            "yulius-adopt.service",
            "yulius.slice",
        ),
    ] {
        assert!(guide.contains(&format!("```ini\n{text}```")), "{text}");
    }
}

#[test]
fn another_service_name_gets_its_own_units_and_slice() {
    let mut fake = Fake::fresh();
    let lines = Run::new(
        &mut fake,
        Mode::Setup,
        Options {
            service: "yulius-dev.service".into(),
            ..options()
        },
        &mut |_| {},
    )
    .all();
    assert!(passed(&lines), "{lines:#?}");
    let sessions = fake
        .text("/etc/systemd/system/yulius-dev-sandboxes.service")
        .unwrap_or_default();
    assert!(sessions.contains("Slice=yulius_dev.slice\n"), "{sessions}");
    assert!(fake.has("/etc/systemd/system/yulius-dev-adopt.service"));
    assert!(fake.has("/etc/systemd/system/yulius-dev.service.d/20-yulius-sessions.conf"));
    assert!(!fake.has(SESSIONS));
}

#[test]
fn a_file_that_cannot_be_written_fails_and_starts_nothing() {
    let mut fake = before_the_sessions_unit();
    fake.read_only.insert(Path::new(ADOPT).to_path_buf());
    let lines = run(&mut fake, Mode::Setup);
    let item = line(&lines, ITEM);
    assert_eq!(item.outcome, HostOutcome::Failed, "{lines:#?}");
    assert!(item.detail.contains(ADOPT), "{}", item.detail);
    assert!(!fake.ran("systemctl start yulius-sandboxes.service"));
}

/// LAY-006: a package install must not restart the sessions unit either: that ends every session.
#[test]
fn needrestart_leaves_the_sessions_unit_alone() {
    let rule = super::super::needrestart_rule("yulius.service");
    assert!(rule.contains(r"qr(^yulius\.service$)"), "{rule}");
    assert!(rule.contains(r"qr(^yulius-sandboxes\.service$)"), "{rule}");
}
