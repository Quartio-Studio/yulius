use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Mutex;

use std::sync::atomic::AtomicU8;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const UNIT: &str = "yulius.service";
const HELPER: &str = "yulius-update.service";
const CGROUP: &str = "/system.slice/yulius.service";

/// A host whose `systemctl show` answers come from a queue (the last one repeats) and whose
/// commands are logged.
#[derive(Clone, Default)]
struct Fake {
    log: Rc<RefCell<Vec<String>>>,
    shows: Rc<RefCell<VecDeque<String>>>,
    files: HashMap<String, String>,
    codes: HashMap<String, i32>,
    /// `is-active` answers for the update helper; the last one repeats.
    helper: Rc<RefCell<VecDeque<String>>>,
    /// `systemctl show` answers for the sessions unit (the last one repeats); none: the host
    /// has no such unit.
    sessions: Rc<RefCell<VecDeque<String>>>,
    /// systemd does not answer `show` for the sessions unit.
    sessions_silent: bool,
}

const SESSIONS: &str = "yulius-sandboxes.service";
const SESSIONS_CGROUP: &str = "/yulius.slice/yulius-sandboxes.service";

/// The units a signal thaws.
fn held(units: &[&str]) -> Mutex<Vec<String>> {
    Mutex::new(units.iter().map(|unit| (*unit).to_owned()).collect())
}

fn next(queue: &RefCell<VecDeque<String>>) -> Option<String> {
    let mut queue = queue.borrow_mut();
    if queue.len() > 1 {
        queue.pop_front()
    } else {
        queue.front().cloned()
    }
}

impl Fake {
    fn new(shows: &[(&str, &str)]) -> Self {
        let fake = Self::default();
        fake.helper.borrow_mut().push_back("inactive".into());
        for (active, freezer) in shows {
            fake.shows.borrow_mut().push_back(format!(
                "LoadState=loaded\nActiveState={active}\nFreezerState={freezer}\nControlGroup={CGROUP}\n"
            ));
        }
        fake
    }

    /// The same host with a sessions unit that answers `shows`.
    fn with_sessions(self, shows: &[(&str, &str)]) -> Self {
        for (active, freezer) in shows {
            self.sessions.borrow_mut().push_back(format!(
                "LoadState=loaded\nActiveState={active}\nFreezerState={freezer}\nControlGroup={SESSIONS_CGROUP}\n"
            ));
        }
        self
    }

    fn calls(&self) -> Vec<String> {
        self.log
            .borrow()
            .iter()
            .filter(|c| !c.starts_with("show") && !c.starts_with("is-active"))
            .cloned()
            .collect()
    }
}

impl Host for Fake {
    fn systemctl(&mut self, args: &[&str]) -> Result<(i32, String), String> {
        let call = args.join(" ");
        self.log.borrow_mut().push(call.clone());
        let code = self.codes.get(args[0]).copied().unwrap_or(0);
        match args[0] {
            "show" if args.last() == Some(&SESSIONS) && self.sessions_silent => {
                Err("Failed to connect to bus".into())
            }
            "show" if args.last() == Some(&SESSIONS) => Ok((
                0,
                next(&self.sessions).unwrap_or_else(|| "LoadState=not-found\n".into()),
            )),
            "show" => Ok((0, next(&self.shows).unwrap_or_default())),
            "is-active" => {
                let mut answers = self.helper.borrow_mut();
                let out = if answers.len() > 1 {
                    answers.pop_front().unwrap_or_default()
                } else {
                    answers.front().cloned().unwrap_or_default()
                };
                Ok((3, format!("{out}\n")))
            }
            _ => Ok((code, String::new())),
        }
    }

    fn read(&self, path: &Path) -> Option<String> {
        self.files.get(path.to_str()?).cloned()
    }

    fn sleep(&mut self, _: Duration) {}
}

fn run(fake: &Fake) -> Result<Freeze, String> {
    freeze(Box::new(fake.clone()), UNIT, HELPER)
}

