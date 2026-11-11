//! A scripted host: files, directories and commands answered by rules.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::host_setup::{Cmd, Host, Out};

/// Answers a command from the host's state, or leaves it to the rules.
pub type Script = fn(&mut FakeHost, &[String]) -> Option<Out>;

/// What a command starting with `prefix` answers, and what it changes.
pub struct Rule {
    pub prefix: Vec<String>,
    pub out: Out,
    pub then: Option<fn(&mut FakeHost)>,
}

/// A host made of files and rules. A command no rule knows succeeds and prints nothing.
#[derive(Default)]
pub struct FakeHost {
    /// Path → content, owner, mode.
    pub files: BTreeMap<PathBuf, (String, u32, u32)>,
    pub dirs: BTreeSet<PathBuf>,
    pub rules: Vec<Rule>,
    /// Commands run and files written, in order.
    pub log: Vec<String>,
    /// Paths whose write fails.
    pub read_only: BTreeSet<PathBuf>,
    /// Asked before the rules: answers a command from the host's state, or leaves it to them.
    pub script: Option<Script>,
}

impl FakeHost {
    pub fn file(&mut self, path: &str, content: &str) -> &mut Self {
        self.file_as(path, content, 0, 0o644)
    }

    pub fn file_as(&mut self, path: &str, content: &str, uid: u32, mode: u32) -> &mut Self {
        let path = PathBuf::from(path);
        let mut at = path.parent();
        while let Some(dir) = at {
            self.dirs.insert(dir.to_path_buf());
            at = dir.parent();
        }
        self.files
            .insert(path, (content.to_owned(), uid, 0o100_000 | mode));
        self
    }

    pub fn dir(&mut self, path: &str) -> &mut Self {
        let mut at = Some(Path::new(path));
        while let Some(dir) = at {
            self.dirs.insert(dir.to_path_buf());
            at = dir.parent();
        }
        self
    }

    /// A later rule wins over an earlier one with a matching prefix.
    pub fn on(&mut self, argv: &[&str], code: i32, stdout: &str, then: Option<fn(&mut Self)>) {
        self.rules.push(Rule {
            prefix: argv.iter().map(|&a| a.to_owned()).collect(),
            out: Out {
                code,
                stdout: stdout.to_owned(),
                stderr: if code == 0 {
                    String::new()
                } else {
                    stdout.to_owned()
                },
            },
            then,
        });
    }

    pub fn says(&mut self, argv: &[&str], stdout: &str) {
        self.on(argv, 0, stdout, None);
    }

    pub fn fails(&mut self, argv: &[&str], why: &str) {
        self.on(argv, 1, why, None);
    }

    /// The commands run that start with `start`.
    pub fn runs(&self, start: &str) -> Vec<&str> {
        self.log
            .iter()
            .filter(|l| l.starts_with(start))
            .map(String::as_str)
            .collect()
    }

    pub fn ran(&self, start: &str) -> bool {
        !self.runs(start).is_empty()
    }

    pub fn text(&self, path: &str) -> Option<&str> {
        self.files.get(Path::new(path)).map(|f| f.0.as_str())
    }
}

impl Host for FakeHost {
    fn read(&self, path: &Path) -> Option<String> {
        self.files.get(path).map(|f| f.0.clone())
    }

    fn write(&mut self, path: &Path, content: &str, mode: u32) -> Result<(), String> {
        if self.read_only.contains(path) {
            return Err("Read-only file system (os error 30)".into());
        }
        self.log.push(format!("write {}", path.display()));
        let shown = path.display().to_string();
        self.file_as(&shown, content, 0, mode);
        Ok(())
    }

