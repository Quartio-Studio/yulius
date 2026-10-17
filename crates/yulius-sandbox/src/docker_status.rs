//! Whether isolated sandboxes get Docker on this host: one verdict, "sandbox Docker", for
//! `yulius doctor`, `yulius host setup` and `check`, and Settings → Instance (SPEC-203 HST-013).
//!
//! [`Facts`] are read from the host by [`Facts::read`] (or built by a caller that knows some of
//! them better, as the server does); [`verdict`] turns them into words. The words are the ones
//! the sandboxes' Docker socket answers with (SPEC-196 DKL-005), plus the fix.

use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

use crate::base::Bases;
use crate::manager::{
    DOCKER_NO_DELEGATION, DOCKER_WRITABLE_EXE, DOCKERD_PATHS, REBUILD_DEBOUNCE, own_engine,
};

/// Kernel modules a sandbox's Docker engine needs and cannot load itself (bridges, veths,
/// overlay layers, NAT through `iptables-nft`).
pub const ENGINE_MODULES: [&str; 11] = [
    "overlay",
    "bridge",
    "veth",
    "nf_tables",
    "nft_compat",
    "nft_chain_nat",
    "nf_nat",
    "nf_conntrack",
    "xt_MASQUERADE",
    "xt_addrtype",
    "xt_conntrack",
];

/// Where the modules are listed so that they load at every boot (`systemd-modules-load`).
pub const MODULES_FILE: &str = "/etc/modules-load.d/yulius-docker.conf";

/// The content of [`MODULES_FILE`]: a comment, then one module per line.
#[must_use]
pub fn modules_file_content() -> String {
    let mut text = String::from(
        "# Written by `yulius host setup`: kernel modules a sandbox's Docker engine needs and\n\
         # cannot load itself (docs/operations.md, \"Docker in sandboxes\").\n",
    );
    for module in ENGINE_MODULES {
        text.push_str(module);
        text.push('\n');
    }
    text
}

/// The engine's modules neither loaded (the text of `/proc/modules`) nor built into the kernel
/// (the text of `modules.builtin`).
#[must_use]
pub fn missing_modules(loaded: &str, builtin: &str) -> Vec<&'static str> {
    let mut present: Vec<String> = loaded
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect();
    present.extend(builtin.lines().filter_map(|line| {
        let name = line.rsplit('/').next()?.strip_suffix(".ko")?;
        Some(name.replace('-', "_"))
    }));
    ENGINE_MODULES
        .into_iter()
        .filter(|module| !present.iter().any(|p| p == module))
        .collect()
}

/// The modules this host lacks, read from `/proc/modules` and the running kernel's
/// `modules.builtin`.
#[must_use]
pub fn host_missing_modules() -> Vec<&'static str> {
    let release = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    missing_modules(
        &std::fs::read_to_string("/proc/modules").unwrap_or_default(),
        &std::fs::read_to_string(format!("/lib/modules/{}/modules.builtin", release.trim()))
            .unwrap_or_default(),
    )
}

/// Whether the system under `root` has `dockerd` where an engine looks for it.
#[must_use]
pub fn has_dockerd(root: &Path) -> bool {
    DOCKERD_PATHS.iter().any(|path| root.join(path).is_file())
}

/// Whether an executable owned by `owner` with `mode` could be rewritten by an engine: owned by
/// the service user (`service_uid`, root of every sandbox) or writable by its group or anyone
/// (ADR-0151).
#[must_use]
pub fn exe_writable(owner: u32, mode: u32, service_uid: u32) -> bool {
    owner == service_uid || mode & 0o022 != 0
}

/// The current base, as the verdict needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseFacts {
    /// `base-<n>`.
    pub id: String,
    /// It has `dockerd`.
    pub dockerd: bool,
}

/// What the verdict is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// `Err(why)` when the setting turns sandboxes' Docker off.
    pub setting: Result<(), String>,
    /// The server's cgroup is delegated.
    pub delegated: bool,
    /// The `yulius` executable could be rewritten by an engine.
    pub exe_writable: bool,
    /// Engine modules neither loaded nor built in.
    pub missing_modules: Vec<&'static str>,
    /// The host has `dockerd` (new bases copy it).
    pub host_dockerd: bool,
    /// The current base; `None` before the first.
    pub base: Option<BaseFacts>,
    /// A base build runs now.
    pub building: bool,
}

