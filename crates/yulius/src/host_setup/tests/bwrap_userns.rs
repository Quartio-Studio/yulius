//! The "bwrap namespaces" item against the fake host (SPEC-456): every branch of what a host's
//! AppArmor can look like, with the fake's bwrap behaving as measured on real hosts.

use std::path::{Path, PathBuf};

use yulius_contracts::update::{HostLine, HostOutcome};

use super::super::bwrap_userns::{PROFILE, PROFILE_FILE};
use super::{Fake, Mode, Run, lacks_namespaces, line, options, passed, profiles_in, run};

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

/// The same rule as a person may also write it: on one line.
const ONE_LINE_TEXT: &str = "profile bwrap /usr/bin/bwrap flags=(unconfined) { userns, }\n";

/// The same rule with its path spelled as AppArmor also takes it and Yulius's reading does not
/// know it (the alternation of Ubuntu's own `alsamixer` profile).
const UNREAD_TEXT: &str = "\
abi <abi/4.0>,
include <tunables/global>
profile theirs /{usr,}/bin/bwrap flags=(unconfined) {
  userns,
}
";

/// A person's profile for every program of a directory, bwrap among them: no `userns`.
const CATCH_ALL_TEXT: &str = "\
abi <abi/4.0>,
profile every /usr/bin/* {
  /** r,
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

/// What the item may change: AppArmor's files and links, and the profiles it holds.
fn state(fake: &Fake) -> impl PartialEq + use<> {
    let dir = Path::new("/etc/apparmor.d");
    let files: Vec<_> = fake.files.iter().filter(|f| f.0.starts_with(dir)).collect();
    let links: Vec<_> = fake.links.iter().filter(|l| l.0.starts_with(dir)).collect();
    (
        format!("{files:?}"),
        format!("{links:?}"),
        fake.profiles.clone(),
    )
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
            "wrote and loaded /etc/apparmor.d/yulius-bwrap, loaded the other profiles of /etc/apparmor.d as a boot does: bwrap starts a user namespace as yulius"
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
            "wrote and loaded /etc/apparmor.d/yulius-bwrap, loaded the other profiles of /etc/apparmor.d as a boot does: bwrap starts a user namespace as yulius"
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
            "loaded /etc/apparmor.d/yulius-bwrap, loaded the other profiles of /etc/apparmor.d \
             as a boot does: bwrap starts a user namespace as yulius"
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
            "disabled Ubuntu's AppArmor profile bwrap-userns-restrict (a link in /etc/apparmor.d/disable; its file is untouched), wrote and loaded /etc/apparmor.d/yulius-bwrap, loaded the other profiles of /etc/apparmor.d as a boot does: bwrap starts a user namespace as yulius"
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
            "/usr/sbin/apparmor_parser -r /etc/apparmor.d",
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
            "bwrap cannot start a user namespace as yulius (bwrap: setting up uid map: Permission denied): setup would disable Ubuntu's AppArmor profile bwrap-userns-restrict (a link in /etc/apparmor.d/disable; its file is untouched), load /etc/apparmor.d/yulius-bwrap, load the other profiles of /etc/apparmor.d as a boot does; run `sudo yulius host setup`"
        )
    );
    assert!(!touched(&fake), "{:#?}", fake.log);
    let lines = run(&mut fake, Mode::Setup);
    let (outcome, detail) = item(&lines);
    assert_eq!(outcome, HostOutcome::Changed);
    assert!(
        detail.starts_with("disabled Ubuntu's AppArmor profile")
            && detail.ends_with(&format!(", loaded /etc/apparmor.d/yulius-bwrap, loaded the other profiles of /etc/apparmor.d as a boot does: {WORKS}")),
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

    // Written but never loaded: loaded once, as a boot would. However it is spelled, it is
    // theirs and ours does not join it.
    for text in [HAND_WRITTEN_TEXT, ONE_LINE_TEXT] {
        let mut fake = noble();
        fake.file(HAND_WRITTEN, text);
        let lines = run(&mut fake, Mode::Setup);
        assert_eq!(
            item(&lines),
            (
                HostOutcome::Changed,
                "loaded /etc/apparmor.d/bwrap again: bwrap starts a user namespace as yulius"
            ),
            "{text}"
        );
        assert!(!fake.has(PROFILE_FILE), "{text}");
    }
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
            format!(
                "loaded /etc/apparmor.d/bwrap again, but bwrap cannot start a user namespace as \
                 yulius with /etc/apparmor.d/bwrap (mount: /tmp: permission denied.); {THEIRS}"
            )
            .as_str()
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

    // Beside the shipped profile: the line says what was done before it says what is left.
    let mut fake = resolute();
    fake.file(HAND_WRITTEN, &confined);
    fake.reboot();
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Failed,
            format!(
                "disabled Ubuntu's AppArmor profile bwrap-userns-restrict (a link in \
                 /etc/apparmor.d/disable; its file is untouched), loaded /etc/apparmor.d/bwrap \
                 again, but bwrap cannot start a user namespace as yulius with \
                 /etc/apparmor.d/bwrap (mount: /tmp: permission denied.); {THEIRS}"
            )
            .as_str()
        )
    );
    assert!(!fake.has(PROFILE_FILE));

    // A file of theirs the parser refuses cannot be unloaded by its file either: the line asks
    // for a correct file, and the shipped profile is back.
    for (mut fake, back) in [
        (noble(), ""),
        (
            resolute(),
            "; Ubuntu's AppArmor profile bwrap-userns-restrict is enabled again",
        ),
    ] {
        fake.file(HAND_WRITTEN, &confined);
        fake.failing.push((
            "/usr/sbin/apparmor_parser -r /etc/apparmor.d/bwrap",
            Some(1),
            "AppArmor parser error for /etc/apparmor.d/bwrap at line 3: syntax error",
        ));
        let before = state(&fake);
        let lines = run(&mut fake, Mode::Setup);
        assert_eq!(
            item(&lines),
            (
                HostOutcome::Failed,
                format!(
                    "`/usr/sbin/apparmor_parser -r /etc/apparmor.d/bwrap` failed: AppArmor \
                     parser error for /etc/apparmor.d/bwrap at line 3: syntax error; Yulius \
                     does not change a profile it did not write: correct \
                     /etc/apparmor.d/bwrap, then run `sudo yulius host setup` again{back}"
                )
                .as_str()
            )
        );
        assert!(state(&fake) == before, "{:#?}", fake.log);
    }
}

/// What a person does about a profile of theirs.
const THEIRS: &str = "Yulius does not change a profile it did not write: disable it (`sudo ln -s \
                      /etc/apparmor.d/bwrap /etc/apparmor.d/disable/ && sudo \
                      /usr/sbin/apparmor_parser -R /etc/apparmor.d/bwrap`), then run `sudo \
                      yulius host setup` again";

/// BNS-012: a person's profile for bwrap in a spelling Yulius does not read is found by what
/// it does. Written and never loaded, a boot would load it beside Yulius's and AppArmor would
/// apply neither: setup loads the directory as a boot does, takes its own back and adopts
/// theirs.
#[test]
fn an_unread_rule_is_found_by_what_it_does() {
    let mut fake = noble();
    fake.file(HAND_WRITTEN, UNREAD_TEXT);
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Changed,
            "loaded the profiles of /etc/apparmor.d as a boot does: bwrap starts a user \
             namespace as yulius with a profile there that Yulius did not write; Yulius's own, \
             which AppArmor does not apply beside it, was taken back"
        )
    );
    assert!(passed(&lines), "{lines:#?}");
    assert!(!fake.has(PROFILE_FILE), "{:#?}", fake.log);
    assert_eq!(fake.profiles.keys().collect::<Vec<_>>(), ["theirs"]);
    assert_eq!(fake.text(HAND_WRITTEN), Some(UNREAD_TEXT));

    // A boot later, and on every run after: theirs works, nothing is written.
    fake.reboot();
    fake.log.clear();
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Ok,
            "bwrap starts a user namespace as yulius: allowed by a rule Yulius did not write"
        )
    );
    assert!(!touched(&fake) && !fake.has(PROFILE_FILE));

    // On Ubuntu 26.04 the shipped profile stays disabled, as beside a rule Yulius reads.
    let mut fake = resolute();
    fake.file(HAND_WRITTEN, UNREAD_TEXT);
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Changed,
            "disabled Ubuntu's AppArmor profile bwrap-userns-restrict (a link in \
             /etc/apparmor.d/disable; its file is untouched), loaded the profiles of \
             /etc/apparmor.d as a boot does: bwrap starts a user namespace as yulius with a \
             profile there that Yulius did not write; Yulius's own, which AppArmor does not \
             apply beside it, was taken back"
        )
    );
    assert!(!fake.has(PROFILE_FILE) && fake.links.contains_key(Path::new(SHIPPED_LINK)));
    assert_eq!(
        fake.profiles.keys().collect::<Vec<_>>(),
        [":glycin:bwrap", "theirs"]
    );
}

/// BNS-012: where bwrap does not start with such a profile either, the host is left as it was:
/// Yulius's profile is not left beside it, and the line says what a person does.
#[test]
fn an_unread_rule_that_fails_gets_no_second_profile() {
    let confined = UNREAD_TEXT.replace(" flags=(unconfined)", "");
    let rival = "where another profile of /etc/apparmor.d is for /usr/bin/bwrap, AppArmor \
                 applies neither it nor Yulius's (the kernel's log says \"conflicting profile \
                 attachments\"): disable that profile (`sudo ln -s <its file> \
                 /etc/apparmor.d/disable/ && sudo /usr/sbin/apparmor_parser -R <its file>`), \
                 then run `sudo yulius host setup` again";
    let mut fake = noble();
    fake.file(HAND_WRITTEN, &confined);
    fake.reboot();
    let before = state(&fake);
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Failed,
            format!(
                "wrote and loaded /etc/apparmor.d/yulius-bwrap, loaded the other profiles of \
                 /etc/apparmor.d as a boot does, but bwrap still cannot start a user namespace \
                 as yulius: bwrap: setting up uid map: Permission denied; \
                 /etc/apparmor.d/yulius-bwrap was removed again; {rival}"
            )
            .as_str()
        )
    );
    assert!(state(&fake) == before, "{:#?}", fake.log);
    assert!(!fake.has(PROFILE_FILE));

    // An earlier version of Yulius's file gets its text back, and AppArmor holds that again.
    let earlier = PROFILE.replace("  userns,\n", "");
    let mut fake = noble();
    fake.file(HAND_WRITTEN, &confined);
    fake.file(PROFILE_FILE, &earlier);
    fake.reboot();
    let before = state(&fake);
    let lines = run(&mut fake, Mode::Setup);
    let (outcome, detail) = item(&lines);
    assert_eq!(outcome, HostOutcome::Failed);
    assert!(
        detail.contains("; /etc/apparmor.d/yulius-bwrap has its earlier text again; where"),
        "{detail}"
    );
    assert_eq!(fake.text(PROFILE_FILE), Some(earlier.as_str()));
    assert!(state(&fake) == before, "{:#?}", fake.log);

    // On Ubuntu 26.04 the shipped profile is enabled again too.
    let mut fake = resolute();
    fake.file(HAND_WRITTEN, &confined);
    fake.reboot();
    let before = state(&fake);
    let lines = run(&mut fake, Mode::Setup);
    let (outcome, detail) = item(&lines);
    assert_eq!(outcome, HostOutcome::Failed);
    assert!(
        detail.contains(
            "; /etc/apparmor.d/yulius-bwrap was removed again; Ubuntu's AppArmor profile \
             bwrap-userns-restrict is enabled again; where another profile"
        ),
        "{detail}"
    );
    assert!(state(&fake) == before, "{:#?}", fake.log);

    // A kernel limit is no such profile: Yulius's stays, to work once the limit is lifted.
    let mut fake = noble();
    fake.file("/proc/sys/user/max_user_namespaces", "0\n");
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(item(&lines).0, HostOutcome::Failed);
    assert!(fake.has(PROFILE_FILE) && fake.profiles.contains_key("yulius-bwrap"));

    // A profile an earlier run wrote (before Yulius proved beside the whole directory) stays
    // too: the line says how either of the two goes.
    let mut fake = set_up(noble());
    fake.file(HAND_WRITTEN, UNREAD_TEXT);
    fake.reboot();
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Failed,
            "loaded /etc/apparmor.d/yulius-bwrap, loaded the other profiles of /etc/apparmor.d \
             as a boot does, but bwrap still cannot start a user namespace as yulius: bwrap: \
             setting up uid map: Permission denied; where another profile of /etc/apparmor.d is \
             for /usr/bin/bwrap, AppArmor applies neither it nor Yulius's (the kernel's log says \
             \"conflicting profile attachments\"): disable that profile (`sudo ln -s <its file> \
             /etc/apparmor.d/disable/ && sudo /usr/sbin/apparmor_parser -R <its file>`) or take \
             Yulius's away (`sudo /usr/sbin/apparmor_parser -R /etc/apparmor.d/yulius-bwrap && \
             sudo rm /etc/apparmor.d/yulius-bwrap`), then run `sudo yulius host setup` again"
        )
    );
    assert_eq!(fake.text(PROFILE_FILE), Some(PROFILE));
    // Yulius's taken away as the line says: theirs is adopted on the next run.
    fake.profiles.remove("yulius-bwrap");
    fake.files.remove(Path::new(PROFILE_FILE));
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Ok,
            "bwrap starts a user namespace as yulius: allowed by a rule Yulius did not write"
        )
    );
}

/// BNS-012: a person's profile for many programs gives way to one that names bwrap: Yulius's
/// is written beside it, proven with both loaded, and stays.
#[test]
fn a_catch_all_profile_gives_way() {
    let mut fake = noble();
    fake.file("/etc/apparmor.d/every", CATCH_ALL_TEXT);
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Changed,
            "wrote and loaded /etc/apparmor.d/yulius-bwrap, loaded the other profiles of \
             /etc/apparmor.d as a boot does: bwrap starts a user namespace as yulius"
        )
    );
    assert_eq!(
        fake.profiles.keys().collect::<Vec<_>>(),
        ["every", "yulius-bwrap"]
    );
    fake.reboot();
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(item(&lines).0, HostOutcome::Ok);
}

/// BNS-007: a step that fails midway is undone and named: the host is as it was.
#[test]
fn a_failed_step_is_undone_and_named() {
    const LOAD: &str = "/usr/sbin/apparmor_parser -r /etc/apparmor.d/yulius-bwrap";
    const UNLOAD: &str = "/usr/sbin/apparmor_parser -R /etc/apparmor.d/bwrap-userns-restrict";
    const REMOVED: &str = "; /etc/apparmor.d/yulius-bwrap was removed again";
    const BACK: &str = "; Ubuntu's AppArmor profile bwrap-userns-restrict is enabled again";
    for (failing, says, expected) in [
        (
            "ln -s",
            "ln: failed to create symbolic link: Read-only file system",
            format!(
                "`ln -s {SHIPPED} {SHIPPED_LINK}` failed: ln: failed to create symbolic link: \
                 Read-only file system{REMOVED}"
            ),
        ),
        (
            UNLOAD,
            "apparmor_parser: Unable to remove \"bwrap\".  Permission denied",
            format!(
                "`{UNLOAD}` failed: apparmor_parser: Unable to remove \"bwrap\".  Permission \
                 denied{REMOVED}{BACK}"
            ),
        ),
        (
            LOAD,
            "Could not open 'abi/4.0'",
            format!(
                "unsupported: this host's AppArmor cannot load Yulius's profile (Could not open \
                 'abi/4.0'){REMOVED}{BACK}"
            ),
        ),
    ] {
        let mut fake = resolute();
        fake.failing.push((failing, Some(1), says));
        let before = state(&fake);
        let lines = run(&mut fake, Mode::Setup);
        assert_eq!(item(&lines), (HostOutcome::Failed, expected.as_str()));
        assert!(state(&fake) == before, "{failing}: {:#?}", fake.log);
    }

    // What cannot be undone is named as well.
    let mut fake = resolute();
    fake.failing
        .push((LOAD, Some(1), "Could not open 'abi/4.0'"));
    fake.failing.push((
        "rm -f /etc/apparmor.d/disable/bwrap-userns-restrict",
        Some(1),
        "rm: cannot remove: Read-only file system",
    ));
    let lines = run(&mut fake, Mode::Setup);
    let (outcome, detail) = item(&lines);
    assert_eq!(outcome, HostOutcome::Failed);
    assert!(
        detail.ends_with(&format!(
            "{REMOVED}; Ubuntu's AppArmor profile bwrap-userns-restrict stays disabled by \
             {SHIPPED_LINK}"
        )),
        "{detail}"
    );

    // A profile of Yulius's that was there before the run (an older text, loaded and working)
    // gets its text back: removing it would leave the next boot without a rule.
    let older = PROFILE.replace("# Written by", "# Once written by");
    let mut fake = noble();
    fake.file(PROFILE_FILE, &older);
    fake.reboot();
    fake.failing
        .push((LOAD, Some(1), "Could not open 'abi/4.0'"));
    let before = state(&fake);
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Failed,
            "unsupported: this host's AppArmor cannot load Yulius's profile (Could not open \
             'abi/4.0'); /etc/apparmor.d/yulius-bwrap has its earlier text again"
        )
    );
    assert_eq!(fake.text(PROFILE_FILE), Some(older.as_str()));
    assert!(state(&fake) == before, "{:#?}", fake.log);
}

/// BNS-005: a link alone does not unload the shipped profile (a person's link, an unload that
/// failed): where bwrap fails beside it, setup unloads it. The parser's exit 254 says it was
/// not loaded, which is no failure.
#[test]
fn a_disabled_shipped_profile_that_is_still_loaded_is_unloaded() {
    let mut fake = set_up(resolute());
    fake.profiles.extend(profiles_in(SHIPPED_TEXT));
    let lines = run(&mut fake, Mode::Check);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Failed,
            "bwrap cannot start a user namespace as yulius (bwrap: setting up uid map: Permission denied): setup would make sure Ubuntu's disabled AppArmor profile bwrap-userns-restrict is not loaded, load /etc/apparmor.d/yulius-bwrap, load the other profiles of /etc/apparmor.d as a boot does; run `sudo yulius host setup`"
        )
    );
    assert!(!touched(&fake), "{:#?}", fake.log);
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Changed,
            "made sure Ubuntu's disabled AppArmor profile bwrap-userns-restrict is not loaded, loaded /etc/apparmor.d/yulius-bwrap, loaded the other profiles of /etc/apparmor.d as a boot does: bwrap starts a user namespace as yulius"
        )
    );
    assert!(!fake.ran("ln ") && !fake.ran("write /etc/apparmor.d"));
    assert_eq!(
        fake.profiles.keys().collect::<Vec<_>>(),
        [":glycin:bwrap", "yulius-bwrap"]
    );

    let mut fake = noble();
    fake.file(SHIPPED, SHIPPED_TEXT);
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(item(&lines).0, HostOutcome::Changed, "{lines:#?}");
    assert!(fake.ran("/usr/sbin/apparmor_parser -R /etc/apparmor.d/bwrap-userns-restrict"));
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

/// BNS-007, BNS-008: what no AppArmor rule fixes is named, by setup, a check and a dry run: a
/// namespace limit of zero, a link that keeps Yulius's profile from loading.
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
    // The limits say it all: no word of another profile, and Yulius's stays.
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Failed,
            "wrote and loaded /etc/apparmor.d/yulius-bwrap, loaded the other profiles of \
             /etc/apparmor.d as a boot does, but bwrap still cannot start a user namespace as \
             yulius: bwrap: Creating new namespace failed: No space left on device; \
             kernel.unprivileged_userns_clone is 0 (unprivileged user namespaces are off: set \
             it to 1 in /etc/sysctl.d); user.max_user_namespaces is 0 (no user namespaces at \
             all: raise it in /etc/sysctl.d)"
        )
    );
    assert_eq!(fake.text(PROFILE_FILE), Some(PROFILE));

    // A person's profile is not blamed for a limit: no commands that would disable a good rule.
    let mut fake = noble();
    fake.file(HAND_WRITTEN, HAND_WRITTEN_TEXT);
    fake.reboot();
    fake.file("/proc/sys/user/max_user_namespaces", "0\n");
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Failed,
            "loaded /etc/apparmor.d/bwrap again, but bwrap still cannot start a user namespace \
             as yulius: bwrap: Creating new namespace failed: No space left on device; \
             user.max_user_namespaces is 0 (no user namespaces at all: raise it in \
             /etc/sysctl.d)"
        )
    );
    assert_eq!(fake.text(HAND_WRITTEN), Some(HAND_WRITTEN_TEXT));

    // Nor is a link that disables Yulius's profile: beside theirs it is never loaded.
    fake.links.insert(
        PathBuf::from("/etc/apparmor.d/disable/yulius-bwrap"),
        PathBuf::from(PROFILE_FILE),
    );
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Failed,
            "loaded /etc/apparmor.d/bwrap again, but bwrap still cannot start a user namespace \
             as yulius: bwrap: Creating new namespace failed: No space left on device; \
             user.max_user_namespaces is 0 (no user namespaces at all: raise it in \
             /etc/sysctl.d)"
        )
    );

    // A check and a dry run name every limit too, each as its own clause: setup's steps alone
    // would not let bwrap in.
    let mut fake = noble();
    let switches: Vec<_> = [ENABLED, RESTRICTED]
        .into_iter()
        .filter_map(|path| fake.files.remove_entry(Path::new(path)))
        .collect();
    let mut fake = set_up(fake);
    fake.files.extend(switches);
    fake.file("/proc/sys/user/max_user_namespaces", "0\n");
    let lines = run(&mut fake, Mode::Check);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Failed,
            "bwrap cannot start a user namespace as yulius (bwrap: Creating new namespace failed: No space left on device); user.max_user_namespaces is 0 (no user namespaces at all: raise it in /etc/sysctl.d); setup would write and load /etc/apparmor.d/yulius-bwrap, load the other profiles of /etc/apparmor.d as a boot does; run `sudo yulius host setup`"
        )
    );
    let lines = run(&mut fake, Mode::DryRun);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Changed,
            "would write and load /etc/apparmor.d/yulius-bwrap, load the other profiles of /etc/apparmor.d as a boot does; user.max_user_namespaces is 0 (no user namespaces at all: raise it in /etc/sysctl.d)"
        )
    );
    fake.file("/proc/sys/kernel/unprivileged_userns_clone", "0\n");
    let lines = run(&mut fake, Mode::Check);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Failed,
            "bwrap cannot start a user namespace as yulius (bwrap: Creating new namespace failed: No space left on device); kernel.unprivileged_userns_clone is 0 (unprivileged user namespaces are off: set it to 1 in /etc/sysctl.d); user.max_user_namespaces is 0 (no user namespaces at all: raise it in /etc/sysctl.d); setup would write and load /etc/apparmor.d/yulius-bwrap, load the other profiles of /etc/apparmor.d as a boot does; run `sudo yulius host setup`"
        )
    );
    assert!(!touched(&fake));

    // Where bwrap starts all the same (a setuid bwrap), the switch keeps nothing out: not named.
    let mut fake = set_up(noble());
    fake.file(PROFILE_FILE, "# an earlier version\n");
    fake.file("/proc/sys/kernel/unprivileged_userns_clone", "0\n");
    for (mode, outcome, expected) in [
        (
            Mode::Check,
            HostOutcome::Failed,
            "the AppArmor rules for bwrap are not what setup leaves: setup would write and load /etc/apparmor.d/yulius-bwrap, load the other profiles of /etc/apparmor.d as a boot does; run `sudo yulius host setup`",
        ),
        (
            Mode::DryRun,
            HostOutcome::Changed,
            "would write and load /etc/apparmor.d/yulius-bwrap, load the other profiles of /etc/apparmor.d as a boot does",
        ),
    ] {
        let lines = run(&mut fake, mode);
        assert_eq!(item(&lines), (outcome, expected));
    }
    assert!(!touched(&fake));

    // Nor is the limit left out where nothing can be started yet.
    let mut fake = noble();
    fake.file("/proc/sys/user/max_user_namespaces", "0\n");
    let lines = run(&mut fake, Mode::DryRun);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Changed,
            "would write and load /etc/apparmor.d/yulius-bwrap, load the other profiles of /etc/apparmor.d as a boot does; user.max_user_namespaces is 0 (no user namespaces at all: raise it in /etc/sysctl.d)"
        )
    );
    let lines = run(&mut fake, Mode::Check);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Failed,
            "the AppArmor rules for bwrap are not what setup leaves; user.max_user_namespaces is 0 (no user namespaces at all: raise it in /etc/sysctl.d); setup would write and load /etc/apparmor.d/yulius-bwrap, load the other profiles of /etc/apparmor.d as a boot does; run `sudo yulius host setup`"
        )
    );
    assert!(!touched(&fake) && !fake.ran("/usr/sbin/runuser"));

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

    // Beside an enabled shipped profile too: disabling that one for a profile that cannot load
    // would only change how bwrap fails.
    let mut fake = set_up(resolute());
    fake.links.remove(Path::new(SHIPPED_LINK));
    fake.links.insert(
        PathBuf::from("/etc/apparmor.d/disable/yulius-bwrap"),
        PathBuf::from(PROFILE_FILE),
    );
    fake.reboot();
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Failed,
            "bwrap cannot start a user namespace as yulius: mount: /tmp: permission denied.; \
             /etc/apparmor.d/disable/yulius-bwrap keeps Yulius's profile from loading (remove \
             that link, then run `sudo yulius host setup` again)"
        )
    );
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
            "bwrap cannot start a user namespace as yulius (bwrap: setting up uid map: Permission denied): setup would write and load /etc/apparmor.d/yulius-bwrap, load the other profiles of /etc/apparmor.d as a boot does; run `sudo yulius host setup`",
        ),
        (
            resolute(),
            "bwrap cannot start a user namespace as yulius (mount: /tmp: permission denied.): setup would disable Ubuntu's AppArmor profile bwrap-userns-restrict (a link in /etc/apparmor.d/disable; its file is untouched), write and load /etc/apparmor.d/yulius-bwrap, load the other profiles of /etc/apparmor.d as a boot does; run `sudo yulius host setup`",
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
        (
            noble(),
            "would write and load /etc/apparmor.d/yulius-bwrap, load the other profiles of /etc/apparmor.d as a boot does",
        ),
        (
            resolute(),
            "would disable Ubuntu's AppArmor profile bwrap-userns-restrict (a link in /etc/apparmor.d/disable; its file is untouched), write and load /etc/apparmor.d/yulius-bwrap, load the other profiles of /etc/apparmor.d as a boot does",
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
            "AppArmor does not restrict user namespaces here; not proven: the service user is \
             not set up yet"
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
    // A link that keeps ours from loading is no rule that allows anything.
    let mut fake = noble();
    fake.file(PROFILE_FILE, PROFILE);
    fake.links.insert(
        PathBuf::from("/etc/apparmor.d/disable/yulius-bwrap"),
        PathBuf::from(PROFILE_FILE),
    );
    let lines = run(&mut fake, Mode::DryRun);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Ok,
            "/etc/apparmor.d/disable/yulius-bwrap keeps Yulius's profile from loading; not \
             proven: the service user is not set up yet"
        )
    );
    // A host without `runuser` proves nothing either way.
    let mut fake = set_up(noble());
    fake.failing.push((
        "/usr/sbin/runuser",
        None,
        "cannot run /usr/sbin/runuser: No such file or directory (os error 2)",
    ));
    let lines = run(&mut fake, Mode::Check);
    assert_eq!(
        item(&lines),
        (
            HostOutcome::Ok,
            "allowed by /etc/apparmor.d/yulius-bwrap; not proven: /usr/sbin/runuser cannot be \
             started"
        )
    );
}

/// BNS-010: after its copy, `yulius instance import` asks whether the new host lacks the rule,
/// with the service user it made and nothing else of the setup, and changes nothing.
#[test]
fn the_import_learns_whether_the_host_lacks_the_rule() {
    for (name, mut fake, lacks) in [
        ("noble", noble(), true),
        ("resolute", resolute(), true),
        ("debian", debian(), false),
        ("no AppArmor", Fake::fresh(), false),
    ] {
        let made = Run::new(&mut fake, Mode::Setup, options(), &mut |_| {}).user_and_ids();
        assert!(passed(&made), "{made:#?}");
        fake.log.clear();
        let lines = Run::new(&mut fake, Mode::Check, options(), &mut |_| {}).namespaces();
        let names: Vec<&str> = lines.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["service user", ITEM], "{name}: {lines:#?}");
        assert_eq!(lacks_namespaces(&lines), lacks, "{name}: {lines:#?}");
        assert!(!touched(&fake), "{name}: {:#?}", fake.log);
    }
    let mut fake = set_up(noble());
    let lines = Run::new(&mut fake, Mode::Check, options(), &mut |_| {}).namespaces();
    assert!(!lacks_namespaces(&lines), "{lines:#?}");
}