#[test]
fn a_running_unit_is_frozen_and_thawed_on_release() -> TestResult {
    let fake = Fake::new(&[
        ("active", "running"),
        ("active", "freezing"),
        ("active", "frozen"),
    ]);
    let held = run(&fake)?;
    assert!(held.ours());
    assert_eq!(fake.calls(), ["freeze yulius.service"]);
    held.release()?;
    assert_eq!(
        fake.calls(),
        ["freeze yulius.service", "thaw yulius.service"]
    );
    Ok(())
}

#[test]
fn a_dropped_freeze_thaws() -> TestResult {
    let fake = Fake::new(&[("active", "running"), ("active", "frozen")]);
    drop(run(&fake)?);
    assert_eq!(
        fake.calls(),
        ["freeze yulius.service", "thaw yulius.service"]
    );
    Ok(())
}

#[test]
fn a_move_keeps_it_frozen_and_disables_it() -> TestResult {
    let fake = Fake::new(&[("active", "running"), ("active", "frozen")]);
    run(&fake)?.keep_for_move()?;
    assert_eq!(
        fake.calls(),
        ["freeze yulius.service", "disable yulius.service"]
    );
    Ok(())
}

#[test]
fn a_unit_frozen_before_stays_frozen() -> TestResult {
    let fake = Fake::new(&[("active", "frozen")]);
    let held = run(&fake)?;
    assert!(!held.ours());
    held.release()?;
    assert!(fake.calls().is_empty(), "{:?}", fake.calls());
    Ok(())
}

#[test]
fn a_unit_that_never_freezes_is_thawed_and_refused() -> TestResult {
    let fake = Fake::new(&[("active", "running"), ("active", "freezing")]);
    let err = run(&fake).err().ok_or("expected an error")?;
    assert!(err.contains("did not freeze"), "{err}");
    assert_eq!(
        fake.calls(),
        ["freeze yulius.service", "thaw yulius.service"]
    );
    Ok(())
}

#[test]
fn a_failed_freeze_is_refused() -> TestResult {
    let mut fake = Fake::new(&[("active", "running")]);
    fake.codes.insert("freeze".into(), 1);
    let err = run(&fake).err().ok_or("expected an error")?;
    assert!(err.contains("systemctl freeze"), "{err}");
    Ok(())
}

#[test]
fn an_export_inside_the_unit_is_refused_before_anything() -> TestResult {
    let mut fake = Fake::new(&[("active", "running")]);
    fake.files.insert(
        "/proc/self/cgroup".into(),
        format!("0::{CGROUP}/sandboxes/sb/system\n"),
    );
    let err = run(&fake).err().ok_or("expected an error")?;
    assert!(err.contains("would freeze itself"), "{err}");
    assert!(fake.calls().is_empty());
    Ok(())
}

#[test]
fn a_sibling_cgroup_with_the_same_prefix_is_not_inside() -> TestResult {
    let mut fake = Fake::new(&[("active", "running"), ("active", "frozen")]);
    fake.files.insert(
        "/proc/self/cgroup".into(),
        "0::/system.slice/yulius.service-other\n".into(),
    );
    assert!(run(&fake)?.ours());
    Ok(())
}

#[test]
fn a_running_update_is_refused() -> TestResult {
    let fake = Fake::new(&[("active", "running")]);
    *fake.helper.borrow_mut() = VecDeque::from(["activating".to_owned()]);
    let err = run(&fake).err().ok_or("expected an error")?;
    assert!(err.contains("an update is running"), "{err}");
    assert!(fake.calls().is_empty());
    Ok(())
}

#[test]
fn a_stopped_unit_with_processes_is_refused() -> TestResult {
    let mut fake = Fake::new(&[("inactive", "running")]);
    fake.files.insert(
        format!("/sys/fs/cgroup{CGROUP}/cgroup.events"),
        "populated 1\nfrozen 0\n".into(),
    );
    let err = run(&fake).err().ok_or("expected an error")?;
    assert!(err.contains("still run"), "{err}");
    assert!(fake.calls().is_empty());
    Ok(())
}