impl Facts {
    /// Reads the host's facts for `data_dir`: the executable `exe` against the data directory's
    /// owner (the service user), the modules, the host's and the current base's `dockerd`.
    #[must_use]
    pub fn read(
        data_dir: &Path,
        exe: &Path,
        setting: Result<(), String>,
        delegated: bool,
        building: bool,
    ) -> Self {
        let service_uid = std::fs::metadata(data_dir).map_or(u32::MAX, |m| m.uid());
        let exe_writable = std::fs::metadata(exe)
            .is_ok_and(|meta| exe_writable(meta.uid(), meta.mode(), service_uid));
        let bases = Bases::new(data_dir);
        Self {
            setting,
            delegated,
            exe_writable,
            missing_modules: host_missing_modules(),
            host_dockerd: has_dockerd(Path::new("/")),
            base: bases.current().map(|base| BaseFacts {
                dockerd: has_dockerd(&bases.path(&base.id)),
                id: base.id,
            }),
            building,
        }
    }
}

/// The verdict: whether sandboxes get Docker and, if not, why and the fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// Sandboxes get an engine (or Docker is deliberately off).
    pub ok: bool,
    /// What holds, or what is missing and the fix.
    pub detail: String,
}

/// The name every report uses.
pub const NAME: &str = "sandbox Docker";

/// The host has no `dockerd` (SPEC-199 DSY-004): new bases copy none, but a sandbox's engine
/// runs its own system's, so it can install one itself, with buildx and compose (SPEC-201).
pub const NO_HOST_DOCKERD: &str = "the host has no `dockerd`, so new sandbox bases have none: a \
    sandbox gets its engine once Docker is installed in it (`sudo apt-get install -y docker.io \
    docker-buildx docker-compose-v2`), or an operator runs `sudo yulius host setup`, which \
    installs Docker on the host (docker-ce with docker-buildx-plugin and docker-compose-plugin) \
    so every new base copies it";

/// Says whether sandboxes get Docker from `facts` (see the module docs).
#[must_use]
pub fn verdict(facts: &Facts) -> Verdict {
    let warn = |detail: String| Verdict { ok: false, detail };
    if let Err(off) = &facts.setting {
        return Verdict {
            ok: true,
            detail: format!("no engine in sandboxes: {off}"),
        };
    }
    if !facts.delegated {
        return warn(format!("no engine in sandboxes: {DOCKER_NO_DELEGATION}"));
    }
    if facts.exe_writable {
        return warn(format!(
            "no engine in sandboxes: {DOCKER_WRITABLE_EXE} (`sudo yulius host setup` does it)"
        ));
    }
    if !facts.missing_modules.is_empty() {
        let list = facts.missing_modules.join(" ");
        return warn(format!(
            "kernel modules a sandbox cannot load are missing: {list}; run `sudo yulius host \
             setup`, which loads them and lists them in {MODULES_FILE} (or `sudo modprobe -a \
             {list}` and add them to that file)"
        ));
    }
    if !facts.host_dockerd {
        return warn(NO_HOST_DOCKERD.to_owned());
    }
    match &facts.base {
        None => warn(format!(
            "no sandbox base yet: {}; sandboxes get Docker once it is ready",
            if facts.building {
                "the first one is being built"
            } else {
                "the server builds it in the background, or `sudo yulius host setup` builds it now"
            }
        )),
        Some(base) if !base.dockerd => warn(format!(
            "the current sandbox base ({}) has no `dockerd` though the host has one: {}; until \
             then only a sandbox with Docker installed in it gets an engine",
            base.id,
            if facts.building {
                "a new base is being built".to_owned()
            } else {
                format!(
                    "the server builds a new one once the host has not changed for {} minutes, \
                     or `sudo yulius host setup` builds it now",
                    REBUILD_DEBOUNCE.as_secs() / 60
                )
            }
        )),
        Some(_) => Verdict {
            ok: true,
            detail: "each isolated sandbox gets its own engine at /var/run/docker.sock \
                     (DOCKER_HOST) on first use; `sudo yulius doctor --docker-smoke` proves it on \
                     this host"
                .into(),
        },
    }
}

