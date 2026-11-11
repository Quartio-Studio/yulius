//! Whether a restart of the server's unit would fail (SPEC-458 RST-001, ADR-0298).
//!
//! systemd 258 and later start a unit inside the unit's own group, which the kernel refuses
//! while that group passes controllers to children that hold processes. Sessions of the old
//! layout live exactly there (`<unit>/sandboxes`) and `KillMode=process` keeps them over the
//! stop, so the stop would be the last thing that works. The root adopt unit moves them before
//! every start; a unit that does not pull it in, or whose sessions unit (which the adopt unit
//! requires) does not run, must not be stopped while sessions run.

use std::path::{Component, Path, PathBuf};

use yulius_sandbox::units;

/// The first systemd that starts a unit inside the unit's own group.
pub const FIRST_AFFECTED: u32 = 258;
/// The server's own subgroup of its unit: what it holds ends or moves with the server.
const SERVER: &str = "server";
/// Subgroups of a unit looked at, at most.
const MAX_CHILDREN: usize = 4096;

/// What a host says about a restart of its server unit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    /// The major version of systemd; `None` when it cannot be asked.
    pub systemd: Option<u32>,
    /// Whether the unit is loaded at all.
    pub loaded: bool,
    /// Whether processes live in the server's unit outside the server's own subgroup: its
    /// `sandboxes`, or what an earlier server left (`legacy`).
    pub sessions_in_unit: bool,
    /// Whether the unit pulls in its adopt unit (`Wants=`).
    pub adopt_wired: bool,
    /// Whether the sessions unit runs: the adopt unit requires it.
    pub sessions_active: bool,
}

/// Why a restart of `service` would leave it down, or `None` when it would come back.
#[must_use]
pub fn blocker(service: &str, facts: &Facts) -> Option<String> {
    let version = facts.systemd.filter(|version| *version >= FIRST_AFFECTED)?;
    if !facts.sessions_in_unit {
        return None;
    }
    if !facts.adopt_wired {
        return Some(format!(
            "{service} would not start again: its sessions live in its own group, where \
             systemd {version} starts no unit while one runs. Run `sudo yulius host setup` on \
             the host once, then try again"
        ));
    }
    if facts.sessions_active {
        return None;
    }
    let sessions = units::sessions_unit(service).unwrap_or_default();
    Some(format!(
        "{service} would not start again: its sessions live in its own group, where systemd \
         {version} starts no unit while one runs, and {sessions} is not running: it has to run \
         for them to be moved out before the start. Start it (`sudo systemctl start \
         {sessions}`) or see why it does not (`systemctl status {sessions}`), then try again"
    ))
}

/// Whether a process lives in the unit group `group` outside the server's own subgroup and
/// systemd's (`.control`).
fn holds_sessions(group: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(group) else {
        return false;
    };
    entries.flatten().take(MAX_CHILDREN).any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        name != SERVER
            && !name.starts_with('.')
            && std::fs::read_to_string(entry.path().join("cgroup.events"))
                .is_ok_and(|events| populated(&events))
    })
}

/// The major version in the first line of `systemctl --version` (`systemd 259 (259.5-0ubuntu1)`).
#[must_use]
pub fn systemd_version(text: &str) -> Option<u32> {
    let mut words = text.lines().next()?.split_whitespace();
    if words.next()? != "systemd" {
        return None;
    }
    let word = words.next()?;
    let digits = word.len() - word.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    word.get(..digits)?.parse().ok()
}

/// Whether a `cgroup.events` file says the group or a descendant holds a process.
#[must_use]
pub fn populated(events: &str) -> bool {
    events.lines().any(|line| line.trim() == "populated 1")
}

/// The value of `name` in the output of `systemctl show` (`Name=value` lines).
#[must_use]
pub fn property<'a>(show: &'a str, name: &str) -> Option<&'a str> {
    show.lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
        .map(str::trim)
}

/// The groups the unit `service` may hold below `root`: the one systemd names (`shown`, its
/// `ControlGroup`), then its place of the old layout and its place in its slice.
#[must_use]
pub fn unit_groups(root: &Path, service: &str, shown: Option<&str>) -> Vec<PathBuf> {
    let named = shown
        .and_then(|shown| shown.trim().strip_prefix('/'))
        .filter(|relative| {
            !relative.is_empty()
                && Path::new(relative)
                    .components()
                    .all(|part| matches!(part, Component::Normal(_)))
        })
        .map(|relative| root.join(relative));
    let slice = units::slice(service).map(|slice| root.join(slice).join(service));
    let mut groups: Vec<PathBuf> = Vec::new();
    for group in [named, Some(root.join("system.slice").join(service)), slice]
        .into_iter()
        .flatten()
    {
        if !groups.contains(&group) {
            groups.push(group);
        }
    }
    groups
}

