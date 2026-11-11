use std::path::PathBuf;
use std::time::Duration;

use yulius_contracts::update::{HostLine, HostOutcome};

use super::fake::{FakeHost, NEW_ADDRESS, STAGED_BUILD, new_host};
use crate::host_setup::{Host, Out};
use crate::instance::Manifest;
use crate::instance::inventory::{Folder, Inventory};
use crate::instance::migrate::profile::{Release, TextFile};
use crate::instance::migrate::target::*;

const BUILD: &str = "0123456789abcdef0123456789abcdef01234567";

pub fn setup() -> Setup {
    Setup {
        data_dir: PathBuf::from("/var/lib/yulius"),
        user: "yulius".into(),
        service: "yulius.service".into(),
        exe: PathBuf::from("/usr/local/bin/yulius"),
    }
}

/// Runs one request with an environment that never waits and counts the copy as the old host.
pub fn step(host: &mut FakeHost, request: &Request) -> Reply {
    let mut env = Env {
        address: Some(NEW_ADDRESS.into()),
        ssh_port: Some(2222),
        peer: Some("109.199.123.240".into()),
        hold: &mut |_| Box::new(()),
        health: &mut |_| Some(BUILD.into()),
        sleep: &mut |_| {},
        count: &mut |_, old| Ok(old.clone()),
        setup: &mut |_, part, _| vec![line(&format!("{part:?}"), HostOutcome::Ok)],
    };
    handle(host, &mut env, request)
}

fn line(name: &str, outcome: HostOutcome) -> HostLine {
    HostLine {
        name: name.to_owned(),
        outcome,
        detail: String::new(),
    }
}

/// The one line of a step, as `outcome name: detail`.
fn only(reply: &Reply) -> (HostOutcome, &str) {
    assert_eq!(reply.lines.len(), 1, "{:?}", reply.lines);
    (reply.lines[0].outcome, reply.lines[0].detail.as_str())
}

fn facts_of(host: &mut FakeHost) -> Facts {
    step(
        host,
        &Request::Facts {
            data_dir: "/var/lib/yulius".into(),
            service: "yulius.service".into(),
            ports: vec![80, 443, 8080, 8081],
        },
    )
    .facts
    .unwrap_or_default()
}

#[test]
fn facts_describe_a_fresh_host() {
    let mut host = new_host();
    host.says(&["zstd", "--version"], "zstd 1.5.7\n");
    let f = facts_of(&mut host);
    assert_eq!(f.release.name(), "ubuntu 26.04");
    assert_eq!((f.arch.as_str(), f.systemd, f.cpus), ("x86_64", 259, 64));
    assert_eq!(f.memory, 791_674_880 * 1024);
    assert_eq!(f.address.as_deref(), Some(NEW_ADDRESS));
    assert_eq!(f.instance, Instance::None);
    // Counted where the data directory will be: its nearest existing parent.
    assert!(host.ran("stat -f -c %S %a %d /var/lib"));
    assert_eq!(
        (f.free_bytes, f.free_inodes),
        (4096 * 100_000_000, 30_000_000)
    );
    let blank: Vec<&str> = f.blank.iter().map(|d| d.path.as_str()).collect();
    assert_eq!(blank, ["/dev/nvme0n1", "/dev/nvme1n1"]);
    assert!(f.busy_ports.is_empty() && f.zstd && !f.mounted);
}

#[test]
fn an_instance_and_foreign_listeners_are_seen() {
    let mut host = new_host();
    host.says(
        &["ss", "-H", "-ltnp"],
        "LISTEN 0 511 0.0.0.0:80 0.0.0.0:* users:((\"nginx\",pid=7,fd=6),(\"nginx\",pid=8,fd=6))\n\
         LISTEN 0 4096 *:443 *:* users:((\"caddy\",pid=9,fd=7))\n\
         LISTEN 0 4096 [::]:8080 [::]:* users:((\"node\",pid=10,fd=20))\n\
         LISTEN 0 4096 127.0.0.1:5432 0.0.0.0:* users:((\"postgres\",pid=11,fd=5))\n",
    );
    host.fails(&["zstd", "--version"], "not found");
    host.file("/var/lib/yulius/yulius.db", "db");
    let f = facts_of(&mut host);
    assert_eq!(f.busy_ports, ["80 (nginx)", "8080 (node)"]);
    assert_eq!(f.instance, Instance::Present);
    assert!(!f.zstd);

    host.file("/var/lib/yulius/.yulius-import-incomplete", "");
    assert_eq!(facts_of(&mut host).instance, Instance::Incomplete);
    host.says(&["systemctl", "is-active", "yulius.service"], "active\n");
    assert_eq!(facts_of(&mut host).instance, Instance::Running);
}

fn disk_script(host: &mut FakeHost, argv: &[String]) -> Option<Out> {
    let said = |stdout: &str| {
        Some(Out {
            code: 0,
            stdout: stdout.to_owned(),
            stderr: String::new(),
        })
    };
    match argv[0].as_str() {
        "mount" => {
            host.file("/mounted", "");
            None
        }
        "findmnt" if host.is_file(std::path::Path::new("/mounted")) => said("/dev/md127\n"),
        "blkid" if argv[1] == "-s" => said("0a1b2c3d-1111-2222-3333-444455556666\n"),
        "mdadm" if argv[1] == "--version" && !host.ran("apt-get install -y mdadm") => Some(Out {
            code: 127,
            ..Out::default()
        }),
        "mdadm" if argv[1..] == ["--detail", "--scan"] => {
            said("ARRAY /dev/md/yulius metadata=1.2 UUID=aa:bb:cc:dd name=new:yulius\n")
        }
        _ => None,
    }
}

fn disk_request(disks: &[&str]) -> Request {
    Request::Disk {
        setup: setup(),
        disks: disks.iter().map(|&d| d.to_owned()).collect(),
    }
}

