use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

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
            "show" => {
                let mut shows = self.shows.borrow_mut();
                let out = if shows.len() > 1 {
                    shows.pop_front().unwrap_or_default()
                } else {
                    shows.front().cloned().unwrap_or_default()
                };
                Ok((0, out))
            }
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
    let state = AtomicU8::new(THAW);
    let mut fake = Fake::new(&[]);
    assert_eq!(on_signal(&state, &mut fake, UNIT), None);
    assert_eq!(fake.calls(), ["thaw yulius.service"]);

    let mut fake = Fake::new(&[]);
    state.store(LEAVE, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(on_signal(&state, &mut fake, UNIT), None);
    assert!(fake.calls().is_empty());

    // Still taking the freeze when the wait ends: no thaw that could come before the freeze.
    let mut fake = Fake::new(&[]);
    state.store(ARMING, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(on_signal(&state, &mut fake, UNIT), None);
    assert!(fake.calls().is_empty());
}

#[test]
fn a_failed_thaw_on_a_signal_says_to_run_it_by_hand() {
    let state = AtomicU8::new(THAW);
    let mut fake = Fake::new(&[]);
    fake.codes.insert("thaw".into(), 1);
    let why = on_signal(&state, &mut fake, UNIT).unwrap_or_default();
    assert!(why.contains("run it by hand"), "{why}");
}