#[test]
fn a_stopped_unit_without_processes_needs_no_freeze() -> TestResult {
    let mut fake = Fake::new(&[("inactive", "running")]);
    fake.files.insert(
        format!("/sys/fs/cgroup{CGROUP}/cgroup.events"),
        "populated 0\nfrozen 0\n".into(),
    );
    let held = run(&fake)?;
    assert!(!held.ours());
    drop(held);
    assert!(fake.calls().is_empty());
    Ok(())
}

#[test]
fn an_unknown_unit_is_refused() -> TestResult {
    let fake = Fake::new(&[]);
    fake.shows
        .borrow_mut()
        .push_back("LoadState=not-found\nActiveState=inactive\n".into());
    let err = run(&fake).err().ok_or("expected an error")?;
    assert!(err.contains("no unit"), "{err}");
    Ok(())
}

#[test]
fn an_update_that_started_during_the_freeze_thaws_and_refuses() -> TestResult {
    let fake = Fake::new(&[("active", "running"), ("active", "frozen")]);
    *fake.helper.borrow_mut() = VecDeque::from(["inactive".to_owned(), "activating".to_owned()]);
    let err = run(&fake).err().ok_or("expected a refusal")?;
    assert!(err.contains("an update is running"), "{err}");
    assert_eq!(
        fake.calls(),
        ["freeze yulius.service", "thaw yulius.service"]
    );
    Ok(())
}

#[test]
fn the_helper_is_checked_again_on_request() -> TestResult {
    let fake = Fake::new(&[("active", "running"), ("active", "frozen")]);
    let mut held = run(&fake)?;
    held.helper_idle()?;
    *fake.helper.borrow_mut() = VecDeque::from(["active".to_owned()]);
    assert!(held.helper_idle().is_err());
    Ok(())
}

#[test]
fn a_signal_thaws_only_a_freeze_the_export_holds() {
    use std::sync::atomic::Ordering::SeqCst;
    let state = AtomicU8::new(THAW);
    let mut fake = Fake::new(&[]);
    assert_eq!(
        on_signal(&state, &mut fake, &held(&[UNIT])),
        Signal::Exit(None)
    );
    assert_eq!(fake.calls(), ["thaw yulius.service"]);
    assert_eq!(
        state.load(SeqCst),
        CLAIMED,
        "the export can no longer keep it for a move"
    );

    let mut fake = Fake::new(&[]);
    state.store(LEAVE, SeqCst);
    assert_eq!(
        on_signal(&state, &mut fake, &held(&[UNIT])),
        Signal::Exit(None)
    );
    assert!(fake.calls().is_empty());

    // Still taking the freeze when the wait ends: no thaw that could come before the freeze.
    let mut fake = Fake::new(&[]);
    state.store(ARMING, SeqCst);
    assert_eq!(
        on_signal(&state, &mut fake, &held(&[UNIT])),
        Signal::Exit(None)
    );
    assert!(fake.calls().is_empty());
}

#[test]
fn a_signal_during_a_refused_freeze_leaves_the_exit_to_the_refusal() {
    let state = AtomicU8::new(REFUSED);
    let mut fake = Fake::new(&[]);
    assert_eq!(
        on_signal(&state, &mut fake, &held(&[UNIT])),
        Signal::LeaveToMain
    );
    assert!(fake.calls().is_empty());
}