#[test]
fn two_blank_disks_become_a_mirror_at_the_data_directory() {
    let mut host = new_host();
    host.script = Some(disk_script);
    let reply = step(&mut host, &disk_request(&["/dev/nvme0n1", "/dev/nvme1n1"]));
    let (outcome, detail) = only(&reply);
    assert_eq!(outcome, HostOutcome::Changed, "{detail}");
    assert!(
        detail.contains("mirror of /dev/nvme0n1 and /dev/nvme1n1"),
        "{detail}"
    );
    // In this order, and nothing before the disks were found blank.
    let did: Vec<&str> = host
        .log
        .iter()
        .map(String::as_str)
        .filter(|l| !l.starts_with("findmnt") && !l.starts_with("lsblk"))
        .collect();
    assert_eq!(
        did,
        [
            "blkid -L yulius-data",
            "mdadm --detail /dev/md/yulius",
            "mdadm --version",
            "apt-get install -y mdadm",
            "mdadm --create /dev/md/yulius --run --level=1 --raid-devices=2 --metadata=1.2 \
             --name=yulius /dev/nvme0n1 /dev/nvme1n1",
            "udevadm settle",
            "mkfs.ext4 -q -L yulius-data /dev/md/yulius",
            "blkid -s UUID -o value /dev/md/yulius",
            "write /etc/fstab",
            "mkdir -p -- /var/lib/yulius",
            "mount -- /var/lib/yulius",
            "mdadm --detail --scan",
            "write /etc/mdadm/mdadm.conf",
            "update-initramfs -u",
            "write /etc/systemd/system/yulius.service.d/10-yulius-data-mount.conf",
            "useradd --system --home-dir /var/lib/yulius --shell /usr/sbin/nologin -- yulius",
            "chown -- yulius:yulius /var/lib/yulius",
            "chmod 0700 -- /var/lib/yulius",
        ]
    );
    assert_eq!(
        host.text("/etc/fstab"),
        Some(
            "UUID=1111-aaaa / ext4 defaults 0 1\n\
             UUID=0a1b2c3d-1111-2222-3333-444455556666 /var/lib/yulius ext4 defaults,nofail 0 2\n"
        )
    );
    assert_eq!(
        host.text("/etc/mdadm/mdadm.conf"),
        Some("ARRAY /dev/md/yulius metadata=1.2 UUID=aa:bb:cc:dd name=new:yulius\n")
    );
    let drop_in = host.text("/etc/systemd/system/yulius.service.d/10-yulius-data-mount.conf");
    assert!(drop_in.is_some_and(|t| t.ends_with("[Unit]\nRequiresMountsFor=/var/lib/yulius\n")));

    // Again: mounted, nothing made.
    host.log.clear();
    let reply = step(&mut host, &disk_request(&["/dev/nvme0n1", "/dev/nvme1n1"]));
    assert_eq!(only(&reply).0, HostOutcome::Ok);
    assert!(!host.ran("mkfs") && !host.ran("mdadm") && !host.ran("write"));
}

#[test]
fn one_disk_gets_no_mirror_and_an_existing_user_is_kept() {
    let mut host = new_host();
    host.script = Some(disk_script);
    host.file(
        "/etc/passwd",
        "root:x:0:0::/root:/bin/sh\nyulius:x:996:995::/var/lib/yulius:/n\n",
    );
    let reply = step(&mut host, &disk_request(&["/dev/nvme1n1"]));
    assert_eq!(only(&reply).0, HostOutcome::Changed);
    assert!(host.ran("mkfs.ext4 -q -L yulius-data /dev/nvme1n1"));
    assert!(!host.ran("mdadm") && !host.ran("update-initramfs") && !host.ran("useradd"));
}

#[test]
fn a_disk_that_is_not_blank_changes_nothing() {
    for disks in [
        vec!["/dev/nvme0n1", "/dev/nvme2n1"],
        vec!["/dev/sda"],
        vec!["/dev/nope"],
        vec![],
        vec!["/dev/nvme0n1", "/dev/nvme1n1", "/dev/nvme1n1"],
        vec!["/dev/nvme0n1", "/dev/nvme0n1"],
    ] {
        let mut host = new_host();
        let reply = step(&mut host, &disk_request(&disks));
        assert_eq!(only(&reply).0, HostOutcome::Failed, "{disks:?}");
        assert!(
            !host.ran("mkfs") && !host.ran("mdadm --create") && !host.ran("write"),
            "{:?}",
            host.log
        );
    }
    // A data directory that already holds files is never mounted over.
    let mut host = new_host();
    host.file("/var/lib/yulius/keep", "x");
    let reply = step(&mut host, &disk_request(&["/dev/nvme0n1"]));
    let (outcome, detail) = only(&reply);
    assert_eq!(outcome, HostOutcome::Failed);
    assert!(
        detail.contains("holds files") && !host.ran("mkfs"),
        "{detail}"
    );
}

fn source(name: &str, new: &str, old: &str) -> SourceFile {
    SourceFile {
        file: TextFile {
            path: PathBuf::from(format!("/etc/apt/sources.list.d/{name}")),
            mode: 0o644,
            content: new.to_owned(),
        },
        fallback: old.to_owned(),
    }
}

fn two_sources() -> Vec<SourceFile> {
    vec![
        source(
            "docker.list",
            "deb https://download.docker.com/linux/ubuntu resolute stable\n",
            "deb https://download.docker.com/linux/ubuntu noble stable\n",
        ),
        source(
            "gh.sources",
            "URIs: https://cli.github.com/packages\nSuites: stable\n",
            "URIs: https://cli.github.com/packages\nSuites: stable\n",
        ),
    ]
}

/// `apt-get update` fails for a repository whose source names a release in `MISSING`.
fn apt_script(host: &mut FakeHost, argv: &[String]) -> Option<Out> {
    if argv[..2] != ["apt-get", "update"] {
        return None;
    }
    let docker = host.text("/etc/apt/sources.list.d/docker.list")?;
    let suite = docker.split_whitespace().nth(2)?.to_owned();
    let missing = host.text("/missing-suites").unwrap_or_default();
    missing.split(' ').any(|m| m == suite).then(|| Out {
        code: 100,
        stdout: String::new(),
        stderr: format!(
            "E: The repository 'https://download.docker.com/linux/ubuntu {suite} Release' does \
             not have a Release file.\n"
        ),
    })
}

#[test]
fn sources_are_written_once() {
    let mut host = new_host();
    let reply = step(
        &mut host,
        &Request::Sources {
            files: two_sources(),
        },
    );
    let (outcome, detail) = only(&reply);
    assert_eq!(
        (outcome, detail),
        (HostOutcome::Changed, "2 third-party sources")
    );
    assert_eq!(host.runs("write").len(), 2);
    assert!(host.ran("apt-get update"));
    host.log.clear();
    let reply = step(
        &mut host,
        &Request::Sources {
            files: two_sources(),
        },
    );
    assert_eq!(only(&reply).0, HostOutcome::Ok);
    assert!(!host.ran("write"));
}

#[test]
fn a_source_without_the_new_suite_keeps_the_old_one() {
    let mut host = new_host();
    host.script = Some(apt_script);
    host.file("/missing-suites", "resolute");
    let reply = step(
        &mut host,
        &Request::Sources {
            files: two_sources(),
        },
    );
    let (outcome, detail) = only(&reply);
    assert_eq!(outcome, HostOutcome::Changed, "{detail}");
    assert!(detail.contains("kept the old release's suite") && detail.contains("docker.list"));
    assert_eq!(
        host.text("/etc/apt/sources.list.d/docker.list"),
        Some("deb https://download.docker.com/linux/ubuntu noble stable\n")
    );
    assert_eq!(host.runs("apt-get update").len(), 2);
}