    fn is_file(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    fn list(&self, dir: &Path) -> Vec<String> {
        let below = |p: &PathBuf| {
            (p.parent() == Some(dir))
                .then(|| p.file_name()?.to_str().map(str::to_owned))
                .flatten()
        };
        let mut names: BTreeSet<String> = self.files.keys().filter_map(below).collect();
        names.extend(self.dirs.iter().filter_map(below));
        names.into_iter().collect()
    }

    fn stat(&self, path: &Path) -> Option<(u32, u32)> {
        self.files
            .get(path)
            .map(|f| (f.1, f.2))
            .or_else(|| self.dirs.contains(path).then_some((0, 0o040_755)))
    }

    fn gid(&self, _: &Path) -> Option<u32> {
        Some(0)
    }

    fn canonical(&self, path: &Path) -> Option<PathBuf> {
        Some(path.to_path_buf())
    }

    fn own_by_root(&mut self, _: &Path) -> Result<(), String> {
        Ok(())
    }

    fn fingerprint(&self) -> String {
        "fingerprint".into()
    }

    fn run(&mut self, cmd: &Cmd) -> Result<Out, String> {
        self.log.push(cmd.argv.join(" "));
        // `rm` removes what it names, so a step's cleanup shows in the host's files.
        if cmd.argv[0] == "rm" {
            for gone in cmd.argv.iter().filter(|a| a.starts_with('/')) {
                let gone = Path::new(gone);
                self.files.retain(|path, _| !path.starts_with(gone));
                self.dirs.retain(|path| !path.starts_with(gone));
            }
        }
        if let Some(script) = self.script
            && let Some(out) = script(self, &cmd.argv)
        {
            return Ok(out);
        }
        let Some(at) = self
            .rules
            .iter()
            .rposition(|r| cmd.argv.starts_with(&r.prefix))
        else {
            return Ok(Out::default());
        };
        let out = self.rules[at].out.clone();
        if let Some(then) = self.rules[at].then {
            then(self);
        }
        Ok(out)
    }
}

pub const OLD_URL: &str = "https://109-199-123-240.sslip.io";
pub const OLD_HOST: &str = "109-199-123-240.sslip.io";

/// The production host that was moved by hand: Ubuntu 24.04 with a unit, drop-ins, key files,
/// the update helper, Caddy, ufw, third-party sources and software in `/usr/local` and `/opt`.
pub fn old_host() -> FakeHost {
    let mut h = FakeHost::default();
    h.file(
        "/etc/os-release",
        "PRETTY_NAME=\"Ubuntu 24.04.5 LTS\"\nID=ubuntu\nVERSION_ID=\"24.04\"\n\
         VERSION_CODENAME=noble\n",
    );
    h.file(
        "/proc/meminfo",
        "MemTotal:       49283072 kB\nMemFree: 1 kB\n",
    );
    h.file(
        "/etc/systemd/system/yulius.service",
        "[Unit]\nDescription=Yulius\n\n[Service]\nUser=yulius\nGroup=yulius\n\
         Environment=YULIUS_DATA=/var/lib/yulius\n\
         Environment=YULIUS_PUBLIC_URL=https://109-199-123-240.sslip.io\n\
         Environment=HOME=/var/lib/yulius\nWorkingDirectory=/var/lib/yulius\nUMask=0077\n\
         ExecStart=/usr/local/bin/yulius serve\nKillMode=process\n",
    );
    h.file(
        "/etc/systemd/system/yulius.service.d/20-limits.conf",
        "[Service]\nEnvironment=YULIUS_MAX_SESSIONS=8\nEnvironment=YULIUS_SANDBOX_DISK_GB=300\n\
         Environment=YULIUS_CI_JOB_TIMEOUT_MIN=60\n",
    );
    h.file(
        "/etc/systemd/system/yulius.service.d/30-keys.conf",
        "[Service]\nEnvironment=YULIUS_PROXY_SECRET_FILE=/var/lib/yulius/proxy.secret\n\
         Environment=YULIUS_JEV_API_KEY_FILE=/etc/yulius/jev.key\nEnvironment=RUST_LOG=info\n\
         EnvironmentFile=/etc/yulius/env\n",
    );
    h.file_as("/etc/yulius/jev.key", "jev-secret-value", 998, 0o600);
    h.file_as(
        "/etc/yulius/env",
        "YULIUS_UPDATE_TOKEN=token-value\n",
        0,
        0o600,
    );
    h.file_as(
        "/var/lib/yulius/proxy.secret",
        "proxy-secret-value",
        998,
        0o600,
    );
    h.says(
        &["systemctl", "show", "yulius.service"],
        "LoadState=loaded\nFragmentPath=/etc/systemd/system/yulius.service\n\
         DropInPaths=/usr/lib/systemd/system/service.d/10-timeout-abort.conf \
         /etc/systemd/system/yulius.service.d/20-limits.conf \
         /etc/systemd/system/yulius.service.d/30-keys.conf\nUser=yulius\n\
         ExecStart={ path=/usr/local/bin/yulius ; argv[]=/usr/local/bin/yulius serve ; }\n\
         Environment=YULIUS_DATA=/var/lib/yulius \
         YULIUS_PUBLIC_URL=https://109-199-123-240.sslip.io HOME=/var/lib/yulius \
         YULIUS_MAX_SESSIONS=8 YULIUS_SANDBOX_DISK_GB=300 YULIUS_CI_JOB_TIMEOUT_MIN=60 \
         YULIUS_PROXY_SECRET_FILE=/var/lib/yulius/proxy.secret \
         YULIUS_JEV_API_KEY_FILE=/etc/yulius/jev.key RUST_LOG=info\n\
         EnvironmentFiles=/etc/yulius/env (ignore_errors=no)\n",
    );
    // The update helper.
    h.file(
        "/etc/systemd/system/yulius-update.service",
        "[Service]\nExecStart=/usr/local/bin/yulius update-helper\n",
    );
    h.file(
        "/etc/systemd/system/yulius-update.path",
        "[Path]\nPathChanged=/var/lib/yulius/update/request.json\n",
    );
    h.file("/var/lib/yulius-update/installed-commit", "0123abc");
    h.says(
        &["systemctl", "show", "yulius-update.service"],
        "LoadState=loaded\nFragmentPath=/etc/systemd/system/yulius-update.service\n\
         DropInPaths=\nEnvironment=YULIUS_UPDATE_STATE=/var/lib/yulius-update HOME=/root\n",
    );
    h.says(
        &["systemctl", "show", "yulius-update.path"],
        "LoadState=loaded\nFragmentPath=/etc/systemd/system/yulius-update.path\nDropInPaths=\n",
    );
    h.says(&["uname", "-m"], "x86_64\n");
    h.says(&["ls", "--version"], "ls (GNU coreutils) 9.4\nCopyright\n");
    h.says(&["nproc"], "12\n");
    // 4 KiB blocks: 387 GB, 316 GiB used; 6.4 million inodes used.
    h.says(
        &["stat", "-f", "-c"],
        "4096 94482422 11639808 25600000 19200000\n",
    );
    // Apt.
    h.file(
        "/etc/apt/sources.list.d/ubuntu.sources",
        "Types: deb\nURIs: http://archive.ubuntu.com/ubuntu\nSuites: noble noble-updates\n",
    );
    h.file(
        "/etc/apt/sources.list.d/docker.list",
        "deb [arch=amd64 signed-by=/etc/apt/keyrings/docker.asc] \
         https://download.docker.com/linux/ubuntu noble stable\n",
    );
    h.file(
        "/etc/apt/sources.list.d/github-cli.sources",
        "Types: deb\nURIs: https://cli.github.com/packages\nSuites: stable\n\
         Components: main\nSigned-By: /usr/share/keyrings/githubcli-archive-keyring.gpg\n",
    );
    h.file(
        "/etc/apt/sources.list.d/old.list.save",
        "deb http://x y z\n",
    );
    h.file(
        "/etc/apt/keyrings/docker.asc",
        "-----BEGIN PGP PUBLIC KEY BLOCK-----\n",
    );
    h.file(
        "/usr/share/keyrings/githubcli-archive-keyring.gpg",
        "binary",
    );
    h.says(
        &["apt-mark", "showmanual"],
        "caddy\ndocker-ce\ngh\nlibicu74\nlinux-image-6.8.0-87-generic\nufw\nxvfb\nzstd\n",
    );
    h.says(
        &["apt-cache", "policy"],
        "caddy:\n  Installed: 2.6.2\n  Candidate: 2.6.2\n  Version table:\n *** 2.6.2 500\n        \
         500 http://archive.ubuntu.com/ubuntu noble/universe amd64 Packages\n\
         docker-ce:\n  Installed: 5:28.0\n  Candidate: 5:28.0\n  Version table:\n *** 5:28.0 500\n\
         \x20       500 https://download.docker.com/linux/ubuntu noble/stable amd64 Packages\n\
         gh:\n  Installed: 2.83.0\n  Candidate: 2.83.0\n  Version table:\n *** 2.83.0 500\n\
         \x20       500 https://cli.github.com/packages stable/main amd64 Packages\n\
         \x20       100 /var/lib/dpkg/status\n\
         libicu74:\n  Installed: 74.2\n  Candidate: 74.2\n  Version table:\n *** 74.2 500\n\
         \x20       500 http://archive.ubuntu.com/ubuntu noble/main amd64 Packages\n",
    );
    // Software outside apt.
    h.dir("/usr/local/lib/node_modules/@anthropic-ai/claude-code");
    h.dir("/opt/openvscode-server");
    h.dir("/opt/google/chrome");
    h.dir("/root/.rustup");
    h.dir("/root/.cargo");
    h.fails(&["dpkg-query", "-S"], "no path found matching pattern");
    h.says(
        &["dpkg-query", "-S", "/opt/google"],
        "google-chrome-stable: /opt/google\n",
    );
    h.says(&["du", "-sxb", "--"], "1048576\t/x\n");
    h.says(&["npm", "prefix", "-g"], "/usr/local\n");
    // Caddy and ufw.
    h.file(
        "/etc/caddy/Caddyfile",
        "109-199-123-240.sslip.io {\n\treverse_proxy 127.0.0.1:8080 {\n\t\theader_up \
         X-Yulius-Proxy-Secret {env.YULIUS_PROXY_SECRET}\n\t}\n}\n",
    );
    h.file_as(
        "/etc/caddy/yulius-proxy.env",
        "YULIUS_PROXY_SECRET=proxy-secret-value\n",
        0,
        0o600,
    );
    h.file(
        "/etc/systemd/system/caddy.service.d/override.conf",
        "[Service]\nEnvironmentFile=/etc/caddy/yulius-proxy.env\n",
    );
    h.says(&["systemctl", "is-active", "caddy"], "active\n");
    h.file("/etc/ufw/ufw.conf", "# ufw\nENABLED=yes\nLOGLEVEL=low\n");
    h.file(
        "/etc/ufw/user.rules",
        "*filter\n-A ufw-user-input -p tcp --dport 22 -j ACCEPT\n",
    );
    h.file("/etc/ufw/user6.rules", "*filter\n");
    h.file("/etc/default/ufw", "IPV6=yes\n");
    h
}

pub const NEW_ADDRESS: &str = "185.189.47.160";

/// `lsblk --json` of the new host: the system disk and two blank NVMe disks.
pub const LSBLK: &str = r#"{"blockdevices":[
 {"name":"sda","path":"/dev/sda","type":"disk","size":480103981056,"fstype":null,"pttype":"gpt","mountpoints":[null],"ro":false,
  "children":[{"name":"sda1","path":"/dev/sda1","type":"part","size":480102932480,"fstype":"ext4","pttype":"gpt","mountpoints":["/"],"ro":false}]},
 {"name":"nvme0n1","path":"/dev/nvme0n1","type":"disk","size":3840755982336,"fstype":null,"pttype":null,"mountpoints":[null],"ro":false},
 {"name":"nvme1n1","path":"/dev/nvme1n1","type":"disk","size":3840755982336,"fstype":null,"pttype":null,"mountpoints":[null],"ro":false},
 {"name":"nvme2n1","path":"/dev/nvme2n1","type":"disk","size":1000,"fstype":"ext4","pttype":null,"mountpoints":[null],"ro":false},
 {"name":"sr0","path":"/dev/sr0","type":"rom","size":1073741312,"fstype":null,"pttype":null,"mountpoints":[null],"ro":false},
 {"name":"loop0","path":"/dev/loop0","type":"loop","size":4096,"fstype":null,"pttype":null,"mountpoints":[null],"ro":true}
]}"#;