#[test]
fn the_export_and_a_signal_never_both_own_the_end() {
    use std::sync::atomic::Ordering::SeqCst;
    // The export claims first: a later signal leaves the thaw (and the exit) to it, so the
    // process cannot end before the export's own `systemctl thaw` or `disable` runs.
    let state = AtomicU8::new(THAW);
    assert!(claim_end(&state));
    let mut fake = Fake::new(&[]);
    assert_eq!(
        on_signal(&state, &mut fake, &held(&[UNIT])),
        Signal::LeaveToMain
    );
    assert!(fake.calls().is_empty());
    // The signal claims first: the export must not report a move or a thaw of its own.
    let state = AtomicU8::new(THAW);
    let mut fake = Fake::new(&[]);
    let _ = on_signal(&state, &mut fake, &held(&[UNIT]));
    assert!(!claim_end(&state));
    // Not the export's freeze: the export still ends it itself once it claimed the end.
    let state = AtomicU8::new(LEAVE);
    assert!(claim_end(&state));
    assert_eq!(state.load(SeqCst), ENDING);
    let mut fake = Fake::new(&[]);
    assert_eq!(
        on_signal(&state, &mut fake, &held(&[UNIT])),
        Signal::LeaveToMain
    );
}

#[test]
fn a_failed_thaw_on_a_signal_says_to_run_it_by_hand() {
    let state = AtomicU8::new(THAW);
    let mut fake = Fake::new(&[]);
    fake.codes.insert("thaw".into(), 1);
    let Signal::Exit(Some(why)) = on_signal(&state, &mut fake, &held(&[UNIT])) else {
        panic!("expected a failed thaw");
    };
    assert!(why.contains("run it by hand"), "{why}");
}

/// SPEC-458 EXP-001: the sessions unit is frozen after the server's and thawed before it.
#[test]
fn the_sessions_unit_is_frozen_with_the_server_and_thawed_first() -> TestResult {
    let fake = Fake::new(&[("active", "running"), ("active", "frozen")]).with_sessions(&[
        ("active", "running"),
        ("active", "freezing"),
        ("active", "frozen"),
    ]);
    let held = run(&fake)?;
    assert!(held.ours());
    assert_eq!(held.covered(), [UNIT, SESSIONS]);
    assert_eq!(held.frozen(), [SESSIONS, UNIT]);
    let said = held.describe();
    assert!(
        said.contains("Frozen yulius.service and yulius-sandboxes.service: every Yulius process"),
        "{said}"
    );
    assert!(
        said.contains(
            "thaw them with `sudo systemctl thaw yulius-sandboxes.service yulius.service`"
        ),
        "{said}"
    );
    held.release()?;
    assert_eq!(
        fake.calls(),
        [
            "freeze yulius.service",
            "freeze yulius-sandboxes.service",
            "thaw yulius-sandboxes.service",
            "thaw yulius.service"
        ]
    );
    Ok(())
}

/// SPEC-458 EXP-001: a stopped server whose sessions still run is exported with them frozen.
#[test]
fn a_stopped_server_with_live_sessions_freezes_the_sessions_unit() -> TestResult {
    let mut fake = Fake::new(&[("inactive", "running")])
        .with_sessions(&[("active", "running"), ("active", "frozen")]);
    fake.files.insert(
        "/sys/fs/cgroup/system.slice/yulius.service/cgroup.events".into(),
        "populated 0\nfrozen 0\n".into(),
    );
    let held = run(&fake)?;
    assert!(held.ours());
    assert_eq!(held.covered(), [SESSIONS]);
    assert!(
        held.describe()
            .starts_with("Frozen yulius-sandboxes.service: ")
    );
    assert!(held.describe().contains("thaw it with"));
    drop(held);
    assert_eq!(
        fake.calls(),
        [
            "freeze yulius-sandboxes.service",
            "thaw yulius-sandboxes.service"
        ]
    );
    Ok(())
}