#[test]
fn a_source_that_never_works_is_removed_and_named() {
    let mut host = new_host();
    host.script = Some(apt_script);
    host.file("/missing-suites", "resolute noble");
    let reply = step(
        &mut host,
        &Request::Sources {
            files: two_sources(),
        },
    );
    let (outcome, detail) = only(&reply);
    assert_eq!(outcome, HostOutcome::Manual, "{detail}");
    assert!(detail.starts_with("1 third-party sources") && detail.contains("docker.list"));
    assert!(host.ran("rm -f -- /etc/apt/sources.list.d/docker.list"));

    // Something else is wrong with apt: the step fails.
    let mut host = new_host();
    host.fails(
        &["apt-get", "update"],
        "E: Could not get lock /var/lib/apt/lists/lock",
    );
    let reply = step(
        &mut host,
        &Request::Sources {
            files: two_sources(),
        },
    );
    assert_eq!(only(&reply).0, HostOutcome::Failed);
    // And a path outside apt's directory is never written.
    let mut host = new_host();
    let mut files = two_sources();
    files[0].file.path = PathBuf::from("/etc/cron.d/x.list");
    let reply = step(&mut host, &Request::Sources { files });
    assert_eq!(only(&reply).0, HostOutcome::Failed);
    assert!(!host.ran("write"));
}

fn package_host() -> FakeHost {
    let mut host = new_host();
    host.says(
        &["apt-cache", "policy"],
        "caddy:\n  Installed: (none)\n  Candidate: 2.10.0\n  Version table:\n     2.10.0 500\n\
         gh:\n  Installed: 2.83.0\n  Candidate: 2.83.0\n\
         libicu74:\n  Installed: (none)\n  Candidate: (none)\n\
         zstd:\n  Installed: 1.5.7\n  Candidate: 1.5.7\n",
    );
    host.says(
        &[
            "apt-cache",
            "--names-only",
            "search",
            "--",
            "^libicu[0-9.]+(t64)?$",
        ],
        "libicu78 - International Components for Unicode\n",
    );
    host.says(
        &["dpkg-query", "-W"],
        "gh installed\nzstd installed\ncaddy not-installed\n",
    );
    host
}

fn names() -> Vec<String> {
    [
        "caddy",
        "gh",
        "libicu74",
        "linux-image-6.8.0-87-generic",
        "oldtool",
        "zstd",
        "--purge",
        // Selected by the coreutils step: asked for here it conflicts with the other flavour.
        "coreutils-from-gnu",
    ]
    .map(String::from)
    .to_vec()
}

#[test]
fn the_plan_says_which_packages_fit_the_new_release() {
    let mut host = package_host();
    let reply = step(
        &mut host,
        &Request::Packages {
            names: names(),
            install: false,
        },
    );
    let (outcome, detail) = only(&reply);
    assert_eq!(outcome, HostOutcome::Manual, "{detail}");
    assert_eq!(
        detail,
        "would install 4 of the old host's packages; renamed: libicu74 → libicu78; 1 left to \
         this machine (kernel, boot, network); not on this release, install what replaces them: \
         oldtool"
    );
    assert!(!host.log.iter().any(|l| l.contains("coreutils-from-gnu")));
    // Nothing installed, and an option look-alike never reaches apt.
    assert!(
        host.ran("apt-get update")
            && !host.ran("apt-get install")
            && !host.log.iter().any(|l| l.contains("--purge"))
    );
}

#[test]
fn packages_are_installed_in_one_transaction_and_only_once() {
    let mut host = package_host();
    let mut wanted = names();
    wanted.retain(|n| n != "oldtool");
    let reply = step(
        &mut host,
        &Request::Packages {
            names: wanted.clone(),
            install: true,
        },
    );
    let (outcome, detail) = only(&reply);
    assert_eq!(outcome, HostOutcome::Changed, "{detail}");
    assert_eq!(
        host.runs("apt-get install"),
        ["apt-get install -y -o Dpkg::Options::=--force-confdef -o \
          Dpkg::Options::=--force-confold -- caddy libicu78"]
    );
    host.says(
        &["dpkg-query", "-W"],
        "gh installed\nzstd installed\ncaddy installed\nlibicu78 installed\n",
    );
    host.log.clear();
    let reply = step(
        &mut host,
        &Request::Packages {
            names: wanted.clone(),
            install: true,
        },
    );
    assert_eq!(only(&reply).0, HostOutcome::Ok);
    assert!(!host.ran("apt-get install"));

    host.says(&["dpkg-query", "-W"], "");
    host.fails(&["apt-get", "install"], "E: Unable to correct problems");
    let reply = step(
        &mut host,
        &Request::Packages {
            names: wanted,
            install: true,
        },
    );
    assert_eq!(only(&reply).0, HostOutcome::Failed);
}

fn flavour_script(host: &mut FakeHost, argv: &[String]) -> Option<Out> {
    if argv.iter().any(|a| a == "coreutils-from-gnu") && argv[0] == "apt-get" {
        host.file("/gnu-selected", "");
    }
    (argv[0] == "ls" && host.is_file(std::path::Path::new("/gnu-selected"))).then(|| Out {
        code: 0,
        stdout: "ls (GNU coreutils) 9.7\n".into(),
        stderr: String::new(),
    })
}

#[test]
fn coreutils_become_the_old_hosts_flavour() {
    let request = Request::Coreutils {
        flavour: "GNU".into(),
    };
    let mut host = new_host();
    host.script = Some(flavour_script);
    host.says(
        &["apt-cache", "policy", "--", "coreutils-from-gnu"],
        "coreutils-from-gnu:\n  Installed: (none)\n  Candidate: 0.0.0~ubuntu25\n",
    );
    let reply = step(&mut host, &request);
    let (outcome, detail) = only(&reply);
    assert_eq!(outcome, HostOutcome::Changed, "{detail}");
    assert!(host.ran(
        "apt-get install -y --allow-remove-essential coreutils-from-gnu coreutils-from-uutils-"
    ));
    // Already GNU: nothing to do.
    host.log.clear();
    assert_eq!(only(&step(&mut host, &request)).0, HostOutcome::Ok);
    assert!(!host.ran("apt-get"));

    // A release without the selector package, and an install that changes nothing.
    let mut host = new_host();
    assert_eq!(only(&step(&mut host, &request)).0, HostOutcome::Manual);
    host.says(
        &["apt-cache", "policy", "--", "coreutils-from-gnu"],
        "coreutils-from-gnu:\n  Candidate: 0.0.0\n",
    );
    assert_eq!(only(&step(&mut host, &request)).0, HostOutcome::Failed);
}