/// A sandbox that runs Docker installed in it instead of its base's (SPEC-203 HST-017).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnEngine {
    /// The sandbox's id.
    pub sandbox: String,
    /// The `dockerd` its engine runs (relative to its system).
    pub dockerd: &'static str,
}

/// How a sandbox on its own Docker goes back to Docker's packages (apt replaces `docker.io`;
/// removing it instead would leave whiteouts over the base's files at the same paths).
pub const OWN_ENGINE_FIX: &str = "to use Docker's packages, as the base does, run in that \
    sandbox `sudo apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin \
    docker-compose-plugin` (apt replaces docker.io; the engine switches when it next starts, \
    after it was idle); never `sudo apt-get remove docker.io`, which hides the base's Docker too";

/// Sandboxes under `data_dir` whose own system layer provides the `dockerd` their engine runs
/// while the current base has one too (smoke tests' throwaway sandboxes excluded).
#[must_use]
pub fn own_engines(data_dir: &Path) -> Vec<OwnEngine> {
    let bases = Bases::new(data_dir);
    let Some(base) = bases.current().map(|base| bases.path(&base.id)) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(data_dir.join("sysroots")) else {
        return Vec::new();
    };
    let mut found: Vec<OwnEngine> = entries
        .flatten()
        .filter_map(|entry| {
            let sandbox = entry.file_name().to_str()?.to_owned();
            if sandbox.starts_with("docker-smoke-") {
                return None;
            }
            let dockerd = own_engine(&base, &entry.path().join("upper"))?;
            Some(OwnEngine { sandbox, dockerd })
        })
        .collect();
    found.sort_by(|a, b| a.sandbox.cmp(&b.sandbox));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> Facts {
        Facts {
            setting: Ok(()),
            delegated: true,
            exe_writable: false,
            missing_modules: Vec::new(),
            host_dockerd: true,
            base: Some(BaseFacts {
                id: "base-9".into(),
                dockerd: true,
            }),
            building: false,
        }
    }

    #[test]
    fn a_ready_host_gives_sandboxes_docker() {
        let v = verdict(&ready());
        assert!(v.ok, "{}", v.detail);
        assert!(v.detail.contains("--docker-smoke"), "{}", v.detail);
    }

    /// SPEC-203 HST-013 (#5 of the task): the doctor never said a writable binary gives no engine.
    #[test]
    fn a_writable_binary_gives_no_engine_and_names_the_fix() {
        let v = verdict(&Facts {
            exe_writable: true,
            ..ready()
        });
        assert!(!v.ok);
        assert!(v.detail.contains(DOCKER_WRITABLE_EXE), "{}", v.detail);
        assert!(v.detail.contains("yulius host setup"), "{}", v.detail);
        assert!(exe_writable(1000, 0o100755, 1000));
        assert!(exe_writable(0, 0o100775, 1000));
        assert!(exe_writable(0, 0o100757, 1000));
        assert!(!exe_writable(0, 0o100755, 1000));
    }

    /// SPEC-203 HST-013 (#3): a base copied before the host had Docker gives no engine until it is
    /// rebuilt, and the verdict says whether that is happening.
    #[test]
    fn a_base_without_dockerd_is_named_with_its_rebuild() {
        let stale = Facts {
            base: Some(BaseFacts {
                id: "base-3".into(),
                dockerd: false,
            }),
            ..ready()
        };
        let waiting = verdict(&stale);
        assert!(!waiting.ok);
        for needle in ["base-3", "10 minutes", "yulius host setup"] {
            assert!(
                waiting.detail.contains(needle),
                "{needle}: {}",
                waiting.detail
            );
        }
        let building = verdict(&Facts {
            building: true,
            ..stale
        });
        assert!(
            building.detail.contains("being built"),
            "{}",
            building.detail
        );
        let none = verdict(&Facts {
            base: None,
            ..ready()
        });
        assert!(!none.ok);
        assert!(
            none.detail.contains("no sandbox base yet"),
            "{}",
            none.detail
        );
    }

    #[test]
    fn the_verdict_names_what_sandboxes_lack_in_order() {
        let off = verdict(&Facts {
            setting: Err("off for a reason".into()),
            delegated: false,
            ..ready()
        });
        assert!(off.ok);
        assert!(off.detail.contains("off for a reason"));
        let undelegated = verdict(&Facts {
            delegated: false,
            exe_writable: true,
            ..ready()
        });
        assert!(
            undelegated.detail.contains("Delegate=yes"),
            "{}",
            undelegated.detail
        );
        let modules = verdict(&Facts {
            missing_modules: vec!["overlay", "veth"],
            host_dockerd: false,
            ..ready()
        });
        assert!(
            modules.detail.contains("overlay veth"),
            "{}",
            modules.detail
        );
        assert!(modules.detail.contains(MODULES_FILE), "{}", modules.detail);
        let no_host = verdict(&Facts {
            host_dockerd: false,
            ..ready()
        });
        assert_eq!(no_host.detail, NO_HOST_DOCKERD);
        for v in [&undelegated, &modules, &no_host] {
            assert!(!v.ok);
            assert!(!v.detail.contains("  "), "{}", v.detail);
        }
    }

    /// SPEC-203 HST-018: the persisted list is exactly the modules the verdict requires.
    #[test]
    fn the_modules_file_lists_exactly_the_engine_modules() {
        let content = modules_file_content();
        let listed: Vec<&str> = content
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .collect();
        assert_eq!(listed, ENGINE_MODULES);
        let loaded: String = listed
            .iter()
            .map(|m| format!("{m} 1 0 - Live 0\n"))
            .collect();
        assert!(missing_modules(&loaded, "").is_empty());
        assert_eq!(
            missing_modules("", "kernel/net/bridge/bridge.ko\n").len(),
            ENGINE_MODULES.len() - 1
        );
    }

    /// SPEC-203 HST-017 (#7): a sandbox's own `docker.io` (`usr/sbin`) shadows the base's
    /// `docker-ce` (`usr/bin`); without one in the base it is no shadow; smokes are skipped.
    #[test]
    fn sandboxes_on_their_own_engine_are_listed() -> std::io::Result<()> {
        let data = tempfile::tempdir()?;
        let system = data.path().join("system");
        let base = system.join("base-2");
        std::fs::create_dir_all(base.join("usr/bin"))?;
        std::fs::write(base.join("usr/bin/dockerd"), "")?;
        std::fs::write(
            system.join("current.json"),
            format!(
                r#"{{"id":"base-2","fingerprint":"f","built_at_ms":0,"format":{}}}"#,
                crate::base::BASE_FORMAT
            ),
        )?;
        for sandbox in ["product", "docker-smoke-12", "plain", "same"] {
            std::fs::create_dir_all(data.path().join("sysroots").join(sandbox).join("upper"))?;
        }
        for sandbox in ["product", "docker-smoke-12"] {
            let sbin = data
                .path()
                .join("sysroots")
                .join(sandbox)
                .join("upper/usr/sbin");
            std::fs::create_dir_all(&sbin)?;
            std::fs::write(sbin.join("dockerd"), "")?;
        }
        // A sandbox that reinstalled Docker's packages in its own layer (the fix) is not listed.
        let same = data.path().join("sysroots/same/upper/usr/bin");
        std::fs::create_dir_all(&same)?;
        std::fs::write(same.join("dockerd"), "")?;
        let found = own_engines(data.path());
        assert_eq!(
            found,
            vec![OwnEngine {
                sandbox: "product".into(),
                dockerd: "usr/sbin/dockerd"
            },]
        );
        std::fs::remove_file(base.join("usr/bin/dockerd"))?;
        assert!(own_engines(data.path()).is_empty());
        Ok(())
    }
}