/// Review of SPEC-458 (nit): what was frozen before the export is named as it is. With a stopped
/// server that is the sessions unit, not the server's.
#[test]
fn units_frozen_before_the_export_are_named_as_they_are() -> TestResult {
    let mut fake = Fake::new(&[("inactive", "running")]).with_sessions(&[("active", "frozen")]);
    fake.files.insert(
        "/sys/fs/cgroup/system.slice/yulius.service/cgroup.events".into(),
        "populated 0\nfrozen 0\n".into(),
    );
    let held = run(&fake)?;
    assert!(!held.ours());
    assert_eq!(
        held.describe(),
        "yulius-sandboxes.service was frozen before the export; it stays frozen after it."
    );
    drop(held);
    let both = Fake::new(&[("active", "frozen")]).with_sessions(&[("active", "frozen")]);
    let held = run(&both)?;
    assert_eq!(
        held.describe(),
        "yulius.service and yulius-sandboxes.service were frozen before the export; they stay \
         frozen after it."
    );
    Ok(())
}

/// SPEC-458 EXP-001, for `yulius instance migrate` (SPEC-460): thawing the instance by its
/// server's unit thaws its sessions unit too, first, and leaves one that is not there alone.
#[test]
fn thawing_the_instance_thaws_its_sessions_unit_first() -> TestResult {
    let mut fake = Fake::new(&[("active", "frozen")]).with_sessions(&[("active", "frozen")]);
    assert_eq!(sessions_state(&mut fake, UNIT), Ok(Some(true)));
    assert_eq!(thaw_units(&mut fake, UNIT), [SESSIONS, UNIT]);
    thaw_instance(&mut fake, UNIT)?;
    assert_eq!(
        fake.calls(),
        ["thaw yulius-sandboxes.service", "thaw yulius.service"]
    );

    let mut plain = Fake::new(&[("active", "frozen")]);
    assert_eq!(sessions_state(&mut plain, UNIT), Ok(None));
    assert_eq!(thaw_units(&mut plain, UNIT), [UNIT]);
    thaw_instance(&mut plain, UNIT)?;
    assert_eq!(plain.calls(), ["thaw yulius.service"]);

    // A sessions unit that runs unfrozen is not the frozen instance.
    let mut running = Fake::new(&[("active", "frozen")]).with_sessions(&[("active", "running")]);
    assert_eq!(sessions_state(&mut running, UNIT), Ok(Some(false)));
    Ok(())
}

/// Review of the merge (low): systemd refuses to thaw a unit that does not run. A stopped
/// server's unit is left out, so its sessions unit is thawed and the thaw does not fail.
#[test]
fn a_stopped_servers_unit_is_left_out_of_the_thaw() -> TestResult {
    let mut fake = Fake::new(&[("inactive", "running")]).with_sessions(&[("active", "frozen")]);
    assert_eq!(sessions_state(&mut fake, UNIT), Ok(Some(true)));
    assert_eq!(thaw_units(&mut fake, UNIT), [SESSIONS]);
    thaw_instance(&mut fake, UNIT)?;
    assert_eq!(fake.calls(), ["thaw yulius-sandboxes.service"]);
    // A sessions unit that does not run is no part of the instance to thaw.
    let mut none = Fake::new(&[("active", "frozen")]).with_sessions(&[("inactive", "running")]);
    assert_eq!(sessions_state(&mut none, UNIT), Ok(None));
    assert_eq!(thaw_units(&mut none, UNIT), [UNIT]);
    Ok(())
}

/// A systemd that does not say what the sessions unit does is not a host without one: the
/// state is unknown (nothing is started on it), and a thaw tries the unit all the same, since
/// a sessions unit left frozen is worse than a thaw that says it failed.
#[test]
fn an_unanswered_question_about_the_sessions_unit_is_not_no_unit() {
    let mut fake = Fake::new(&[("active", "frozen")]).with_sessions(&[("active", "frozen")]);
    fake.sessions_silent = true;
    assert!(sessions_state(&mut fake, UNIT).is_err());
    assert_eq!(thaw_units(&mut fake, UNIT), [SESSIONS, UNIT]);
}