#[test]
fn npm_packages_get_the_old_hosts_versions() {
    let wanted = vec![
        "@anthropic-ai/claude-code@2.1.311".to_owned(),
        "@openai/codex@0.98.0".to_owned(),
    ];
    let request = Request::Npm { packages: wanted };
    let mut host = new_host();
    host.says(
        &["npm", "ls", "-g"],
        r#"{"dependencies":{"@openai/codex":{"version":"0.98.0"},"@anthropic-ai/claude-code":{"version":"2.0.0"}}}"#,
    );
    let reply = step(&mut host, &request);
    assert_eq!(only(&reply).0, HostOutcome::Changed);
    assert!(host.ran("npm install -g -- @anthropic-ai/claude-code@2.1.311"));
    assert!(
        !host
            .log
            .iter()
            .any(|l| l.starts_with("npm install") && l.contains("codex"))
    );

    let mut host = new_host();
    host.fails(&["npm", "--version"], "not found");
    assert_eq!(only(&step(&mut host, &request)).0, HostOutcome::Failed);
    // Nothing asked, nothing said.
    assert!(
        step(&mut host, &Request::Npm { packages: vec![] })
            .lines
            .is_empty()
    );
}

#[test]
fn the_staged_build_is_installed_at_the_units_path() {
    let request = Request::Binary { setup: setup() };
    let mut host = new_host();
    let reply = step(&mut host, &request);
    assert_eq!(only(&reply).0, HostOutcome::Changed);
    assert!(host.ran(&format!(
        "install -D -m 0755 -o root -g root -- {STAGED_BUILD} /usr/local/bin/yulius.migrate-new"
    )));
    assert!(host.ran("mv -f -- /usr/local/bin/yulius.migrate-new /usr/local/bin/yulius"));

    host.says(&["cmp", "-s"], "");
    host.log.clear();
    assert_eq!(only(&step(&mut host, &request)).0, HostOutcome::Ok);
    assert!(!host.ran("install"));

    // A running service with another build is never replaced under it.
    let mut host = new_host();
    host.says(&["systemctl", "is-active", "yulius.service"], "active\n");
    assert_eq!(only(&step(&mut host, &request)).0, HostOutcome::Failed);
    assert!(!host.ran("install"));
    let mut host = new_host();
    host.files.remove(std::path::Path::new(STAGED_BUILD));
    assert_eq!(only(&step(&mut host, &request)).0, HostOutcome::Failed);
}

#[test]
fn the_data_directory_is_made_ready_for_a_copy() {
    let request = Request::DataDir { setup: setup() };
    // Not there yet.
    let mut host = new_host();
    assert_eq!(only(&step(&mut host, &request)).0, HostOutcome::Ok);

    // A new disk's mount point belongs to root.
    let mut host = new_host();
    host.dir("/var/lib/yulius/lost+found");
    host.file(
        "/etc/passwd",
        "yulius:x:996:995::/var/lib/yulius:/usr/sbin/nologin\n",
    );
    assert_eq!(only(&step(&mut host, &request)).0, HostOutcome::Changed);
    assert!(host.ran("chown -- yulius:yulius /var/lib/yulius"));
    assert!(host.ran("chmod 0700 -- /var/lib/yulius"));

    // An unfinished copy is removed, the filesystem's lost+found stays.
    let mut host = new_host();
    host.dir("/var/lib/yulius/lost+found");
    host.dir("/var/lib/yulius/sandboxes");
    host.file("/var/lib/yulius/yulius.db", "half");
    host.file("/var/lib/yulius/.yulius-import-incomplete", "");
    let reply = step(&mut host, &request);
    assert_eq!(only(&reply).0, HostOutcome::Changed);
    assert_eq!(
        host.runs("rm"),
        [
            "rm -rf --one-file-system -- /var/lib/yulius/.yulius-import-incomplete",
            "rm -rf --one-file-system -- /var/lib/yulius/sandboxes",
            "rm -rf --one-file-system -- /var/lib/yulius/yulius.db",
        ]
    );

    // The base this host built before the copy is its own: it stays, also when an unfinished
    // copy beside it is removed.
    let mut host = new_host();
    host.dir("/var/lib/yulius/system/base-1");
    host.file("/etc/passwd", "yulius:x:996:995::/var/lib/yulius:/n\n");
    host.file_as("/var/lib/yulius/.bashrc", "skel", 996, 0o644);
    // Not refused for holding the base (the fake's directories belong to root: it is given).
    assert_eq!(only(&step(&mut host, &request)).0, HostOutcome::Changed);
    assert!(!host.ran("rm"));
    host.file("/var/lib/yulius/.yulius-import-incomplete", "");
    host.file("/var/lib/yulius/master.key", "half");
    assert_eq!(only(&step(&mut host, &request)).0, HostOutcome::Changed);
    assert!(
        !host.runs("rm").iter().any(|l| l.ends_with("/system")),
        "{:?}",
        host.runs("rm")
    );

    // An instance, or anything else, is never removed.
    let mut host = new_host();
    host.file("/var/lib/yulius/yulius.db", "db");
    let reply = step(&mut host, &request);
    assert_eq!(only(&reply).0, HostOutcome::Failed);
    assert!(!host.ran("rm"));
}

#[test]
fn text_files_are_written_when_they_differ() {
    let files = vec![
        TextFile {
            path: "/etc/systemd/system/yulius.service".into(),
            mode: 0o644,
            content: "[Service]\n".into(),
        },
        TextFile {
            path: "/etc/systemd/system/yulius.service.d/20-limits.conf".into(),
            mode: 0o600,
            content: "[Service]\nEnvironment=YULIUS_MAX_SESSIONS=8\n".into(),
        },
    ];
    let request = Request::Texts {
        name: "unit".into(),
        files,
    };
    let mut host = new_host();
    let reply = step(&mut host, &request);
    let (outcome, detail) = only(&reply);
    assert_eq!(outcome, HostOutcome::Changed);
    assert!(detail.contains("yulius.service") && detail.contains("20-limits.conf"));
    assert_eq!(reply.lines[0].name, "unit");
    assert!(host.ran("systemctl daemon-reload"));
    let mode = host.stat(std::path::Path::new(
        "/etc/systemd/system/yulius.service.d/20-limits.conf",
    ));
    assert_eq!(mode.map(|(_, m)| m & 0o7777), Some(0o600));

    host.log.clear();
    assert_eq!(only(&step(&mut host, &request)).0, HostOutcome::Ok);
    assert!(host.log.is_empty());

    // Not a unit: no reload. A path that climbs is refused.
    let other = Request::Texts {
        name: "proxy files".into(),
        files: vec![TextFile {
            path: "/etc/caddy/Caddyfile".into(),
            mode: 0o644,
            content: "x".into(),
        }],
    };
    assert_eq!(only(&step(&mut host, &other)).0, HostOutcome::Changed);
    assert!(!host.ran("systemctl"));
    let bad = Request::Texts {
        name: "unit".into(),
        files: vec![TextFile {
            path: "/etc/../root/.ssh/authorized_keys".into(),
            mode: 0o644,
            content: "x".into(),
        }],
    };
    assert_eq!(only(&step(&mut host, &bad)).0, HostOutcome::Failed);
}

