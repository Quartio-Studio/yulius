//! Whether a restart of the server's unit would fail (SPEC-458 RST-001, ADR-0298).
//!
//! systemd 258 and later start a unit inside the unit's own group, which the kernel refuses
//! while that group passes controllers to children that hold processes. Sessions of the old
//! layout live exactly there (`<unit>/sandboxes`) and `KillMode=process` keeps them over the
//! stop, so the stop would be the last thing that works. The root adopt unit moves them before
//! every start; a unit that does not pull it in must not be stopped while sessions run.

use std::path::{Component, Path, PathBuf};

use yulius_sandbox::units;

/// The first systemd that starts a unit inside the unit's own group.
pub const FIRST_AFFECTED: u32 = 258;
/// The group of the old layout that holds the sessions, below the server's unit.
const SANDBOXES: &str = "sandboxes";

/// What a host says about a restart of its server unit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    /// The major version of systemd; `None` when it cannot be asked.
    pub systemd: Option<u32>,
    /// Whether the unit is loaded at all.
    pub loaded: bool,
    /// Whether processes live in the server unit's own `sandboxes` group.
    pub sessions_in_unit: bool,
    /// Whether the unit pulls in its adopt unit (`Wants=`).
    pub adopt_wired: bool,
}

/// Why a restart of `service` would leave it down, or `None` when it would come back.
#[must_use]
pub fn blocker(service: &str, facts: &Facts) -> Option<String> {
    let version = facts.systemd.filter(|version| *version >= FIRST_AFFECTED)?;
    (facts.sessions_in_unit && !facts.adopt_wired).then(|| {
        format!(
            "{service} would not start again: its sessions live in its own group, where \
             systemd {version} starts no unit while one runs. Run `sudo yulius host setup` on \
             the host once, then try again"
        )
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
        .any(|group| {
            std::fs::read_to_string(group.join(SANDBOXES).join("cgroup.events"))
                .is_ok_and(|events| populated(&events))
        });
    Facts {
        systemd,
        loaded: property(&show, "LoadState") == Some("loaded"),
        sessions_in_unit,
        adopt_wired,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(systemd: Option<u32>, sessions_in_unit: bool, adopt_wired: bool) -> Facts {
        Facts {
            systemd,
            loaded: true,
            sessions_in_unit,
            adopt_wired,
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

    fn host(root: &Path, group: &str, events: &str) -> std::io::Result<()> {
        let dir = root.join(group).join(SANDBOXES);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("cgroup.events"), events)
    }

    fn answers(version: &'static str, show: &'static str) -> impl Fn(&[&str]) -> Option<String> {
        move |args: &[&str]| match args.first().copied() {
            Some("--version") => Some(version.to_owned()),
            Some("show") => Some(show.to_owned()),
            _ => None,
        }
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
        assert_eq!(found, facts(Some(259), true, false));
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
