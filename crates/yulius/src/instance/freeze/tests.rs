use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use super::*;

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
    helper: String,
}

impl Fake {
    fn new(shows: &[(&str, &str)]) -> Self {
        let fake = Self {
            helper: "inactive".into(),
            ..Self::default()
        };
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
        let code = self
            .codes
            .get(args[0])
            .copied()
            .unwrap_or(0);
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
            "is-active" => Ok((3, format!("{}\n", self.helper))),
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
fn a_running_unit_is_frozen_and_thawed_on_release() {
    let fake = Fake::new(&[("active", "running"), ("active", "freezing"), ("active", "frozen")]);
    let held = run(&fake).unwrap();
    assert!(held.ours());
    assert_eq!(fake.calls(), ["freeze yulius.service"]);
    held.release().unwrap();
    assert_eq!(fake.calls(), ["freeze yulius.service", "thaw yulius.service"]);
}

#[test]
fn a_dropped_freeze_thaws() {
    let fake = Fake::new(&[("active", "running"), ("active", "frozen")]);
    drop(run(&fake).unwrap());
    assert_eq!(fake.calls(), ["freeze yulius.service", "thaw yulius.service"]);
}

#[test]
fn a_move_keeps_it_frozen_and_disables_it() {
    let fake = Fake::new(&[("active", "running"), ("active", "frozen")]);
    run(&fake).unwrap().keep_for_move().unwrap();
    assert_eq!(fake.calls(), ["freeze yulius.service", "disable yulius.service"]);
}

#[test]
fn a_unit_frozen_before_stays_frozen() {
    let fake = Fake::new(&[("active", "frozen")]);
    let held = run(&fake).unwrap();
    assert!(!held.ours());
    held.release().unwrap();
    assert!(fake.calls().is_empty(), "{:?}", fake.calls());
}

#[test]
fn a_unit_that_never_freezes_is_thawed_and_refused() {
    let fake = Fake::new(&[("active", "running"), ("active", "freezing")]);
    let err = run(&fake).unwrap_err();
    assert!(err.contains("did not freeze"), "{err}");
    assert_eq!(fake.calls(), ["freeze yulius.service", "thaw yulius.service"]);
}

#[test]
fn a_failed_freeze_is_refused() {
    let mut fake = Fake::new(&[("active", "running")]);
    fake.codes.insert("freeze".into(), 1);
    let err = run(&fake).unwrap_err();
    assert!(err.contains("systemctl freeze"), "{err}");
}

#[test]
fn an_export_inside_the_unit_is_refused_before_anything() {
    let mut fake = Fake::new(&[("active", "running")]);
    fake.files.insert(
        "/proc/self/cgroup".into(),
        format!("0::{CGROUP}/sandboxes/sb/system\n"),
    );
    let err = run(&fake).unwrap_err();
    assert!(err.contains("would freeze itself"), "{err}");
    assert!(fake.calls().is_empty());
}

#[test]
fn a_sibling_cgroup_with_the_same_prefix_is_not_inside() {
    let mut fake = Fake::new(&[("active", "running"), ("active", "frozen")]);
    fake.files.insert(
        "/proc/self/cgroup".into(),
        "0::/system.slice/yulius.service-other\n".into(),
    );
    assert!(run(&fake).unwrap().ours());
}

#[test]
fn a_running_update_is_refused() {
    let mut fake = Fake::new(&[("active", "running")]);
    fake.helper = "activating".into();
    let err = run(&fake).unwrap_err();
    assert!(err.contains("an update is running"), "{err}");
    assert!(fake.calls().is_empty());
}

#[test]
fn a_stopped_unit_with_processes_is_refused() {
    let mut fake = Fake::new(&[("inactive", "running")]);
    fake.files.insert(
        format!("/sys/fs/cgroup{CGROUP}/cgroup.events"),
        "populated 1\nfrozen 0\n".into(),
    );
    let err = run(&fake).unwrap_err();
    assert!(err.contains("still run"), "{err}");
    assert!(fake.calls().is_empty());
}

#[test]
fn a_stopped_unit_without_processes_needs_no_freeze() {
    let mut fake = Fake::new(&[("inactive", "running")]);
    fake.files.insert(
        format!("/sys/fs/cgroup{CGROUP}/cgroup.events"),
        "populated 0\nfrozen 0\n".into(),
    );
    let held = run(&fake).unwrap();
    assert!(!held.ours());
    drop(held);
    assert!(fake.calls().is_empty());
}

#[test]
fn an_unknown_unit_is_refused() {
    let fake = Fake::new(&[]);
    fake.shows
        .borrow_mut()
        .push_back("LoadState=not-found\nActiveState=inactive\n".into());
    let err = run(&fake).unwrap_err();
    assert!(err.contains("no unit"), "{err}");
}
