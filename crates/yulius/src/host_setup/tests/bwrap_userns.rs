//! The "bwrap namespaces" item against the fake host (SPEC-456): every branch of what a host's
//! AppArmor can look like, with the fake's bwrap behaving as measured on real hosts.

use std::path::{Path, PathBuf};

use yulius_contracts::update::{HostLine, HostOutcome};

use super::super::bwrap_userns::{PROFILE, PROFILE_FILE};
use super::{Fake, Mode, line, passed, run};

const ITEM: &str = "bwrap namespaces";
const ENABLED: &str = "/sys/module/apparmor/parameters/enabled";
const RESTRICTED: &str = "/proc/sys/kernel/apparmor_restrict_unprivileged_userns";
const SHIPPED: &str = "/etc/apparmor.d/bwrap-userns-restrict";
const SHIPPED_LINK: &str = "/etc/apparmor.d/disable/bwrap-userns-restrict";
const HAND_WRITTEN: &str = "/etc/apparmor.d/bwrap";
const WORKS: &str = "bwrap starts a user namespace as yulius";

/// Ubuntu 26.04's `bwrap-userns-restrict` (apparmor 5.0.2), without its comments and most of
/// its rules.
const SHIPPED_TEXT: &str = "\
abi <abi/5.0>,

include <tunables/global>