#[test]
fn the_proxy_starts_only_with_a_valid_configuration() {
    let mut host = new_host();
    let reply = step(&mut host, &Request::Proxy { changed: true });
    assert_eq!(only(&reply).0, HostOutcome::Changed);
    assert!(host.ran("caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile"));
    assert!(host.ran("systemctl enable caddy") && host.ran("systemctl reload-or-restart caddy"));

    // Running and unchanged.
    host.says(&["systemctl", "is-active", "caddy"], "active\n");
    host.says(&["systemctl", "is-enabled", "caddy"], "enabled\n");
    host.log.clear();
    assert_eq!(
        only(&step(&mut host, &Request::Proxy { changed: false })).0,
        HostOutcome::Ok
    );
    assert!(!host.ran("systemctl reload-or-restart"));
    // Running, but its files changed.
    assert_eq!(
        only(&step(&mut host, &Request::Proxy { changed: true })).0,
        HostOutcome::Changed
    );

    let mut host = new_host();
    host.fails(
        &["caddy", "validate"],
        "Error: adapting config: unrecognized directive",
    );
    let reply = step(&mut host, &Request::Proxy { changed: true });
    let (outcome, detail) = only(&reply);
    assert_eq!(outcome, HostOutcome::Failed);
    assert!(detail.contains("unrecognized directive") && !host.ran("systemctl enable"));
}

fn inventory_of(entries: u64) -> Inventory {
    Inventory {
        entries,
        bytes: 5 << 30,
        fingerprint: "f".repeat(64),
        folders: vec![Folder {
            name: "sandboxes".into(),
            entries,
            bytes: 5 << 30,
            fingerprint: "a".repeat(64),
        }],
    }
}

fn inventory_request() -> Request {
    Request::Inventory {
        manifest: Manifest {
            data_dir: "/var/lib/yulius".into(),
            ..Manifest::default()
        },
        old: inventory_of(6_388_083),
        release: Release::parse("ID=ubuntu\nVERSION_ID=\"24.04\"\nVERSION_CODENAME=noble\n"),
    }
}

#[test]
fn an_equal_inventory_proves_the_copy_and_a_different_one_fails() {
    let mut host = new_host();
    let reply = step(&mut host, &inventory_request());
    let (outcome, detail) = only(&reply);
    assert_eq!(outcome, HostOutcome::Ok);
    assert_eq!(
        detail,
        "6388083 entries and 5.0 GiB in 1 folders, the same on both hosts"
    );
    assert_eq!(reply.inventory, Some(inventory_of(6_388_083)));

    let mut env = Env {
        address: None,
        ssh_port: None,
        peer: None,
        hold: &mut |_| Box::new(()),
        health: &mut |_| None,
        sleep: &mut |_| {},
        count: &mut |_, _| Ok(inventory_of(6_388_082)),
        setup: &mut |_, _, _| Vec::new(),
    };
    let reply = handle(&mut host, &mut env, &inventory_request());
    let (outcome, detail) = only(&reply);
    assert_eq!(outcome, HostOutcome::Failed);
    assert!(detail.contains("sandboxes: 6388083 entries"), "{detail}");

    let mut env = Env {
        count: &mut |_, _| Err("data/x belongs to 5:5".into()),
        ..env
    };
    let reply = handle(&mut host, &mut env, &inventory_request());
    assert_eq!(only(&reply).0, HostOutcome::Failed);
}

#[test]
fn sandboxes_built_on_another_release_are_named() {
    let mut host = new_host();
    host.file(
        "/var/lib/yulius/sysroots/sb-one/upper/var/lib/dpkg/status",
        "Package: x\n",
    );
    host.file(
        "/var/lib/yulius/sysroots/sb-two/upper/var/lib/dpkg/status",
        "Package: y\n",
    );
    host.dir("/var/lib/yulius/sysroots/sb-plain/upper");
    let reply = step(&mut host, &inventory_request());
    assert_eq!(reply.lines.len(), 2);
    let note = &reply.lines[1];
    assert_eq!(
        (note.name.as_str(), note.outcome),
        ("sandbox packages", HostOutcome::Manual)
    );
    assert!(
        note.detail
            .starts_with("2 sandboxes hold packages installed on ubuntu 24.04")
    );
    assert!(note.detail.ends_with("sb-one, sb-two"), "{}", note.detail);

    // The same release: nothing to say.
    host.file(
        "/etc/os-release",
        "ID=ubuntu\nVERSION_ID=\"24.04\"\nVERSION_CODENAME=noble\n",
    );
    assert_eq!(step(&mut host, &inventory_request()).lines.len(), 1);
}