/// The facts about `service` on this host: `systemctl` runs one bounded `systemctl` call and
/// gives its output, the cgroup tree is read below `root`.
pub fn read(root: &Path, service: &str, systemctl: &dyn Fn(&[&str]) -> Option<String>) -> Facts {
    let systemd = systemctl(&["--version"]).and_then(|text| systemd_version(&text));
    let show = systemctl(&[
        "show",
        "--property=LoadState",
        "--property=ControlGroup",
        "--property=Wants",
        "--",
        service,
    ])
    .unwrap_or_default();
    let adopt_wired = units::adopt_unit(service).is_some_and(|adopt| {
        property(&show, "Wants")
            .is_some_and(|wants| wants.split_whitespace().any(|unit| unit == adopt))
    });
    let sessions_in_unit = unit_groups(root, service, property(&show, "ControlGroup"))
        .iter()
        .any(|group| holds_sessions(group));
    let sessions_active = units::sessions_unit(service).is_some_and(|sessions| {
        systemctl(&["show", "--property=ActiveState", "--", &sessions])
            .is_some_and(|show| property(&show, "ActiveState") == Some("active"))
    });
    Facts {
        systemd,
        loaded: property(&show, "LoadState") == Some("loaded"),
        sessions_in_unit,
        adopt_wired,
        sessions_active,
    }
}

/// The service unit a process in the group `own` (its path in `/proc/self/cgroup`) belongs to:
/// the innermost `*.service` on the path.
#[must_use]
pub fn own_service(own: &str) -> Option<&str> {
    own.split('/')
        .rev()
        .find(|part| part.len() > ".service".len() && part.ends_with(".service"))
}