/// A fresh Ubuntu 26.04 server: uutils coreutils, no instance, two blank disks.
pub fn new_host() -> FakeHost {
    let mut h = FakeHost::default();
    h.file(
        "/etc/os-release",
        "PRETTY_NAME=\"Ubuntu 26.04.1 LTS\"\nID=ubuntu\nVERSION_ID=\"26.04\"\n\
         VERSION_CODENAME=resolute\n",
    );
    h.file("/proc/meminfo", "MemTotal:       791674880 kB\n");
    h.file(
        "/etc/passwd",
        "root:x:0:0:root:/root:/bin/bash\nubuntu:x:1000:1000::/home/ubuntu:/bin/bash\n",
    );
    h.file("/etc/fstab", "UUID=1111-aaaa / ext4 defaults 0 1\n");
    h.file_as(STAGED_BUILD, "build", 0, 0o755);
    h.dir("/var/lib");
    h.says(&["uname", "-m"], "x86_64\n");
    h.says(&["nproc"], "64\n");
    h.says(
        &["systemctl", "--version"],
        "systemd 259 (259.5-0ubuntu1)\n+PAM +AUDIT\n",
    );
    h.says(&["systemctl", "is-active"], "inactive\n");
    h.says(&["systemctl", "is-enabled"], "disabled\n");
    h.says(&["stat", "-f", "-c"], "4096 100000000 30000000\n");
    h.says(&["lsblk"], LSBLK);
    h.says(
        &["ss", "-H", "-ltnp"],
        "LISTEN 0 4096 0.0.0.0:22 0.0.0.0:* users:((\"sshd\",pid=900,fd=3))\n",
    );
    h.fails(&["findmnt"], "");
    h.says(&["ls", "--version"], "ls (uutils coreutils) 0.10.0\n");
    h.fails(&["cmp", "-s"], "");
    h
}

pub const STAGED_BUILD: &str = "/var/lib/yulius-migrate/yulius";