#[test]
fn the_copy_is_marked_for_its_first_start() {
    let request = Request::Mark {
        setup: setup(),
        from: Some("https://old.example".into()),
        to: "https://new.example".into(),
    };
    let mut host = new_host();
    assert_eq!(only(&step(&mut host, &request)).0, HostOutcome::Changed);
    let mark = host
        .text("/var/lib/yulius/move-pending.json")
        .unwrap_or_default();
    assert!(mark.contains(r#""from":"https://old.example""#) && mark.contains(r#""version":1"#));
    assert!(host.ran("chown -- yulius:yulius /var/lib/yulius/move-pending.json"));
    host.log.clear();
    assert_eq!(only(&step(&mut host, &request)).0, HostOutcome::Ok);
    assert!(host.log.is_empty());

    // The copy of an instance that moved before carries that move's report, and it is still
    // marked unfinished until this step: the report goes, the mark is written, the copy is
    // proven.
    let mut host = new_host();
    host.file(
        "/var/lib/yulius/moved.json",
        r#"{"queued":7,"left":0,"failed":0}"#,
    );
    host.file("/var/lib/yulius/.yulius-import-incomplete", "");
    assert_eq!(only(&step(&mut host, &request)).0, HostOutcome::Changed);
    assert!(host.text("/var/lib/yulius/moved.json").is_none());
    assert!(host.text("/var/lib/yulius/move-pending.json").is_some());
    assert!(
        host.text("/var/lib/yulius/.yulius-import-incomplete")
            .is_none()
    );
}

#[test]
fn health_says_whether_the_instance_reports_the_build() {
    let request = |build: &str| Request::Health {
        build: Some(build.into()),
        health: "127.0.0.1:8081".into(),
    };
    let mut host = new_host();
    let reply = step(&mut host, &request(BUILD));
    assert!(reply.started && reply.lines.is_empty());
    assert!(!step(&mut host, &request(&"f".repeat(40))).started);
}

#[test]
fn a_copy_that_is_not_proven_is_never_started() {
    let mut host = new_host();
    host.file("/var/lib/yulius/.yulius-import-incomplete", "");
    let reply = step(&mut host, &start_request());
    assert!(!reply.started);
    let (outcome, detail) = only(&reply);
    assert_eq!(outcome, HostOutcome::Failed);
    assert!(detail.contains("not proven"), "{detail}");
    assert!(!host.ran("systemctl"));
}

fn start_request() -> Request {
    Request::Start {
        setup: setup(),
        build: Some(BUILD.into()),
        health: "127.0.0.1:8081".into(),
        helper: Some("yulius-update.path".into()),
    }
}

#[test]
fn a_started_instance_must_report_the_old_hosts_build() {
    let mut host = new_host();
    let mut asked = 0;
    let mut slept = Duration::ZERO;
    let mut env = Env {
        address: None,
        ssh_port: None,
        peer: None,
        hold: &mut |_| Box::new(()),
        health: &mut |address| {
            assert_eq!(address, "127.0.0.1:8081");
            asked += 1;
            // Not up yet, then an older build still answering, then the right one.
            match asked {
                1 => None,
                2 => Some("f".repeat(40)),
                _ => Some(BUILD.into()),
            }
        },
        sleep: &mut |d| slept += d,
        count: &mut |_, old| Ok(old.clone()),
        setup: &mut |_, part, _| vec![line(&format!("{part:?}"), HostOutcome::Ok)],
    };
    let reply = handle(&mut host, &mut env, &start_request());
    let names: Vec<&str> = reply.lines.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(
        names,
        ["BeforeStart", "new instance", "update helper", "AfterStart"]
    );
    assert!(reply.started);
    assert_eq!(reply.lines[1].outcome, HostOutcome::Changed);
    assert!(reply.lines[1].detail.ends_with("runs build 0123456"));
    assert_eq!(slept, Duration::from_secs(2));
    assert_eq!(
        host.runs("systemctl"),
        [
            "systemctl daemon-reload",
            "systemctl enable yulius.service",
            "systemctl start yulius.service",
            "systemctl enable --now yulius-update.path",
        ]
    );
}

#[test]
fn an_instance_that_never_reports_fails_and_a_host_not_ready_is_not_started() {
    let mut host = new_host();
    let mut slept = Duration::ZERO;
    let mut env = Env {
        address: None,
        ssh_port: None,
        peer: None,
        hold: &mut |_| Box::new(()),
        health: &mut |_| None,
        sleep: &mut |d| slept += d,
        count: &mut |_, old| Ok(old.clone()),
        setup: &mut |_, part, _| vec![line(&format!("{part:?}"), HostOutcome::Ok)],
    };
    let reply = handle(&mut host, &mut env, &start_request());
    let last = reply.lines.last().map(|l| (l.name.as_str(), l.outcome));
    assert_eq!(last, Some(("new instance", HostOutcome::Failed)));
    assert!(!reply.started);
    assert_eq!(slept, Duration::from_secs(120));
    assert!(!host.ran("systemctl enable --now yulius-update.path"));

    let mut host = new_host();
    let mut env = Env {
        address: None,
        ssh_port: None,
        peer: None,
        hold: &mut |_| Box::new(()),
        health: &mut |_| Some(BUILD.into()),
        sleep: &mut |_| {},
        count: &mut |_, old| Ok(old.clone()),
        setup: &mut |_, _, _| vec![line("sandbox base", HostOutcome::Failed)],
    };
    let reply = handle(&mut host, &mut env, &start_request());
    assert_eq!(reply.lines.len(), 2);
    assert_eq!(reply.lines[1].outcome, HostOutcome::Failed);
    assert!(!host.ran("systemctl") && !reply.started);

    // It reports the build, then something after that fails: it has started all the same.
    let mut host = new_host();
    host.fails(
        &["systemctl", "enable", "--now", "yulius-update.path"],
        "Unit not found",
    );
    let reply = step(&mut host, &start_request());
    assert!(reply.started);
    let helper = reply.lines.iter().find(|l| l.name == "update helper");
    assert_eq!(helper.map(|l| l.outcome), Some(HostOutcome::Failed));
}

#[test]
fn stopping_leaves_nothing_of_the_copy_running() {
    let mut host = new_host();
    let reply = step(
        &mut host,
        &Request::Stop {
            service: "yulius.service".into(),
        },
    );
    assert_eq!(only(&reply).0, HostOutcome::Changed);
    assert_eq!(
        host.runs("systemctl")[..2],
        [
            "systemctl disable --now yulius.service",
            "systemctl kill --kill-whom=all --signal=SIGKILL yulius.service",
        ]
    );

    // An older systemd knows the option by its earlier name.
    let mut host = new_host();
    host.fails(
        &["systemctl", "kill", "--kill-whom=all"],
        "unrecognized option",
    );
    let reply = step(
        &mut host,
        &Request::Stop {
            service: "yulius.service".into(),
        },
    );
    assert_eq!(only(&reply).0, HostOutcome::Changed);
    assert!(host.ran("systemctl kill --kill-who=all --signal=SIGKILL yulius.service"));

    // Processes that outlive the kill: not stopped, said so after a bounded wait.
    let mut host = new_host();
    host.says(
        &["systemctl", "show", "--property=ControlGroup"],
        "/system.slice/yulius.service\n",
    );
    host.file(
        "/sys/fs/cgroup/system.slice/yulius.service/cgroup.procs",
        "4242\n",
    );
    let mut slept = 0;
    let mut env = Env {
        address: None,
        ssh_port: None,
        peer: None,
        hold: &mut |_| Box::new(()),
        health: &mut |_| None,
        sleep: &mut |_| slept += 1,
        count: &mut |_, old| Ok(old.clone()),
        setup: &mut |_, _, _| Vec::new(),
    };
    let reply = handle(
        &mut host,
        &mut env,
        &Request::Stop {
            service: "yulius.service".into(),
        },
    );
    let (outcome, detail) = only(&reply);
    assert_eq!(outcome, HostOutcome::Failed);
    assert!(
        detail.contains("still run") && slept == 15,
        "{detail} after {slept}"
    );
}

#[test]
fn a_disk_step_that_was_cut_is_taken_up_where_it_stopped() {
    // The array was made, the filesystem was not: no second create, and its members are not
    // refused for no longer being blank.
    let mut host = new_host();
    host.script = Some(disk_script);
    host.says(
        &["mdadm", "--detail", "/dev/md/yulius"],
        "/dev/md/yulius:\n Raid Level : raid1\n 0 /dev/nvme0n1\n 1 /dev/nvme1n1\n",
    );
    host.says(&["lsblk"], r#"{"blockdevices":[]}"#);
    let reply = step(&mut host, &disk_request(&["/dev/nvme0n1", "/dev/nvme1n1"]));
    assert_eq!(only(&reply).0, HostOutcome::Changed, "{:?}", reply.lines);
    assert!(!host.ran("mdadm --create") && host.ran("mkfs.ext4 -q -L yulius-data /dev/md/yulius"));

    // The filesystem was made, the mount was not: nothing is formatted again, and the fstab
    // line is written once.
    let mut host = new_host();
    host.script = Some(disk_script);
    host.says(&["blkid", "-L", "yulius-data"], "/dev/md127\n");
    host.says(&["lsblk"], r#"{"blockdevices":[]}"#);
    host.file(
        "/etc/fstab",
        "UUID=0a1b2c3d-1111-2222-3333-444455556666 /var/lib/yulius ext4 defaults,nofail 0 2\n",
    );
    let reply = step(&mut host, &disk_request(&["/dev/nvme0n1", "/dev/nvme1n1"]));
    assert_eq!(only(&reply).0, HostOutcome::Changed, "{:?}", reply.lines);
    assert!(!host.ran("mkfs") && !host.ran("mdadm --create") && !host.ran("write /etc/fstab"));
    assert!(host.ran("mount -- /var/lib/yulius"));
}

#[test]
fn the_first_starts_report_on_the_tasks_is_read() {
    let request = Request::Tasks {
        data_dir: "/var/lib/yulius".into(),
        wait_secs: 5,
    };
    let mut host = new_host();
    host.file(
        "/var/lib/yulius/moved.json",
        r#"{"queued":11,"left":1,"failed":0}"#,
    );
    let reply = step(&mut host, &request);
    let (outcome, detail) = only(&reply);
    assert_eq!(outcome, HostOutcome::Ok);
    assert!(detail.starts_with("11 tasks that were live are back in the queue; 1 a person"));

    host.file(
        "/var/lib/yulius/moved.json",
        r#"{"queued":9,"left":0,"failed":2}"#,
    );
    let reply = step(&mut host, &request);
    let (outcome, detail) = only(&reply);
    assert_eq!(outcome, HostOutcome::Manual);
    assert!(detail.contains("2 could not be queued"), "{detail}");

    // Still pending after the wait: said, not failed.
    let mut host = new_host();
    host.file("/var/lib/yulius/move-pending.json", "{}");
    let mut slept = 0;
    let mut env = Env {
        address: None,
        ssh_port: None,
        peer: None,
        hold: &mut |_| Box::new(()),
        health: &mut |_| None,
        sleep: &mut |_| slept += 1,
        count: &mut |_, old| Ok(old.clone()),
        setup: &mut |_, _, _| Vec::new(),
    };
    let reply = handle(&mut host, &mut env, &request);
    assert_eq!(only(&reply).0, HostOutcome::Manual);
    assert_eq!(slept, 5);
}

#[test]
fn finishing_removes_the_key_and_the_flows_files_and_enables_the_firewall() {
    let mut host = new_host();
    host.file_as(
        "/root/.ssh/authorized_keys",
        "ssh-ed25519 AAAA1 admin@laptop\nssh-ed25519 AAAA2 yulius-migrate-ab12\nssh-rsa AAAA3 backup",
        0,
        0o600,
    );
    let request = Request::Finish {
        key: "yulius-migrate-ab12".into(),
        firewall: true,
    };
    let reply = step(&mut host, &request);
    let outcomes: Vec<(&str, HostOutcome)> = reply
        .lines
        .iter()
        .map(|l| (l.name.as_str(), l.outcome))
        .collect();
    assert_eq!(
        outcomes,
        [
            ("one-time key", HostOutcome::Changed),
            ("firewall", HostOutcome::Changed)
        ]
    );
    assert_eq!(
        host.text("/root/.ssh/authorized_keys"),
        Some("ssh-ed25519 AAAA1 admin@laptop\nssh-rsa AAAA3 backup\n")
    );
    // The port the move came in at is allowed before the old host's rules take effect.
    let ufw = host.runs("ufw");
    assert_eq!(
        ufw,
        [
            "ufw allow from 109.199.123.240 to any port 2222 proto tcp",
            "ufw --force enable"
        ]
    );
    assert!(host.ran("rm -rf -- /var/lib/yulius-migrate"));

    // Again, without a firewall: nothing to remove, nothing enabled.
    host.log.clear();
    let reply = step(
        &mut host,
        &Request::Finish {
            key: "yulius-migrate-ab12".into(),
            firewall: false,
        },
    );
    assert_eq!(only(&reply).0, HostOutcome::Ok);
    assert!(!host.ran("ufw") && !host.ran("write"));
}

#[test]
fn a_request_travels_as_tagged_json() -> Result<(), serde_json::Error> {
    let request = Request::Expect(Expect {
        data_dir: "/var/lib/yulius".into(),
        zstd: true,
    });
    let json = serde_json::to_string(&request)?;
    assert_eq!(
        json,
        r#"{"step":"expect","data_dir":"/var/lib/yulius","zstd":true}"#
    );
    assert_eq!(serde_json::from_str::<Request>(&json)?, request);
    for request in [
        start_request(),
        inventory_request(),
        disk_request(&["/dev/a"]),
    ] {
        let back: Request = serde_json::from_str(&serde_json::to_string(&request)?)?;
        assert_eq!(back, request);
    }
    let reply: Reply = serde_json::from_str("{}")?;
    assert_eq!(reply, Reply::default());
    Ok(())
}

#[test]
fn an_array_that_is_not_the_moves_own_is_never_formatted() {
    let ours =
        "/dev/md/yulius:\n 0 259 0 active sync /dev/nvme0n1\n 1 259 1 active sync /dev/nvme1n1\n";
    // Of other disks (a longer name is another disk), or with something on it, or one that
    // cannot be looked at.
    for (detail, wipefs, code) in [
        ("/dev/md/yulius:\n 0 /dev/sdx\n 1 /dev/sdy\n", "", 0),
        (
            "/dev/md/yulius:\n 0 /dev/nvme0n1p1\n 1 /dev/nvme1n1\n",
            "",
            0,
        ),
        (ours, "0x438 ext4 [filesystem]\n", 0),
        (ours, "0x1fe dos [partition table]\n", 0),
        (ours, "", 1),
    ] {
        let mut host = new_host();
        host.says(&["mdadm", "--detail", "/dev/md/yulius"], detail);
        host.on(
            &["wipefs", "--no-act", "/dev/md/yulius"],
            code,
            wipefs,
            None,
        );
        let reply = step(&mut host, &disk_request(&["/dev/nvme0n1", "/dev/nvme1n1"]));
        let (outcome, why) = only(&reply);
        assert_eq!(outcome, HostOutcome::Failed, "{detail:?} {wipefs:?}: {why}");
        assert!(
            why.contains("is not an empty mirror") && !host.ran("mkfs"),
            "{why}"
        );
    }
}

/// A Start that outlived its connection must never start the unit after a Stop said it is
/// stopped: the Stop leaves its word, and both take the same hold around the switch.
#[test]
fn a_start_still_in_flight_finds_the_stop_and_starts_nothing() {
    let mut host = new_host();
    let mut holds = Vec::new();
    let stop = Request::Stop {
        service: "yulius.service".into(),
    };
    {
        let mut env = Env {
            address: None,
            ssh_port: None,
            peer: None,
            hold: &mut |hold| {
                holds.push(hold);
                Box::new(())
            },
            health: &mut |_| Some(BUILD.into()),
            sleep: &mut |_| {},
            count: &mut |_, old| Ok(old.clone()),
            setup: &mut |_, _, _| Vec::new(),
        };
        assert_eq!(
            only(&handle(&mut host, &mut env, &stop)).0,
            HostOutcome::Changed
        );
        assert!(host.text(STOPPED).is_some());
        // The first thing a Stop does, before it stops anything.
        assert_eq!(
            host.log.first().map(String::as_str),
            Some("write /var/lib/yulius-migrate/stopped")
        );
        host.log.clear();

        let reply = handle(&mut host, &mut env, &start_request());
        assert!(!reply.started);
        let last = reply.lines.last().map(|l| (l.outcome, l.detail.as_str()));
        assert!(
            last.is_some_and(
                |(o, why)| o == HostOutcome::Failed && why.contains("stopped this copy")
            ),
            "{last:?}"
        );
        assert!(!host.ran("systemctl"));
        // A look at its health waits for a Start in flight.
        let health = Request::Health {
            build: None,
            health: "127.0.0.1:8081".into(),
        };
        assert!(handle(&mut host, &mut env, &health).started);
    }
    assert_eq!(
        holds,
        [Hold::Switch, Hold::Flight, Hold::Switch, Hold::Landed]
    );

    // A copy that is proven anew is not the one that was stopped; and until then a finish
    // leaves the Stop's word where a Start in flight looks for it.
    host.file_as(STAGED_BUILD, "build", 0, 0o700);
    step(
        &mut host,
        &Request::Finish {
            key: String::new(),
            firewall: false,
        },
    );
    assert!(host.text(STOPPED).is_some() && host.text(STAGED_BUILD).is_none());
    let mark = Request::Mark {
        setup: setup(),
        from: None,
        to: "https://new.example".into(),
    };
    assert_eq!(only(&step(&mut host, &mark)).0, HostOutcome::Changed);
    assert!(host.text(STOPPED).is_none());
}

#[test]
fn an_installed_copy_is_marked_unproven_once() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    mark_unproven(dir.path())?;
    assert!(dir.path().join(".yulius-import-incomplete").is_file());
    // Already there: something else is wrong, and it says so.
    assert!(mark_unproven(dir.path()).is_err());
    Ok(())
}

/// What a hold's guard notes when it is taken and when it is dropped.
struct Noted(Hold, std::rc::Rc<std::cell::RefCell<Vec<String>>>);

impl Drop for Noted {
    fn drop(&mut self) {
        self.1.borrow_mut().push(format!("drop {:?}", self.0));
    }
}

#[test]
fn the_unit_is_started_while_a_stop_is_excluded() {
    let trace = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut host = new_host();
    host.trace = Some(std::rc::Rc::clone(&trace));
    let noted = std::rc::Rc::clone(&trace);
    let mut env = Env {
        address: None,
        ssh_port: None,
        peer: None,
        hold: &mut |hold| {
            noted.borrow_mut().push(format!("hold {hold:?}"));
            Box::new(Noted(hold, std::rc::Rc::clone(&noted)))
        },
        health: &mut |_| Some(BUILD.into()),
        sleep: &mut |_| {},
        count: &mut |_, old| Ok(old.clone()),
        setup: &mut |_, _, _| Vec::new(),
    };
    assert!(handle(&mut host, &mut env, &start_request()).started);
    let said = trace.borrow();
    let at = |what: &str| {
        said.iter()
            .position(|line| line == what)
            .unwrap_or_else(|| panic!("no {what}: {said:?}"))
    };
    // The whole Start is one flight; the look at the stop mark's place and the start itself
    // are inside the switch, and the health wait is not.
    assert!(at("hold Flight") < at("hold Switch"));
    assert!(at("hold Switch") < at("systemctl daemon-reload"));
    assert!(at("systemctl start yulius.service") < at("drop Switch"));
    assert!(at("drop Switch") < at("systemctl enable --now yulius-update.path"));
    assert_eq!(said.last().map(String::as_str), Some("drop Flight"));
}

/// The real holds: `flock` on files of the state directory.
#[test]
fn holds_exclude_as_the_protocol_needs() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::mpsc;
    let dir = tempfile::tempdir()?;
    let long = Duration::from_secs(30);
    // A second Switch waits for the first.
    let first = hold_in(dir.path(), Hold::Switch, long);
    let (taken, heard) = mpsc::channel();
    let path = dir.path().to_path_buf();
    let second = std::thread::spawn(move || {
        let held = hold_in(&path, Hold::Switch, long);
        let _ = taken.send(());
        drop(held);
    });
    assert!(
        heard.recv_timeout(Duration::from_millis(300)).is_err(),
        "a second Switch was taken while the first is held"
    );
    // A Flight is another lock: a Stop is not kept out for the whole of a Start.
    drop(hold_in(dir.path(), Hold::Flight, long));
    drop(first);
    heard.recv_timeout(Duration::from_secs(10))?;
    second.join().map_err(|_| "the second holder panicked")?;

    // A look at the health waits for a Start in flight, and only so long.
    let flight = hold_in(dir.path(), Hold::Flight, long);
    let started = std::time::Instant::now();
    drop(hold_in(
        dir.path(),
        Hold::Landed,
        Duration::from_millis(400),
    ));
    assert!(
        started.elapsed() >= Duration::from_millis(400),
        "it did not wait"
    );
    let (landed, heard) = mpsc::channel();
    let path = dir.path().to_path_buf();
    let waiter = std::thread::spawn(move || {
        drop(hold_in(&path, Hold::Landed, long));
        let _ = landed.send(());
    });
    assert!(heard.recv_timeout(Duration::from_millis(300)).is_err());
    drop(flight);
    heard.recv_timeout(Duration::from_secs(10))?;
    waiter.join().map_err(|_| "the waiter panicked")?;
    // Nothing in flight: at once. No such directory: nothing held, nothing waited for.
    let started = std::time::Instant::now();
    drop(hold_in(dir.path(), Hold::Landed, long));
    drop(hold_in(&dir.path().join("gone"), Hold::Switch, long));
    drop(hold_in(&dir.path().join("gone"), Hold::Landed, long));
    assert!(started.elapsed() < Duration::from_secs(5));
    Ok(())
}
