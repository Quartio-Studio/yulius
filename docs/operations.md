# Operating Yulius

Yulius is one Rust binary, `yulius`, that serves the web UI and API, supervises agent sessions and
provides the CLI (`yulius onboard` alias `yulius import`, `yulius login`, `yulius secrets import`,
and `yulius remote|project|task|ci` for a laptop's agent, [remote-control.md](remote-control.md)).
This guide covers a single Linux VM with a TLS reverse proxy in front: install, configure, run,
upgrade, back up and move a server. The guide as it stood until 2026-11-26, with history and notes
for past upgrades, is in [archive/operations-until-2026-11-26.md](archive/operations-until-2026-11-26.md).

## 1. Prerequisites

| Component | Needed for | Notes |
| --- | --- | --- |
| Linux x86_64 or arm64, systemd | the service | macOS works for development (runs are "not contained"). |
| `git` ≥ 2.40 | sandboxes, worktrees, delivery | on `PATH` of the service; `yulius host setup` installs it. |
| Landlock in the kernel (Linux ≥ 5.13, `landlock` in `/sys/kernel/security/lsm`) | the server's own Git, confined to Git's programs and its sandbox's folders | Ubuntu 22.04+ and Debian 12 have it. Without it the server runs no Git (clones, worktrees, fetches and merges fail with that reason); `yulius doctor` checks it as "git confinement". |
| Docker Engine (`docker-ce` with `docker-buildx-plugin` and `docker-compose-plugin`) | Docker in sandboxes: every new base copies `dockerd`, buildx and compose | from Docker's apt repository; `yulius host setup` installs it and stops the host's own daemon, which nothing uses ([Docker in sandboxes](#docker-in-sandboxes)). |
| Codex CLI (`codex`) | Codex tasks and Codex account sign-in | nothing to install: Yulius downloads the pinned release (0.170.0, sha512 in the source) from `registry.npmjs.org` into `<data>/codex-cli/` on the first Codex use, or at start when Codex accounts exist (about 167 MB, 423 MB unpacked). Without the registry it uses a host `codex` if there is one. A host move leaves `codex-cli/` behind: the new host installs its own. |
| Claude Code (`claude`) | Claude tasks and Claude account login | install with npm system-wide (`npm install -g @anthropic-ai/claude-code`; verified with 2.1.292 and 2.1.322). Isolated sandboxes then get newer releases by themselves: [Claude Code updates](#claude-code-updates). |
| GitHub CLI (`gh`) ≥ 2.82.1 | `gh` in every sandbox, logged in as the person (their shells) or with the App's restricted token (agents; [GitHub App permissions](#github-app-permissions)) | install from [GitHub's package repository](https://github.com/cli/cli/blob/trunk/docs/install_linux.md), not the distribution's (Ubuntu 24.04 ships 2.45; `gh pr edit` and `gh issue view --comments` fail below 2.82.1). Rebuild the base afterwards (`yulius sandbox-base build`). `yulius doctor` warns without it or below 2.82.1. |
| Chrome's managed policy (`yulius.json` in `/etc/opt/chrome/policies/managed`, `/etc/chromium/policies/managed` and `/etc/chromium-browser/policies/managed`) | clicks and keys in the sandbox browsers on a virtual display | `yulius host setup` writes it and the update's host check keeps it current; without it the browsers show but take no clicks and keys. It applies to every Chrome on the host: no file dialogs, downloads, printing or extension installs, only web addresses. |
| bubblewrap (`bwrap`) ≥ 0.10.0, or a distribution update with `--ro-bind-fd` | containment (`--contained`) | strongly recommended in production; without it runs are marked "not contained". `yulius host setup` installs it. Ubuntu 24.04's `0.9.0-1ubuntu0.1` and Debian 12's `0.8.0-2+deb12u1` carry `--ro-bind-fd`; an older one fails every contained start, and `yulius doctor` fails its "bubblewrap" line. |
| An AppArmor rule that lets `/usr/bin/bwrap` create user namespaces | every sandbox, on Ubuntu 24.04 and later (`kernel.apparmor_restrict_unprivileged_userns = 1`) | `yulius host setup` writes `/etc/apparmor.d/yulius-bwrap` and, on Ubuntu 26.04, disables the shipped `bwrap-userns-restrict`; never write one by hand ([Sandbox isolation](#sandbox-isolation)). Debian needs none. |
| `uidmap` (`newuidmap`, `newgidmap`) and a subordinate id range for the service user | sandbox isolation: `apt` in sandboxes, private networks | `/etc/subuid` and `/etc/subgid` lines of at least 65,536 ids; `yulius host setup` installs both. |
| `slirp4netns` | sandbox egress (isolation) | `yulius host setup` installs it. |
| Chromium / Google Chrome | sandbox browser | optional; found on `PATH` or set `--chromium` (`YULIUS_CHROMIUM`). Google Chrome (`google-chrome`) is recommended: it plays H.264 pages and is what captchas expect. |
| `xvfb` | the sandbox browser's virtual display | `apt install xvfb`. Without it (or ffmpeg) the browser runs headless: JPEG frames, no real input, captchas fail. `xauth` is not needed; `x11-utils` (`xdpyinfo`) only for diagnostics. |
| `ffmpeg` with libx264 and x11grab | the browser video | `apt install ffmpeg` (Ubuntu's build has both; `yulius doctor` checks). |
| Fonts | pages in the sandbox browser | at least `fonts-noto-core fonts-noto-color-emoji fonts-liberation` (Ubuntu 24.04 names); add `fonts-noto-cjk` for CJK pages. |

Contained runs see only `/usr`, `/bin`, `/lib`, `/lib64`, `/etc`, `/opt` (with isolation, the
sandbox's own system layer), their sandbox directory and the `yulius` binary. Install Claude Code
system-wide under `/usr/local` or `/opt`; a per-user install (`~/.local/bin/claude`) needs
`--contained-readonly <dir>[:<dir>…]` (`YULIUS_CONTAINED_READONLY`).

`yulius host setup` (§2) installs and fixes what it can of these. Run `yulius doctor --data-dir
/var/lib/yulius [--contained]` to check all of them, the data directory permissions and the session
socket path budget (the data directory path must be short enough that
`<data>/logins/<uuid>/pty.sock` fits the 103-byte Unix socket path limit).

## 2. Build and install

Two commands on a fresh Ubuntu 24.04 or 26.04 (or Debian) host: install the binary, then run the
host setup as root.

```sh
cargo build --release -p yulius            # from a checkout; output: target/release/yulius
sudo install -m 0755 target/release/yulius /usr/local/bin/yulius
sudo yulius host setup --public-url https://yulius.example.com
```

`yulius host setup` prints one line per item, `ok` (left alone), `changed`, `manual` (with the
reason) or `FAIL`, then a summary; it exits 1 when an item failed. In order:

1. **needrestart**: `/etc/needrestart/conf.d/yulius.conf` keeps package installs from restarting
   `yulius.service` and `yulius-sandboxes.service`, written before any install. A restart of the
   first outside an update pauses no task; a restart of the second ends every session.
2. **host packages**: `git`, `uidmap`, `slirp4netns`, `bubblewrap`.
3. **Docker Engine**: `docker-ce`, `docker-ce-cli`, `containerd.io`, `docker-buildx-plugin` and
   `docker-compose-plugin` from Docker's repository (`/etc/apt/sources.list.d/docker.sources`, key
   `/etc/apt/keyrings/docker.asc`). A host with Ubuntu's `docker.io` keeps it and only gets
   `docker-buildx` and `docker-compose-v2`.
4. **host Docker units**: `docker.socket`, `docker.service` and `containerd.service` are disabled
   and stopped, unless they run containers (manual).
5. **kernel modules**: `/etc/modules-load.d/yulius-docker.conf`, loaded now and at every boot.
6. **service user**: the data directory's owner, else `--user` (default `yulius`), a system user
   with the data directory (mode 0700) as its home.
7. **subordinate ids**: a range of 65,536 ids after every range in use.
8. **bwrap namespaces**: the AppArmor rule where needed ([Sandbox isolation](#sandbox-isolation)).
9. **yulius.service**: written from §3 and enabled when missing (`--public-url` is needed then);
   otherwise a drop-in `yulius.service.d/10-yulius-sandboxes.conf` adds `Delegate=yes`,
   `DelegateSubgroup=server` and `KillMode=process` if missing (restart by hand when it suits).
10. **yulius binary**: the running binary and the unit's are made root-owned, writable by root
    alone; one in a directory the service user or others may write fails.
11. **sessions unit**: `yulius-sandboxes.service`, `yulius-adopt.service` and the drop-in
    `yulius.service.d/20-yulius-sessions.conf` ([Where sessions live](#where-sessions-live)) are
    written and the sessions unit started; the server's next restart moves its sessions there. It
    fails unless the server's `ExecStart=` path is a regular file (not a link) only root may write,
    in root's directories.
12. **update helper**: when `yulius-update.service` is installed, a drop-in lets its re-check write
    `/etc/modules-load.d`, `/etc/needrestart/conf.d` and Chrome's managed policy directories (§8).
13. **sandbox base**: built as the service user when missing, without the host's `dockerd`, or out of
    date; live sandboxes' bases are kept, so it is safe while sandboxes run.
14. Units it wrote are started; then "sandbox Docker" as `yulius doctor` judges it, the contained
    doctor and the Docker smoke test (`--no-smoke` skips it; it needs Docker Hub).

Running it again changes nothing ("nothing to change"); it is safe on a running instance.
`--dry-run` prints what it would change. With a unit named other than `yulius.service`, pass
`--service <name>` (the update helper passes its `YULIUS_UPDATE_SERVICE`). A setup or `yulius host
check` run keeps its report in `/var/lib/yulius-update/host-check.json` when that directory exists,
so Settings → Instance shows the latest root check; a helper with another `YULIUS_UPDATE_STATE`
needs `--update-state <dir>`.

**Still manual**: the reverse proxy and TLS (§4); Claude Code, once, system-wide with npm (§1); the
GitHub CLI from GitHub's repository (§1); the sandbox browser's packages (Chrome, `xvfb`, `ffmpeg`,
fonts; optional); the update helper (§8), which needs root's Rust toolchain and this server's values.

The agent CLIs must be installed for the `yulius` user, and logged-in state is never taken from it:
Claude subscriptions are added inside Yulius (user menu → Administration → Subscriptions), not with
the CLIs' own `login` on the VM.

[details](archive/operations-until-2026-11-26.md#2-build-and-install)

## 3. systemd unit

`yulius host setup` writes this unit when there is none (§2), and gives an existing one
`Delegate=yes`, `DelegateSubgroup=server` and `KillMode=process` by a drop-in.

`KillMode=process` is required: each agent session and each user terminal runs in its own detached
`yulius ptyhost` process so that it survives a restart or upgrade of the server (the next start
adopts the terminals from `shells/<id>/shell.json`). With the default `KillMode=control-group`,
stopping the service would kill what the server itself started and still holds in its unit. The
sessions themselves live in a unit of their own: see [Where sessions live](#where-sessions-live).

```ini
# /etc/systemd/system/yulius.service
[Unit]
Description=Yulius
After=network-online.target
Wants=network-online.target

[Service]
User=yulius
Group=yulius
Environment=YULIUS_DATA=/var/lib/yulius
Environment=YULIUS_PUBLIC_URL=https://yulius.example.com
Environment=YULIUS_BIND=127.0.0.1:8080
Environment=YULIUS_INTERNAL_BIND=127.0.0.1:8081
# Resource limits (live sessions, CI jobs and runs, sandbox memory and disk, CD run disk) are
# sized from this host; pin one only on purpose (see "Resource limits" below), or change it in
# Settings → Instance, which wins over this file.
#Environment=YULIUS_MAX_SESSIONS=8
# Minutes a finished task (its turn ended, it waits for a person) keeps its idle session before it
# is parked: the process ends, and the task resumes in the same session when someone writes to it
# (default 30; 0 never). Before rolling back to a server older than parking, set 0 and resume or
# archive the tasks marked "parked". What parking does: docs/product.md, "Waiting and parking".
#Environment=YULIUS_PARK_AFTER_MINUTES=30
# Percent of a window binding a task's model past which a subscription of that harness gets no
# new task while another has room, and its idle tasks move to the one with the most room every 2
# minutes: 50 to 98, default 95; 98 turns it off.
#Environment=YULIUS_REBALANCE_PERCENT_CLAUDE_CODE=95
#Environment=YULIUS_REBALANCE_PERCENT_CODEX=95
#Environment=YULIUS_REBALANCE_PERCENT_ANTIGRAVITY=95
Environment=YULIUS_CONTAINED=true
# Largest file dropped onto the editor or a terminal, in MiB (default 1024; keep the proxy's body
# limit above it).
#Environment=YULIUS_UPLOAD_MAX_MB=1024
Environment=PATH=/usr/local/bin:/usr/bin:/bin
# The sandbox browser: a headful Chrome on a virtual display when xvfb and ffmpeg are installed
# (auto, the default). See "Sandbox browser" below for the other settings.
Environment=YULIUS_BROWSER_DISPLAY=auto
# Optional: a Jev key file (see "Task intake (Jev)" below); the superadmin can also set the key in
# Settings → Instance, which wins over the file.
#Environment=YULIUS_JEV_API_KEY_FILE=/var/lib/yulius/jev.key
# Optional: a Vidoc key file (see "Security scans (Vidoc)" below); Settings → Instance wins too.
#Environment=YULIUS_VIDOC_API_KEY_FILE=/var/lib/yulius/vidoc.key
# Optional: another Telegram Bot API (see "Telegram notifications" below); the bot is set in Settings.
#Environment=YULIUS_TELEGRAM_API_URL=https://api.telegram.org
# Optional: a token that may read Quartio-Studio/yulius, when the GitHub App cannot (see §8).
#Environment=YULIUS_UPDATE_TOKEN_FILE=/var/lib/yulius/update.token
# Recommended behind the proxy: the secret it shares with Yulius, so sign-in limits are per client
# (see "Per-client sign-in limits" in §4).
#Environment=YULIUS_PROXY_SECRET_FILE=/var/lib/yulius/proxy.secret
# Yulius-run CI (see "CI on this machine" below): minutes per job, seconds between checks. Jobs
# at once and runs at once per sandbox are resource limits (host-sized).
#Environment=YULIUS_CI_MAX_JOBS=4
#Environment=YULIUS_CI_LANES=2
# CPUs an agent session, terminal or Cockpit plans for (YULIUS_CPUS; default a third of a
# sandbox's, at least 2). CI jobs get what they declare in yulius.toml.
#Environment=YULIUS_AGENT_CPUS=4
Environment=YULIUS_CI_JOB_TIMEOUT_MIN=30
Environment=YULIUS_CI_POLL_SECS=60
# GB a CD or delivery environment run's files and cache may use (a resource limit, host-sized).
#Environment=YULIUS_CD_RUN_DISK_GB=20
# Sandbox isolation: auto (default), namespaces or legacy; see "Sandbox isolation" below.
#Environment=YULIUS_ISOLATION=auto
ExecStart=/usr/local/bin/yulius serve
KillMode=process
# Per-sandbox CPU, memory and task limits: the server keeps itself in server/ and gives every
# sandbox its own cgroup, in yulius-sandboxes.service once `yulius host setup` wrote it ("Where
# sessions live" below), else under this unit (systemd 254 or later).
Delegate=yes
DelegateSubgroup=server
Restart=on-failure
RestartSec=2
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

```sh
sudo systemctl daemon-reload && sudo systemctl enable --now yulius
journalctl -u yulius -f
```

### Where sessions live

Sessions and sandboxes do not live in `yulius.service`'s own cgroup. systemd 258 and later (Ubuntu
26.04 has 259) start a unit inside the unit's own group, and the kernel refuses that while a
sandbox there holds a process: a server whose sessions lived in its unit could be stopped and never
started again while one session ran
([ADR-0298](decisions/0298-sessions-live-in-their-own-unit.md)). `yulius host setup` therefore
writes three more files and starts the first unit; `deploy/systemd/` holds the same files for
`yulius.service`, the user `yulius` and `/usr/local/bin/yulius`.

`/etc/systemd/system/yulius-sandboxes.service`:

```ini
# Written by `yulius host setup` (docs/operations.md §3, ADR-0298): the group every session and
# sandbox of yulius.service lives in. Restarting yulius.service never touches it; stopping this unit
# ends every session. Its one process is root's, so no signal of yulius reaches it.
[Unit]
Description=Yulius sessions and sandboxes (yulius.service)

[Service]
Slice=yulius.slice
User=yulius
ExecStartPre=+/usr/bin/chown yulius /sys/fs/cgroup/yulius.slice/cgroup.procs
ExecStart=+/usr/bin/sleep infinity
Delegate=yes
DelegateSubgroup=keeper
OOMScoreAdjust=-1000
```

`/etc/systemd/system/yulius-adopt.service`:

```ini
# Written by `yulius host setup` (docs/operations.md §3, ADR-0298): before yulius.service starts, sessions
# an earlier layout left in its own group move to yulius-sandboxes.service.
[Unit]
Description=Move Yulius sessions out of yulius.service
Requires=yulius-sandboxes.service
After=yulius-sandboxes.service

[Service]
Type=oneshot
ExecStart=/usr/local/bin/yulius host adopt-sessions --service yulius.service
TimeoutStartSec=120
```

`/etc/systemd/system/yulius.service.d/20-yulius-sessions.conf`:

```ini
# Written by `yulius host setup` (docs/operations.md §3, ADR-0298): sessions live in yulius-sandboxes.service,
# so a restart of this unit keeps them on every systemd.
[Unit]
Wants=yulius-sandboxes.service yulius-adopt.service
After=yulius-sandboxes.service yulius-adopt.service

[Service]
Slice=yulius.slice
```

- **`yulius-sandboxes.service`** holds every sandbox's groups, under
  `/sys/fs/cgroup/yulius.slice/yulius-sandboxes.service/sandboxes/<sandbox>/`. **Stopping or
  restarting it ends every agent session, terminal and sandbox engine**; nothing does so by itself.
  If it happened anyway, restart `yulius.service` afterwards, or sandboxes run without their limits
  ("cannot apply the sandbox's limits" in the log).
- **`yulius-adopt.service`** runs `yulius host adopt-sessions` as root before every start of
  `yulius.service` (at most 2 minutes): sessions an earlier layout left in `yulius.service`'s group
  move to the sessions unit; a sandbox Docker engine found there is killed with its containers and
  starts again on demand. Output: `journalctl -u yulius-adopt`.
- `systemctl restart yulius`, a crash, Restart… and an update then keep every session.
  `yulius doctor` ("restart with sessions") and `yulius host check` ("sessions unit") report it.
- Without the files, sessions stay in `/sys/fs/cgroup/system.slice/yulius.service/sandboxes/…`; on
  systemd 258 and later Restart… and Update now are then refused while a session runs
  ("yulius.service would not start again: … nothing was stopped"), as where `yulius-sandboxes.service`
  does not run. `sudo yulius host setup` writes the files without restarting. A plain `systemctl
  restart yulius` there leaves the server down until "The way back" in the
  [archived upgrade notes](archive/operations-until-2026-11-26.md#notes-for-specific-upgrades).
- With `--service yulius-dev.service` the files are `yulius-dev-sandboxes.service`,
  `yulius-dev-adopt.service`, `yulius-dev.service.d/20-yulius-sessions.conf`; slice `yulius_dev.slice`.

Limits that bound all sandboxes together (`TasksMax=`, `MemoryMax=`, `CPUQuota=`) belong on
`yulius-sandboxes.service`, not `yulius.service`: `sudo systemctl set-property
yulius-sandboxes.service TasksMax=…` applies at once (never restart that unit for it), and the
server reads the task limit at its next start. A move (`yulius instance migrate`) carries drop-ins
under `/etc/systemd/system/yulius-sandboxes.service.d/`, not what `set-property` wrote.

## 4. Reverse proxy (TLS, WebSockets)

Only the public listener (`YULIUS_BIND`) is proxied. The internal listener (`YULIUS_INTERNAL_BIND`)
serves agents' MCP, hooks and Git credentials; it must stay on loopback and is never proxied.
WebSockets are used for terminals, the event stream and the browser panel. The proxy's body limit
must be above the upload limit (`YULIUS_UPLOAD_MAX_MB`, 1 GiB by default; 1100 MB below; change
both together). Let the proxy compress responses: Yulius serves its web UI uncompressed.

Caddy:

```caddyfile
yulius.example.com {
    request_body {
        max_size 1100MB
    }
    encode zstd gzip
    header Strict-Transport-Security "max-age=31536000"
    reverse_proxy 127.0.0.1:8080
}
```

nginx:

```nginx
server {
    listen 443 ssl http2;
    server_name yulius.example.com;
    # ssl_certificate / ssl_certificate_key ...
    client_max_body_size 1100m;
    gzip on;
    gzip_min_length 1024;
    gzip_types text/css text/javascript application/javascript application/json image/svg+xml;
    add_header Strict-Transport-Security "max-age=31536000" always;
    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 1h;
        proxy_send_timeout 1h;
    }
}
```

`YULIUS_PUBLIC_URL` must be the exact external origin: cookies are `Secure` (the session cookie
`__Host-` prefixed) for `https` URLs, and the `Origin` of WebSockets and cookie-authenticated API
requests is checked against it. Yulius sends `X-Frame-Options: DENY`, a strict
`Content-Security-Policy`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: same-origin` and
`X-Yulius: <process tag>`; the proxy must pass these on, not replace them. HSTS is the TLS proxy's
job, as above; add it once the domain is meant to stay on HTTPS.

[details](archive/operations-until-2026-11-26.md#4-reverse-proxy-tls-websockets)

### Per-client sign-in limits (trusted proxy header)

Password sign-in allows 5 attempts per minute **per client**. Behind the proxy every connection
comes from `127.0.0.1`, so without this setup all clients share one limit and anyone can keep the
superadmin locked out. The proxy passes the client's address in `X-Yulius-Client-Ip` with a secret
only it and Yulius know in `X-Yulius-Proxy-Secret`, overwriting what a client sent. Yulius uses the
address only when the secret matches; otherwise it keys on the TCP peer. An IPv6 client is limited
by its /64 network. `X-Forwarded-For` alone is not trusted.

1. **Create the secret** once, as root, without printing it. Yulius's copy is owned by the service
   user, mode `0600`; the proxy's copy is root's, mode `0600`:

   ```sh
   # Caddy
   sudo sh -c 'umask 077 &&
     openssl rand -hex 32 > /var/lib/yulius/proxy.secret &&
     sed "s/^/YULIUS_PROXY_SECRET=/" /var/lib/yulius/proxy.secret > /etc/caddy/yulius-proxy.env &&
     chown yulius:yulius /var/lib/yulius/proxy.secret'
   # nginx
   sudo sh -c 'umask 077 &&
     openssl rand -hex 32 > /var/lib/yulius/proxy.secret &&
     sed "s/.*/proxy_set_header X-Yulius-Proxy-Secret \"&\";/" /var/lib/yulius/proxy.secret \
       > /etc/nginx/yulius-proxy-secret.conf &&
     chown yulius:yulius /var/lib/yulius/proxy.secret'
   ```

2. **Yulius**: `Environment=YULIUS_PROXY_SECRET_FILE=/var/lib/yulius/proxy.secret` in the unit
   (§3), then `systemctl daemon-reload && systemctl restart yulius` (sessions survive). The file
   must be a regular file of 32–4096 printable characters readable by its owner only; the start
   log says `trusted reverse proxy: …`, or `the proxy secret file is not usable: … make it 0600`.
   `sudo -u yulius yulius doctor --proxy-secret-file /var/lib/yulius/proxy.secret` checks it.

3. **The proxy**, on the Yulius site only (not on preview hosts). Caddy reads the secret from its
   environment, so it stays out of the Caddyfile and Caddy's config API:

   ```ini
   # /etc/systemd/system/caddy.service.d/yulius-proxy.conf
   [Service]
   EnvironmentFile=/etc/caddy/yulius-proxy.env
   ```

   ```caddyfile
   yulius.example.com {
       # request_body, encode and header as above
       reverse_proxy 127.0.0.1:8080 {
           header_up X-Yulius-Client-Ip {remote_host}
           header_up X-Yulius-Proxy-Secret {env.YULIUS_PROXY_SECRET}
       }
   }
   ```

   `systemctl daemon-reload && systemctl restart caddy` (a restart, not a reload). With another
   proxy or a CDN in front of Caddy, configure `trusted_proxies` and use `{client_ip}` instead of
   `{remote_host}`. nginx (a `location` that sets any header inherits none from `server`, so keep
   these next to the others):

   ```nginx
       location / {
           proxy_pass http://127.0.0.1:8080;
           # proxy_http_version, Upgrade, Connection, Host, X-Forwarded-Proto, timeouts as above
           proxy_set_header X-Yulius-Client-Ip $remote_addr;
           include /etc/nginx/yulius-proxy-secret.conf;
       }
   ```

   `nginx -t && systemctl reload nginx`.

4. **Check**: five wrong passwords from one network, then signing in from another (a phone on
   mobile data) works; the first network can try again after a minute.

Either side can be configured first: until both are, requests key on the peer. To rotate the
secret, repeat step 1 and restart Yulius and the proxy.

**Sandboxes and the secret.** Isolated sandboxes cannot reach the public listener (`slirp4netns
--disable-host-loopback`); runs without isolation can, but do not know the secret (never set
`YULIUS_SANDBOX_FORWARD` to the public port). Without containment (`YULIUS_CONTAINED=false`) agents
could read Yulius's copy, like `master.key`. Keep Caddy's admin API unreachable from such runs.

[details](archive/operations-until-2026-11-26.md#per-client-sign-in-limits-trusted-proxy-header)

### Optional: live previews (wildcard DNS and TLS)

**Open live in a new window** (browser panel) opens a sandbox dev server in the user's own browser
through a per-port **preview origin** `<port>-<key>.<yulius host>` (e.g.
`20001-0123456789ab.yulius.example.com`). The proxy must resolve every subdomain to the VM (a
wildcard record `*.yulius.example.com A <VM IP>`; `sslip.io` names such as
`20001-0123456789ab.109-199-123-240.sslip.io` resolve by themselves), terminate TLS for them and
forward them, `Host` unchanged, to `YULIUS_BIND` with WebSockets. `YULIUS_PREVIEW_DOMAIN`
(`--preview-domain`) sets another parent domain or `off`; an IP-address public URL has no previews;
for local development use a name such as `http://yulius.localhost:8080`. Caddy (on-demand TLS; the
`ask` endpoint on the internal listener approves only existing sandbox ports' hosts):

```caddyfile
{
    on_demand_tls {
        ask http://127.0.0.1:8081/internal/preview-tls
    }
}

yulius.example.com {
    # as above
}

# Every other host name: only those Yulius approves get a certificate.
https:// {
    tls {
        on_demand
    }
    reverse_proxy 127.0.0.1:8080 {
        flush_interval -1
    }
}
```

The first visit to a new preview host waits a few seconds for its certificate. `sslip.io` shares
its Let's Encrypt rate limits with all its users: a domain of your own is the better production
setup. nginx (a wildcard certificate for `*.yulius.example.com`, which needs the ACME DNS
challenge, e.g. certbot with your DNS provider's plugin):

```nginx
server {
    listen 443 ssl http2;
    server_name *.yulius.example.com;
    # ssl_certificate / ssl_certificate_key: the wildcard certificate
    client_max_body_size 256m;
    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_buffering off;
        proxy_read_timeout 1h;
        proxy_send_timeout 1h;
    }
}
```

Without DNS or TLS for the previews everything else keeps working; only "Open live in a new window"
fails to load.

[details](archive/operations-until-2026-11-26.md#optional-live-previews-wildcard-dns-and-tls)

### Sandbox browser (virtual displays)

Each sandbox has one Chrome, shared by its users and agents. With `xvfb` and `ffmpeg` installed it
runs **headful on its own virtual display** (one `Xvfb` per running browser, as the service user,
numbers from `:100`), shown to viewers as H.264 video over the browser WebSocket with their mouse
and keys as real input. Nothing to open in the firewall; the reverse proxy needs nothing more.

| Setting (flag / environment) | Default | Meaning |
| --- | --- | --- |
| `--browser-display` / `YULIUS_BROWSER_DISPLAY` | `auto` | `xvfb`: always a virtual display (the browser does not start without Xvfb); `headless`: never (JPEG frames, CDP input); `auto`: `xvfb` on Linux when Xvfb and ffmpeg are found, else `headless`. |
| `--xvfb` / `YULIUS_XVFB`, `--ffmpeg` / `YULIUS_FFMPEG` | `Xvfb`, `ffmpeg` | The executables (a path or a name on `PATH`). |
| `--browser-fps` / `YULIUS_BROWSER_FPS` | 60 | Highest video frames per second (5–60). 30 halves the encoder's CPU. |
| `--browser-max-size` / `YULIUS_BROWSER_MAX_SIZE` | `1920x1200` | Largest viewport in CSS pixels; larger panels show the page scaled down. |
| `--browser-scale` / `YULIUS_BROWSER_SCALE` | 1 | Device pixels per CSS pixel (1–3). 2 gives crisp text on HiDPI screens at about three times the CPU and bandwidth. |
| `--browser-display-base` / `YULIUS_BROWSER_DISPLAY_BASE` | 100 | First X display number; numbers in use are skipped. |
| `--chromium` / `YULIUS_CHROMIUM` | found on `PATH` or in the usual install locations | The Chromium or Google Chrome executable. |
| `--chromium-no-sandbox` / `YULIUS_CHROMIUM_NO_SANDBOX` | off | Starts Chromium with `--no-sandbox`: only where Chromium's own sandbox cannot run (a container without user namespaces). The browsers run on the host as the service user, so a compromised renderer would have the service user's rights (the data directory, the vault key). |

- **Resources** (12-vCPU VM): the encoder takes about one core per watched browser at 1280×800 and
  60 fps, about 1.5 cores at 1920×1080, nothing when nobody watches; 30 fps saves a third to a half.
  Each display is a framebuffer of the maximum size (about 9 MB at 1920×1200).
- **Isolation.** The display listens only on `/tmp/.X11-unix/X<n>` (`-nolisten tcp -nolisten
  local`), which contained runs do not see, with a cookie in `<data>/displays/<sandbox>/Xauthority`
  (0600). `PrivateTmp=yes` in the unit works too.
- **Input needs the managed policy** (`yulius host setup` writes it): without it the browser shows
  but takes no clicks and keys, and the panel says what to run. If the panel says a file dialog may
  still be open, choose **Restart browser** (it keeps the profile). `yulius doctor` reports Xvfb,
  ffmpeg, libx264, x11grab and the browser policy.
- **Network.** Each browser goes out through a loopback proxy of its own, served by the egress
  proxy: the internet as the sandbox reaches it, and of local addresses only its sandbox's
  dev-server ports. "the sandbox browser does not load …" names a local address outside that
  range: serve it on a sandbox port and open `localhost:<port>`. Do not set `ProxyMode` or
  `ProxySettings` in a managed Chrome policy on the host: a policy wins.
- **Profile locks.** Before every start Yulius removes the profile's (`<data>/sandboxes/<id>/browser`)
  `SingletonLock`, `SingletonCookie` and `SingletonSocket` when no running browser holds them. On
  "the browser profile is in use by process N on this host", check `ps -p N -o pid,user,args`; stop
  a Chrome left over from a crashed server (`kill N`) and open the panel again.

[details](archive/operations-until-2026-11-26.md#sandbox-browser-virtual-displays)

### Sandbox isolation

Each sandbox has one **namespace holder**. Agents' task sessions, users' shells and CI jobs join it
(a user's shell runs `--shell` / `YULIUS_SHELL`, default the service's `$SHELL`, then `/bin/bash`,
then `/bin/sh`). A sandbox gets its own user namespace (root inside is the service user, ids
1–65535 from its subordinate range); a private network (egress through `slirp4netns`; host loopback
and other sandboxes unreachable; the internal listener and the dev-server ports relayed); a
writable **system layer** over a shared copy of the host system (the *base*), so `sudo apt-get
install …` persists for that sandbox only; CPU, memory and task limits through the delegated cgroup
and a soft disk quota (CI jobs weigh `ci/` 400 against `agents/` 100 inside, and the sandbox 500
against 100 on the host while its CI runs); a seccomp filter (no nested user namespaces,
`io_uring`, `bpf`, keyrings, module or mount calls); and its own Docker engine
([Docker in sandboxes](#docker-in-sandboxes)).

**Install once** (as root): `sudo yulius host setup` (§2); the first base takes a few minutes and
about 6 GB under `<data>/system`. If it added the drop-in, restart once: `systemctl restart yulius`.
Install `gh` (§1) before the setup, so the base copies it.

**AppArmor and user namespaces**
([ADR-0296](decisions/0296-host-setup-writes-the-apparmor-profile-that-lets-bwrap-create-user-namespaces.md)).
Ubuntu since 24.04 sets `kernel.apparmor_restrict_unprivileged_userns = 1`, so without a rule no
sandbox starts:

| Host | Without the rule | What `host setup` does |
| --- | --- | --- |
| Ubuntu 24.04 | `bwrap: setting up uid map: Permission denied` | writes and loads `/etc/apparmor.d/yulius-bwrap` |
| Ubuntu 26.04 | the package's `bwrap-userns-restrict` denies every capability to what bwrap starts: `mount: … permission denied` | disables that profile (`/etc/apparmor.d/disable/bwrap-userns-restrict`, a link; the package's file is untouched), then writes and loads `/etc/apparmor.d/yulius-bwrap` |
| Debian 12, 13; no AppArmor | works | nothing |

- The profile is `profile yulius-bwrap /usr/bin/bwrap flags=(unconfined) { userns, }`. Add your own
  rules in `/etc/apparmor.d/local/yulius-bwrap`, not in the file (setup rewrites it).
- `yulius host check` and `yulius doctor --contained` ("sandbox namespaces") report; the fix is
  `sudo yulius host setup`, which reloads `/etc/apparmor.d` (`apparmor_parser -r`, at most 300 s).
- A person's own profile for `/usr/bin/bwrap` is adopted when bwrap works with it, else named. To
  switch to Yulius's: `sudo ln -s /etc/apparmor.d/bwrap /etc/apparmor.d/disable/ && sudo
  apparmor_parser -R /etc/apparmor.d/bwrap`, then run the setup again.
- **After a release upgrade from 24.04 to 26.04**, run `sudo yulius host setup` once: sandboxes fail
  until the shipped profile is disabled.

**A build by hand** (`sudo -u yulius yulius sandbox-base build --data-dir /var/lib/yulius`, e.g.
after installing a host package) is safe while sandboxes run: it keeps every base named by `--keep`
or mounted by a live holder (`<data>/runtime/sandboxes/*/host.json`). Run it as the service user
(or root). Builds take turns under `<data>/system/.build.lock`.

**Settings** (environment of the unit):

| Variable | Default | Meaning |
| --- | --- | --- |
| `YULIUS_ISOLATION` | `auto` | `auto`: the holder when its prerequisites hold, else the previous containment; `namespaces`: refuse runs without it; `legacy`: the previous containment. |
| `YULIUS_SANDBOX_EGRESS` | `direct` | `none`: no egress, only the relayed endpoints (the internal listener, which also serves the secrets proxy, and `YULIUS_SANDBOX_FORWARD`). |
| `YULIUS_SANDBOX_MEMORY_GB` | a [resource limit](#resource-limits) | `memory.max` per sandbox. One sandbox runs all its agents and CI jobs: size it to the sessions. |
| `YULIUS_SANDBOX_CPUS` | none | `cpu.max` per sandbox. Without it the CPU is shared by weight. The sandbox's capacity for budgets is this limit, else the host's CPUs. |
| `YULIUS_SANDBOX_PIDS` | a quarter of the smaller of `kernel.threads-max` and `kernel.pid_max`, at least 8192, at most three quarters of `yulius.service`'s `pids.max` | `pids.max` per sandbox, for all its tasks, CI jobs and containers; CI jobs (`ci/`) and the Docker engine (`engine/`) each get 3/8 of it. All sandboxes together stop at three quarters of the service's limit. |
| `YULIUS_SANDBOX_DISK_GB` | a [resource limit](#resource-limits) | Soft quota of the sandbox directory plus its packages, measured every 10 minutes. From 90 % Yulius prunes rebuildable caches ([Leftovers](#leftovers-what-is-kept-what-is-cleaned-and-when)); above it new sessions are refused. `0` turns it off. A project declares its own caches in `yulius.toml`: `[disk]` `caches = "target build/cache"`. |
| `YULIUS_SANDBOX_FORWARD` | — | Extra host loopback ports reachable inside every sandbox at the same port (e.g. a loopback MCP server), comma-separated. |
| `YULIUS_SANDBOX_IDLE_MINUTES` | 5 | A holder without runs stops after this long; the next run starts a fresh one on the newest base. A sandbox's Docker engine stops after this long without a client or running container. |
| `YULIUS_CLAUDE_UPDATES` | `latest` | Claude Code releases isolated sandboxes follow: `latest`, `stable` or `off` (the host's install only). A channel chosen in Settings → Instance wins. |
| `YULIUS_SANDBOX_DOCKER` | `on` | `on`: every isolated sandbox gets its own Docker engine when its system has `dockerd` and the cgroup delegation is on; `off`: none. |

- **Where things live.** `<data>/system/base-<n>/`: the base (`current.json` names it; rebuilt 10
  minutes after host packages, `/usr/local` or `/opt` last changed; `yulius sandbox-base status`).
  `<data>/sysroots/<sandbox>/`: installed packages (`upper/`) and Docker data (`docker/`).
  `<data>/runtime/sandboxes/<sandbox>/`: holder records, relay sockets, `docker/docker.sock`,
  `docker.json`, `docker-front.json`, `docker-front.log`, `engine/`, `sys/`, `engine.log`.
- **Ports.** Each sandbox gets the lowest free block of 20 ports in 20000–32759 (keep that outside
  the host's ephemeral range); its dev servers are reachable on host loopback only there
  (`YULIUS_PORT_BASE`…).
- **Packages.** One whose scripts add a system user fails in a user's shell: install it from an
  agent's session. A broken third-party apt source on the host breaks `apt-get update` there too.
- **Without the prerequisites** (`yulius doctor` lists them), contained runs keep the previous
  containment: read-only host system, the host's network, no limits.

[details](archive/operations-until-2026-11-26.md#sandbox-isolation)

#### Harness memory is read-only

Every contained session sees its harness's memory and user-level instruction files read-only
([ADR-0479](decisions/0479-harness-memory-is-read-only-in-every-session-yulius-starts.md)); a
sandbox's Docker engine, CI jobs and script watches see them as empty read-only folders. On the
host they are ordinary files, which a person edits in place:

| Session | Claude Code memory and instructions | Codex | Antigravity |
| --- | --- | --- | --- |
| Task run | `<data>/sandboxes/<id>/home/.claude/projects/<key>/memory/`, `…/home/.claude/CLAUDE.md`, `…/home/.claude/rules/` | `<data>/sandboxes/<id>/codex/<account>/{AGENTS.md,AGENTS.override.md,memories/}` | none kept: each run's `.gemini` is new |
| A person's Superagent | `<data>/users/<user>/home/.claude/…` (the same three) | | |
| Global Superagent | `<data>/users/<user>/superagent/.claude/…` | | |

`<key>` is `<data>/sandboxes/<id>/repo` with every character outside `a-z`, `A-Z`, `0-9` written
as `-` (e.g. `-var-lib-yulius-sandboxes-<id>-repo`). A protected path that is a link or of the
wrong kind is moved aside to `<name>.yulius-moved-<ms>` before a session starts.

[details](archive/operations-until-2026-11-26.md#harness-memory-is-read-only)

### Subscription rebalance

Every 2 minutes Yulius moves idle tasks off a subscription past its harness's threshold
(`YULIUS_REBALANCE_PERCENT_CLAUDE_CODE`, `_CODEX`, `_ANTIGRAVITY`; default 95, 50 to 98) to the one
with the most room; new tasks keep off it while another has room. Tasks a person put on a
subscription ("Run on…", "Continue on…") stay. Moves are recorded as `rebalance: …` and logged as
`rebalance pass`. 98 turns it off for that harness.

[details](archive/operations-until-2026-11-26.md#subscription-rebalance)

### Resource limits

Six limits decide how much work the server takes at once
([ADR-0294](decisions/0294-resource-limits-are-sized-from-the-host-and-pins-remember-their-host.md)).
Without a pin each is sized from the host at every start (`cpus` logical CPUs, memory in GiB, the
data directory's filesystem in GB):

| Limit | Variable | Host-sized | 12 CPUs, 47 GiB, 387 GB | 64 CPUs, 755 GiB, 3.5 TB | A change applies |
| --- | --- | --- | --- | --- | --- |
| Live sessions | `YULIUS_MAX_SESSIONS` | `cpus / 2 + 2`, at most one per 2 GiB of memory, at least 2 | 8 | 34 | at once (a raise starts queued tasks) |
| CI jobs at once | `YULIUS_CI_MAX_JOBS` | `cpus / 6 + 2`, at most 64 | 4 | 12 | at once (running jobs finish) |
| CI runs at once per sandbox | `YULIUS_CI_LANES` | `cpus / 32 + 2`, at most 8 | 2 | 4 | at once |
| Sandbox memory (GiB) | `YULIUS_SANDBOX_MEMORY_GB` | the memory minus an eighth (at least 4 GiB kept back), at least 2 | 41 | 660 | at the next restart |
| Sandbox disk quota (GB) | `YULIUS_SANDBOX_DISK_GB` | a quarter of the data disk, at least 100, at most half the disk | 100 | 875 | at once |
| CD run disk (GB) | `YULIUS_CD_RUN_DISK_GB` | a twentieth of the data disk, at least 20, at most half the disk | 20 | 175 | from the next run |

A fact the host does not report leaves that limit at 8, 4, 2, 2 GiB, 100 GB or 20 GB.

- **Where a value comes from** (Settings → Instance → Resource limits shows each with its source):
  a number saved there (`settings`); "host-sized" saved there, overruling the unit (`default`); the
  unit's `YULIUS_*` variable (`unit`); else host-sized (`default`). Changes are audited
  (`limits.set`, `limits.removed`). `0` for the sandbox disk quota means none.
- **Unreadable saved limits** ("the saved resource limits could not be read"): the server runs on
  the unit's and host's values and refuses changes (409). If a restart warns again, stop the
  service, run `sudo -u yulius sqlite3 /var/lib/yulius/yulius.db "UPDATE instance SET limits =
  NULL"` (the `sqlite3` package) and start; then check the card's values by hand.
- **A host of another size.** A pin recorded on a host whose facts differ by a quarter or more is
  warned about at start ("… is pinned to …, sized for another host …"); the card offers "Use
  host-sized values" and "Keep these numbers". After `yulius instance import` the first start takes
  host-sized values by itself (audited `limits.adapted`).

[details](archive/operations-until-2026-11-26.md#resource-limits)

### Claude Code updates

Claude Code's own updater is off in every session; Yulius updates it instead, with no root step and
no sign-in again
([ADR-0293](decisions/0293-claude-code-follows-latest-and-must-know-the-default-model.md)):

- **Channel.** About every hour (and on "Check now") the server reads npm's dist-tags of
  `@anthropic-ai/claude-code` from `registry.npmjs.org` (egress on port 443) and follows `latest` by
  default. A superadmin chooses Latest, Stable or Off in **Settings → Instance → Claude Code**, which
  wins over `YULIUS_CLAUDE_UPDATES` and needs no restart.
- **Checked before use.** A newer version is downloaded as its platform package
  (`@anthropic-ai/claude-code-linux-x64`, `-linux-arm64`, or their `-musl` builds), checked against
  the registry's sha512, and must name Yulius's default model (`claude-opus-5-5` today), else it is
  refused and reported. It lands in `<data>/claude-code/versions/<version>/bin/claude.exe`, with
  `<data>/claude-code/current` pointing at it; the previous version is kept, older ones go once no
  process runs them (about 300 MB each). Keep `/proc` readable to the service user (no `hidepid=`,
  no `ProtectProc=` in `yulius.service`), or a version in use could be removed.
- **Runs** in isolated sandboxes use the newer of that version and the host's npm install (Off: the
  host's); running tasks move at a safe turn end, or at once by stop and continue.
- **Default model unknown** (red banner; `yulius doctor --contained`: "Claude Code in sandboxes"
  `FAIL`): "Check now", or `sudo npm install -g @anthropic-ai/claude-code@latest` on the host; else
  pick a model Claude Code knows. The host's install is never changed by Yulius. State:
  `<data>/claude-code/status.json`; failures log "Claude Code was not updated".

[details](archive/operations-until-2026-11-26.md#claude-code-updates)

### Language servers and Claude Code plugins

Yulius switches the language servers of Claude Code plugins (rust-analyzer, pyright, gopls, …) off
in every session it starts, whatever the home's or repository's settings say; nothing to set up or
clean. A run's view: `enabledPlugins` in `<data>/runs/<attempt>/claude-settings.json`, the log line
"switching off the plugins that bring a language server for this run", and `claude --settings <that
file> plugin list`.

[details](archive/operations-until-2026-11-26.md#language-servers-and-claude-code-plugins)

### Messages Yulius types into a session

Yulius types nothing into a session (a `task_message`, pull request news, "Yulius is back; continue",
`/mcp reconnect yulius`) while a person's draft may sit in its prompt box
([decision 0309](decisions/0309-yulius-types-nothing-onto-a-persons-possible-draft.md)): a
`task_message` is refused at once, Yulius's own messages retry, and after 10 minutes the task reads
**Needs your action**. To release it, send or clear what was typed in the task's terminal, or send
any prompt (`continue`); the owner can also send past it with `task_message` and `with_draft: true`.
The mark survives restarts as `person-typed` in the run's folder (`<data>/runs/…`).

[details](archive/operations-until-2026-11-26.md#messages-yulius-types-into-a-session)

### Docker in sandboxes

Each isolated sandbox has its own Docker engine: `docker`, `docker compose`, buildx and
testcontainers work in agents' sessions, the Superagent, Shells and CI jobs (not in production CD).
The host's Docker is never reachable from a sandbox, and no daemon needs to run on the host. Runs see the socket at `/var/run/docker.sock` and
`DOCKER_HOST=unix:///run/yulius/docker/docker.sock`, served by a Docker front beside each holder.
On first use the front starts `dockerd` as root of the sandbox's user namespace, in the sandbox's
network and cgroup, under a seccomp profile every container inherits (`--privileged` included).
Containers count against the sandbox's limits and reach the egress proxy at
`host.docker.internal:<internal port>` (with `--add-host=host.docker.internal:host-gateway`); a port
published in the sandbox's range is relayed like a dev server's.

**Verify** (as root, after a deploy), or with **Run smoke check** on the **Sandbox Docker** card in
Settings → Instance:

```sh
sudo yulius doctor --docker-smoke
```

It runs as the data directory's owner (`systemd-run`) in a throwaway sandbox (`docker-smoke-<pid>`),
is safe beside a running server, and exits 0 only when sandboxes get Docker (needs Docker Hub).

**Prerequisites** (`sudo yulius host setup` brings and proves them; every update re-checks them;
`sudo -u yulius yulius doctor --contained` reports them as "sandbox Docker"):

- `dockerd` in the base: Docker on the host (`docker-ce` with `docker-buildx-plugin` and
  `docker-compose-plugin`) gives every new base `dockerd`, `containerd`, `runc`, `docker-proxy` and
  the CLI plugins; the base is rebuilt 10 minutes after the host last changed, or at once by `sudo
  yulius host setup`. Without Docker on the host an agent can install it in its sandbox (`sudo
  apt-get install -y docker.io docker-buildx docker-compose-v2`); on a base from a `docker-ce` host
  a missing plugin is `sudo apt-get install -y docker-buildx-plugin docker-compose-plugin`.
- A sandbox with its own `docker.io` runs that engine (`yulius doctor`: "sandbox own Docker"); to
  switch, run in it `sudo apt-get install -y docker-ce docker-ce-cli containerd.io
  docker-buildx-plugin docker-compose-plugin`, never `sudo apt-get remove docker.io`.
- The cgroup delegation (§3).
- These kernel modules, listed in `/etc/modules-load.d/yulius-docker.conf`: `overlay bridge veth
  nf_tables nft_compat nft_chain_nat nf_nat nf_conntrack xt_MASQUERADE xt_addrtype xt_conntrack`.
- The `yulius` executable owned by root and writable by no group or other (§2).

- **Disk.** `<data>/sysroots/<sandbox>/docker` counts toward the quota; from 90 % a running
  engine's build cache and unused images are pruned, never containers or volumes.
- **Lifetime.** The engine stops after `YULIUS_SANDBOX_IDLE_MINUTES` idle and survives Yulius
  restarts and updates.
- **Limits.** No image files owned by ids above 65535. `--pids-limit` works; leave out memory and CPU
  flags (`docker run -m`, `--cpus`, compose `mem_limit`, `cpus`). Bind mounts resolve in the
  engine's view (sandbox directory, system layer, engine `/tmp`).
- **Failures.** "the sandbox's Docker engine is not available: …" names the reason and fix, or quotes
  `<data>/runtime/sandboxes/<sandbox>/engine.log` (the front logs to `docker-front.log`).
- **CI leftovers.** Containers and networks labelled `yulius.ci.job=<job id>` go when the job ends
  (volumes kept; journal: `removed what CI jobs left in the sandbox's Docker engine`).
- **A full front.** The front holds at most 256 connections, closing the longest-idle one (300 s or
  more) for a new call; the panel shows "Docker full: <sandbox>". Yulius restarts a front full for 2
  minutes, or whose engine does not answer `GET /_ping`, once no CI job runs (running containers
  stop, volumes and images stay). By hand: the Superagent's `docker_restart`, or `kill -TERM` of the
  pid in `docker-front.json`.
- Engines run with `DOCKER_IGNORE_BR_NETFILTER_ERROR=1`, so networks with `enable_icc=false` work.

[details](archive/operations-until-2026-11-26.md#docker-in-sandboxes)

### Task intake (Jev)

Optional. With a TypeSafe Jev key, Yulius offers to split new tasks that mix separate workstreams,
proposes broad work as a supertask, holds prompts typed into a running task that change its scope,
and chooses a new task's mode and workflow when the composer leaves them on Auto. Without a key
these are off and Auto takes Goal with the workflow named "Feature" (none if there is no such
workflow, and none for a prompt-mode task).

The superadmin sets, replaces, tests and removes the key in setup's optional last step or under
Settings → Instance; it is sealed with `master.key`, takes effect at once and is never logged. A key
file is honoured while no key is saved (shown as "from config"; **Save it here** moves it into
settings, after which the variable can go from the unit).

| Setting (flag / environment) | Default | Meaning |
| --- | --- | --- |
| `--jev-api-key-file` / `YULIUS_JEV_API_KEY_FILE` | unset | File holding a Jev API key, owned by the service user, mode `0600` (a group- or world-readable file is refused). Used while no key is saved in settings; read at start. |
| `--jev-split-threshold` / `YULIUS_JEV_SPLIT_THRESHOLD` | 0.9 | Probability of "several workstreams" Jev must give before a split is offered (0.5–1). |
| `--jev-scope-threshold` / `YULIUS_JEV_SCOPE_THRESHOLD` | 0.8 | Probability Jev must give before a typed prompt is held (0.5–1). |

- The start log says `task intake is on (Jev jev-1.13.0, key saved in settings)` (or `… key from
  YULIUS_JEV_API_KEY_FILE`) or `task intake is off: no Jev key`.
- Requests go to `https://api.typesafe.ai/v1/systemone` (outbound HTTPS). Split proposals run the
  `claude` on the service's `PATH`, uncontained, without tools, in `<data>/runtime/intake/`.
- `journalctl -u yulius | grep 'scope check'` shows whether intake runs (audit: `task.auto_choice`,
  `prompt.held`).

[details](archive/operations-until-2026-11-26.md#task-intake-jev)

### Telegram notifications

An instance administrator sets one Telegram bot (from @BotFather) in Settings → Administration →
Telegram bot; people link their chats in Settings → Notifications. Only outbound HTTPS (long polling),
no open port. The card says `retrying` and why: `refused` (token revoked: set a new one) or
`conflict` (another program polls with the same token).

| Setting (flag / environment) | Default | Meaning |
| --- | --- | --- |
| `--telegram-api-url` / `YULIUS_TELEGRAM_API_URL` | `https://api.telegram.org` | Telegram's Bot API. HTTPS, or HTTP on a loopback host only (a fake Bot API in tests). The bot token is set in Settings → Administration, never here. |

[details](archive/operations-until-2026-11-26.md#telegram-notifications)

### Security scans (Vidoc)

Optional. With a Vidoc key, agents and granted CI jobs run `yulius security-scan`: Vidoc's `vidoc-1`
model reviews the files a branch changed (`--all`: the whole repository). It fails on open critical
or high findings, and on medium ones in changed lines; findings in untouched code become GitHub
issues. Yulius CI adds a built-in `security` job (`yulius security-scan --ci`) to every project
whose CI does not scan already, unless it is a playground or Vidoc is off for it. **The scanned
code is sent to Vidoc** (`https://model.vidocsecurity.com`); keys come from
<https://model.vidocsecurity.com/#key>. Without a key the scan says "not run".

The key is managed like the Jev key (setup's step after Jev, or Settings → Instance; the test is
`GET /v1/models`, which must list `vidoc-1`; audited `vidoc.key_set`, `vidoc.key_removed`).

| Setting (flag / environment) | Default | Meaning |
| --- | --- | --- |
| `--vidoc-api-key-file` / `YULIUS_VIDOC_API_KEY_FILE` | unset | File holding the instance's Vidoc API key, owned by the service user, mode `0600`. Used while no key is saved in settings; read at start. |

- Runs get `VIDOC_API_KEY=yulius-secret-VIDOC_API_KEY`, replaced by the egress proxy toward
  `model.vidocsecurity.com` only (CI jobs when `yulius.toml` says `secrets = "VIDOC_API_KEY"`); an
  agent secret of that name wins ([architecture](architecture.md#agent-secret-scopes)).
- The scan needs `python3` 3.11 or later in the sandboxes (`YULIUS_PYTHON` names another, set in
  the job's command or the session, not on the service).
- **Off for one project** (e.g. while the key fails with 401, 403, 402 or a quota 429): the
  sandbox's admins switch "Scan this project with Vidoc" off on the CI tab's **Security scan
  (Vidoc)** card; a superadmin can on the Instance page's Vidoc card. Audited
  `delivery.vidoc.update`. A repository committing `enabled = false` in `.config/vidoc.toml` stays
  off. A failing scan names the card's address (`YULIUS_VIDOC_SWITCH`, set from `--public-url`).
- **Off for every project**: "Scan every project with Vidoc" on Settings → Instance's Vidoc card
  (`PUT /api/admin/vidoc/enabled`; audited `vidoc.disabled` / `vidoc.enabled`; the key stays).
- The start log says `Vidoc is on (instance key saved in settings)` (or `… from
  YULIUS_VIDOC_API_KEY_FILE`) or `Vidoc is off: no instance key`, plus `Vidoc scans are switched
  off for the instance` while switched off.

[details](archive/operations-until-2026-11-26.md#security-scans-vidoc)

## 5. First run

1. Start the service. On the first start it writes a one-time setup token to
   `/var/lib/yulius/setup.token` (0600) and logs where it is.
2. Open the public URL, enter the token, choose the superadmin password (at least 8 characters, 12
   or more recommended) and confirm both warnings: **the password can be recovered only with the
   recovery code**. Store the recovery code. The next steps ask for the Jev and Vidoc API keys
   (optional; later in Settings → Instance) and the first Claude subscription (skippable).
3. User menu → Administration → GitHub connection: create the GitHub App with the manifest flow for
   the owner account or organization, then install it on the repositories Yulius may use. Its
   callback URLs point at `YULIUS_PUBLIC_URL` (when that changes, see
   [the App after the public URL changes](#the-app-after-the-public-url-changes)).
4. User menu → Administration → Users: admit GitHub logins; they sign in with GitHub, and their
   repository access mirrors GitHub (read / write / admin). The superadmin sees every sandbox with
   admin access without GitHub; merging or releasing as a person needs GitHub sign-in.
5. User menu → Administration → Subscriptions: add Claude subscriptions (Claude's own sign-in in a
   browser terminal); they form one pool, and the capacity panel says when to add more. After buying
   credits for a "Limit reached" one, press **Refresh**. **Do this before importing**: without a
   usable Claude account every agent start is refused, Start onboarding included.
6. Import or create projects (sandboxes). Product and Studio are detected and get the `product` and
   `studio` delivery presets.

[details](archive/operations-until-2026-11-26.md#5-first-run)

### Immediate CI for pull requests opened on GitHub

New Yulius GitHub Apps subscribe to pull request openings at registration. For an existing App, set
`YULIUS_GITHUB_WEBHOOK_SECRET_FILE` on the service to an owner-only file with a random signing secret
(e.g. `openssl rand -hex 32`; it takes precedence over a secret issued at registration) and restart.
In the App registration activate the webhook with the public URL followed by `/api/github/webhook`
and the same secret, and select **Pull request** under **Permissions & events**
([GitHub's webhook settings](https://docs.github.com/en/webhooks/using-webhooks/editing-webhooks)).
Check: a new same-repository PR shows `pull_request.opened` with HTTP 202 in the App's recent
deliveries and a queued run on the CI page. The periodic CI sync recovers missed deliveries.

[details](archive/operations-until-2026-11-26.md#immediate-ci-for-pull-requests-opened-on-github)

### GitHub App permissions

Yulius uses one GitHub App. Its installation tokens and people's sign-in (user-to-server) tokens can
do only what its permissions allow (the latter also capped by the person's own access). What Yulius
does on GitHub for a task (push, pull request, merges, promotions, issues) uses its owner's sign-in
token; without a usable one the App acts. CI statuses, branch cleanup and background reads use the
App. The agent's own Git and `gh` never hold a person's token
([ADR-0114](decisions/0114-agents-hold-a-token-github-refuses-on-production.md)): they get an
installation token restricted to the sandbox's repository and to Contents, Pull requests and Issues
(write), Checks, Commit statuses and Metadata (read), so they cannot touch rulesets, settings, deploy
keys or workflows.

**Protecting the production branch.** Only a person may move it. Add a branch ruleset on each
repository (Settings → Rules → Rulesets → New branch ruleset): target the production branch
(`main`), enforcement Active, rules **Restrict updates**, **Restrict deletions** and **Block force
pushes**, bypass only for the **Repository admin** role, never the Yulius App. New projects get it;
onboarding adds `yulius-people-only-<branch>` with `production_protect` when the task's owner is a
repository admin, else asks with `needs_you`. Without rulesets (a private repository on GitHub Free),
a sandbox admin accepts the risk by switching off **Require a GitHub ruleset on `<branch>`** on the
Git page (`<public_url>/s/<slug>/git?show=production-protection`).

The manifest flow requests these repository permissions:

| Permission | Access | What Yulius does with it |
| --- | --- | --- |
| Contents | Read and write | Clone and fetch repositories; push task branches (as the task's creator); merge pull requests; fast-forward promotions; compare branches. |
| Pull requests | Read and write | Open, list and read pull requests. |
| Issues | Read and write | Agents log what lies outside their change as issues with Definition-of-Ready labels (`issue_create`); missing labels are created. An App created before 2026-10-10 needs it added (below). |
| Checks | Read | Read check runs on pull request heads: GitHub Actions and every other Checks API app. |
| Commit statuses | Read and write | Publish Yulius CI's result as the status `yulius/ci` on each tested commit, and read statuses of projects without Yulius CI. |
| Workflows | Read and write | Push the onboarding commit that deletes `.github/workflows/` (Yulius is the only CI). |
| Administration | Read and write | Create repositories (New sandbox, in an organization or a personal account) and add branch rulesets to them. Repository creation (Read and write) also lets GitHub create them, without the rest of Administration. |
| Metadata | Read | Mandatory for every App: repository facts and the signed-in person's permission level, which Yulius mirrors. |
| Members (organization) | Read | Requested by the manifest; no current Yulius call uses it. |

**Actions: read** is optional. Since **Workflows** lets a task *add* a workflow that runs with the
repository's secrets, turn GitHub Actions off on repositories off Actions (Settings → Actions →
General → Disable actions). A missing permission shows as 403 "Resource not accessible by
integration", named on the Git page; unreadable checks never count as passed. **Creating a project
fails** with either "GitHub refused to create …: the Yulius
GitHub App lacks the repository permission Administration: Read and write …" (add it as below;
nothing was created) or "… was created on GitHub, but the Yulius GitHub App cannot reach it: its
installation covers only selected repositories" (add the repository under the installation's
Repository access, then Import repository). An empty repository can be imported as it is.

**Adding a permission to an existing App:** set it (e.g. Repository permissions → Commit statuses →
Read-only) on `https://github.com/organizations/<org>/settings/apps/<app-slug>/permissions` (personal
account: `https://github.com/settings/apps/<app-slug>/permissions`) and save; the installation's
owner accepts it under Settings → GitHub Apps → the App → Configure; then `systemctl restart yulius`
to drop installation tokens cached for up to an hour.

[details](archive/operations-until-2026-11-26.md#github-app-permissions)

### The App after the public URL changes

GitHub signs people in only from a callback URL the App holds: `<public URL>/api/auth/github/callback`.
After the public URL changes, GitHub refuses sign-ins from the new address ("redirect_uri is not
associated with this application") until a person adds it in the App's settings; GitHub has no API
for this
([ADR-0302](decisions/0302-the-instance-asks-github-about-its-own-callback-and-clients-follow-only-a-moves-redirect.md)).
**Settings → Instance → Address** checks it (at start and on **Check again**) and, when missing,
links **Open the App's settings on GitHub** (`https://github.com/organizations/<owner>/settings/apps/<slug>`,
or `https://github.com/settings/apps/<slug>` for a person's App) with the **callback URL** to add (the
old one may stay until the old host is retired) and the **homepage URL** to replace. The superadmin
password keeps working meanwhile, and `yulius instance migrate` checks the same in its plan. The
check sends GitHub the App's client id and secret with a made-up code; nothing changes at GitHub.

### CI on this machine

Yulius is the projects' only CI; no GitHub Actions or paid GitHub feature is involved.

- **Configuration.** `[[ci.jobs]]`, or `[commands] gate`, in `yulius.toml` on the integration branch
  ([API](api.md)), or `gate` on the sandbox profile (`PATCH /api/sandboxes/{sid}`). A change is
  judged by those jobs plus its own changed ones (`<job> (this change)`); an invalid `yulius.toml`
  fails its run with the parser's message, and while the integration branch's own file is invalid
  its last valid gate runs (`config_blocked_minutes` in `GET /api/analytics/merge-queue`).
- **What runs.** Every open same-repository pull request into the integration branch and every new
  integration or production commit, once. Jobs run in parallel as the `yulius` user, contained when
  `YULIUS_CONTAINED=true`, without any token; a failed job cancels the others (the `security` job
  neither cancels nor is cancelled). Results are on the CI/CD page and on GitHub as `yulius/ci`.
  `YULIUS_CI=off` stops all of it; merges then fall back to GitHub's checks.
- **Toolchains**: jobs see `/usr`, `/opt` and `YULIUS_CONTAINED_READONLY` paths read-only; install
  what projects need system-wide (rustup's toolchains under `/opt`, Node, Python).
- **Directories** in `<data>/sandboxes/<id>/ci/`: `jobs/<job>[.<n>][@<lane>]` (a persistent checkout
  per job, matrix place and lane; keep Cargo's `target/` there), `home` (the jobs' shared `HOME`, no
  build outputs) and `cache/<job>[.<n>][@<lane>]` (`YULIUS_CI_CACHE`). Budget a Rust `target/` per
  job and lane; removing `ci/` with no run active resets the caches.
- **The CI budget.** All sandboxes' jobs share the host's CPUs and memory less an eighth. A job
  starts when the running jobs' declared CPUs (`cpus`, else an equal share of the run) with its own
  stay within three times the budget and its start fits beside what CI is measured to use
  (`memory_gb` counts at the start). Jobs get `YULIUS_CPUS`, `YULIUS_MEMORY_MB` and the
  repository's `[resources.env]`; `YULIUS_CI_MAX_JOBS` still bounds their number. Merge trains go
  first, then pull requests, then branches; `ci_status` says what a job waits for
  ([ADR-0353](decisions/0353-ci-jobs-start-by-what-ci-uses-within-a-bounded-worst-case.md)). A
  project cuts its share with `cpus` per `[[ci.jobs]]` entry (6 jobs on 12 CPUs get 2 each by
  default; `cpus = 1` halves it).
- **Timeouts.** `YULIUS_CI_JOB_TIMEOUT_MIN`, or a job's `timeout_minutes` (1-360), counted from its
  start; waiting for a slot or an `exclusive` turn (one run of it per sandbox) never counts.
- **Weights.** A sandbox whose CI runs weighs 500 on the host, else 100 (`cat
  /sys/fs/cgroup/yulius.slice/yulius-sandboxes.service/sandboxes/<sandbox>/cpu.weight`).
- **Docker networks** are /24s from 172.17.0.0/16–172.31.0.0/16 and 192.168.0.0/16 (253 containers
  each; a project sets its own `subnet` for more); a job waits while fewer than 32 are left.
- **Flaky tests.** A test failing in the same job on two pull requests within 48 hours, but not on
  the integration branch, is quarantined with a `flaky test` issue. Jobs get the names in
  `YULIUS_CI_QUARANTINE` and a nextest filter in `YULIUS_CI_QUARANTINE_NEXTEST`, e.g.
  `cargo nextest run ${YULIUS_CI_QUARANTINE_NEXTEST:+-E "not ($YULIUS_CI_QUARANTINE_NEXTEST)"}`.
  Closing the issue ends the quarantine within an hour.
- **Lanes and shards.** `YULIUS_CI_LANES` (a [resource limit](#resource-limits)) runs of one sandbox
  take a checkout at once; each extra lane has its own checkouts and caches. A job with
  `matrix = "1/2 2/2"` runs one shard per value, each in its own checkout, with the value in
  `YULIUS_CI_MATRIX`, e.g. `cargo nextest run --partition "hash:$YULIUS_CI_MATRIX"` (or
  `pytest-split`'s `--group`).

**Moving a repository off GitHub Actions** (the onboarding step; agents use `ci_setup`):

1. Translate each workflow job's checks into `[[ci.jobs]]`: matrices for splits; drop `${{ }}`
   expressions, Actions secrets and services; install what `uses:` steps installed.
2. `git rm -r .github/workflows` in the same commit and open the pull request; Yulius tests it with
   the CI it adds.
3. Merge once it passes (the App needs **Workflows: Read and write**; Yulius pushes with the task
   owner's token).
4. Turn GitHub Actions off for the repository; rulesets that require Actions checks switch to
   `yulius/ci`, or go.

**Hygiene.** Every 6 hours Yulius removes safe stale branches and worktrees and lists the rest on the
CI/CD page (Repository hygiene card; agents use `repo_cleanup`; audited `hygiene.cleanup`).

[details](archive/operations-until-2026-11-26.md#ci-on-this-machine)

### Script watches on this machine

Script watches run through the process host in each sandbox's containment and CI's resource group;
nothing needs setting up. Definitions and state are in the database (`watches`, `watch_pending`), so
backups and moves carry them. Files: `<data>/watches/<sandbox>/` (`home`, `state/<watch>`,
`runs/<run>`) and `<data>/runs/watch-<run>`. Each run gets 120 s of CPU time (+2 s before the kill),
a 1 GiB data segment, 1 MiB per written file, no core file (`ulimit -c 0`) and its watch's timeout
(default 5 minutes, at most 1 hour); a project's watch files may use 1 GiB, at most 4 runs go at
once and a project has at most 20 watches. A run cut by a restart is recorded as a lost failure. A
watch gets only the agent secrets it names (audit actor `ci:watch:<name>`). Changes are audited
`watch.create`, `watch.update`, `watch.run`, `watch.pause`, `watch.resume`, `watch.remove`.

[details](archive/operations-until-2026-11-26.md#script-watches-on-this-machine)

### CD on this machine

CD (deployment) needs no server configuration: projects define `[cd.dev]`, `[cd.prod]` and any other
`[cd.<name>]` with its `stage` in `yulius.toml` ([API](api.md#cd-deployment)). A run uses the CI job
timeout (`--ci-job-timeout-min`, `YULIUS_CI_JOB_TIMEOUT_MIN`) and a disk bound (`--cd-run-disk-gb`,
`YULIUS_CD_RUN_DISK_GB`, a [resource limit](#resource-limits); past it the run stops as `error` and
its cache is emptied). Dev runs inside the sandbox; production joins the sandbox's production holder
(its own network, no host loopback, its own Docker engine) on the host's read-only `/usr`, `/etc`
and `/opt`, so its scripts install their tools into `$YULIUS_CD_CACHE`.

Files (none visible to agents): `<data>/cd/<id>/source.git` (the verified mirror),
`<data>/cd/<id>/environments.git`, `<data>/cd/<id>/cache/<name>` (`$YULIUS_CD_CACHE`;
`cache/<stage>.<name>` for environments other than `dev` and `prod`), and per run
`<data>/runs/cd-<run>/work` or `<data>/runs/env-<run>/work` (production:
`<data>/cd/<id>/runs/cd-<run>/work` or `…/env-<run>/work`), removed after the run. Runs are kept 14
days, 50 per environment, and each environment's last passed run always.

- **Secrets.** Dev runs get a secret their table names once a person allowed it outside the proxy;
  production runs get a secret agents set up ("Let agents set it up", the default) or one marked
  "Allow for production deploys", never a personal one. Production needs a separate secret from one
  agents use. Output is redacted, but scripts hold the values: allow only what a deploy needs.
- **Agent states.** `states = ["NAME"]` holds those states for the run, restores the newest version
  into `<run files>/work/states/<NAME>` (`$YULIUS_STATE_<NAME>`) and saves changed ones back after
  deploy and verify (audited `vault.state.restore`/`vault.state.save` as `cd:<environment>`).
- **Variables.** Dev runs get all project variables; production only those named
  (`variables = ["NAME"]`); a missing named variable refuses the deploy.
- **Docker.** Dev runs use the sandbox's engine; production its holder's (`<slug>-prod`, data in
  `<data>/sysroots/<id>.prod/docker`, out of every agent's reach; only the run's own files can be
  mounted). Without `dockerd` in the base, run `sudo yulius host setup`.
- **SSH.** Runs reach Internet hosts (port 22 too), not host loopback; pin keys in the project
  (`StrictHostKeyChecking=yes`, own `UserKnownHostsFile`). `YULIUS_SANDBOX_EGRESS=none`: no network.
- **Production.** Only an administrator deploys it, on the CD page, typing the project's slug. With
  `promote = true` it first promotes under that person's GitHub identity; the superadmin, who has
  none, promotes through the GitHub App, and the run says so. The ruleset that keeps agents off the
  production branch keeps the App out too (never add the App to its bypass list: agents' tokens are
  the App's, ADR-0114), so on a protected branch GitHub refuses it, the deploy says so, and a
  repository admin signed in with GitHub deploys instead.
- **Restarts.** A run running when the server stopped is recorded `error` ("outcome unknown") and
  never restarted by itself: check the environment and deploy again.

[details](archive/operations-until-2026-11-26.md#cd-on-this-machine)

### Yulius's own staging VM

Yulius's own repository deploys `dev` to a staging VM: `[cd.dev]` in its `yulius.toml`, scripts in
`scripts/cd/`. No person relies on it; production instances still install `main` through their update helper
(§8). Today's VM is the former production host, `deciding-whale`; its production data was erased.

**A deploy** (`scripts/cd/deploy.sh dev`, run by Yulius CD) builds the commit in the sandbox's
Docker (`scripts/cd/build.sh`, `deploy/build/Dockerfile`: Ubuntu 24.04, pinned toolchain,
`YULIUS_BUILD_COMMIT`; kept in the CD cache), connects as `root@$STAGING_VM_HOST` with
`STAGING_VM_SSH_KEY` and the host key in `scripts/cd/known_hosts`, uploads to
`/var/lib/yulius-releases/<commit>/yulius` (SHA-256 checked), migrates a copy of the live store
(`yulius store-migrate`; lost rows other than a migration's own `DELETE FROM`,
`scripts/cd/migration_deletes.py`, fail it with nothing changed), stops **only** `yulius.service`,
keeps the old binary and store in `/var/lib/yulius-releases/<old commit>/`, installs, starts and
waits for the commit at `/internal/version`, else puts the old pair back. The last 10 releases are
kept; one CD step runs at a time (`/var/lib/yulius-releases/.lock`).

**Verify** (`scripts/cd/verify.sh dev`): the release at `/internal/version`, `/healthz` 200 at the
unit's `YULIUS_PUBLIC_URL`, the contained `yulius doctor`, the store's SQLite check and schema
(`<commit>/migration.json`), and after a deploy `yulius doctor --docker-smoke`.

**Rollback** (`scripts/cd/rollback.sh dev`; the CD page, `cd_rollback`, and the drill after every
deploy) puts back a kept release, and across a schema change the store it left (what staging wrote
since stays in `/var/lib/yulius-releases/<newer>/store`). By hand, as root: `systemctl stop
yulius`, copy the binary and the store back from `/var/lib/yulius-releases/<commit>/`, `chown
yulius:yulius` the store files, `systemctl start yulius`. A run that could not put the previous
store back leaves the server stopped and writes `/var/lib/yulius-releases/HELD`; every deploy,
rollback and verify refuses while it exists. Restore by hand, remove the file and deploy again.

**Who deploys**
([ADR-0436](decisions/0436-yuliuss-full-suite-runs-after-the-merge-and-promotion-comes-from-verified-staging.md)).
A pull request merges on its pull-request jobs (`quality`, `lint`, `macos`, `docs`, `deps`,
`security` and the affected `file-contracts`); `test` and `e2e` run once on each merged revision
of `dev`. A merged task's agent asks with `cd_deploy`, and a person with Deploy on the CD page; each
deploys once the revision's full suite passed. Staging also follows `dev` by itself once 5 merges
behind, or after 60 minutes while `dev` is ahead; set both `serve_*` keys to `0` to stop that.
**Production is promoted only from what staging verified**, by a person. `yulius-update.path` stays
disabled there ("Update now" would install `main`); a deploy refuses while `yulius-update.service`
runs.

**Preparing a staging VM (or swapping it for a smaller one):**

1. Install Yulius as in §2 on Ubuntu 24.04 or later, x86_64: `/usr/local/bin/yulius` and `sudo
   yulius host setup --public-url https://<name>`; put Caddy in front (§4) so
   `https://<name>/healthz` answers 200.
2. Leave the setup token unused, or use a GitHub App of its own, never production's.
3. Add the public half of `STAGING_VM_SSH_KEY` to `/root/.ssh/authorized_keys`, or make a new key
   with `secret_create_from_file` and allow it for `.env` files.
4. Set the project variable `STAGING_VM_HOST` (`[user@]host[:port]`, root unless named; another user
   needs passwordless sudo).
5. Pin the host key in a pull request: the line `yulius-staging ssh-ed25519 <key>` from
   `/etc/ssh/ssh_host_ed25519_key.pub` in `scripts/cd/known_hosts`, after checking its fingerprint
   on the VM (`ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub`).
6. Deploy dev on the CD page once that pull request merged. The binary found on the VM was never
   recorded as a release: return to it by hand.

**Who can reach it.** Every task of this project can get `STAGING_VM_SSH_KEY` and be root there:
keep nothing of value on it, and never give it production's GitHub App or credentials.

[details](archive/operations-until-2026-11-26.md#yuliuss-own-staging-vm)

## 6. Onboarding a repository and your Claude sessions (`yulius onboard`)

On a developer machine with the `yulius` binary:

```sh
yulius login --server https://yulius.example.com          # browser approval (device flow)
cd ~/code/product && yulius onboard                       # --server once signed in to one server
yulius onboard --status                                   # the sandbox's onboarding state
```

`yulius import` is the same command. The repository must be on GitHub under the instance owner (a new
sandbox needs admin or maintain, more sessions need write). Options: `--sessions
all|recent|pick|none`, `--since 14d`, `--show-redactions`, `--dry-run`, `--sessions-only`, `--yes`,
`--no-color` (or `NO_COLOR`). Secrets are redacted locally; run it again to resume an upload.
Sessions are stored read-only under `<data>/sandboxes/<sid>/sessions/` (at most 128 MiB per
transcript, 10 000 sessions and 8 GiB per sandbox; deletions audited `session.deleted`). A new
sandbox's onboarding task waits for **Start onboarding** on the sandbox page, or `yulius project
onboard` with an `operate` token ([remote-control.md](remote-control.md)), not a `yulius login`
token.

A checkout not on GitHub goes to a playground: `yulius onboard --playground [path] [--name <name>]`
(branches, tags, uncommitted work as one commit, sessions; later runs land under `import/<login>/`).

**Removing a sandbox** asks what happens to its files, with no default. **Delete its files
permanently** (`POST /api/sandboxes/{sid}/purge`, or for an already removed one Settings → Instance →
Leftovers → Kept files of removed sandboxes) deletes `<data>/sandboxes/<id>/` (clone, home with the
sessions, worktrees with uncommitted work, CI, browser), `<data>/sysroots/<id>`,
`<data>/runtime/sandboxes/<id>`, `<data>/cd/<id>`, `<data>/displays/<id>`, its runs, transcripts,
the removal's export and a playground's `<data>/playgrounds/<id>.git` once none of its processes is
left; the GitHub repository, task records and audit (`sandbox.purge_requested`, `leftovers.delete`,
`sandbox.purged`) stay. **Keep its files** keeps them, listed with their size
([Leftovers](#leftovers-what-is-kept-what-is-cleaned-and-when)).

[details](archive/operations-until-2026-11-26.md#6-onboarding-a-repository-and-your-claude-sessions-yulius-onboard)

### Importing secrets (`yulius secrets import`)

Keys held on a computer go into the vault from the CLI, signed in with `yulius login`, or with
`yulius remote login` and a token from Settings → Connect your local agent:

```sh
yulius secrets import --to agent .env      # agent secrets of this checkout's sandbox
yulius secrets import --to agent --sandbox acme/shop keys.env
yulius secrets import --to team shared.env                 # team secrets of this checkout's sandbox
yulius secrets import --to team --instance shared.env      # team-wide team secrets
gpg -d keys.env.gpg | yulius secrets import --to team -   # a dotenv text on stdin
yulius secrets import --to agent                           # type each name and its hidden value
```

- **Format.** Dotenv: `NAME=value` lines (optionally `export`), values bare, single-quoted
  (literal) or double-quoted (`\n`, `\t`, `\"`, `\\` read; may span lines), `${…}` kept as written;
  `#` lines directly above a key become its description or notes. A malformed file is refused by
  line number before anything is asked.
- **Review.** Every name is listed as `new`, `exists` (skipped; nothing is overwritten) or skipped
  with the reason (agent names are `[A-Z][A-Z0-9_]*`, values at least 8 characters on one line; a
  PEM or OpenSSH key is kept as a key file). Values that are ordinary text (`production`, a region,
  a host name, a URL without credentials, …) are skipped. Nothing is sent until `yes` is typed,
  except with `--yes` and the `operate` token of `yulius remote login`.
- **Options.** `--from-file NAME=PATH` imports a whole file as one secret; `--env-files` creates
  agent secrets allowed into `.env` files ([remote-control.md](remote-control.md#secrets));
  `--sandbox` names the target (else the checkout's GitHub repository); `--instance` makes team
  secrets team-wide and agent secrets global; `--confirm-hosts` makes a person allow each host and
  production deploys instead of the agents.
- `yulius secrets delete NAME` (operate token; `-p owner/name`) deletes one imported by mistake. Put
  multi-line keys in a file: the hidden prompt reads one line.
- Values never appear in output, on disk or in logs. Exit 0 when every secret was imported, already
  there or declined, 1 otherwise; a second run skips the imported ones.

[details](archive/operations-until-2026-11-26.md#importing-secrets-yulius-secrets-import)

### Dropped files

Files dragged onto the editor go into the repository or a task worktree; files dropped or pasted
onto a terminal are attachments in `<data>/sandboxes/<sid>/attachments/{tasks,shells}/<id>/`,
outside every worktree: at most 1 GiB each (`YULIUS_UPLOAD_MAX_MB`), 500 per task or terminal, 8
uploads at once, refused when the data disk would reach 95 %. A deleted task's attachments wait 7 days in
`attachments/trash/<secs>-<number>-<id>/` (`attachments_trash` restores them); composer drafts in
`attachments/drafts/<user>/` go after a week. Audited `file.uploaded`, `attachment.uploaded`.

[details](archive/operations-until-2026-11-26.md#dropped-files)

## 7. Backup and restore

```sh
sudo -u yulius yulius backup --data-dir /var/lib/yulius /var/backups/yulius-$(date +%F).db
```

`backup` writes a consistent SQLite copy (`VACUUM INTO`). Also back up `/var/lib/yulius/master.key`
(it encrypts every stored secret; without it the database's credentials are unreadable) and, if
wanted, `sandboxes/` (clones and worktrees can be re-created from GitHub, uncommitted work cannot).
Restore: stop the service, put the database copy at `yulius.db`, restore `master.key`, start. The
database copy holds the secrets vault and the egress CA key as ciphertext only; keep `master.key`
apart from database backups (whoever has both can read every secret). For a copy of everything,
files included, see [Copy or move the whole instance](#copy-or-move-the-whole-instance); to move to
another host, use the next section.

### Move the instance to another host

`yulius instance migrate`
([ADR-0297](decisions/0297-one-root-command-moves-an-instance-and-drives-the-new-host-over-a-one-time-key.md))
moves the whole instance with one command. Run it as root on the old host over SSH (not from a
Yulius terminal: it freezes the service), or let a superadmin ask root for it from the page
([Move from Settings → Instance](#move-from-settings--instance)):

```sh
sudo yulius instance migrate root@new-host            # shows the plan, asks once, then moves
sudo yulius instance migrate root@new-host --yes      # without the question
sudo yulius instance migrate root@new-host --max-downtime 5   # never freeze for longer than about 5 minutes
sudo yulius instance migrate root@new-host --data-disk /dev/nvme0n1 --data-disk /dev/nvme1n1
sudo yulius instance migrate root@new-host --public-url https://yulius.example.com
sudo yulius instance migrate root@new-host --no-sign-in   # never sign in as root: print the key's line to add
sudo yulius instance migrate --status                 # the recorded move and its checklist
sudo yulius instance migrate --abort                  # back to the old host
```

It prints one checklist (`ok`, `changed`, `manual`, `FAIL`); exit 0: nothing failed, 1: a step
failed, 2: refused before anything changed. **Run the same command again to continue a move that
stopped.** The new host needs SSH for root and the same architecture, and no Yulius instance; the old
unit must run unfrozen (else use `yulius instance export`, below). `--public-url` is `https://host`.

1. **Plan** (nothing frozen; only `/var/lib/yulius-migrate` changes). It signs in once to add a
   one-time key to root's `authorized_keys` on the new host (with `--no-sign-in`, or when that fails,
   it prints the line to add and stops), checks the new host (release, architecture, systemd, CPUs,
   memory, ports, disk; blank disks for `--data-disk`), estimates copy and freeze, and decides the
   URL (`--public-url`, else the old one, else the same `sslip.io` form). Change DNS when the
   checklist says; for a new URL add `<new URL>/api/auth/github/callback` and the homepage to the
   GitHub App first ([the App after the public URL changes](#the-app-after-the-public-url-changes)).
   Not carried: another proxy than Caddy, another firewall than ufw, packages missing on the release.
2. **Prepare** (old instance runs). The new host gets the data disk (ext4 or RAID1, by UUID,
   `nofail`, `RequiresMountsFor=`), apt sources and manual packages, `/usr/local`, unowned `/opt`,
   root's Rust toolchain, the binary, units, drop-ins, environment and key files, the Caddyfile (URL
   and host name replaced) and ufw's rules. With an `https://` URL its port 443 must answer (else
   `systemctl status caddy`, `journalctl -u caddy -n 50`), so TLS ending at a CDN cannot be moved so.
3. **Copy** (the old instance runs). The data directory goes into a staged copy in passes (`zstd`
   when both hosts have it); later passes send only what changed, by receipts in
   `/var/lib/yulius-migrate/copy`. The line `downtime` gives the estimated freeze. Without
   `--max-downtime` it copies again while the changes would take over a minute (at most five
   times); with it, it freezes only within the bound, else ends with exit 1 and nothing frozen: run
   again later or with a higher bound. **Do not update Yulius on the old host during the copy.**
4. **Cutover.** The old unit and `yulius-sandboxes.service` are frozen, a last pass runs, both hosts
   count the whole copy, and only when equal is the new instance started (it must report the old
   host's build). Until then any failure or signal thaws the old instance. If the new one does not
   come up, it is stopped and disabled before the old one thaws; remove the copy (`rm -rf` the data
   directory on the new host) before the next run. Once the new instance reports the build, the old
   unit stays frozen and is disabled; later failures are lines to act on, never a rollback.
5. **After.** Live tasks are queued again; the old Caddy site redirects (307, header
   `X-Yulius-Moved-To`) to the new URL; `/var/lib/yulius-migrate/arrived.json` and `<data>/moved.json`
   on the new host keep the checklist and note (remove to drop); keys go and ufw is enabled (check
   its rules admit you on the SSH port). Left for people: DNS, the GitHub App, laptops (`yulius
   remote` follows the redirect, [remote control](remote-control.md#2-connect-your-agent); Claude
   Code needs `claude mcp remove yulius` and `claude mcp add`), and the old frozen copy with
   `master.key` and every secret.

**Abort.** `sudo yulius instance migrate --abort` on the old host stops the new instance (after the
finish, first run there `systemctl disable --now yulius.service; systemctl stop
yulius-sandboxes.service`; `--yes` confirms it), restores the old proxy, thaws and enables the old
unit. The copy on the new host stays until you remove it; laptops run `yulius remote login <old URL>`.

A run that ordered the start ignores Ctrl-C and SIGTERM; if it hangs, `kill -9` it and rerun or
`--abort`. Processes a stopped old unit left (`KillMode=process`, §3) end a run with a `FAIL` line
and the command (`--kill-who` is the older spelling, which systemd 252 and later still take
beside `--kill-whom`):

```sh
sudo systemctl kill --kill-who=all --signal=SIGKILL yulius.service
```

Run it, then the same `yulius instance migrate` command again.

#### The old instance held

The move is finished only while the old instance is held, and every run of the same command checks
again ([ADR-0311](decisions/0311-a-move-is-finished-only-while-its-old-instance-is-held-and-every-run-looks.md)):
no unit of it runs unfrozen (both frozen, or the server's unit `inactive`/`failed` and the sessions
unit not running unfrozen), no stopped unit left running processes, and `systemctl is-enabled
yulius.service` says `disabled` (or `static`). Held: `ok` and `finished`, exit 0. Otherwise a `FAIL`
line under **Cutover** says what to run, exit 1, and `--status` shows it until a run finds it held:

```sh
sudo systemctl freeze yulius.service yulius-sandboxes.service   # the units the line names
sudo systemctl disable yulius.service
sudo yulius instance migrate root@new-host                      # looks again
```

**Never stop or kill a frozen unit yourself**: systemd 259 refuses to stop one, and after a kill it
stays frozen on its way to a restart that nothing but a host restart reaches. Packages installed
inside sandboxes keep their files; on another release the checklist names those sandboxes.

[details](archive/operations-until-2026-11-26.md#move-the-instance-to-another-host)

### Move from Settings → Instance

A superadmin moves the instance from **Settings → Instance → Move to another host**; root runs the
same command for it through [the move helper](#the-move-helper-moves-from-settings--instance), and
the page shows its checklist (without the helper the card names the command above).

1. **Show the plan** with the new host (`root@new-host`; root's ssh configuration is not read), its
   SSH port, a new public URL (empty: as the command decides), one or two blank disks and an
   optional longest freeze in minutes. The first plan ends with **the key line to add**: copy it
   into root's `~/.ssh/authorized_keys` on the new host and show the plan again.
2. **Run migration…**, confirmed by typing the host's name, runs the plan shown at most an hour ago.
3. **While frozen** nothing answers, the page included. Afterwards the new instance answers (with a
   new URL the old one redirects; sign in again there). If the move did not complete, the old
   instance offers **Continue the move…** and **Abort…**; after a finished move go back with `sudo
   yulius instance migrate --abort` on the old host.

Only the superadmin sees the card; plans, moves and aborts are audited (`migration.planned`,
`migration.started`, `migration.aborted`). While a move is requested or running, **Update now** and
**Restart…** are refused.

### Copy or move the whole instance

`yulius instance export` and `yulius instance import` copy everything: the database, `master.key`,
every sandbox (clones, worktrees with uncommitted work, homes with the Claude sessions), installed
packages and Docker images and volumes (`sysroots/`), transcripts and runs. Both run as root.

```sh
# straight from the old VM to a new one; nothing is written on the old VM's disk
sudo yulius instance export --move | ssh root@new-vm yulius instance import
# or from a laptop, between two VMs
ssh root@old-vm yulius instance export --move | ssh root@new-vm yulius instance import
# or keep a copy (as sensitive as the server, hence umask 077) and install it later
(umask 077; ssh root@old-vm yulius instance export > yulius-$(date +%F).tar)
ssh root@new-vm yulius instance import < yulius-2026-10-26.tar
# then, on the new VM
sudo yulius host setup --public-url https://yulius.example.com
```

**Export** freezes `yulius.service` and `yulius-sandboxes.service` (the web UI does not answer
meanwhile), prints the exact size, streams an uncompressed tar to standard output (not to a
terminal; pipe it through `zstd` yourself on a slow link) and thaws them, also on failure or a
signal. After a SIGKILL thaw by hand: `sudo systemctl thaw yulius-sandboxes.service yulius.service`.
`--move` keeps the old instance frozen and disables its unit, so it never runs beside the new one;
undo with `sudo systemctl thaw yulius.service yulius-sandboxes.service && sudo systemctl enable
yulius.service`. Left out (the new host makes its own): `system/`, `runtime/`, `update/`,
`claude-code/`, `yulius.db-shm`, sockets, FIFOs and `.yulius-limits-adapt`. Refused (exit 2, nothing
frozen) while the update helper runs, from a Yulius terminal, or while the service is stopped but its
agents run. `--service` and `--update-service` name other units.

**Import** (the same or a newer `yulius` installed, §2, before `host setup`) refuses (exit 2, nothing
written) a newer database schema, a copy from another path than `--data-dir` (default
`/var/lib/yulius`, as `YULIUS_DATA`), a non-empty data directory (`lost+found` and dotfiles aside) or
one of another user or root (it prints the `useradd` and `chown` to run), too little space or inodes,
and a smaller subordinate id range. It makes the service user and ids, extracts, remaps ids and
checks SHA-256 and length; file capabilities and ACLs are not restored. A cut stream fails (exit 1):
`sudo rm -rf /var/lib/yulius` and import again. Then run `host setup` with the public URL the import
prints, copy other `Environment=` lines you changed (keys; leave the resource limits behind), and
point the proxy and DNS at the new VM. The mark `<data>/.yulius-limits-adapt` makes the first start
replace every [resource limit](#resource-limits) pinned for the old host. Running tasks come back as
after a reboot.

### Leftovers: what is kept, what is cleaned and when

Yulius deletes what nothing needs any more by itself: at every start (two minutes in) and every hour.
Each deletion checks its rule again right before it removes anything (a running agent, shell,
browser, CI run or deploy, a live holder, or a task that started working again keeps its files).
Every deletion is audited as `leftovers.delete`, every failure as `leftovers.failed`.

| What | When it goes | By itself? |
| --- | --- | --- |
| A removed sandbox's `worktrees/`, `ci/`, `browser/`, `<data>/import-worktrees/<id>`, `<data>/cd/<id>`, `<data>/displays/<id>` and its agent runs' `<data>/runs/<attempt>` | 10 minutes after the removal, once nothing of it runs | yes |
| A removed sandbox's `<data>/sysroots/<id>` (installed packages, Docker images and volumes) and `<data>/runtime/sandboxes/<id>` | once its holder has stopped (a minute after the removal); removed as root of a mapped namespace | yes |
| Everything else of a removed sandbox: its clone, home with the Claude sessions, attachments, `<data>/transcripts/<task>`, a playground's `origin`, `<data>/exports/sandbox-<slug>-<ms>.json` | **Delete its files permanently**: as soon as nothing of it runs; **Keep**: kept, listed under Kept files of removed sandboxes; the database records always stay | on request |
| A task's rebuildable caches (`target`, `node_modules`, `[disk] caches`, …) | closed, stopped or failed: after 10 minutes; **archived**: at once; **done**: after 30 quiet minutes; waiting: after 4 hours; any idle task at 90 % of the quota or with the data disk critical | yes (the worktree and uncommitted work stay) |
| Units of a Cargo `target` (worktrees and CI checkouts) | unused for 48 hours (6 h with the disk low, 1 h critical), then the least recently used incremental ones above 5 % of the disk (at least 8 GB; half when low, a quarter when critical); never while Cargo compiles there; compiled artifacts only where no task, CI run or Shell uses them | yes, hourly, and every 5-minute round while the disk is low (audited `disk.sweep`) |
| A closed or cancelled task's clean worktree | after 10 minutes (its branch keeps the work) | yes |
| Git's record of a worktree inside the sandbox directory whose directory is gone (`.git/worktrees/<name>`), not locked | always (that record only) | yes |
| A CI checkout of a job or lane no run used for 14 days, and its cache (`ci/cache/<job>[.<n>][@<lane>]`); an entry of `ci/cache` named like no job | while no CI run of that sandbox runs | yes |
| A draft attachment older than 7 days that no task's request or automation's prompt names | always | yes |
| `<data>/runs/<name>` that no attempt record, live run, active CI job, running deploy or running delivery environment run owns, untouched for a day (an ended task's last run stays) | always | yes |
| Transcripts of a deleted task; a Docker smoke test's sandbox whose process is gone | always | yes |
| An unused sandbox base (not current, no live holder mounts it) | always | yes |
| A worktree no task owns; a snapshot import's checkout an interrupted import left in `<data>/import-worktrees/<id>/` (after an hour); a directory under `sandboxes/`, `sysroots/`, `runtime/sandboxes/`, `cd/`, `displays/` or `import-worktrees/` that no sandbox record owns | listed | **only a person** |
| The host's `/var/lib/docker` and `/var/lib/containerd` (docker-ce; Yulius's host daemons are off) | listed, never deleted: remove them on the host if nothing else uses them | report only |
| Update backups under `/var/lib/yulius-update/backups/` | the helper keeps the last 3 | yes (not listed) |

**When the disk runs low** (read every minute): from 85 % used a "Disk 88% full" pill shows, build
directories are swept harder and the safe leftovers go; from 95 % ("Disk almost full") idle tasks'
and CI checkouts' caches go too, until 85 % (audited `disk.pressure`). If it stays low, delete kept
files of removed sandboxes.

**Settings → Instance → Leftovers** (superadmin) lists every item with its size, reason and whether
Yulius deletes it, the last automatic cleanup, and kept files of removed sandboxes (**Delete
files**); **Delete** and **Delete all safe** ask first. From SSH, while the server runs:

```sh
sudo -u yulius yulius leftovers list                 # rescans; id, size, safe/needs you, kind, path
sudo -u yulius yulius leftovers delete --all-safe    # shows them and the total, then asks for yes
sudo -u yulius yulius leftovers delete 3f9c0a1b2c3d4e5f 0a1b2c3d4e5f6a7b --yes
```

It talks to the loopback listener (`--url`, default `http://127.0.0.1:8081`) with the key in
`<data>/leftovers-cli.key` (0600, new at every start; `--data-dir` if elsewhere). Exit 0: done, 1: a
deletion failed, 2: not run. Nothing reached through a link is listed or deleted: bind-mount a part
of the data directory moved to another disk instead of linking it.

### Secrets vault (agent secrets, ADR-20)

Nothing to configure. On first start the server creates the egress CA (its key sealed in the
database) and serves the agents' egress proxy on the internal listener (`YULIUS_INTERNAL_BIND`):
agent runs get `HTTPS_PROXY` pointing there. Claude Code's own API traffic (`*.anthropic.com`,
`claude.ai`) and loopback bypass it. Runs trust the CA through `<run>/egress-ca.pem`, built from the
host's CA roots (`/etc/ssl/certs/ca-certificates.crt` on Ubuntu, from the `ca-certificates`
package); if the server logs that the roots were not found, install `ca-certificates`. Every Bash
command of an agent runs through `<run>/scrub-exec` (`yulius --scrub-exec`), every tool result
through the `PostToolUse` hook. "Yulius no longer accepts this session": the run ended, continue the
task. "Yulius could not be reached" while the server is up: see the next section.

Brokered commands are managed on each project's Secrets page, Commands tab, and through
`GET/POST /api/sandboxes/{sid}/commands` ([API](api.md)). A run's files live under
`<data>/cd/<sid>/runs/cmd-<run>` only while it runs; the 200 newest ended runs of a project are kept
in the database with their redacted output. Audit actions: `command.define`, `command.update`,
`command.remove`, `command.refused`, `command.ask`, `command.approve`, `command.reject`,
`command.expired`, `command.run`.

**Rotation.** Replace a value on the Secrets page after rotating it at its provider (the page marks
values older than 180 days). If `master.key` and a database copy may have leaked together, rotate
every team and agent secret at its source.

[details](archive/operations-until-2026-11-26.md#secrets-vault-agent-secrets-adr-20)

### Leftover agent processes

A process an agent's command leaves behind (a detached test job, a `(server &)`, a headless
browser) is adopted by its session's sandbox init (`bwrap`); Claude Code neither shows nor stops it.
Yulius handles them:

- A run's whole PID namespace ends with the run (stop, delete, archive). In an idle task's or
  Superagent's session leftovers are logged (`leftover processes in the run`) and killed 2 hours
  later (`reaped leftover processes`); working tasks and the agent's own shells are never touched.
- **Connection limits.** The egress proxy holds 256 connections (half per sandbox at most); the two
  relays to Yulius's port 8081 (the holder's forward and the server's `out.sock`) 256 each. At a
  limit the longest-idle (5 minutes) or busiest run's oldest connection gives way; only when all are
  active is a new one refused (`503`, `too many connections; try again`, `ECONNRESET`). A request
  without a response head for 300 s gets `504`. Logs: `egress proxy:` and `sandbox relay:` in the
  journal, `holder.log` and `docker-front.log` in `<data>/runtime/sandboxes/<sandbox-id>/`. A
  *connection trouble* (`connection trouble began`) notifies, naming the task whose run holds most;
  `GET /api/connections` lists them.
- A Superagent that cannot reach Yulius twice in a row (`the Superagent's sandbox cannot reach
  Yulius`) offers **Restart Superagent**, which resumes its conversation.
- **Restarts** need nothing from tasks: `yulius mcp` holds tool calls while Yulius is down, idle tasks
  get "Yulius is back; continue" (`<data>/alive-at`, `<data>/outage-pending`). "MCP server yulius is
  not connected" after a restart: `/mcp` in the terminal, or continue the task.

To find and clear them by hand, without a restart (as root on the host):

```sh
# Processes of one sandbox's agents, oldest first (the sandbox id is in the project's URL). A run
# a pause froze is in a run-* group below agents/, so read every group there.
S=<sandbox-id>
ps -o pid,ppid,etimes,pcpu,args --sort=-etimes -p "$(find \
  /sys/fs/cgroup/yulius.slice/yulius-sandboxes.service/sandboxes/$S/agents -name cgroup.procs \
  -exec cat {} + | paste -sd,)"
# A session's init is the inner bwrap: a `bwrap … -- claude …` whose parent is another bwrap.
# Its children other than `claude` are the leftovers; list one init's children with:
ps -o pid,etimes,pcpu,args --ppid <init-pid>
# End one leftover and everything below it (it never belongs to a running command).
kill -KILL $(pstree -p <leftover-pid> | grep -o '([0-9]*)' | tr -d '()')
# Connections a sandbox's processes hold to Yulius (port 8081 by default), counted per process.
# They are made inside the sandbox's network namespace (on the host, the relay makes them all as
# the server), so look there, through the holder process inside it (`inner_pid` of host.json in
# the data directory's runtime/sandboxes/<sandbox-id>/).
H=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["inner_pid"])' \
  /var/lib/yulius/runtime/sandboxes/$S/host.json)
nsenter -t "$H" -n ss -tnp state established '( dport = :8081 )' \
  | awk -F'pid=' 'NR>1{split($2,a,",");print a[1]}' | sort | uniq -c | sort -rn | head
```

Stopping the task (or closing the Superagent) also ends its session's leftovers at once.

[details](archive/operations-until-2026-11-26.md#leftover-agent-processes)

#### A saturated sandbox relay

Signs: a Superagent panel that cannot reach Yulius; `the Superagent's sandbox cannot reach Yulius` or
`sandbox relay: a connection was dropped at the limit` in the journal or `holder.log`; agents' calls
to `127.0.0.1:8081` reset. About 256 connections in the `nsenter … ss` count above means the relay is
full. End the leftover processes that hold them (the `kill` recipe above), then press **Restart
Superagent** if its tools stay disconnected. A Yulius restart does not free the relay: the holder
keeps running. A holder older than the relay that gives way (`ps -o lstart= -p "$H"` against the
update's time) still refuses when full: end every process in the sandbox and the holder stops after
`YULIUS_SANDBOX_IDLE_MINUTES`; the next run starts a new one.

## 8. Upgrades

### Updates from main (the update helper)

Yulius updates itself from `main` of **Quartio-Studio/yulius**. It knows the commit it was built
from (`yulius version`, `GET /internal/version` on the internal listener) and checks `main` every
hour and when a superadmin opens **Settings → Instance** (after a failed check, 5 minutes later; an
hour after a rate limit). Superadmins get a banner, "Update available: …". Nothing updates by itself:
a superadmin chooses **Update now** and confirms. Then:

1. **Build** (tasks keep working): the root helper fetches `main`, checks the commit is on it and not
   older than the last one installed, and runs `cargo build --release --locked -p yulius` with
   `YULIUS_BUILD_COMMIT`. Running deploys are waited for and new ones refused until the install
   ends; one past the longest a deploy can last (70 minutes by default) cancels the update.
2. **Pause tasks**: agents are frozen with the cgroup v2 freezer (into
   `…/sandboxes/<sandbox>/agents/run-<attempt>`; needs §3's delegation); a CI gate with one job left
   gets 30 s. A task still in a turn after 30 minutes cancels the update; the superadmin can
   **Continue without waiting** or **Cancel** (a cancel during a rustup install may need `rustup
   toolchain uninstall <version>` as root). `cat
   /sys/fs/cgroup/yulius.slice/yulius-sandboxes.service/sandboxes/*/agents/run-*/cgroup.events` shows
   `frozen 1` (`sandboxes/*/run-*` without the `cpu` controller). A starting server thaws leftovers
   except tasks a person paused (`SELECT number, paused_by FROM tasks WHERE paused_by IS NOT NULL`).
   By hand: `for f in /sys/fs/cgroup/yulius.slice/yulius-sandboxes.service/sandboxes/*/{,agents/}run-*/cgroup.freeze; do echo 0 > "$f"; done`.
3. **Restart**: the helper checks that the unit would start again
   ([Where sessions live](#where-sessions-live)), stops `yulius.service`, copies the binary and
   `yulius.db` (with its WAL) to `/var/lib/yulius-update/backups/<run>/`, installs, starts and waits
   up to 2 minutes for the new commit at `/internal/version`, else restores the previous binary
   **and database** ("rolled back"). Then `yulius host check --json` (report in
   `/var/lib/yulius-update/host-check.json`); a failed item shows on the Sandbox Docker card.
4. **Resume**: tasks thaw one by one; tasks a person paused stay paused.

**Restart…** (Settings → Instance) pauses and resumes tasks the same way around a plain `restart`
(nothing backed up or installed). It needs the helper, is refused while a deploy runs, and is better
than `systemctl restart`, which pauses nothing. Restart… and Update now are refused on a host where
the server could not start again while a session runs; the answer names `sudo yulius host setup`.

**In running sessions**, hooks and MCP tools answer from the new build at once, and changed skills
are reinstalled into each sandbox's `home/.claude/skills` (audited `skills.refreshed`); briefings and
run settings change at a session's next launch. To list refreshes: `sudo -u yulius sqlite3
/var/lib/yulius/yulius.db "SELECT at, target, detail FROM audit_log WHERE action =
'skills.refreshed' ORDER BY at DESC LIMIT 20"`.

**How the helper is reached.** A root-owned **path unit** starts it when the server writes
`/var/lib/yulius/update/request.json` ("build" or "install" one commit, or "restart"; nothing else).
It refuses a request not a regular file of the service user, over 16 KiB, malformed, older than 15
minutes or handled, and reports in `/var/lib/yulius-update/status.json`.

**Install the helper** (once, as root; the unit files are in the repository under
`deploy/systemd/`):

```sh
install -m 0644 deploy/systemd/yulius-update.path deploy/systemd/yulius-update.service /etc/systemd/system/
install -d -m 0755 -o root -g root /var/lib/yulius-update
# Edit the lines marked "local" in yulius-update.service if this server differs: the data
# directory, the internal listener (YULIUS_INTERNAL_BIND), rustup's cargo for root.
systemctl daemon-reload
systemctl enable --now yulius-update.path
systemctl status yulius-update.path        # active (waiting)
```

```ini
# /etc/systemd/system/yulius-update.path
[Unit]
Description=Yulius update requests

[Path]
PathChanged=/var/lib/yulius/update/request.json
Unit=yulius-update.service

[Install]
WantedBy=multi-user.target
```

```ini
# /etc/systemd/system/yulius-update.service
[Unit]
Description=Yulius update helper
StartLimitIntervalSec=0
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
User=root
ExecStart=/usr/local/bin/yulius update-helper
Environment=YULIUS_DATA=/var/lib/yulius
Environment=YULIUS_UPDATE_HEALTH=http://127.0.0.1:8081/internal/version
Environment=YULIUS_UPDATE_BINARY=/usr/local/bin/yulius
Environment=YULIUS_UPDATE_SERVICE=yulius.service
Environment=YULIUS_UPDATE_STATE=/var/lib/yulius-update
Environment=HOME=/root
Environment=PATH=/root/.cargo/bin:/usr/local/bin:/usr/bin:/bin
TimeoutStartSec=3h
Nice=10
IOSchedulingClass=idle
PrivateTmp=yes
NoNewPrivileges=yes
ProtectSystem=strict
ReadWritePaths=/var/lib/yulius-update /var/lib/yulius /usr/local/bin /root/.cargo /root/.rustup
# The re-check after an install (`yulius host check`) keeps the kernel modules list, the
# needrestart rule and Chrome's managed policy for the sandbox browsers (the directories are made
# by `yulius host setup`).
ReadWritePaths=/etc/modules-load.d -/etc/needrestart/conf.d
ReadWritePaths=-/etc/opt/chrome/policies/managed -/etc/chromium/policies/managed -/etc/chromium-browser/policies/managed
UMask=0022
```

- Without `StartLimitIntervalSec=0` quick refusals can stop the path unit: add it under `[Unit]`,
  `systemctl daemon-reload`, and if `yulius-update.path` failed with `unit-start-limit-hit`:

  ```sh
  systemctl reset-failed yulius-update.service yulius-update.path
  systemctl start yulius-update.path
  systemctl status yulius-update.path        # active (waiting)
  ```

- `yulius host setup` adds a missing second `ReadWritePaths` line as a drop-in
  (`yulius-update.service.d/10-yulius-host-check.conf`). No sudo or polkit rule is needed. The build
  needs rustup for root (`rust-toolchain.toml` picks the version), `git`, a C linker, GitHub and
  crates.io.

**Reading the repository** (private): Yulius hands the helper the GitHub App's installation token
when the App is installed on Quartio-Studio with access to `yulius` (Contents: read); otherwise a
fine-grained personal access token with Contents: read on that repository, in a file:

```sh
install -o yulius -g yulius -m 0600 /dev/null /var/lib/yulius/update.token
sh -c 'cat > /var/lib/yulius/update.token'   # paste the token, then Ctrl-D
# in yulius.service: Environment=YULIUS_UPDATE_TOKEN_FILE=/var/lib/yulius/update.token
```

Settings → Instance says which credential read `main` and why a check failed.

| Setting (flag / environment) | Default | Meaning |
| --- | --- | --- |
| `--update-token-file` / `YULIUS_UPDATE_TOKEN_FILE` | unset | Owner-only file with a token that may read Quartio-Studio/yulius, used when the App cannot. |
| `--update-state-dir` / `YULIUS_UPDATE_STATE` | `/var/lib/yulius-update` | The helper's state directory; "Update now" is offered when it exists. |
| `--update-check-minutes` / `YULIUS_UPDATE_CHECK_MINUTES` | 60 | Minutes between checks of `main` (0: only on opening Instance and "Check now"). |

**Logs and recovery.** `journalctl -u yulius-update` shows each step. A failed rollback (the helper
says "restore by hand") leaves the previous binary and database in
`/var/lib/yulius-update/backups/<run>/`: stop `yulius`, copy `yulius` to `/usr/local/bin/yulius` and
`yulius.db` (and `yulius.db-wal`, if present) to `/var/lib/yulius/` owned by `yulius`, mode 0600,
remove `yulius.db-shm`, start `yulius`. The helper keeps the last 3 backups. A manual build should
set the commit it is built from: `YULIUS_BUILD_COMMIT=$(git rev-parse HEAD) cargo build --release
--locked -p yulius` (a checkout does it by itself; a tarball has no Git).

**By hand (without the helper)**: replace the binary and `systemctl restart yulius`. Live sessions
and terminals keep running and are reattached (dead runs are marked stopped, never respawned);
queued tasks start when capacity allows.

[details](archive/operations-until-2026-11-26.md#updates-from-main-the-update-helper)

### The move helper (moves from Settings → Instance)

[Move from Settings → Instance](#move-from-settings--instance) asks root for `yulius instance
migrate` the way updates ask for an install
([ADR-0305](decisions/0305-a-root-helper-runs-the-move-asked-from-the-page-and-the-checklist-travels-with-it.md)):
the server writes `/var/lib/yulius/migrate/request.json`, a root-owned **path unit** starts `yulius
migrate-helper`, and the helper runs the command. A request can only say `plan`, `move` or `abort`
and name the new host, its SSH port, a public URL and data disks; nothing secret is in it.

**What enabling it allows.** The service user can then have root move the instance to a host where
a person added the move's one-time key to root's `authorized_keys`, and to no other (the helper
always passes `--no-sign-in`). A takeover of the server process would gain the host files a move
carries (the unit's environment and key files, the proxy's configuration, `/usr/local`, root's Rust
toolchain) and the stop of this instance. So `yulius host setup` does not install it: root does, on
purpose. To limit it further, enable the path unit only for a move (`systemctl disable --now
yulius-migrate.path` afterwards), or pin the hosts with `YULIUS_MIGRATE_TARGETS`.

**Install the helper** (once, as root; a second instance names the files after its own unit,
`<unit>-migrate`):

```sh
install -m 0644 deploy/systemd/yulius-migrate.path deploy/systemd/yulius-migrate.service /etc/systemd/system/
# Edit the lines marked "local" in yulius-migrate.service if this server differs.
systemctl daemon-reload
systemctl enable --now yulius-migrate.path
systemctl status yulius-migrate.path       # active (waiting): the card offers "Show the plan"
```

```ini
# /etc/systemd/system/yulius-migrate.path
[Unit]
Description=Yulius move requests

[Path]
PathChanged=/var/lib/yulius/migrate/request.json
Unit=yulius-migrate.service

[Install]
WantedBy=multi-user.target
```

```ini
# /etc/systemd/system/yulius-migrate.service
[Unit]
Description=Yulius move helper
After=network-online.target
Wants=network-online.target
StartLimitIntervalSec=0

[Service]
Type=oneshot
User=root
ExecStart=/usr/local/bin/yulius migrate-helper
Environment=YULIUS_DATA=/var/lib/yulius
Environment=YULIUS_MIGRATE_STATE=/var/lib/yulius-migrate
Environment=YULIUS_MIGRATE_SERVICE=yulius.service
Environment=YULIUS_MIGRATE_UPDATE_SERVICE=yulius-update.service
#Environment=YULIUS_MIGRATE_TARGETS=new-host.example.com
Environment=HOME=/root
Environment=PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
TimeoutStartSec=infinity
TimeoutStopSec=10min
KillMode=mixed
UMask=0022
```

- A stop or shutdown sends the command SIGTERM (`KillMode=mixed`): before the new instance's start
  it thaws the old one; after, it finishes the cutover within ten minutes. A failed path unit needs
  `systemctl reset-failed yulius-migrate.service yulius-migrate.path` and a start.
- The helper takes only a regular file of the data directory's owner (not root), at most 16 KiB and
  15 minutes old, with a new id and valid values; not while the update helper runs or for a host
  outside `YULIUS_MIGRATE_TARGETS`. It runs `yulius instance migrate --no-sign-in
  --request-id=<the request's id>` without a shell (`--yes` only for a move).
- Files in `/var/lib/yulius-migrate` (0711): `helper.json`, `handled`, `status.json`, `state.json`,
  the one-time key and, after an arrival, `arrived.json`. None holds a secret.

| Setting (flag / environment) | Default | Meaning |
| --- | --- | --- |
| `--migrate-state-dir` / `YULIUS_MIGRATE_STATE` (server and helper) | `/var/lib/yulius-migrate` | Where the helper and the command report, and where a move to this host left its checklist. |
| `YULIUS_MIGRATE_TARGETS` (helper's unit) | unset | The only hosts a request may name, separated by commas; unset or empty: any host. |
| `YULIUS_MIGRATE_SERVICE`, `YULIUS_MIGRATE_UPDATE_SERVICE` (helper's unit) | `yulius.service`, `yulius-update.service` | The server's unit and the update helper's, as the command takes them. |

**Logs.** `journalctl -u yulius-migrate`. "The move helper did not take the request" on the page:
`systemctl status yulius-migrate.path yulius-migrate.service`.

[details](archive/operations-until-2026-11-26.md#the-move-helper-moves-from-settings--instance)

### Notes for specific upgrades

Notes for upgrades that every host has already taken (and "The way back" for a host that restarted
on systemd 258 or later without the sessions units) are in the
[archived guide](archive/operations-until-2026-11-26.md#notes-for-specific-upgrades).

## 9. Security notes

- The data directory is owner-only (0700); secrets are sealed with `master.key`
  (XChaCha20-Poly1305); vault secrets under envelope encryption (per-secret data keys wrapped by a
  key derived from `master.key`), with re-authenticated, audited reveals.
- Agent secrets are never given to agents: the egress proxy adds them for their bound hosts and every
  channel an agent reads is redacted. Remaining threats: a bound host echoing a credential in another
  form, misuse on its hosts, weak values. For a token agents only read with, set `GET` rules on the
  Secrets page (Edit): the proxy refuses other uses (403, audited `vault.agent.refused`;
  [ADR-0458](decisions/0458-an-agent-secret-can-be-limited-to-methods-and-paths.md)); do not allow it
  into `.env` files. Rules cannot tell request bodies apart (a GraphQL mutation is a `POST /graphql`).
- Brokered commands give a CLI an agent secret as a key file outside the agents' sandbox
  ([ADR-0459](decisions/0459-brokered-commands-run-in-the-production-holder-with-a-temporary-key-file.md)).
  When you define one:
  - It runs in the project's production holder on the host's read-only `/usr`, `/etc` and `/opt`
    (no Docker; the network of production CD runs): install the binary on the host.
  - Keep `subcommands` to what agents need, put every flag that names a key, a config file or a
    server in `refuse`, and use `approve: always` (or `send`) for anything that deploys or signs.
  - Output is redacted of the project's agent secrets, but not of a key printed transformed
    (`openssl rsa -text`, a hex dump): do not define such subcommands.
  - A non-ASCII argument shows single-quoted: check it on the Secrets page before approving. Lines
    Telegram cannot show exactly (over 380 characters, or a backtick) are approved on the page only;
    a Telegram approval needs only the linked chat, so unlink chats you no longer hold (`/stop`).
  - Env values are not secret; the key goes into env only as `{secret}`. At most 4 runs of a
    project's commands run at once; each run's files, its key with them, are removed when it ends
    and at the next start after a crash.
- Agents receive per-run task tokens (MCP, hooks). Their own Git and `gh` get an App installation
  token restricted to their sandbox repository (see [GitHub App permissions](#github-app-permissions));
  provider credentials reach the agent process only through its environment.
- The sandbox browser speaks DevTools over a pipe and listens on no port.
- Isolated sandboxes cannot reach the host's loopback services (Yulius's public listener, Caddy's
  admin API, databases bound to 127.0.0.1). Services bound to the host's public addresses remain
  reachable like any internet host.
- Password sign-in is limited per client only with the proxy's shared secret configured
  (`YULIUS_PROXY_SECRET_FILE`, §4 "Per-client sign-in limits"); without it every client behind the
  proxy shares one limit.

[details](archive/operations-until-2026-11-26.md#9-security-notes)