profile bwrap /usr/bin/bwrap flags=(attach_disconnected,mediate_deleted) {
  allow capability,
  allow userns,
  allow mount,
  allow pix /** -> &bwrap//&unpriv_bwrap,
  include if exists <local/bwrap-userns-restrict>
}

profile unpriv_bwrap flags=(attach_disconnected,mediate_deleted) {
  allow userns,
  allow mount,
  allow pix /** -> &unpriv_bwrap,
  audit deny capability,
  include if exists <local/unpriv_bwrap>
}
";

/// The rule production ran on, written by hand on 2026-10-07.
const HAND_WRITTEN_TEXT: &str = "\
abi <abi/4.0>,
include <tunables/global>
profile bwrap /usr/bin/bwrap flags=(unconfined) {
  userns,
  include if exists <local/bwrap>
}
";

/// Ubuntu 26.04's profile for glycin's loaders: it names bwrap in rules, attaches to nothing.
const GLYCIN_TEXT: &str = "\
abi <abi/4.0>,
profile :glycin:bwrap flags=(attach_disconnected,attach_disconnected.path=@{att}) {
  /{,usr/}bin/bwrap mr,
  /usr/libexec/glycin-loaders/@{d}+/glycin-* Px -> bwrap//&loaders,
}
";

/// Debian: AppArmor on, user namespaces not restricted.
fn debian() -> Fake {
    let mut fake = Fake::fresh();
    fake.file(ENABLED, "Y\n");
    fake
}

/// Ubuntu 24.04: user namespaces need a `userns` rule, bwrap has none.
fn noble() -> Fake {
    let mut fake = debian();
    fake.file(RESTRICTED, "1\n");
    fake
}

/// Ubuntu 26.04: the same switch, and the package's profile for bwrap, loaded.
fn resolute() -> Fake {
    let mut fake = noble();
    fake.file(SHIPPED, SHIPPED_TEXT);
    fake.file("/etc/apparmor.d/glycin.bwrap", GLYCIN_TEXT);
    fake.reboot();
    fake
}

/// The host after a setup, its log cleared.
fn set_up(mut fake: Fake) -> Fake {
    let lines = run(&mut fake, Mode::Setup);
    assert!(passed(&lines), "{lines:#?}");
    fake.log.clear();
    fake
}

fn item(lines: &[HostLine]) -> (HostOutcome, &str) {
    let item = line(lines, ITEM);
    (item.outcome, item.detail.as_str())
}

/// Whether the run touched AppArmor: a write of ours, a link, the parser.
fn touched(fake: &Fake) -> bool {
    fake.ran(&format!("write {PROFILE_FILE}"))
        || fake.ran("ln ")
        || fake.ran("rm ")
        || fake.ran("/usr/sbin/apparmor_parser")
}

/// BNS-001, BNS-003: a fresh Ubuntu 24.04 gets Yulius's profile, loaded, and bwrap is proven
/// before and after.
#[test]
fn a_fresh_noble_host_gets_the_profile() {
    let mut fake = noble();
    assert!(fake.bwrap_proof().is_err(), "a fresh 24.04 denies bwrap");
    let lines = run(&mut fake, Mode::Setup);
    assert!(passed(&lines), "{lines:#?}");
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Changed,
            "wrote and loaded /etc/apparmor.d/yulius-bwrap: bwrap starts a user namespace as \
             yulius"
        )
    );
    let text = fake.text(PROFILE_FILE).unwrap_or_default();
    assert_eq!(text, PROFILE);
    for part in [
        "abi <abi/4.0>,",
        "profile yulius-bwrap /usr/bin/bwrap flags=(unconfined) {",
        "  userns,",
        "  include if exists <local/yulius-bwrap>",
    ] {
        assert!(text.lines().any(|l| l == part), "{part}: {text}");
    }
    assert_eq!(fake.files[Path::new(PROFILE_FILE)].2 & 0o777, 0o644);
    assert!(fake.bwrap_proof().is_ok());
    let proofs = fake
        .log
        .iter()
        .filter(|c| c.starts_with("/usr/sbin/runuser -u yulius -- bwrap"))
        .count();
    assert_eq!(proofs, 2, "{:#?}", fake.log);
    let names: Vec<&str> = lines.iter().map(|l| l.name.as_str()).collect();
    let at = |name: &str| names.iter().position(|n| *n == name);
    assert!(
        at("subordinate ids") < at(ITEM) && at(ITEM) < at("yulius.service"),
        "{names:?}"
    );
}

/// BNS-004: a second run proves and changes nothing.
#[test]
fn a_set_up_host_changes_nothing() {
    let mut fake = set_up(noble());
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Ok,
            "bwrap starts a user namespace as yulius: allowed by /etc/apparmor.d/yulius-bwrap"
        )
    );
    assert!(!touched(&fake), "{:#?}", fake.log);
}

/// BNS-004: AppArmor's unit loads the profile at boot.
#[test]
fn a_rebooted_host_changes_nothing() {
    for host in [noble(), resolute()] {
        let mut fake = set_up(host);
        fake.reboot();
        let lines = run(&mut fake, Mode::Setup);
        assert_eq!(item(&lines).0, HostOutcome::Ok, "{lines:#?}");
        assert!(!touched(&fake), "{:#?}", fake.log);
    }
}

/// BNS-004: a host without AppArmor (none, or switched off) needs no rule.
#[test]
fn no_apparmor_writes_nothing() {
    let mut off = Fake::fresh();
    off.file(ENABLED, "N\n");
    off.file(SHIPPED, SHIPPED_TEXT);
    for mut fake in [Fake::fresh(), off] {
        let lines = run(&mut fake, Mode::Setup);
        assert_eq!(
            item(&lines),
            (
                HostOutcome::Ok,
                "bwrap starts a user namespace as yulius: AppArmor is not active here"
            )
        );
        assert!(!touched(&fake) && !fake.has(PROFILE_FILE));
    }
}

/// BNS-004: Debian's AppArmor does not restrict user namespaces.
#[test]
fn debian_writes_nothing() {
    let mut fake = debian();
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Ok,
            "bwrap starts a user namespace as yulius: AppArmor does not restrict user namespaces \
             here"
        )
    );
    assert!(!touched(&fake) && !fake.has(PROFILE_FILE));
}

/// BNS-003: the file is Yulius's: another text is replaced by this version's.
#[test]
fn a_changed_profile_is_rewritten() {
    let mut fake = set_up(noble());
    fake.file(PROFILE_FILE, &PROFILE.replace("  userns,\n", ""));
    fake.reboot();
    assert!(fake.bwrap_proof().is_err());
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Changed,
            "wrote and loaded /etc/apparmor.d/yulius-bwrap: bwrap starts a user namespace as \
             yulius"
        )
    );
    assert_eq!(fake.text(PROFILE_FILE), Some(PROFILE));
}

/// BNS-003: a current file the kernel does not hold is loaded, not rewritten.
#[test]
fn an_unloaded_profile_is_loaded() {
    let mut fake = set_up(noble());
    fake.profiles.clear();
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Changed,
            "loaded /etc/apparmor.d/yulius-bwrap: bwrap starts a user namespace as yulius"
        )
    );
    assert!(!fake.ran("write /etc/apparmor.d"), "{:#?}", fake.log);
    assert!(fake.bwrap_proof().is_ok());
}

/// BNS-005: Ubuntu 26.04's profile is disabled by AppArmor's link and unloaded, its file left
/// as the package wrote it; ours is written before anything is unloaded.
#[test]
fn a_resolute_host_disables_the_shipped_profile() {
    let mut fake = resolute();
    assert_eq!(
        fake.bwrap_proof(),
        Err("mount: /tmp: permission denied."),
        "a fresh 26.04 starts bwrap and denies its child the mount"
    );
    let lines = run(&mut fake, Mode::Setup);
    assert!(passed(&lines), "{lines:#?}");
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Changed,
            "disabled Ubuntu's AppArmor profile bwrap-userns-restrict (a link in \
             /etc/apparmor.d/disable; its file is untouched), wrote and loaded \
             /etc/apparmor.d/yulius-bwrap: bwrap starts a user namespace as yulius"
        )
    );
    assert_eq!(fake.text(SHIPPED), Some(SHIPPED_TEXT));
    assert_eq!(
        fake.links.get(Path::new(SHIPPED_LINK)),
        Some(&PathBuf::from(SHIPPED))
    );
    let steps: Vec<&str> = fake
        .log
        .iter()
        .map(String::as_str)
        .filter(|c| {
            c.starts_with("write /etc/apparmor.d")
                || c.starts_with("ln ")
                || c.starts_with("/usr/sbin/apparmor_parser")
        })
        .collect();
    assert_eq!(
        steps,
        [
            "write /etc/apparmor.d/yulius-bwrap",
            "ln -s /etc/apparmor.d/bwrap-userns-restrict \
             /etc/apparmor.d/disable/bwrap-userns-restrict",
            "/usr/sbin/apparmor_parser -R /etc/apparmor.d/bwrap-userns-restrict",
            "/usr/sbin/apparmor_parser -r /etc/apparmor.d/yulius-bwrap",
        ]
    );
    assert_eq!(
        fake.profiles.keys().collect::<Vec<_>>(),
        [":glycin:bwrap", "yulius-bwrap"]
    );

    // Second run: nothing.
    fake.log.clear();
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Ok,
            "bwrap starts a user namespace as yulius: allowed by /etc/apparmor.d/yulius-bwrap"
        )
    );
    assert!(!touched(&fake), "{:#?}", fake.log);
}

/// BNS-005: a release upgrade from 24.04 puts the shipped profile next to ours: two profiles
/// for bwrap cancel each other. The check says so, setup disables the shipped one.
#[test]
fn a_release_upgrade_is_reported_and_fixed() {
    let mut fake = set_up(noble());
    fake.file(SHIPPED, SHIPPED_TEXT);
    fake.reboot();
    let lines = run(&mut fake, Mode::Check);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Failed,
            "bwrap cannot start a user namespace as yulius (bwrap: setting up uid map: \
             Permission denied): setup would disable Ubuntu's AppArmor profile \
             bwrap-userns-restrict (a link in /etc/apparmor.d/disable; its file is untouched), \
             load /etc/apparmor.d/yulius-bwrap; run `sudo yulius host setup`"
        )
    );
    assert!(!touched(&fake), "{:#?}", fake.log);
    let lines = run(&mut fake, Mode::Setup);
    let (outcome, detail) = item(&lines);
    assert_eq!(outcome, HostOutcome::Changed);
    assert!(
        detail.starts_with("disabled Ubuntu's AppArmor profile")
            && detail.ends_with(&format!(", loaded /etc/apparmor.d/yulius-bwrap: {WORKS}")),
        "{detail}"
    );
    assert!(!fake.ran("write /etc/apparmor.d"), "{:#?}", fake.log);
}

/// BNS-005, BNS-006: a hand-written `bwrap` profile beside the shipped one of the same name
/// works only while it was loaded last. The shipped one is disabled also when bwrap works at
/// that moment, theirs is loaded again (`-R` removes the name) and ours is not written.
#[test]
fn a_shipped_profile_beside_a_hand_written_one_is_disabled() {
    let mut fake = resolute();
    fake.file(HAND_WRITTEN, HAND_WRITTEN_TEXT);
    assert!(fake.load_profiles(Path::new(HAND_WRITTEN)));
    assert!(fake.bwrap_proof().is_ok(), "loaded last, theirs wins");
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Changed,
            "disabled Ubuntu's AppArmor profile bwrap-userns-restrict (a link in \
             /etc/apparmor.d/disable; its file is untouched), loaded /etc/apparmor.d/bwrap \
             again: bwrap starts a user namespace as yulius"
        )
    );
    assert!(!fake.has(PROFILE_FILE));
    assert_eq!(fake.text(HAND_WRITTEN), Some(HAND_WRITTEN_TEXT));
    assert_eq!(fake.text(SHIPPED), Some(SHIPPED_TEXT));
    // Whatever order a boot loads them in, theirs is the only one now.
    fake.reboot();
    fake.log.clear();
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(item(&lines).0, HostOutcome::Ok, "{lines:#?}");
    assert!(!touched(&fake), "{:#?}", fake.log);
}

/// BNS-006: production's hand-written rule is adopted: named, not changed, not joined.
#[test]
fn a_hand_written_rule_is_adopted() {
    let mut fake = noble();
    fake.file(HAND_WRITTEN, HAND_WRITTEN_TEXT);
    fake.reboot();
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Ok,
            "bwrap starts a user namespace as yulius: allowed by /etc/apparmor.d/bwrap (not \
             written by Yulius, left as it is)"
        )
    );
    assert!(
        !touched(&fake) && !fake.has(PROFILE_FILE),
        "{:#?}",
        fake.log
    );
    assert_eq!(fake.text(HAND_WRITTEN), Some(HAND_WRITTEN_TEXT));

    // Written but never loaded: loaded once, as a boot would.
    let mut fake = noble();
    fake.file(HAND_WRITTEN, HAND_WRITTEN_TEXT);
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Changed,
            "loaded /etc/apparmor.d/bwrap again: bwrap starts a user namespace as yulius"
        )
    );
    assert!(!fake.has(PROFILE_FILE));
}

/// BNS-006: a person's profile that keeps bwrap from working is theirs to remove: the item
/// fails with the commands, and works once they ran them.
#[test]
fn a_hand_written_rule_that_fails_is_named_not_touched() {
    let confined = HAND_WRITTEN_TEXT.replace(" flags=(unconfined)", "");
    let mut fake = noble();
    fake.file(HAND_WRITTEN, &confined);
    fake.reboot();
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Failed,
            "bwrap cannot start a user namespace as yulius with /etc/apparmor.d/bwrap (mount: \
             /tmp: permission denied.); Yulius does not change a profile it did not write: \
             disable it (`sudo ln -s /etc/apparmor.d/bwrap /etc/apparmor.d/disable/ && sudo \
             /usr/sbin/apparmor_parser -R /etc/apparmor.d/bwrap`), then run `sudo yulius host \
             setup` again"
        )
    );
    assert_eq!(fake.text(HAND_WRITTEN), Some(confined.as_str()));
    assert!(!fake.has(PROFILE_FILE) && fake.links.is_empty());
    let loads = fake
        .log
        .iter()
        .filter(|c| c.starts_with("/usr/sbin/apparmor_parser"))
        .collect::<Vec<_>>();
    assert_eq!(
        loads,
        ["/usr/sbin/apparmor_parser -r /etc/apparmor.d/bwrap"]
    );

    // The printed commands, then setup again.
    fake.links.insert(
        PathBuf::from("/etc/apparmor.d/disable/bwrap"),
        PathBuf::from(HAND_WRITTEN),
    );
    fake.profiles.remove("bwrap");
    let lines = run(&mut fake, Mode::Setup);
    let (outcome, detail) = item(&lines);
    assert_eq!(outcome, HostOutcome::Changed, "{detail}");
    assert!(detail.starts_with("wrote and loaded"), "{detail}");
    assert_eq!(fake.text(HAND_WRITTEN), Some(confined.as_str()));
}

/// BNS-007: an AppArmor that restricts user namespaces and cannot read the rule is unsupported;
/// the file would fail AppArmor's unit at every boot, so it is removed again.
#[test]
fn a_parser_that_cannot_read_the_rule_is_unsupported() {
    let mut fake = noble();
    fake.parser_fails = true;
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Failed,
            "unsupported: this host's AppArmor cannot load Yulius's profile (AppArmor parser \
             error for /etc/apparmor.d/yulius-bwrap in profile /etc/apparmor.d/yulius-bwrap at \
             line 3: Could not open 'abi/4.0': No such file or directory); \
             /etc/apparmor.d/yulius-bwrap was removed again"
        )
    );
    assert!(!fake.has(PROFILE_FILE));
    assert!(!passed(&lines));
}

/// BNS-007: what no AppArmor rule fixes is named: a namespace limit of zero, a link that keeps
/// Yulius's profile from loading.
#[test]
fn a_namespace_limit_is_named() {
    let mut fake = Fake::fresh();
    fake.file("/proc/sys/user/max_user_namespaces", "0\n");
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Failed,
            "bwrap cannot start a user namespace as yulius: bwrap: Creating new namespace \
             failed: No space left on device; user.max_user_namespaces is 0 (no user namespaces \
             at all: raise it in /etc/sysctl.d)"
        )
    );
    assert!(!touched(&fake));

    let mut fake = noble();
    fake.file("/proc/sys/kernel/unprivileged_userns_clone", "0\n");
    fake.file("/proc/sys/user/max_user_namespaces", "0\n");
    let lines = run(&mut fake, Mode::Setup);
    let (outcome, detail) = item(&lines);
    assert_eq!(outcome, HostOutcome::Failed);
    assert!(
        detail.starts_with(
            "wrote and loaded /etc/apparmor.d/yulius-bwrap, but bwrap still cannot start a user \
             namespace as yulius: bwrap: Creating new namespace failed"
        ) && detail.contains("; kernel.unprivileged_userns_clone is 0 (")
            && detail.contains("; user.max_user_namespaces is 0 ("),
        "{detail}"
    );

    let mut fake = set_up(noble());
    fake.links.insert(
        PathBuf::from("/etc/apparmor.d/disable/yulius-bwrap"),
        PathBuf::from(PROFILE_FILE),
    );
    fake.reboot();
    for mode in [Mode::Setup, Mode::Check] {
        let lines = run(&mut fake, mode);
        assert_eq!(
            item(&lines),
            (
                HostOutcome::Failed,
                "bwrap cannot start a user namespace as yulius: bwrap: setting up uid map: \
                 Permission denied; /etc/apparmor.d/disable/yulius-bwrap keeps Yulius's profile \
                 from loading (remove that link, then run `sudo yulius host setup` again)"
            )
        );
    }
    assert!(!touched(&fake), "{:#?}", fake.log);
}

/// BNS-007: a failed write is named once and leaves the host as it was (nothing unloaded).
#[test]
fn a_failed_write_is_named() {
    for mut fake in [noble(), resolute()] {
        fake.read_only.insert(PathBuf::from(PROFILE_FILE));
        let lines = run(&mut fake, Mode::Setup);
        assert_eq!(
            item(&lines),
            (
                HostOutcome::Failed,
                "cannot write /etc/apparmor.d/yulius-bwrap: Read-only file system (os error 30)"
            )
        );
        assert!(
            !fake.ran("ln ") && !fake.ran("/usr/sbin/apparmor_parser"),
            "{:#?}",
            fake.log
        );
    }
}

/// BNS-008: `host check` (the update helper's re-check) proves, reports what setup would do
/// as a failure that names setup, and changes nothing; after setup it is ok.
#[test]
fn a_check_reports_and_changes_nothing() {
    for (mut fake, expected) in [
        (
            noble(),
            "bwrap cannot start a user namespace as yulius (bwrap: setting up uid map: \
             Permission denied): setup would write and load /etc/apparmor.d/yulius-bwrap; run \
             `sudo yulius host setup`",
        ),
        (
            resolute(),
            "bwrap cannot start a user namespace as yulius (mount: /tmp: permission denied.): \
             setup would disable Ubuntu's AppArmor profile bwrap-userns-restrict (a link in \
             /etc/apparmor.d/disable; its file is untouched), write and load \
             /etc/apparmor.d/yulius-bwrap; run `sudo yulius host setup`",
        ),
    ] {
        // A host that was set up before its AppArmor restricted anything.
        let switches: Vec<_> = [ENABLED, RESTRICTED]
            .into_iter()
            .filter_map(|path| fake.files.remove_entry(Path::new(path)))
            .collect();
        assert_eq!(switches.len(), 2);
        let mut fake = set_up(fake);
        fake.files.extend(switches);
        let before = (
            fake.files.clone(),
            fake.links.clone(),
            fake.profiles.clone(),
        );
        let lines = run(&mut fake, Mode::Check);
        assert_eq!(item(&lines), (HostOutcome::Failed, expected));
        assert!(!touched(&fake), "{:#?}", fake.log);
        assert_eq!(
            (
                fake.files.clone(),
                fake.links.clone(),
                fake.profiles.clone()
            ),
            before
        );
        assert!(
            fake.ran("/usr/sbin/runuser -u yulius -- bwrap"),
            "proven by a start, not by files"
        );

        let mut fake = set_up(fake);
        let lines = run(&mut fake, Mode::Check);
        assert_eq!(item(&lines).0, HostOutcome::Ok, "{lines:#?}");
        assert!(!touched(&fake), "{:#?}", fake.log);
    }
}

/// BNS-002, BNS-008: a dry run says what setup would do and changes nothing. On a host
/// without the service user nothing can be started: the files decide, and the line says so.
#[test]
fn a_dry_run_says_what_it_would_do() {
    for (mut fake, expected) in [
        (noble(), "would write and load /etc/apparmor.d/yulius-bwrap"),
        (
            resolute(),
            "would disable Ubuntu's AppArmor profile bwrap-userns-restrict (a link in \
             /etc/apparmor.d/disable; its file is untouched), write and load \
             /etc/apparmor.d/yulius-bwrap",
        ),
    ] {
        let before = (
            fake.files.clone(),
            fake.links.clone(),
            fake.profiles.clone(),
        );
        let lines = run(&mut fake, Mode::DryRun);
        assert_eq!(item(&lines), (HostOutcome::Changed, expected));
        assert!(!touched(&fake) && !fake.ran("/usr/sbin/runuser"));
        assert_eq!(
            (
                fake.files.clone(),
                fake.links.clone(),
                fake.profiles.clone()
            ),
            before
        );
    }
    let mut fake = debian();
    let lines = run(&mut fake, Mode::DryRun);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Ok,
            "AppArmor does not restrict user namespaces here; not proven: the service user does \
             not exist yet"
        )
    );
    // The user exists, bubblewrap does not: still nothing to start.
    let mut fake = set_up(noble());
    fake.packages.remove("bubblewrap");
    fake.profiles.clear();
    let lines = run(&mut fake, Mode::DryRun);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Ok,
            "allowed by /etc/apparmor.d/yulius-bwrap; not proven: bubblewrap is not installed yet"
        )
    );
}