#[test]
fn a_failed_thaw_of_the_instance_names_the_command_for_both_units() {
    let mut fake = Fake::new(&[("active", "frozen")]).with_sessions(&[("active", "frozen")]);
    fake.codes.insert("thaw".into(), 1);
    let why = thaw_instance(&mut fake, UNIT).err().unwrap_or_default();
    assert!(
        why.contains("systemctl thaw yulius-sandboxes.service yulius.service"),
        "{why}"
    );
    // Both were tried.
    assert_eq!(fake.calls().len(), 2);
}

#[test]
fn a_move_keeps_both_units_frozen_and_disables_the_server() -> TestResult {
    let fake = Fake::new(&[("active", "running"), ("active", "frozen")])
        .with_sessions(&[("active", "running"), ("active", "frozen")]);
    run(&fake)?.keep_for_move()?;
    assert_eq!(
        fake.calls(),
        [
            "freeze yulius.service",
            "freeze yulius-sandboxes.service",
            "disable yulius.service"
        ]
    );
    Ok(())
}

#[test]
fn a_sessions_unit_that_never_freezes_thaws_the_server_too() -> TestResult {
    let fake = Fake::new(&[("active", "running"), ("active", "frozen")])
        .with_sessions(&[("active", "running"), ("active", "freezing")]);
    let why = run(&fake).err().ok_or("expected a refusal")?;
    assert!(
        why.contains("yulius-sandboxes.service did not freeze within 60 s"),
        "{why}"
    );
    assert_eq!(
        fake.calls(),
        [
            "freeze yulius.service",
            "freeze yulius-sandboxes.service",
            "thaw yulius-sandboxes.service",
            "thaw yulius.service"
        ]
    );
    Ok(())
}

#[test]
fn an_export_inside_the_sessions_unit_is_refused_before_anything() -> TestResult {
    let mut fake = Fake::new(&[("active", "running")]).with_sessions(&[("active", "running")]);
    fake.files.insert(
        "/proc/self/cgroup".into(),
        format!("0::{SESSIONS_CGROUP}/sandboxes/x/agents\n"),
    );
    let why = run(&fake).err().ok_or("expected a refusal")?;
    assert!(
        why.contains("runs inside yulius-sandboxes.service"),
        "{why}"
    );
    assert!(fake.calls().is_empty(), "{:?}", fake.calls());
    Ok(())
}

#[test]
fn only_what_the_export_froze_is_thawed() -> TestResult {
    // The sessions unit was frozen before; a loaded one that does not run is left alone.
    let fake = Fake::new(&[("active", "running"), ("active", "frozen")])
        .with_sessions(&[("active", "frozen")]);
    let held = run(&fake)?;
    assert_eq!(held.frozen(), [UNIT]);
    assert_eq!(held.covered(), [UNIT, SESSIONS]);
    held.release()?;
    assert_eq!(
        fake.calls(),
        ["freeze yulius.service", "thaw yulius.service"]
    );

    let fake = Fake::new(&[("active", "running"), ("active", "frozen")])
        .with_sessions(&[("inactive", "running")]);
    let held = run(&fake)?;
    assert_eq!(held.covered(), [UNIT]);
    drop(held);
    assert_eq!(
        fake.calls(),
        ["freeze yulius.service", "thaw yulius.service"]
    );
    Ok(())
}

#[test]
fn a_signal_thaws_every_unit_the_export_froze() {
    let state = AtomicU8::new(THAW);
    let mut fake = Fake::new(&[]);
    fake.codes.insert("thaw".into(), 1);
    let Signal::Exit(Some(why)) = on_signal(&state, &mut fake, &held(&[SESSIONS, UNIT])) else {
        panic!("expected failed thaws");
    };
    assert_eq!(
        fake.calls(),
        ["thaw yulius-sandboxes.service", "thaw yulius.service"],
        "the second is tried although the first failed"
    );
    assert!(why.contains("thaw yulius-sandboxes.service") && why.contains("thaw yulius.service"));
}