/// Why a restart of the unit this server runs in would leave it down (SPEC-458 RST-003); `None`
/// when it would come back, and outside a service unit.
#[must_use]
pub fn of_this_server() -> Option<String> {
    let own = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let own = yulius_sandbox::cgroup::own_path(&own)?;
    let service = own_service(&own)?;
    let root = Path::new(yulius_sandbox::cgroup::CGROUP_ROOT);
    let facts = read(root, service, &|args: &[&str]| {
        crate::doctor::output_of(Path::new("systemctl"), args)
    });
    blocker(service, &facts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_own_service_is_the_innermost_unit_on_the_path() {
        let cases = [
            (
                "/system.slice/yulius.service/server",
                Some("yulius.service"),
            ),
            ("/yulius.slice/yulius.service", Some("yulius.service")),
            (
                "/system.slice/a.service/x/b.service/server",
                Some("b.service"),
            ),
            ("/user.slice/user-1000.slice/session-3.scope", None),
            ("/system.slice/.service/server", None),
            ("/", None),
        ];
        for (own, service) in cases {
            assert_eq!(own_service(own), service, "{own}");
        }
    }

    fn facts(systemd: Option<u32>, sessions_in_unit: bool, adopt_wired: bool) -> Facts {
        Facts {
            systemd,
            loaded: true,
            sessions_in_unit,
            adopt_wired,
            sessions_active: adopt_wired,
        }
    }

    /// Review of SPEC-458 (low): the adopt unit needs the sessions unit (`Requires=`). Wired
    /// but with a sessions unit that does not run, the sessions may not be moved out.
    #[test]
    fn a_wired_unit_whose_sessions_unit_does_not_run_is_blocked_too() {
        let stopped = Facts {
            sessions_active: false,
            ..facts(Some(259), true, true)
        };
        let why = blocker("yulius.service", &stopped).unwrap_or_default();
        assert!(
            why.contains("yulius-sandboxes.service is not running"),
            "{why}"
        );
        assert!(
            why.contains("systemctl status yulius-sandboxes.service"),
            "{why}"
        );
        // Nothing to move, or a systemd that restarts the unit anyway: not blocked.
        for free in [
            Facts {
                sessions_active: false,
                ..facts(Some(259), false, true)
            },
            Facts {
                sessions_active: false,
                ..facts(Some(255), true, true)
            },
        ] {
            assert_eq!(blocker("yulius.service", &free), None, "{free:?}");
        }
    }

    #[test]
    fn only_an_affected_systemd_with_sessions_in_the_unit_and_no_adopt_unit_blocks() {
        let table = [
            (Some(259), true, false, true),
            (Some(258), true, false, true),
            (Some(257), true, false, false),
            (Some(255), true, false, false),
            (None, true, false, false),
            (Some(259), false, false, false),
            (Some(259), true, true, false),
            (Some(259), false, true, false),
        ];
        for (systemd, sessions, wired, blocked) in table {
            let why = blocker("yulius.service", &facts(systemd, sessions, wired));
            assert_eq!(why.is_some(), blocked, "{systemd:?} {sessions} {wired}");
        }
    }

    #[test]
    fn the_reason_names_the_unit_the_version_and_the_way_out() {
        let why = blocker("yulius-dev.service", &facts(Some(259), true, false)).unwrap_or_default();
        assert!(
            why.contains("yulius-dev.service would not start again"),
            "{why}"
        );
        assert!(why.contains("systemd 259"), "{why}");
        assert!(why.contains("`sudo yulius host setup`"), "{why}");
    }

    #[test]
    fn the_version_is_the_first_number_of_the_first_line() {
        let noble = "systemd 255 (255.4-1ubuntu8.11)\n+PAM +AUDIT +SELINUX";
        assert_eq!(systemd_version(noble), Some(255));
        assert_eq!(systemd_version("systemd 259 (259.5-0ubuntu1)\n"), Some(259));
        assert_eq!(systemd_version("systemd 258~rc1 (258~rc1-1)"), Some(258));
        assert_eq!(systemd_version("systemd"), None);
        assert_eq!(systemd_version("openrc 0.5"), None);
        assert_eq!(systemd_version("systemd unknown"), None);
        assert_eq!(systemd_version(""), None);
    }

    #[test]
    fn a_group_is_populated_only_when_its_events_say_so() {
        assert!(populated("populated 1\nfrozen 0\n"));
        assert!(!populated("populated 0\nfrozen 1\n"));
        assert!(!populated(""));
    }

    #[test]
    fn a_property_is_read_by_its_whole_name() {
        let show = "LoadState=loaded\nControlGroup=/system.slice/yulius.service\nWants=a.service\n";
        assert_eq!(
            property(show, "ControlGroup"),
            Some("/system.slice/yulius.service")
        );
        assert_eq!(property(show, "Wants"), Some("a.service"));
        assert_eq!(property(show, "Want"), None);
        assert_eq!(property(show, "After"), None);
    }

    #[test]
    fn the_unit_groups_are_the_named_one_then_both_layouts() {
        let root = Path::new("/cg");
        assert_eq!(
            unit_groups(
                root,
                "yulius.service",
                Some("/system.slice/yulius.service\n")
            ),
            [
                PathBuf::from("/cg/system.slice/yulius.service"),
                PathBuf::from("/cg/yulius.slice/yulius.service"),
            ]
        );
        assert_eq!(
            unit_groups(root, "y-dev.service", Some("/other.slice/y-dev.service")).len(),
            3
        );
        // A stopped unit names none, and a name that leaves the tree is not followed.
        for shown in [None, Some(""), Some("/../etc"), Some("relative/x")] {
            assert_eq!(
                unit_groups(root, "yulius.service", shown).len(),
                2,
                "{shown:?}"
            );
        }
    }

    fn child(root: &Path, group: &str, name: &str, events: &str) -> std::io::Result<()> {
        let dir = root.join(group).join(name);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("cgroup.events"), events)
    }

    fn host(root: &Path, group: &str, events: &str) -> std::io::Result<()> {
        child(root, group, "sandboxes", events)
    }

    /// systemd's answers: its version, `show` of the server's unit, and a running sessions unit.
    fn answers(version: &'static str, show: &'static str) -> impl Fn(&[&str]) -> Option<String> {
        answers_with(version, show, "active")
    }

    fn answers_with(
        version: &'static str,
        show: &'static str,
        sessions: &'static str,
    ) -> impl Fn(&[&str]) -> Option<String> {
        move |args: &[&str]| match (args.first().copied(), args.last().copied()) {
            (Some("--version"), _) => Some(version.to_owned()),
            (Some("show"), Some("yulius-sandboxes.service")) => {
                Some(format!("ActiveState={sessions}\n"))
            }
            (Some("show"), _) => Some(show.to_owned()),
            _ => None,
        }
    }

    /// Review of SPEC-458 (low): what keeps the unit's group is any process below it outside
    /// the server's own subgroup, not only one in `sandboxes`.
    #[test]
    fn processes_an_earlier_server_left_outside_sandboxes_count_as_well()
    -> Result<(), Box<dyn std::error::Error>> {
        let show = "LoadState=loaded\nControlGroup=/system.slice/yulius.service\nWants=\n";
        let unit = "system.slice/yulius.service";
        for (name, counts) in [("legacy", true), ("server", false), (".control", false)] {
            let tmp = tempfile::tempdir()?;
            child(tmp.path(), unit, name, "populated 1\n")?;
            let found = read(
                tmp.path(),
                "yulius.service",
                &answers("systemd 259 (259.5)\n", show),
            );
            assert_eq!(found.sessions_in_unit, counts, "{name}");
        }
        Ok(())
    }

    #[test]
    fn the_sessions_units_state_is_read() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        host(tmp.path(), "system.slice/yulius.service", "populated 1\n")?;
        let show = "LoadState=loaded\nControlGroup=/system.slice/yulius.service\nWants=yulius-sandboxes.service yulius-adopt.service\n";
        for (state, active) in [("active", true), ("failed", false), ("inactive", false)] {
            let found = read(
                tmp.path(),
                "yulius.service",
                &answers_with("systemd 259 (259.5)\n", show, state),
            );
            assert_eq!(found.sessions_active, active, "{state}");
            assert_eq!(
                blocker("yulius.service", &found).is_some(),
                !active,
                "{state}"
            );
        }
        Ok(())
    }

    #[test]
    fn the_facts_of_the_old_layout_on_systemd_259_block_the_restart()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        host(
            tmp.path(),
            "system.slice/yulius.service",
            "populated 1\nfrozen 0\n",
        )?;
        let show = "LoadState=loaded\nControlGroup=/system.slice/yulius.service\nWants=network-online.target\n";
        let found = read(
            tmp.path(),
            "yulius.service",
            &answers("systemd 259 (259.5)\n", show),
        );
        assert_eq!(
            found,
            Facts {
                sessions_active: true,
                ..facts(Some(259), true, false)
            }
        );
        assert!(blocker("yulius.service", &found).is_some());
        Ok(())
    }

    #[test]
    fn a_unit_that_pulls_in_its_adopt_unit_is_not_blocked() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        host(tmp.path(), "system.slice/yulius.service", "populated 1\n")?;
        let show = "LoadState=loaded\nControlGroup=/system.slice/yulius.service\nWants=yulius-sandboxes.service yulius-adopt.service\n";
        let found = read(
            tmp.path(),
            "yulius.service",
            &answers("systemd 259 (259.5)\n", show),
        );
        assert_eq!(found, facts(Some(259), true, true));
        assert_eq!(blocker("yulius.service", &found), None);
        Ok(())
    }

    #[test]
    fn sessions_of_a_stopped_unit_are_found_where_the_old_layout_left_them()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        host(tmp.path(), "system.slice/yulius.service", "populated 1\n")?;
        let show = "LoadState=loaded\nControlGroup=\nWants=\n";
        let found = read(
            tmp.path(),
            "yulius.service",
            &answers("systemd 259 (259.5)\n", show),
        );
        assert!(found.sessions_in_unit);
        Ok(())
    }

    #[test]
    fn a_host_that_answers_nothing_blocks_nothing() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let found = read(tmp.path(), "yulius.service", &|_: &[&str]| None);
        assert_eq!(found, Facts::default());
        assert_eq!(blocker("yulius.service", &found), None);
        // Sessions in the sessions unit are none of the server's unit.
        host(
            tmp.path(),
            "yulius.slice/yulius-sandboxes.service",
            "populated 1\n",
        )?;
        let show = "LoadState=loaded\nControlGroup=/yulius.slice/yulius.service\nWants=\n";
        let found = read(
            tmp.path(),
            "yulius.service",
            &answers("systemd 259 (259.5)\n", show),
        );
        assert!(!found.sessions_in_unit);
        Ok(())
    }
}
