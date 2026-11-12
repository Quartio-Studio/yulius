# Operating Yulius

Yulius is one Rust binary, `yulius`, that serves the web UI and API, supervises agent sessions and
provides the CLI (`yulius onboard` alias `yulius import`, `yulius login`, `yulius secrets import`,
and `yulius remote|project|task|ci` for a laptop's agent, [remote-control.md](remote-control.md)). This guide covers a single Linux VM with a TLS
reverse proxy in front. It satisfies SPEC-001 AC-OPS-1 as documentation only: **a deployment has not
been performed or verified** as part of the implementation work.

## 1. Prerequisites

| Component | Needed for | Notes |
| --- | --- | --- |
| Linux x86_64 or arm64, systemd | the service | macOS works for development (runs are "not contained"). |
| `git` ≥ 2.40 | sandboxes, worktrees, delivery | on `PATH` of the service; `yulius host setup` installs it. |
| Docker Engine (`docker-ce` with `docker-buildx-plugin` and `docker-compose-plugin`) | Docker in sandboxes: every new base copies `dockerd`, buildx and compose | from Docker's apt repository; `yulius host setup` installs it and stops the host's own daemon, which nothing uses (see [Docker in sandboxes](#docker-in-sandboxes)). |
| Claude Code (`claude`) | Claude tasks and Claude account login | installed with npm system-wide (`npm install -g @anthropic-ai/claude-code`; verified with 2.1.292 and 2.1.322). Isolated sandboxes then get newer releases by themselves, each checked for Yulius's default model first: see [Claude Code updates](#claude-code-updates). |
| GitHub CLI (`gh`) ≥ 2.82.1 | `gh` in every sandbox, logged in as the person (their shells) or the task's creator (agents) | install it on the host from [GitHub's package repository](https://github.com/cli/cli/blob/trunk/docs/install_linux.md), not the distribution's package: Ubuntu 24.04 ships 2.45, and since GitHub retired Projects (classic) `gh pr edit` and `gh issue view --comments` fail with a GraphQL error before 2.82.1. Sandboxes copy the host system, so rebuild the base afterwards (`yulius sandbox-base build`). `yulius doctor` warns without it or below 2.82.1. |
| bubblewrap (`bwrap`) | containment (`--contained`) | strongly recommended in production; without it runs are marked "not contained". `yulius host setup` installs it. |
| An AppArmor rule that lets `/usr/bin/bwrap` create user namespaces | every sandbox, on Ubuntu 24.04 and later (`kernel.apparmor_restrict_unprivileged_userns = 1`) | `yulius host setup` writes `/etc/apparmor.d/yulius-bwrap` and, on Ubuntu 26.04, disables the shipped `bwrap-userns-restrict`; never write one by hand (see [Sandbox isolation](#sandbox-isolation)). Debian needs none. |
| `uidmap` (`newuidmap`, `newgidmap`) and a subordinate id range for the service user | sandbox isolation (SPEC-002 ADR-22): `apt` in sandboxes, private networks | `/etc/subuid` and `/etc/subgid` lines of at least 65,536 ids (see [Sandbox isolation](#sandbox-isolation)); `yulius host setup` installs both. |
| `slirp4netns` | sandbox egress (isolation) | `yulius host setup` installs it. |
| Chromium / Google Chrome | sandbox browser | optional; found on `PATH` or set `--chromium`. Google Chrome (`google-chrome`) is recommended: it decodes and plays H.264 pages and is what captchas expect. |
| `xvfb` | the sandbox browser's virtual display (SPEC-002 ADR-15) | `apt install xvfb`. Without it (or ffmpeg) the browser runs headless: JPEG frames, no real input, captchas fail. `xauth` is not needed (Yulius writes the cookie itself); `x11-utils` (`xdpyinfo`) only for diagnostics. |
| `ffmpeg` with libx264 and x11grab | the browser video | `apt install ffmpeg` (Ubuntu's build has both; `yulius doctor` checks). |
| Fonts | pages in the sandbox browser | at least `fonts-noto-core fonts-noto-color-emoji fonts-liberation` (Ubuntu 24.04 names); add `fonts-noto-cjk` for CJK pages. |

Inside containment an agent sees only `/usr`, `/bin`, `/lib`, `/lib64`, `/etc`, `/opt`, its own
sandbox directory and the `yulius` binary (with isolation, `/usr`, `/etc`, `/var` and `/opt` come
from the sandbox's own system layer, a copy-on-write view of the host system). Install Claude Code system-wide under `/usr/local` or
`/opt`; a per-user install such as Claude Code's
default `~/.local/bin/claude` is invisible to contained runs unless its directories are added with
`--contained-readonly <dir>[:<dir>…]` (`YULIUS_CONTAINED_READONLY`).

`yulius host setup` (§2) installs and fixes what it can of these. Run `yulius doctor --data-dir
/var/lib/yulius [--contained]` to check all of them, the data directory
permissions and the session socket path budget (the data directory path must be short enough that
`<data>/logins/<uuid>/pty.sock` fits the 103-byte Unix socket path limit).

## 2. Build and install

Two commands on a fresh Ubuntu 24.04 or Ubuntu 26.04 (or Debian) host: install the binary, then
run the host setup as root (SPEC-203, [ADR-0162](decisions/0162-one-host-setup-command-brings-and-keeps-docker-in-sandboxes.md)).

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
   `docker-compose-plugin` from Docker's repository. The source goes to
   `/etc/apt/sources.list.d/docker.sources` and the key to `/etc/apt/keyrings/docker.asc`, unless
   a source already names `download.docker.com`. A host with Ubuntu's `docker.io` keeps it and
   only gets `docker-buildx` and `docker-compose-v2`. A broken third-party apt source does not
   stop the install.
4. **host Docker units**: `docker.socket`, `docker.service` and `containerd.service` are
   disabled and stopped; sandboxes run their own engines. A host Docker or containerd that runs
   containers (`docker ps`, and `ctr` tasks outside Docker's namespace), or whose containers
   cannot be counted, is left running, and the item says so (manual).
5. **kernel modules**: `/etc/modules-load.d/yulius-docker.conf` lists the engine's modules for
   every boot, and missing ones are loaded now.
6. **service user**: the data directory's owner, else `--user` (default `yulius`), created as a
   system user with the data directory (mode 0700) as its home.
7. **subordinate ids**: a range of 65,536 ids after every range in use.
8. **bwrap namespaces**: starts bwrap as the service user in a user namespace whose child mounts
   a tmpfs, which is what a sandbox does first. When that works, nothing is written. When
   AppArmor restricts user namespaces (Ubuntu 24.04 and later), it writes and loads
   `/etc/apparmor.d/yulius-bwrap`; on Ubuntu 26.04 it first disables the package's
   `bwrap-userns-restrict` with a link in `/etc/apparmor.d/disable/` (the file itself is never
   edited). With its profile it loads the directory's other profiles as a boot does, and proves
   then; where bwrap starts only without Yulius's profile, a person's profile lets it in: that
   one is adopted and Yulius's removed. A step that fails is undone and the line says so. A
   rule a person wrote for `/usr/bin/bwrap` is left as it is and named.
   See [Sandbox isolation](#sandbox-isolation).
9. **yulius.service**: written from §3 and enabled when missing (`--public-url` is needed then).
   Otherwise, a drop-in `yulius.service.d/10-yulius-sandboxes.conf` adds `Delegate=yes`,
   `DelegateSubgroup=server` and `KillMode=process` if they are missing. The restart that applies
   it is manual: run it when it suits. A unit file systemd has not loaded is never overwritten.
10. **yulius binary**: the running binary and the unit's, links resolved, are made root-owned,
    writable by root alone. A binary in a directory the service user or others may write fails
    instead: fixing the file would not stop it being replaced.
11. **sessions unit**: `yulius-sandboxes.service`, `yulius-adopt.service` and the drop-in
    `yulius.service.d/20-yulius-sessions.conf` ([Where sessions live](#where-sessions-live),
    [ADR-0298](decisions/0298-sessions-live-in-their-own-unit.md)) are written when they differ,
    and the sessions unit is started. A running server is not restarted: its next restart moves
    its sessions there and keeps them. The adopt unit runs the server's binary as root, so the
    item writes nothing and fails unless the path the server's unit names is a regular file
    that only root may write, in directories that are root's and not writable by everyone or
    by a group the service user is in. A path through a link is refused: name the file itself
    in `ExecStart=`. Units that systemd has not loaded (files copied in without `systemctl
    daemon-reload`) are loaded, and a sessions unit that does not run is started. `yulius host
    check` only reports this item, in each of these cases.
12. **update helper**: when `yulius-update.service` is installed, a drop-in lets its re-check
    write `/etc/modules-load.d` and `/etc/needrestart/conf.d` (§8).
13. **sandbox base**: built now, as the service user, when there is none, when it lacks the
    host's `dockerd`, or when it no longer matches the host. Live sandboxes' bases are kept
    ([ADR-0161](decisions/0161-a-base-a-live-holder-mounts-is-never-collected.md)), so this is
    safe while sandboxes run.
14. A unit it wrote is started; then "sandbox Docker", the verdict `yulius doctor` gives; the
    contained doctor as the service user; and the Docker smoke test (`--no-smoke` skips it; it
    needs Docker Hub).

Running it again changes nothing ("nothing to change"); it is safe on a running instance.
`--dry-run` prints what it would change and changes nothing. With a unit named other than
`yulius.service`, pass `--service <name>` (the update helper passes its `YULIUS_UPDATE_SERVICE`).
A setup or `yulius host check` run keeps its report in `/var/lib/yulius-update/host-check.json`
when that directory exists, so Settings → Instance shows the latest root check. A helper with
another `YULIUS_UPDATE_STATE` needs `--update-state <dir>`.

**Still manual**, and why:

- the reverse proxy and TLS (§4): they depend on your domain;
- Claude Code: install it once system-wide with npm (§1); Yulius keeps the version isolated
  sandboxes run current by itself ([Claude Code updates](#claude-code-updates));
- the GitHub CLI: it comes from GitHub's own repository (§1), and the doctor warns without it;
- the sandbox browser's packages (Chrome, `xvfb`, `ffmpeg`, fonts): optional;
- the update helper (§8): it needs root's Rust toolchain and this server's values.

The agent CLIs must be installed for (and logged-in state is never taken from) the `yulius` user:
Claude subscriptions are added inside Yulius (user menu → Administration → Subscriptions), not with the CLIs' own
`login` on the VM.

## 3. systemd unit

`yulius host setup` writes this unit when there is none (§2), and gives an existing one
`Delegate=yes`, `DelegateSubgroup=server` and `KillMode=process` by a drop-in.

`KillMode=process` is required: each agent session and each user terminal (sandbox shell) runs in
its own detached `yulius ptyhost` process so that they survive a restart or upgrade of the server
(NFR-001, UI-016; the next start adopts the terminals from `shells/<id>/shell.json`). With the
default `KillMode=control-group`, stopping the service would kill what the server itself started
and still holds in its unit. The sessions themselves live in a unit of their own, which a restart
of the server never touches: see [Where sessions live](#where-sessions-live).

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
# sandbox's, at least 2). CI jobs get what they declare in yulius.toml (SPEC-082).
#Environment=YULIUS_AGENT_CPUS=4
Environment=YULIUS_CI_JOB_TIMEOUT_MIN=30
Environment=YULIUS_CI_POLL_SECS=60
# GB a CD or delivery environment run's files and cache may use (ADR-0216; a resource limit,
# host-sized).
#Environment=YULIUS_CD_RUN_DISK_GB=20
# Sandbox isolation (SPEC-002 ADR-22): auto (default), namespaces or legacy; see "Sandbox
# isolation" below for limits and egress.
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

Sessions and sandboxes do not live in `yulius.service`'s own cgroup. systemd 258 and later
(Ubuntu 26.04 has 259) start a unit inside the unit's own group, and the kernel refuses that
while a sandbox there holds a process: a server whose sessions lived in its unit could be stopped
and never started again while one session ran (the outage of 2026-11-11,
[ADR-0298](decisions/0298-sessions-live-in-their-own-unit.md)). `yulius host setup` therefore
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
  `/sys/fs/cgroup/yulius.slice/yulius-sandboxes.service/sandboxes/<sandbox>/`. Its one process is
  a `sleep` that belongs to root, so no signal of the service user reaches it (a stray `pkill`
  cannot end it). **Stopping or restarting this unit ends every agent session, terminal and
  sandbox engine.** Nothing does so by itself: the needrestart rule of §2 names it. If it was
  stopped or restarted all the same, restart `yulius.service` afterwards: the server prepares
  the unit's groups only when it starts, and sandboxes started before that restart run without
  their limits ("cannot apply the sandbox's limits" in the log).
- **`yulius-adopt.service`** runs before every start of `yulius.service` (`yulius host
  adopt-sessions`, as root, at most 2 minutes). Sessions that an earlier layout left in
  `yulius.service`'s own group move to the sessions unit with their limits; a sandbox's Docker
  engine found there is killed with its containers (no shutdown: what a container had not
  written is lost) and starts again on demand. Nothing to move is the normal case, and while the
  server runs the step moves nothing (`systemctl start` on a running unit starts it again). Its
  output is in `journalctl -u yulius-adopt`. Because it runs the server's binary as root, that
  binary must be a regular file only root may write, reached through directories that are
  root's (§2, item 11), and not through a link; `yulius host setup` refuses to write the unit
  otherwise.
- `systemctl restart yulius`, a crash (`Restart=on-failure`), Restart… and an update then keep
  every session, on systemd 255 and on 259.
- `yulius doctor` says which holds in its line "restart with sessions", and `yulius host check`
  (after every update, shown on the Sandbox Docker card) reports "sessions unit" when a file is
  missing or differs.
- A host without the files keeps sessions in `yulius.service`'s own group
  (`/sys/fs/cgroup/system.slice/yulius.service/sandboxes/…`). That still restarts on systemd 257
  and earlier. On 258 and later, Restart… and Update now are refused while a session runs
  ("yulius.service would not start again: … nothing was stopped"), by the server and again by
  the update helper before it stops anything; they are refused the same way where the files are
  there but `yulius-sandboxes.service` does not run (the adopt step needs it). `sudo yulius host setup` writes the files and
  restarts nothing; the next restart then moves the sessions and keeps them. A plain `systemctl
  restart yulius` is not guarded: on such a host it leaves the server down until the steps under
  "the way back" in §8 are done.
- With another unit name (`--service yulius-dev.service`) the files are
  `yulius-dev-sandboxes.service`, `yulius-dev-adopt.service` and
  `yulius-dev.service.d/20-yulius-sessions.conf`, and the slice is `yulius_dev.slice`.

## 4. Reverse proxy (TLS, WebSockets)

Only the public listener (`YULIUS_BIND`) is proxied. The internal listener (`YULIUS_INTERNAL_BIND`)
serves agents' MCP, hooks and Git credentials; it must stay on loopback and is never proxied.
WebSockets are used for terminals, the event stream and the browser panel; import
chunks are up to 8 MiB and a dropped file up to the upload limit (`YULIUS_UPLOAD_MAX_MB`, 1 GiB
by default, for screen recordings: ADR-0219), so the proxy's body limit must be above it (1100 MB
below; change both together). Let the proxy compress responses: Yulius serves its web UI uncompressed,
and compression cuts the first load from about 1 MB to about 300 KB (the editor's first open from
4.6 MB to about 1.2 MB), which matters most on phones.

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

`YULIUS_PUBLIC_URL` must be the exact external origin: cookies are `Secure` (and the session
cookie `__Host-` prefixed) for `https` URLs, and the `Origin` of WebSockets and of every
cookie-authenticated API request is checked against it.

Yulius itself sends `X-Frame-Options: DENY`, `Content-Security-Policy: frame-ancestors 'none'`
(nothing may frame Yulius, not even its own browser panel), `X-Content-Type-Options: nosniff`,
`Referrer-Policy: same-origin` and `X-Yulius: <process tag>` on every response (stricter route
policies are kept). HSTS is the TLS proxy's job, as in the examples above; add it once the
domain is meant to stay on HTTPS.

### Per-client sign-in limits (trusted proxy header)

Password sign-in allows 5 attempts per minute **per client** (SPEC-001 ACC-013). Behind the proxy
every connection comes from `127.0.0.1`, so without this setup all clients share one limit and
anyone can keep the superadmin's password sign-in locked by sending 5 wrong passwords a minute.
The proxy therefore passes the client's address in `X-Yulius-Client-Ip` together with a secret
that only it and Yulius know in `X-Yulius-Proxy-Secret`, overwriting whatever a client sent in
either. Yulius uses the address only when the secret matches (a constant-time comparison);
otherwise it keys on the TCP peer, as without the setting. An IPv6 client is limited by its /64
network. `X-Forwarded-For` alone is not trusted: a process that reaches the listener directly
could name a new address on every request.

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
   log says `trusted reverse proxy: …`, and an unusable file is logged (`the proxy secret file is
   not usable: … make it 0600`) and changes nothing. `sudo -u yulius yulius doctor --proxy-secret-file
   /var/lib/yulius/proxy.secret` checks it. The secret is never logged, returned by the API or
   shown by `doctor`.

3. **The proxy**, on the Yulius site only (sign-in is never served on preview hosts, and Yulius
   removes both headers before a preview request reaches a sandbox's app).

   Caddy reads the secret from its environment at run time, so it stays out of the Caddyfile and
   out of Caddy's config API (which shows only the placeholder):

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

   `systemctl daemon-reload && systemctl restart caddy` (a restart, not a reload: the environment
   is read when Caddy starts). `header_up` replaces a header the client sent. `{remote_host}` is
   the address that connected to Caddy; if another proxy or a CDN is in front of Caddy, configure
   Caddy's `trusted_proxies` and use `{client_ip}` instead.

   nginx (it reads its configuration as root, so the included file stays root-only;
   `proxy_set_header` replaces the client's header of the same name, and a `location` that sets
   any header inherits none from `server`, so keep these next to the others):

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

Either side can be configured first: until both are, requests key on the peer as before (Yulius
logs once that it saw an untrusted `X-Yulius-Client-Ip`). To rotate the secret, repeat step 1 and
restart Yulius and the proxy.

**Sandboxes and the secret.** Isolated sandboxes (ADR-22) cannot reach Yulius's public listener at
all: `slirp4netns` runs with `--disable-host-loopback`, so the gateway `10.0.2.2` and every host
loopback port refuse their connections (AC-NFR-8; the opt-in `tests/isolation.rs` on the VM). Through the VM's public address they
reach the proxy like any internet client and are limited as one client, the VM's address. Runs
without isolation (legacy containment, `YULIUS_CONTAINED=false`, or a `YULIUS_SANDBOX_FORWARD` of
the public port, which you should never configure) can connect to the listener directly; they do
not know the secret, so whatever headers they send count against `127.0.0.1`. Neither copy of the
secret is visible to contained runs: `/var/lib/yulius` is never bound into them, and root-only
files under `/etc` are unreadable to the service user and skipped when the sandbox base is built.
Without containment (`YULIUS_CONTAINED=false`) agents run as the service user and could read
Yulius's copy, like `master.key`. Task runs and the Superagent then also keep Claude Code's
approvals (no `--dangerously-skip-permissions`), so a person answers each prompt
([decision 0165](decisions/0165-the-superagent-bypasses-approvals-on-contained-servers.md)); the
Global Superagent has only Yulius's tools, each checked at the call, and they stay pre-allowed
([decision 0274](decisions/0274-the-global-superagent-takes-its-persons-keys.md)). Keep Caddy's admin API unreachable from runs without
isolation: it can rewrite the proxy.

### Optional: live previews (wildcard DNS and TLS)

The browser panel is the sandbox's Chrome streamed from the VM (§ "Sandbox browser" below) and
needs nothing here. Previews add one action to it: **Open live in a new window** on a tab that
shows a sandbox dev server, which opens the app in the user's own browser (with their devtools)
through a per-port **preview origin** (SPEC-002 ADR-14). Every sandbox port gets its own preview
origin: a subdomain of the Yulius host,
`<port>-<key>.<yulius host>` (for example `20001-0123456789ab.yulius.example.com`). The public
listener serves them too. The proxy in front only has to:

1. **Resolve** every subdomain of the host to the VM: a wildcard DNS record
   `*.yulius.example.com A <VM IP>`. With `sslip.io` nothing is needed:
   `20001-0123456789ab.109-199-123-240.sslip.io` already resolves to 109.199.123.240.
2. **Terminate TLS** for them and forward them, `Host` unchanged, to the same `YULIUS_BIND`, with
   WebSockets and streaming as for the main host.

Preview origins must be subdomains of the Yulius host (the default): they are then the same site
as Yulius, so their cookies work inside the panel's frames (no third-party cookie blocking), and
Yulius never relies on `SameSite` for its own protection. `YULIUS_PREVIEW_DOMAIN` (or
`--preview-domain`) sets another parent domain or `off`. A public URL that is an IP address has no
previews. For local development use a name such as `http://yulius.localhost:8080`: browsers
resolve `*.yulius.localhost` by themselves, and a bare `localhost` would not work
(`*.localhost` hosts are sites of their own).

Caddy (on-demand TLS, one certificate per preview host on its first visit; Yulius approves only
preview hosts of existing sandbox ports through the `ask` endpoint on the internal listener —
use your `YULIUS_INTERNAL_BIND` and `YULIUS_BIND` addresses):

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

The first visit to a new preview host waits a few seconds for its certificate; the panel waits
up to 30 s and then offers the sandbox Chrome instead. Certificates come from Let's Encrypt
(ZeroSSL as Caddy's fallback). `sslip.io` hosts share sslip.io's Let's Encrypt rate limits with
all its users, and every `*.sslip.io` site is the same site as yours (sslip.io is not on the
Public Suffix List): Yulius does not depend on either for security, but a domain of your own is
the better production setup. Certificate Transparency logs publish each preview host name; the
key in it is opaque.

nginx (a wildcard certificate for `*.yulius.example.com`, which needs the ACME DNS challenge,
e.g. certbot with your DNS provider's plugin):

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

Without DNS or TLS for the previews everything else keeps working; only "Open live in a new
window" fails to load.

### Sandbox browser (virtual displays)

Each sandbox has one Chrome, shared by its users and agents (SPEC-002 ADR-15). With `xvfb` and
`ffmpeg` installed it runs **headful on its own virtual display**: Yulius starts one `Xvfb` per
running sandbox browser (as the service user, numbers from `:100`), shows it to viewers as H.264
video over the browser WebSocket (nothing to open in the firewall: it is the same HTTPS
connection) and passes their mouse and keys as real input, so captchas and sign-in popups work.
Encoding runs only while someone watches.

| Setting (flag / environment) | Default | Meaning |
| --- | --- | --- |
| `--browser-display` / `YULIUS_BROWSER_DISPLAY` | `auto` | `xvfb`: always a virtual display (the browser does not start without Xvfb); `headless`: never (JPEG frames, CDP input); `auto`: `xvfb` on Linux when Xvfb and ffmpeg are found, else `headless`. |
| `--xvfb` / `YULIUS_XVFB`, `--ffmpeg` / `YULIUS_FFMPEG` | `Xvfb`, `ffmpeg` | The executables (a path or a name on `PATH`). |
| `--browser-fps` / `YULIUS_BROWSER_FPS` | 60 | Highest video frames per second (5–60); the stream uses fewer on slow links. 30 halves the encoder's CPU. |
| `--browser-max-size` / `YULIUS_BROWSER_MAX_SIZE` | `1920x1200` | Largest viewport in CSS pixels; larger panels show the page scaled down. |
| `--browser-scale` / `YULIUS_BROWSER_SCALE` | 1 | Device pixels per CSS pixel (1–3). 2 gives crisp text on HiDPI screens at about three times the CPU and bandwidth. |
| `--browser-display-base` / `YULIUS_BROWSER_DISPLAY_BASE` | 100 | First X display number; numbers in use are skipped. |

Resources (measured on a 12-vCPU VM, Chrome 155, ffmpeg 6.1; SPEC-002 ADR-15): the encoder takes
about one core per watched browser at 1280×800 and 60 fps (three quarters of one on a static page),
about 1.5 cores at 1920×1080, and nothing when nobody watches; 30 fps saves a third to a half.
Video is about 0.02 Mbit/s on a static page and at most the stream's cap while scrolling: the
viewers acknowledge frames, and the bitrate (12 down to 0.8 Mbit/s) and frame rate (60 down to 30)
follow the slowest viewer's link, so a slow link gets a softer picture rather than a growing delay
(SPEC-002 ADR-15). A bitrate change or a keyframe a viewer needs briefly runs a second ffmpeg
(about 0.2 s). Each display is a framebuffer of the maximum size (about 9 MB at 1920×1200).

The reverse proxy needs nothing for the video: Caddy's `reverse_proxy` copies the WebSocket as it
reads it (`encode` and `flush_interval` do not apply to it), and flow control keeps the proxy's
socket buffers short.

Isolation: the display listens on neither TCP nor an abstract socket (`-nolisten tcp -nolisten
local`), only on `/tmp/.X11-unix/X<n>`, which contained agents and users' shells do not see (their
`/tmp` is private), and it requires a per-display cookie stored in
`<data>/displays/<sandbox>/Xauthority` (0600), outside every directory containment binds.
Xvfb, Chrome and ffmpeg are children of the service and share its `/tmp`, so `PrivateTmp=yes` in
the unit works too.

Check with `yulius doctor`: it reports Xvfb, ffmpeg, libx264 and x11grab.

### Sandbox isolation

Each sandbox has one **namespace holder** (SPEC-002 ADR-22). Agents' task sessions, users'
shells and CI jobs join it. A sandbox gets:

- its own user namespace: root inside is the service user, ids 1–65535 come from its subordinate
  range;
- a private network: egress through `slirp4netns`; the host's loopback services and other
  sandboxes are unreachable; the internal listener and the sandbox's dev-server ports are relayed;
- a writable **system layer** over a shared, Yulius-owned copy of the host system (the *base*), so
  `sudo apt-get install …` works and persists for that sandbox only;
- limits (CPU, memory, tasks) through the unit's delegated cgroup, and a soft disk quota. Inside
  a sandbox, CI jobs get four times the agents' CPU weight (`ci/` 400 against `agents/` 100), so
  CI gets up to 80 % of the sandbox's CPU while agents build ([ADR-0039](decisions/0039-ci-jobs-get-the-cpu-first.md)).
  On the host, a sandbox's own group weighs 500 while any of its CI jobs runs (else 100), so its CI
  also out-weighs other sandboxes' agents ([ADR-0247](decisions/0247-a-sandbox-running-ci-jobs-weighs-more-on-the-host.md));
- a seccomp filter: no nested user namespaces, `io_uring`, `bpf`, keyrings, module or mount calls;
- its own **Docker engine** ([ADR-0151](decisions/0151-a-sandbox-runs-its-own-docker-engine-under-an-engine-seccomp-profile.md)),
  started on the first use of `/var/run/docker.sock` (`DOCKER_HOST`) by an agent, the
  Superagent, a Shell or a CI job, also in sessions started before, and stopped once idle. See
  "Docker in sandboxes" below.

**Install once** (as root): `sudo yulius host setup` (§2). It installs the packages, gives the
service user a subordinate range, lets bwrap create user namespaces, gives the unit its
delegation, and builds the first base (a
few minutes, about 6 GB under `<data>/system`). Then, if it added the drop-in, restart once:
`systemctl restart yulius`. Install `gh` from GitHub's repository (§1) before the setup, so
the base copies it.

**AppArmor and user namespaces** (SPEC-456,
[ADR-0296](decisions/0296-host-setup-writes-the-apparmor-profile-that-lets-bwrap-create-user-namespaces.md)).
Ubuntu since 24.04 sets `kernel.apparmor_restrict_unprivileged_userns = 1`: a program gets a
working user namespace only when an AppArmor profile allows it `userns`. A sandbox is such a
namespace, made by `/usr/bin/bwrap` as the service user, so without a rule no sandbox starts:

| Host | Without the rule | What `host setup` does |
| --- | --- | --- |
| Ubuntu 24.04 | `bwrap: setting up uid map: Permission denied` | writes and loads `/etc/apparmor.d/yulius-bwrap` |
| Ubuntu 26.04 | the package's `bwrap-userns-restrict` allows the namespace but denies every capability to what bwrap starts: `mount: … permission denied` | disables that profile (`/etc/apparmor.d/disable/bwrap-userns-restrict`, a link; the package's file is untouched and the link survives its upgrades), then writes and loads `/etc/apparmor.d/yulius-bwrap` |
| Debian 12, 13; no AppArmor | works | nothing |

The profile is `profile yulius-bwrap /usr/bin/bwrap flags=(unconfined) { userns, }`, as Ubuntu
ships for other programs that need user namespaces; AppArmor loads it at every boot. Add your
own rules in `/etc/apparmor.d/local/yulius-bwrap`, not in the file (setup rewrites it). It lets
any local user create a user namespace through bwrap, as on Debian; the restriction stays for
every other program. On Ubuntu 26.04 other users of bwrap (Flatpak) lose the shipped confinement
of what bwrap starts.

Nothing is decided from files alone: `host setup`, `yulius host check` (so the update helper's
re-check and Settings → Instance) and `yulius doctor --contained` ("sandbox namespaces") start
bwrap and mount inside. `host check` and the doctor only report, with
`sudo yulius host setup` as the fix. A profile for `/usr/bin/bwrap` that a person wrote (a
hand-written `/etc/apparmor.d/bwrap`, for example) is never changed: when bwrap works with it,
it is adopted ("not written by Yulius, left as it is"); when it does not, the item fails and
prints the two commands that disable it (for a file AppArmor's parser refuses: the parser's
words, and the file is to be corrected first, since the parser cannot unload it either). To move from such a rule to Yulius's, disable it
(`sudo ln -s /etc/apparmor.d/bwrap /etc/apparmor.d/disable/ && sudo apparmor_parser -R /etc/apparmor.d/bwrap`)
and run the setup again. Yulius knows such a profile by its text only in the plain spellings
(`profile bwrap /usr/bin/bwrap …`). One in any other spelling (an alternation, a pattern, an
`alias`, a variable) it finds by what it does: with its own profile, setup loads every other
profile of `/etc/apparmor.d` as a boot does (`apparmor_parser -r /etc/apparmor.d`, at most
300 s; edits of those files that were waiting for a boot or a `systemctl reload apparmor`
become live with it) and proves bwrap then. AppArmor applies neither of two profiles that name
one program (the kernel's log may say "conflicting profile attachments"), so where bwrap does
not start and no kernel limit explains it, setup takes Yulius's profile out of AppArmor,
whichever run wrote it, and proves once more. If bwrap starts, the person's profile is adopted
and `/etc/apparmor.d/yulius-bwrap` is removed; the line says so, and later runs and checks are
ok. If not, Yulius's profile is loaded again and its file is as before the run (none where
there was none), the profiles that load loaded stay loaded as after a boot, and the line says
how to disable the second profile. The same proof decides where a person's profile in a plain
spelling was added to a host that Yulius had set up before (the old host's hand-written rule
copied to a new one, say). While only one of the two is loaded, bwrap keeps starting, and
setup and check name both files and say what follows; once both are loaded (a boot, or the
person loading theirs) bwrap stops starting, the check fails, and the next setup adopts the
person's profile and removes Yulius's own. A profile of Yulius's that a person disabled with
a link in `/etc/apparmor.d/disable/` and did not unload is unloaded by that setup, and its
file is left where it is. A dry run and a check name the steps up to the proof
("would load …"); what the proofs then decide only the run says. During that last proof Yulius's profile is out of AppArmor: a sandbox that starts
in that moment fails as bwrap does without a rule. A person's profile that only AppArmor holds
(loaded by hand from a file outside `/etc/apparmor.d`) is adopted like any other and is gone
after a boot: the check fails then, and the next setup writes Yulius's profile again. Beside
a kernel limit Yulius's profile stays, to work once the limit is lifted: run the setup again
then. A profile for many programs (`/usr/bin/bw*`,
`/usr/bin/*`) is no such second profile: AppArmor prefers the one that names the path.
**After a release upgrade from 24.04 to 26.04**, run
`sudo yulius host setup` once: the upgrade installs the shipped profile next to Yulius's, and
sandboxes fail until it is disabled. A host whose AppArmor cannot load the profile is reported
as unsupported.

**A build by hand** (`sudo -u yulius yulius sandbox-base build --data-dir /var/lib/yulius`, for
example after installing a package on the host) is safe while sandboxes run: it keeps the new
current base, every base named by `--keep` and every base a live holder has mounted (read from
`<data>/runtime/sandboxes/*/host.json`, [ADR-0161](decisions/0161-a-base-a-live-holder-mounts-is-never-collected.md)).
Running sandboxes stay on their base until their holder restarts (after the idle time, or a
minute without runs once superseded); a later build removes the base once no holder uses it. Run
it as the service user (or root), which can see the holders. Builds take turns under a lock
(`<data>/system/.build.lock`): a hand-run build, the setup's and the server's wait for each
other instead of overlapping.

**Settings** (environment of the unit):

| Variable | Default | Meaning |
| --- | --- | --- |
| `YULIUS_ISOLATION` | `auto` | `auto`: the holder when its prerequisites hold, else the previous containment (and a failing holder never blocks work); `namespaces`: refuse runs without it; `legacy`: the previous containment. |
| `YULIUS_SANDBOX_EGRESS` | `direct` | `none`: no egress, only the relayed endpoints (the internal listener, which also serves the secrets proxy, and `YULIUS_SANDBOX_FORWARD`). |
| `YULIUS_SANDBOX_MEMORY_GB` | the RAM but an eighth (at least 4 GiB kept for the host); a [resource limit](#resource-limits) | `memory.max` per sandbox. One sandbox runs all its agents and CI jobs: size it to the sessions, not to half the host. |
| `YULIUS_SANDBOX_CPUS` | none | `cpu.max` per sandbox. Without it the CPU is shared by weight: a quota of all CPUs but two throttled the sandbox in half of its busy periods while the two CPUs it left idled (SPEC-082). The sandbox's capacity for budgets is this limit, else the host's CPUs. |
| `YULIUS_SANDBOX_PIDS` | a quarter of the host's `kernel.threads-max` or `kernel.pid_max`, whichever is smaller, at least 8192, at most three quarters of `yulius.service`'s own `pids.max` | `pids.max` per sandbox: processes and threads of all its tasks, CI jobs and containers together. With the CPU split, CI jobs (`ci/`) and the Docker engine with its containers (`engine/`) each get 3/8 of it, so agent sessions keep at least a quarter (ADR-0198). All sandboxes together stop at three quarters of the service's limit (ADR-0201): a higher explicit value is not capped, but that common limit binds it. |
| `YULIUS_SANDBOX_DISK_GB` | a quarter of the data disk, at least 100, at most half the disk; a [resource limit](#resource-limits) | Soft quota: the sandbox directory plus its installed packages, measured every 10 minutes (again when a refusal would rest on a reading older than a minute); from 90 % Yulius prunes the build caches of idle tasks, ended tasks' caches and closed or cancelled tasks' worktrees always (every 5 minutes), see [SPEC-008](../.ai/specs/SPEC-008-sandbox-disk.md); above it, new sessions are refused (live ones continue). `0` turns it off. Persistent CI job checkouts, and each one's own `YULIUS_CI_CACHE` (`ci/cache/<checkout>`, SPEC-351), are pruned the same way while no CI run executes, and one job's cache beyond a quarter of the quota at once. A project declares its own caches in `yulius.toml`: `[disk]` `caches = "target build/cache"`. Caches are found and removed only with no link from the sandbox directory down, and on Linux only ([SPEC-263](../.ai/specs/SPEC-263-disk-upkeep-refuses-links-at-worktrees-a.md)). |
| `YULIUS_SANDBOX_FORWARD` | — | Extra host loopback ports reachable inside every sandbox at the same port (for example a loopback MCP server), comma-separated. |
| `YULIUS_SANDBOX_IDLE_MINUTES` | 5 | A holder without runs is stopped after this long; the next run starts a fresh one on the newest base. A sandbox's Docker engine stops after this long without a client and without a running container. |
| `YULIUS_CLAUDE_UPDATES` | `latest` | Claude Code releases isolated sandboxes follow: `latest` or `stable` (npm's dist-tags), or `off` (the host's install only). A channel chosen in Settings, Instance wins over it. See [Claude Code updates](#claude-code-updates). |
| `YULIUS_SANDBOX_DOCKER` | `on` | `on`: every isolated sandbox gets its own Docker engine when its system has `dockerd` and the cgroup delegation is on; `off`: none (the briefing then says there is no engine, and `docker` answers why). |

**Where things live.**
- `<data>/system/base-<n>/`: the base (`current.json` names it). The server rebuilds it when the host's packages, `/usr/local` or `/opt` change; it waits until they have been unchanged for 10 minutes and checks every 30 minutes. A sandbox moves to the new base the next time its holder starts; the packages it installed stay.
- `<data>/sysroots/<sandbox>/`: a sandbox's installed packages (the overlay's upper layer, `upper/`) and its Docker engine's images, containers and volumes (`docker/`).
- `<data>/runtime/sandboxes/<sandbox>/`: the holder's records and relay sockets, the sandbox's Docker directory (`docker/docker.sock`, served by its Docker front: `docker.json`, `docker-front.json`, `docker-front.log`), the engine's own socket (`engine/`), sysfs (`sys/`) and log (`engine.log`). A holder of SPEC-187 also serves `docker.sock` itself.

`yulius sandbox-base status` tells whether the base matches the host.

**Ports.** A sandbox's dev servers are reachable on the host's loopback only on its port range
(PRJ-010, `YULIUS_PORT_BASE`…), and only while something listens on them inside. Other ports
stay private to the sandbox. Two sandboxes can both use 3000 inside.

**Packages.**
- `sudo apt-get install …` works in users' shells and agents' sessions; `sudo NAME=value command` is supported.
- A package whose scripts add a system user or group (a database server, for example) fails in a user's shell, whose `/etc/passwd` is the user's generated identity file. Install it from an agent's session (ask the agent), where it works.
- The base copies the host's apt sources: a broken third-party source on the host (on the reference VM, Caddy's Cloudsmith repository answers 402) makes `apt-get update` exit 100 in sandboxes too, while installs from the other sources still work.

**Without the prerequisites** (`yulius doctor` lists what is missing), contained runs keep the
previous containment: read-only host system, the host's network, no limits.

### Resource limits

Six limits decide how much work the server takes at once (SPEC-459, ADR-0294). Without a pin each
is sized from the host at every start (integers, rounded down; `cpus` logical CPUs, memory in
GiB, the data directory's filesystem in GB), so a bigger host takes more by itself:

| Limit | Variable | Host-sized | 12 CPUs, 47 GiB, 387 GB | 64 CPUs, 755 GiB, 3.5 TB | A change applies |
| --- | --- | --- | --- | --- | --- |
| Live sessions | `YULIUS_MAX_SESSIONS` | `cpus / 2 + 2`, at most one per 2 GiB of memory, at least 2 | 8 | 34 | at once (a raise starts queued tasks) |
| CI jobs at once | `YULIUS_CI_MAX_JOBS` | `cpus / 6 + 2`, at most 64 | 4 | 12 | at once (running jobs finish) |
| CI runs at once per sandbox | `YULIUS_CI_LANES` | `cpus / 32 + 2`, at most 8 | 2 | 4 | at once |
| Sandbox memory (GiB) | `YULIUS_SANDBOX_MEMORY_GB` | the memory minus an eighth (at least 4 GiB kept back), at least 2 | 41 | 660 | at the next restart |
| Sandbox disk quota (GB) | `YULIUS_SANDBOX_DISK_GB` | a quarter of the data disk, at least 100, at most half the disk | 100 | 875 | at once |
| CD run disk (GB) | `YULIUS_CD_RUN_DISK_GB` | a twentieth of the data disk, at least 20, at most half the disk | 20 | 175 | from the next run |

A fact the host does not report leaves that limit at 8, 4, 2, 2 GiB (the memory), 100 GB or
20 GB.

**Where a value comes from.** Settings → Instance → Resource limits shows each value with its
source and the formula. In this order: a number saved there (`settings`); "host-sized" saved
there, which overrules the unit (`default`); the unit's `YULIUS_*` variable (`unit`); else
host-sized (`default`). A superadmin saves a number, goes back to host-sized or removes the
setting there, without editing the unit; each change is audited (`limits.set`,
`limits.removed`). `0` for the sandbox disk quota means none.
If the server could not read the saved limits at start (it warns: "the saved resource limits
could not be read", with the reason), it runs on the unit's and the host's values, writes nothing
over them, leaves an import's mark and refuses changes (409) until a restart reads them. When a
restart warns again the record itself is damaged: stop the service, clear it with
`sudo -u yulius sqlite3 /var/lib/yulius/yulius.db "UPDATE instance SET limits = NULL"` (the
`sqlite3` package) and start. The settings
saved in Settings → Instance are then gone and the unit's numbers are recorded for this host, so
after a move check the card's values by hand.

**A host of another size.** Every pinned number is recorded with the host it was configured on
(a unit value at the first start that sees it, a setting when it is saved). When a fact its
formula uses differs from this host's by a quarter or more (the larger of the two readings is at
least a quarter above the smaller, so 48 GiB against 38 GiB counts and 47 against 45 does not)
and the number is not what this host would get, the limit no longer fits: the server warns at
start ("… is pinned to …, sized for another host …; this host would get …", with the number, the
host it was configured on and the host-sized value) and the card lists it,
with "Use host-sized values" (host-sized for all of them, overruling the unit) and "Keep these
numbers" (they are recorded for this host). After `yulius instance import` the first start takes
the host-sized values by itself (audited as `limits.adapted` by `migration`).

### Claude Code updates

Claude Code's own updater is off in every session (it cannot write the system install from a
contained run). Yulius updates it instead ([ADR-0266](decisions/0266-yulius-keeps-claude-code-current-in-its-data-directory.md),
SPEC-421; [ADR-0293](decisions/0293-claude-code-follows-latest-and-must-know-the-default-model.md),
SPEC-461), with no root step and no sign-in again:

- **Channel.** About every hour (the first check 2 minutes after a start, and at once when the
  channel changes or a superadmin presses "Check now") the server reads npm's dist-tags of
  `@anthropic-ai/claude-code` from `registry.npmjs.org` and follows `latest` by default. A
  superadmin chooses Latest, Stable or Off in **Settings, Instance, Claude Code**; the choice is
  stored in the database, wins over `YULIUS_CLAUDE_UPDATES` and needs no restart ("Use the
  server's" goes back to the unit's value).
- **Checked before it is used.** A newer version than the one runs get is downloaded as its
  platform package (`@anthropic-ai/claude-code-linux-x64`, `-linux-arm64`, or their `-musl`
  builds), checked against the registry's sha512, and its binary must be an ELF for this platform
  that embeds that version and names Yulius's default model (`claude-fable-5-2` today). It is
  read, never run on the host. A release that does not know the default model is refused and
  reported; one that lacks another model the composer offers is installed and the gap reported.
  On `stable`, `latest`'s release is taken when neither the version in use nor stable's release
  knows the default model.
- It lands in `<data>/claude-code/versions/<version>/bin/claude.exe`; `<data>/claude-code/current`
  points at it. The version before it is kept, older ones are removed once no process runs them
  (about 300 MB each). Only a host install laid out as `bin/claude.exe` is updated over.
  Whether a process runs a version is read from `/proc/<pid>/exe`: keep `/proc` readable to the
  service user (no `hidepid=` mount option, no `ProtectProc=` in `yulius.service`), or a version a
  running session uses could be removed under it.
- **New runs.** Every run that joins an isolated sandbox (tasks, Superagent, shells, CI jobs, dev
  CD runs) binds the newer of that version and the host's npm install at the host package's path,
  when it starts (with Off: the host's install). Subscriptions stay signed in.
- **Running tasks.** A task's session keeps the version it started with until a turn of it ends:
  then Yulius restarts it on the version new runs get, in the same Claude session and on the
  same subscription, and says so in one line in its terminal. It waits while the session has
  background shells, agents or monitors (they would be lost; a `yulius wait` counts), while a
  person typed into it in the last minute, and while a draft may sit in its prompt box: a person
  typed into it since its last prompt was sent, however long ago (Yulius sees keystrokes and
  sent prompts, not the box, so anything typed and not sent counts, and so does a prompt Yulius
  holds for its person's choice, which Claude Code keeps in the box). A message Yulius types
  into the session (one sent through a Superagent, a pull request's news) counts in the same way
  until Claude Code submits it, which it does at once. Not covered: a prompt entered while the
  agent works, a person's or such a message, waits in Claude Code's queue until the turn ends,
  and a restart at that turn's end loses it (issue #1204; a person's is safe while they typed in
  the last minute). It tries once per version, one restart of a task at a time. A goal session
  moves at a turn it works on after, while its goal is set and not yet achieved; an idle one,
  and a goal task that is Done, keep their version until their next start. A prompt-mode session
  in which a person set a goal of their own (`/goal …` in its terminal) keeps its version while
  that goal is open: Yulius types nothing into such a session and learns of a goal only from
  what the new process writes, so after a restart it would not know the goal any more (Claude
  Code restores an open goal by itself when a session is resumed, at least since 2.1.292; Yulius
  does not rely on it). The session moves at a later turn's end, once the goal is met, failed or
  cleared, or at its next start: stop the task and continue it to move it at once. An idle
  session that is restarted reads "Continuing" for a moment and then as it did: its person is
  not notified a second time. The card lists the tasks still on an older version, and each
  task's info shows the version each session ran.
  Yulius does not know a schedule a session set for itself (Claude Code's `/loop`): such a
  session is restarted like any other, and the schedule ends with its process.
- **A version a session cannot continue on gets no other running session.** That is: the
  restart could not be started, the restarted session ended by itself before it started, or it
  did not start within two minutes (a dialog a release opens on; that session is left as it is,
  for its person). The card names the task and what happened; from then on the card's list and
  each task's info promise that version only at the task's next start. Sessions that start or
  continue still get that version, so choose Off (the host's install) when it does not work at
  all. A
  newer version, or a restart of the server, lifts it. A restart that cannot be made right now
  (the subscription is at its limit, the sandbox cannot be joined) says nothing, is no entry in
  the task's switches, and is tried again at a turn's end ten minutes later or more.
- **When the version in use does not know the default model**, the card shows a red banner with
  the fix and `yulius doctor --contained` prints "Claude Code in sandboxes" as `FAIL`. The fix is
  the next check (or "Check now"), which installs a release that knows it; or, at once, a newer
  install on the host: `sudo npm install -g @anthropic-ai/claude-code@latest` (runs get the newer
  of the host's and Yulius's). If npm's newest release does not know it either (it was refused,
  or it is what runs get), no release knows the model yet: choose a model Claude Code knows for
  tasks until one does. The card and the doctor say that only from the server's own read of the
  registry in the last two hours: not with Off, and not after a check that failed.
- **When the binary in use cannot be read for its models** (a file the service user may not
  read, a read error), the card shows a "models not read" badge and says that whether it knows
  the default model is not known; `yulius doctor --contained` warns the same and names the
  install. Nothing is claimed either way until it can be read again (at the next view of the
  page).
- The host's install is never changed; updating it by hand past Yulius's version makes runs use it.
  Without an npm install on the host, sandboxes have no Claude Code and nothing is updated.
- The server writes what it last knew (channel, npm's `latest` and `stable`, what the last check
  did, the version in use and the offered models it lacks) to `<data>/claude-code/status.json`:
  `yulius doctor --contained` reads the channel in effect from it and warns about a refused
  release. A failed check is logged ("Claude Code was not updated"), shown on the card and tried
  again an hour later; the server's egress needs `registry.npmjs.org` on port 443.

### Docker in sandboxes

Each isolated sandbox has its own Docker engine (SPEC-187,
[ADR-0151](decisions/0151-a-sandbox-runs-its-own-docker-engine-under-an-engine-seccomp-profile.md);
SPEC-196, [ADR-0158](decisions/0158-docker-reaches-every-run-through-a-front-and-a-directory-moved-into-live-runs.md)).
`docker`, `docker compose`, buildx and testcontainers work unchanged in agents' sessions, the
Superagent, Shells and CI jobs (not in a production CD run). The host's Docker is never reachable
from a sandbox, and no daemon needs to run on the host.

- **Every session, whatever its age.** A Docker front beside each live holder serves the socket;
  runs see it at `/var/run/docker.sock` and `DOCKER_HOST=unix:///run/yulius/docker/docker.sock`.
  After an update the server adopts the holders and gives each a front. Within one upkeep tick
  (30 s) it moves the Docker directory into sessions that started without it, so nothing needs a
  restart or a new session.
- **Settings → Instance** (superadmin) has a **Sandbox Docker** card. It shows whether sandboxes
  get Docker, why not and the fix (the doctor's words); the base being rebuilt and when it will
  be ready; the sandboxes on their own Docker; and the last update's re-check. **Run smoke check**
  runs the smoke test below from the server, as its child in its delegated cgroup. One runs at a
  time, bounded to 40 minutes, and each check appears as it ends.
- **Verify on the host** (as root, after a deploy):

  ```sh
  sudo yulius doctor --docker-smoke
  ```

  Run as root, it always runs as the data directory's owner in a delegated transient unit
  (`systemd-run`), never in the root shell's own cgroup (SPEC-202, #617), and refuses a data
  directory root owns. It uses the current base, and removes its throwaway sandbox
  (`docker-smoke-<pid>`) at the end. It prints one plain line per check and exits 0 only when
  sandboxes get Docker: a run older than the engine gets the socket, its refusal names the reason,
  then `docker run --rm hello-world`, a `docker build` and a two-service `docker compose up` in that
  run, a recreated socket, and `DOCKER_HOST` in a new run. It needs Docker Hub (`hello-world`,
  `busybox`).

- **What it is.** On the first connection to the sandbox's socket, the sandbox's Docker front
  starts `dockerd` as root of the sandbox's user namespace (not the host's): in the sandbox's
  network, with a run's view of the files plus its storage, a cgroup namespace rooted at
  `sandboxes/<sandbox>/engine`, and a seccomp profile of its own that every container inherits,
  `--privileged` included. Containers count against the sandbox's memory, task and CPU limits.
  Their traffic leaves through the sandbox's egress, and they reach the egress proxy at
  `host.docker.internal:<internal port>` (with `--add-host=host.docker.internal:host-gateway`). A
  port published in the sandbox's port range is relayed like a dev server's.
- **Prerequisites.** `sudo yulius host setup` (§2) brings all of them and proves them with the
  smoke test. Every update re-checks them (§8), and a reboot keeps them (the modules file, the
  disabled units, the drop-ins and the base are on disk).
  - `dockerd` in the sandbox's system
    ([ADR-0160](decisions/0160-a-sandbox-s-engine-comes-from-its-own-system-not-the-host-s.md)).
    Docker on the host (`docker-ce` with `docker-buildx-plugin` and `docker-compose-plugin`)
    gives it to every new base, which copies `dockerd`, `containerd`, `runc`, `docker-proxy` and
    the CLI plugins. The host's own daemon is not needed, and the setup stops it. While the host
    has `dockerd` and the current base has none, the server checks the host at every tick, so
    the rebuild starts 10 minutes after the host last changed (Settings → Instance says when it
    will be ready); `sudo yulius host setup` builds it at once.
    - Without Docker on the host, an agent installs it in its own sandbox
      (`sudo apt-get install -y docker.io docker-buildx docker-compose-v2`), and the front starts
      an engine at the next tick (30 s). Ubuntu's `docker.io` alone has no buildx or compose
      (SPEC-201); the same command adds a missing plugin to a sandbox whose engine is Ubuntu's.
    - On a base copied from a `docker-ce` host, apt refuses that command (`containerd.io`
      conflicts with `containerd`) and removes nothing. A missing plugin there is
      `sudo apt-get install -y docker-buildx-plugin docker-compose-plugin`.
    - A sandbox keeps its base's `dockerd` until its holder next starts on a newer base (after
      the idle time). One installed in the sandbox stays.
  - **A sandbox on its own `docker.io`.** Docker an agent installed in a sandbox is kept: Yulius
    removes nothing a sandbox installed. Ubuntu's `docker.io` puts `dockerd` in `/usr/sbin`, which
    comes before the base's `/usr/bin/dockerd`, so that sandbox runs its own, older engine. The
    product sandbox ran 29.1.3 while the base had 29.9.1. `yulius doctor` ("sandbox own Docker")
    and Settings → Instance list such sandboxes. To use Docker's packages, as the base does, run
    in that sandbox `sudo apt-get install -y docker-ce docker-ce-cli containerd.io
    docker-buildx-plugin docker-compose-plugin`: apt replaces `docker.io`, and the engine switches
    when it next starts, after it was idle. Never `sudo apt-get remove docker.io`: the removal's
    whiteouts hide the base's files at the same paths, and that sandbox loses Docker.
  - The cgroup delegation (§3). When `yulius.service` stops, systemd removes every empty group
    in the unit, including the `engine` leaf of a sandbox whose engine stopped as idle. Holders
    and fronts keep running (`KillMode=process`). Before each engine start, the front makes the
    leaf again under the sandbox's group, and so does the server's upkeep tick (30 s) for an engine
    or relay front it keeps (SPEC-201, #614). Before that fix, `docker` answered "cannot join
    …/sandboxes/<sandbox>/engine: No such file or directory" after every update, until a new run
    prepared the sandbox.
  - These kernel modules loaded, since a sandbox cannot load modules: `overlay bridge veth
    nf_tables nft_compat nft_chain_nat nf_nat nf_conntrack xt_MASQUERADE xt_addrtype xt_conntrack`,
    listed in `/etc/modules-load.d/yulius-docker.conf` so that they load at every boot.
  - The `yulius` executable owned by root and writable by no group or other, as §2 installs it.
    The engine's read-only binds are not locked, so an executable the service user owns or may
    write could be remounted and rewritten. With one, sandboxes get no engine.

  `yulius doctor` checks all of them ("sandbox Docker", the same words as Settings → Instance).
  Without `--contained` its "sandbox Docker" line names that flag (`yulius.service` sets
  `YULIUS_CONTAINED=true`); run it as the service user: `sudo -u yulius yulius doctor
  --contained`.

- **Disk.** Images, containers and volumes live in `<data>/sysroots/<sandbox>/docker` and count
  toward the sandbox's soft quota as "containers". This is a lower bound: directories that a
  container's own users keep private (mode 0700, a database's data, for example) are not entered.
  From 90 % of the quota, while the engine runs, Yulius prunes its build cache and the images no
  container uses, before the caches of idle tasks. A stopped engine is never started for that,
  so it keeps its images until the next use. Containers and volumes are never pruned.
- **Lifetime.** The engine stops after `YULIUS_SANDBOX_IDLE_MINUTES` without a client and without
  a running container. While it runs, the holder stays. It survives Yulius restarts and updates
  with the holder. Containers stop when the holder stops; those with a restart policy come back
  when the engine starts again.
- **Limits.** Image files owned by ids above 65535 cannot be stored. Setuid binaries and file
  capabilities do not raise privileges inside containers (`no_new_privs`).
  - Per-container resource flags (`docker run -m`, `--cpus`, `--pids-limit`, compose `mem_limit`,
    `cpus` or `pids_limit`) are not supported; leave them out. The engine lives in its own group,
    so cgroup v2 passes no controller below it: memory flags are discarded, and CPU and task flags
    may make the container fail to start. The sandbox's own limits, and the engine's 3/8 of its
    task limit (SPEC-268), still bound every container.
  - A bind mount resolves in the engine's view: the sandbox directory, the system layer and the
    engine's `/tmp`. A run's own `/tmp`, a Shell user's home and run directories are not there.
- **When it fails.** `docker` prints "the sandbox's Docker engine is not available: …" with the
  reason. Without an engine it names the missing prerequisite and the fix (`YULIUS_SANDBOX_DOCKER=off`,
  no `dockerd` in the sandbox, no cgroup delegation, a writable executable), the same words as `yulius doctor`.
  For an engine that did not start, it gives the last lines of
  `<data>/runtime/sandboxes/<sandbox>/engine.log`. The front's own log is `docker-front.log`
  beside it.

### Task intake (Jev)

Optional (SPEC-002 ADR-16). With a TypeSafe Jev key, Yulius offers to split new tasks that mix
separate workstreams (the proposal comes from one headless `claude -p` run on a connected
account), to run broad work started another way as a supertask (SPEC-188; Jev's `breadth`
question, fixed 85 % threshold, no setting), it holds prompts typed into a running task that add, drop or replace its scope until
someone decides in the task view, and it chooses a new task's mode and workflow when the composer
leaves them on Auto (the default). Without a key the split, breadth (supertask proposal) and scope checks are off, and Auto
always takes its defaults: Goal, with the workflow named "Feature" (none when there is no such
workflow, and none for a prompt-mode task). The Auto thresholds are fixed (75 % for the mode,
60 % for the workflow) and have no setting.

**The key is managed in Yulius** (SPEC-001 ADM-001, ADM-002): setup asks for it as an optional
last step, and the superadmin sets, replaces, tests (one small request) and removes it under
Settings → Instance. It is sealed with `master.key` like every other stored secret, takes effect
at once (no restart), and is never logged or returned by the API. A key file is still honoured
while no key is saved there; the page shows it as "from config" and **Save it here** moves it into
settings, after which the variable can be removed from the unit. A key saved in settings always
wins over the file.

| Setting (flag / environment) | Default | Meaning |
| --- | --- | --- |
| `--jev-api-key-file` / `YULIUS_JEV_API_KEY_FILE` | unset | File holding a Jev API key, owned by the service user, mode `0600` (a group- or world-readable file is refused and the page says why). Used while no key is saved in settings; read at start. The key is never logged. |
| `--jev-split-threshold` / `YULIUS_JEV_SPLIT_THRESHOLD` | 0.9 | Probability of "several workstreams" Jev must give before a split is offered (0.5–1). |
| `--jev-scope-threshold` / `YULIUS_JEV_SCOPE_THRESHOLD` | 0.8 | Probability Jev must give before a typed prompt is held (0.5–1). |

- The start log says `task intake is on (Jev jev-1.13.0, key saved in settings)` (or `… key from
  YULIUS_JEV_API_KEY_FILE`) or `task intake is off: no Jev key`.
- Requests go to `https://api.typesafe.ai/v1/systemone` (outbound HTTPS). Each request carries one
  question: for a split or breadth check (both asked at once) or an Auto choice, the new task's prompt (the workflow question also
  carries the sandbox's workflow names, descriptions, when to use them and step titles); for a
  typed prompt, the task's title, prompt and step titles plus the typed text.
- Every Auto decision is in the audit log (`task.auto_choice`, for example `mode goal (jev 91%),
  workflow Feature (default)`) and in the service log (`auto choice`); a Jev failure is logged as
  `auto choice without Jev's answer; the default applies`.
- Split proposals run the `claude` on the service's `PATH` (the same CLI as sessions), outside
  containment, with no tools, in a temporary directory under `<data>/runtime/intake/`. They use
  the account's usage like a short task.
- Claude Code needs no extra settings. Yulius already registers its `UserPromptSubmit` hook in
  every run's `--settings` file (`yulius hook claude UserPromptSubmit`; the scope check answers
  within 3 s and lets the prompt through on any failure). It checks what a person types into a
  task's terminal (the last keystroke within a minute), also into a task paused for capacity.
- Each check of a typed prompt logs one line without the prompt: `scope check of a typed prompt
  task=… verdict=context|new_scope|drops_scope|redirect percent=…`; a held prompt is also audited
  as `prompt.held`. `journalctl -u yulius | grep 'scope check'` shows whether intake runs.

### Security scans (Vidoc)

Optional ([SPEC-195](../.ai/specs/SPEC-195-vidoc-is-built-into-yulius-an-optional-i.md),
ADR-0157). With a Vidoc key, every project's agents and granted CI jobs can run
`yulius security-scan`. Vidoc's `vidoc-1` model reviews the files a branch changed (or, with
`--all`, the whole repository). The scan fails on open critical or high findings, and on open
medium findings in the lines the branch changed. Findings in code the branch did not touch become
GitHub issues, which Yulius files once, reopens while they are reported and closes once their code
is gone (ADR-0172). When the sandbox can use a Vidoc key (its own `VIDOC_API_KEY` agent secret or
the instance's), Yulius CI adds a built-in `security` job to every project whose CI does not scan
already, unless it is a playground or Vidoc is switched off for the project or the instance (below).
**The scanned code is sent to Vidoc** (`https://model.vidocsecurity.com`). Without a key
the scan says "not run", and nothing else changes. Keys come from <https://model.vidocsecurity.com/#key>.

The key is managed like the Jev key:

- **Setup** offers it as an optional step after the Jev step.
- **Settings → Instance** lets the superadmin set, replace, test and remove it. The test is one
  `GET /v1/models` request, which must list `vidoc-1`.
- **At rest** it is sealed with `master.key`, takes effect at once, is audited as `vidoc.key_set` and
  `vidoc.key_removed`, and is never logged or returned.
- **A key file** (`YULIUS_VIDOC_API_KEY_FILE`, owner-only, mode `0600`) is honoured while no key is
  saved, and **Save it here** moves it into settings.

Agents and CI jobs never get the key. They get `VIDOC_API_KEY=yulius-secret-VIDOC_API_KEY`, and the
egress proxy replaces it toward `model.vidocsecurity.com` only. CI jobs get it only when their
`yulius.toml` entry names it (`secrets = "VIDOC_API_KEY"`). An agent secret named `VIDOC_API_KEY`
that reaches the run wins over the instance key
([architecture](architecture.md#agent-secret-scopes)). Each use is in the secrets audit
(`instance:vidoc`). The built-in `security` job (`yulius security-scan --ci`) is described above;
onboarding adds no job, only the AGENTS.md rule. The scan needs
`python3` 3.11 or later in the sandboxes. Agents and CI jobs start with an environment of their own,
so `YULIUS_PYTHON`, which names another interpreter, is set in the job's command or in the session,
not on the service.

| Setting (flag / environment) | Default | Meaning |
| --- | --- | --- |
| `--vidoc-api-key-file` / `YULIUS_VIDOC_API_KEY_FILE` | unset | File holding the instance's Vidoc API key, owned by the service user, mode `0600`. Used while no key is saved in settings; read at start. |

**Turning Vidoc off for one project.** While the key fails (refused, HTTP 401 or 403, or out of quota,
HTTP 402 or a 429 that says so), the sandbox's admins (GitHub `admin` on its repository) switch its scans off on the
sandbox's CI/CD page: the CI tab's **Security scan (Vidoc)** card, "Scan this project with Vidoc".
No instance administrator is needed; a superadmin can do the same on the Instance page's Vidoc
card, which shows the same switch (and switch Vidoc off for every project there). Off, the project
sends nothing to Vidoc, its scans report "disabled", Yulius adds no built-in `security` job (a
`security` job the repository configures itself ends Skipped), a session started before the switch
went off has its requests refused by the egress proxy (its scan ends "not run"), and its
agents ask nobody about the key or its quota; on again, the next run and CI job scan. Each change is
audited as `delivery.vidoc.update` (who, when, on or off), and the card says who changed it last.
Members with write or maintain access see the card but cannot change it. A repository whose
integration branch commits `enabled = false` in `.config/vidoc.toml` stays off whatever the switch
says; the card says so and offers no way on (a reviewed pull request turns it on). A scan whose key
fails names the card's address (`YULIUS_VIDOC_SWITCH`, which Yulius sets from `--public-url`), and
so does CI's job log (SPEC-359, ADR-0245).

**Turning Vidoc off for every project.** A superadmin switches "Scan every project with Vidoc"
off on Settings → Instance's Vidoc card (`PUT /api/admin/vidoc/enabled`). It takes effect at once,
survives restarts, is audited as `vidoc.disabled` / `vidoc.enabled`, and keeps the key (also one
from `YULIUS_VIDOC_API_KEY_FILE`). Every project then scans nothing, as above, and keeps its own
choice for when the switch goes back on (SPEC-356, ADR-0242).

The start log says `Vidoc is on (instance key saved in settings)` (or `… from
YULIUS_VIDOC_API_KEY_FILE`) or `Vidoc is off: no instance key`; while a superadmin has switched
Vidoc off, it also says `Vidoc scans are switched off for the instance`.

## 5. First run

1. Start the service. On the first start it writes a one-time setup token to
   `/var/lib/yulius/setup.token` (0600) and logs where it is.
2. Open the public URL, enter the token, choose the superadmin password (twice; at least 8
   characters, 12 or more recommended) and confirm both warnings: **the password can be recovered
   only with the recovery code** (SPEC-001 ACC-002, ACC-011). Store the recovery code shown next.
   Steps 3 and 4 of 5 ask for the Jev API key and the Vidoc API key (both optional; skip them to
   add them later in Settings → Instance); the last one adds the first Claude subscription
   (skippable, see step 5).
3. User menu → Administration → GitHub connection: create the GitHub App with the manifest flow for the owner account or
   organization, then install it on the repositories Yulius may use. The App's callback URLs point
   at `YULIUS_PUBLIC_URL`; when that URL changes later, see
   [the App after the public URL changes](#the-app-after-the-public-url-changes). The manifest
   requests the permissions below.
4. User menu → Administration → Users: admit GitHub logins; they sign in with GitHub. Repository access in
   Yulius mirrors their GitHub permissions (read / write / admin). The superadmin owns the
   instance and sees every sandbox with admin access without GitHub (SPEC-001 ACC-012); pull
   requests it opens are authored by the GitHub App, and merging or releasing, which act as a
   person on GitHub, are done on GitHub or after signing in with GitHub as an admitted user.
5. User menu → Administration → Subscriptions: add Claude subscriptions; each login runs Claude's own sign-in in a
   terminal in the browser. They form one pool with nothing to configure: Yulius places them on the capacity that expires soonest, moves them at
   98% at a turn boundary, pauses them (without telling the agent) only while no subscription has
   capacity left and resumes them by itself. The capacity panel there and on the Dashboard says
   when to add subscriptions (SPEC-002 ADR-17). Claude's sign-in here (`claude setup-token`) may
   not read Claude's usage endpoint (it lacks the `user:profile` scope), so a subscription's usage
   shows once a task runs on it: the running sessions report it through Claude Code's status line.
   A subscription Claude refuses (a cancelled or downgraded plan) is marked "Sign in again" with
   Claude's words and its tasks move on; Remove works while tasks use it and moves them; a person
   runs a task on the subscription they choose from the task's subscription button (SPEC-447).
   A subscription out of a model's usage credits (the default model, Fable, runs on a weekly
   allowance of them beside the 5-hour and weekly windows) is at that model's limit: its card
   reads "Limit reached" with "Fable this week" at 100 % until the reset Claude named, its tasks
   of that model move on or wait for the reset by themselves, and tasks of other models still
   run on it. A usage reading does not end that limit, so one lifted early (credits bought)
   holds until its reset or until the subscription is signed in again. When Claude names no
   reset (an older Claude Code, or credits switched off), the card shows no reset time, the
   provider's next reading of the subscription ends the limit (a session's own reading does
   not), and a task with nowhere else to go is tried once more. Refused again, it waits for
   another subscription's reset when one is known, off the subscription that refused it;
   otherwise it is under Needs you (audit `task.limit_without_end`) until a person runs it
   elsewhere or sends it a message, until its agent works again because the limit is over, or
   until Yulius takes it on after all (a restart of Yulius retries it once; audit
   `task.needs_you.resolved` says which; SPEC-464).
   Do this before importing: until a Claude account is usable, Yulius refuses every agent start a
   person asks for, Start onboarding included (SPEC-246, ADR-0190).
6. Import or create projects (sandboxes). Product and Studio are detected and get the `product` and
   `studio` delivery presets.

### GitHub App permissions

Yulius uses one GitHub App (SPEC-002 ADR-08). The App's tokens can do only what its permissions
allow: the server's installation tokens, and people's sign-in (user-to-server) tokens, which are
also capped by the person's own repository access. What Yulius does on GitHub for a task uses its
owner's sign-in token, so GitHub shows that person (GIT-003): Yulius's push and pull request for
the task, its merges, promotions, issues and labels. Without a usable one (no linked account, as
for the superadmin, or a sign-in that can no longer be refreshed) the App acts, and the task's
terminal says so once. Yulius CI's statuses, branch cleanup and background reads stay with the App.

The agent's own Git and `gh` never hold a person's token (ADR-0114): they get an installation token
of the App restricted to the sandbox's repository and to Contents and Pull requests and Issues
(write), Checks, Commit statuses and Metadata (read). Commits stay authored by the task's owner;
an agent's own pushes and comments show the App. Without Administration that token cannot touch
rulesets, settings or deploy keys; without Workflows it cannot add a GitHub Actions workflow.

**Protecting the production branch.** Only a person may move it (ADR-0110). Add a branch ruleset
on each repository (Settings → Rules → Rulesets → New branch ruleset): target the production
branch (`main`), enforcement Active, rules **Restrict updates**, **Restrict deletions** and **Block
force pushes**, and in the bypass list only the **Repository admin** role, never the Yulius App.
GitHub then refuses the agents' token every push, merge, fast-forward or deletion of that branch,
`--no-verify` and `gh` included, while an admin's promotion goes through. Projects Yulius creates
get it; for an imported repository onboarding (`yulius onboard`, the preparation task) says when it
is missing, and the preparation agent adds it with the `production_protect` tool: Yulius creates
`yulius-people-only-<branch>` with those three rules as the task's owner, only when GitHub says that
person is a repository admin (ADR-0129). Otherwise (not an admin, no GitHub sign-in, a free plan's
private repository) the agent asks a person for it with `needs_you`. A promotion needs a repository admin
once the ruleset exists. A private repository without GitHub Pro, Team or Enterprise cannot have
rulesets: a sandbox admin then accepts the risk on the sandbox's Git page, card **Production
protection**, by switching off **Require a GitHub ruleset on `<branch>`**
(`<public_url>/s/<slug>/git?show=production-protection`, ADR-0267); onboarding then stops warning and
asking for it, and every request for the ruleset names that address.

The manifest flow requests these repository permissions:

| Permission | Access | What Yulius does with it |
| --- | --- | --- |
| Contents | Read and write | Clone and fetch repositories; push task branches (as the task's creator); merge pull requests; fast-forward promotions; compare branches. |
| Pull requests | Read and write | Open, list and read pull requests. |
| Issues | Read and write | Agents log what lies outside their change (pre-existing bugs, flaky tests they did not cause, nits left after review; [ADR-0141](decisions/0141-review-findings-in-the-change-s-own-code-are-fixed-before-merge.md)) as issues with Definition-of-Ready labels (`issue_create`, DLV-008); missing labels are created. Requested since 2026-10-10; an App created before needs it added (below). |
| Checks | Read | Read check runs on pull request heads: GitHub Actions and every other Checks API app. |
| Commit statuses | Read and write | Publish the result of Yulius's own CI as the status `yulius/ci` on each tested commit (write, since 2026-10-11), and read statuses of projects without Yulius CI. |
| Workflows | Read and write | Push the onboarding commit that deletes `.github/workflows/` (Yulius is the only CI). Granted on 2026-10-12. |
| Administration | Read and write | Create repositories (New sandbox, in an organization or a personal account) and add branch rulesets to them. Repository creation (Read and write) also lets GitHub create them, without the rest of Administration. |
| Metadata | Read | Mandatory for every App: repository facts and the signed-in person's permission level, which Yulius mirrors. |
| Members (organization) | Read | Requested by the manifest; no current Yulius call uses it. |

Two more are worth knowing:

- **Actions: read** is optional. It is a fallback only: when check runs can't be read, Yulius reads
  the head's GitHub Actions workflow runs in their place. With Checks granted it is never needed.
- **Workflows** lets the App's tokens change files under `.github/workflows/`. Without it, GitHub
  refuses such pushes, and onboarding cannot delete a repository's GitHub Actions. With it, a
  task could also *add* a workflow, which would run with the repository's secrets. So once a
  repository is off Actions, also turn GitHub Actions off for it (Settings → Actions → General →
  Disable actions; free on every plan): no workflow runs any more, whoever pushes one.

**When a permission is missing.** A read GitHub refuses with 403 "Resource not accessible by
integration" means the App lacks the permission. For checks, Yulius keeps working with the
sources it can read. It names the missing permission on the Git page, as one line for instance
administrators, with a link to the App's permission page. Checks it cannot read never count as
passed, so merges and releases that require checks wait.

**Creating a project fails.** Yulius says which of these it is
([SPEC-241](../.ai/specs/SPEC-241-onboarding-works-on-an-empty-github-repo.md)):

- "GitHub refused to create …: the Yulius GitHub App lacks the repository permission
  Administration: Read and write …": add it as below. Nothing was created.
- "… was created on GitHub, but the Yulius GitHub App cannot reach it: its installation covers
  only selected repositories": open the installation page the message links (Settings → GitHub
  Apps, or Applications for a personal account, → the App → Configure), add the repository under
  Repository access (or choose All repositories), then add it in Yulius with Import repository.

**An empty repository** (created on GitHub without a README) can be imported as it is. When its
onboarding starts, Yulius pushes one commit adding `README.md` to the default branch GitHub reports,
then onboards it as any other repository.

**Adding a permission to an existing App** (for example Commit statuses on an App created before
2026-10-10):

1. Open the App's permission page. For an organization:
   `https://github.com/organizations/<org>/settings/apps/<app-slug>/permissions`. For a personal
   account: `https://github.com/settings/apps/<app-slug>/permissions`. The Git page links there.
2. Set the permission (for example Repository permissions → Commit statuses → Read-only) and
   save.
3. Accept the change on the installation. GitHub asks the installation's owner to review the new
   permissions under the organization's (or account's) Settings → GitHub Apps → the App →
   Configure. Until someone accepts, the old permissions apply.
4. New tokens carry the new permission. Installation tokens are cached for up to an hour; restart
   Yulius (`systemctl restart yulius`) to use the new permission at once.

### The App after the public URL changes

GitHub signs people in only from a callback URL the App holds: `<public URL>/api/auth/github/callback`.
After the instance's public URL changes (a move to another host, a new domain) GitHub refuses every
sign-in from the new address ("redirect_uri is not associated with this application") until a
person adds that URL in the App's settings. GitHub has no API for an App's callback URLs or
homepage, and Yulius never uses a person's GitHub session, so this one step is yours
([SPEC-466](../.ai/specs/SPEC-466-run-migration-yulius-checks-its-github-a.md),
[ADR-0302](decisions/0302-the-instance-asks-github-about-its-own-callback-and-clients-follow-only-a-moves-redirect.md)).
Yulius notices and says exactly what to do:

- **Settings → Instance → Address** (superadmin). The instance asks GitHub whether it accepts its
  callback: at every start, when you press **Check again**, and by itself while the page shows an
  answer that is not "registered". When the callback is missing the card has one link, **Open the
  App's settings on GitHub**
  (`https://github.com/organizations/<owner>/settings/apps/<slug>`, or
  `https://github.com/settings/apps/<slug>` for a person's App), and the two values with copy
  buttons: the **callback URL** to add (an App takes several, so the old one may stay until the old
  host is retired) and the **homepage URL** to replace. Save on GitHub; the card says "GitHub
  accepts this address for sign-in" at once when you come back to the page or press Check again,
  and by itself within about 40 seconds while you watch it (a page left open for long asks GitHub
  less often, every four minutes at the least), or as soon as someone signs in with GitHub. When
  GitHub gave no answer (it is down, the server has no route to it, it refuses the App's client id
  and secret) the card says why; sign-in is not affected by that.
- **The sign-in page** says that GitHub does not know this address and opens the superadmin
  password, which keeps working. "Sign in with GitHub" stays and leads to GitHub as always (until
  the callback is saved GitHub shows its own error there); the first sign-in that completes tells
  the instance that the callback is there, and the sentence goes.
- **`yulius instance migrate`** checks the new URL's callback in its plan, before anything is
  moved, with the same link and values ([Move the instance](#move-the-instance-to-another-host)).

The check sends GitHub's token endpoint the App's own client id and secret, a made-up code and
the callback URL; GitHub answers whether the URL is registered. A URL it accepts is followed by
a control question about one that cannot be registered: only when GitHub refuses that one does
"accepted" mean anything, otherwise the card says that GitHub does not say. No sign-in happens
and nothing is changed at GitHub.

### CI on this machine

Yulius is the projects' only CI (SPEC-002 ADR-21). No GitHub Actions or paid GitHub feature is
involved.

**What runs.** For each sandbox:

- The CI comes from `[[ci.jobs]]`, or `[commands] gate`, in `yulius.toml` on the integration
  branch (docs/api.md "CI").
- An administrator can set `gate` on the sandbox profile instead (`PATCH /api/sandboxes/{sid}`).
- Every open pull request into the integration branch from the same repository (never a fork's)
  and every new integration or production commit is tested once. A failed job cancels the run's
  other jobs; the `security` job neither cancels them nor is cancelled.
- The jobs run in parallel as the `yulius` user, inside the sandbox's containment when
  `YULIUS_CONTAINED=true`, without any token.
- Results appear on each sandbox's CI/CD page (CI tab) and on GitHub as the status `yulius/ci`.
- `YULIUS_CI=off` stops all of it. Merges then fall back to GitHub's checks.

**The machine.** Jobs use the tools installed on the VM (`/usr`, `/opt` and
`YULIUS_CONTAINED_READONLY` paths are visible read-only inside containment).

- Install the toolchains the projects need system-wide, for example rustup's toolchains under
  `/opt`, Node and Python.
- Each sandbox keeps `<data>/sandboxes/<id>/ci/`:
  - `jobs/<job>[.<n>][@<lane>]` (`.<n>` for a matrix job's place, `@<lane>` past the first lane):
    one persistent checkout per job and lane; ignored build outputs such as `target/` and `node_modules/` are its cache;
  - `home`: `HOME` for jobs, with `~/.cargo`, `~/.npm` and `~/.cache`;
  - `cache/<job>[.<n>][@<lane>]` (named like the checkout): `YULIUS_CI_CACHE`, the
    job checkout's own persistent cache, never shared with a job running at the same time
    (SPEC-351: a Cargo target directory kept in one shared cache was relinked under the other
    lane's run).

  `home` is shared by the sandbox's lanes, so a job's installs there must tolerate a concurrent
  run; build outputs belong in the checkout or `YULIUS_CI_CACHE`, never in `home`. A Cargo target
  directory is best in the checkout: the hourly sweep removes its unused build units, while a
  job's `YULIUS_CI_CACHE` is pruned whole, like the caches in its checkout (at once past a quarter
  of the quota, otherwise under pressure). Caches of jobs no run used for 14 days, or of lanes past
  the configured ones, go like their checkouts (stale CI leftovers).
- Budget the disk: a Rust workspace's `target/` per job and lane.
- Removing `ci/` (with no run active) resets a sandbox's caches. The next run rebuilds them.
- CI jobs share the sandbox's CPUs and memory by budget (SPEC-082): a job starts once the CPUs
  and memory it declares (`cpus`, `memory_gb` in `[[ci.jobs]]`, else an equal share among its
  run's jobs) are free, so the jobs of all lanes never ask for more than the sandbox has. Each
  is told its budget in `YULIUS_CPUS` and `YULIUS_MEMORY_MB` and the repository's
  `[resources.env]`. `YULIUS_CI_MAX_JOBS` still bounds the number of jobs. Each job gets
  `YULIUS_CI_JOB_TIMEOUT_MIN`, or its own `timeout_minutes` (1-360) in `[[ci.jobs]]`, counted from
  its start: waiting for a slot, or for its turn as an `exclusive` job (one run of it per sandbox
  at a time), never counts (SPEC-435). A project cuts what its jobs take on a busy host with `cpus` on each
  `[[ci.jobs]]` entry: a run of 6 jobs on 12 CPUs gets 2 each by default, and `cpus = 1` halves it
  (SPEC-362).
- While any of a sandbox's CI jobs runs, its group weighs 500 on the host (100 otherwise), so its
  CI gets more of a loaded host than the agents' local builds in every sandbox
  ([ADR-0247](decisions/0247-a-sandbox-running-ci-jobs-weighs-more-on-the-host.md)). Check it with
  `cat /sys/fs/cgroup/yulius.slice/yulius-sandboxes.service/sandboxes/<sandbox>/cpu.weight`. Every start
  and graceful stop of the server writes 100 to every sandbox.
- Agent sessions, terminals and the Cockpit plan for `YULIUS_AGENT_CPUS` CPUs (default a third
  of the sandbox's, at least 2) and the memory in proportion: three of them testing at once fill
  the sandbox.
- `YULIUS_CI_LANES` (host-sized, a [resource limit](#resource-limits); 2 below 32 CPUs) runs of one sandbox execute at once. Each lane past the first
  keeps its own job checkouts and caches (`ci/jobs/<job>[.<n>]@<lane>`, `ci/cache/<job>[.<n>]@<lane>`), so
  budget its disk too.

**Moving a repository off GitHub Actions** (the onboarding step; agents use the `ci_setup` tool):

1. Translate each workflow job's checks into `[[ci.jobs]]`.
   - Use matrices for splits.
   - Drop `${{ }}` expressions, Actions secrets and services.
   - Install what `uses:` steps installed on the machine or in the job's command.
2. `git rm -r .github/workflows` in the same commit, then open the pull request. Yulius tests it with
   the CI it adds, because the integration branch has none yet.
3. Merge once it passes. The App needs **Workflows: Read and write** to push step 2. Yulius pushes
   it with the task owner's token at the end of the agent's turn: the agent's own token never has
   Workflows (ADR-0114).
4. Turn GitHub Actions off for the repository (see "GitHub App permissions").
5. Rulesets that require Actions checks must switch to `yulius/ci`, or go. Yulius's own merge gate
   enforces the delivery profile either way.

**Hygiene.** After a merge through Yulius the head branch is deleted. Every 6 hours (first 10 minutes
after start) Yulius removes each sandbox's safe stale branches and worktrees itself (except `merged` remote
branches with no merged pull request, such as a release branch, which stay for a person) and lists
the rest on its CI/CD page (CI tab, Repository hygiene card), with what the last cleanup removed and kept (SPEC-269, ADR-0199). People clean
up at once there, agents with `repo_cleanup`. Each removal is audited `hygiene.cleanup`.

### CD on this machine

CD (deployment, SPEC-134) needs no configuration on the server: projects define `[cd.dev]`, `[cd.prod]`
and any other `[cd.<name>]` with its `stage` in their `yulius.toml` ([API](api.md#cd-deployment)). A run uses the CI job timeout (`--ci-job-timeout-min`, `YULIUS_CI_JOB_TIMEOUT_MIN`), a disk bound
for its files (`--cd-run-disk-gb`, `YULIUS_CD_RUN_DISK_GB`, a [resource limit](#resource-limits), 20 GB on a data disk up to 400 GB: the run's directory and
its cache, measured every 5 s; past it the run is stopped as an `error` and its cache emptied, a
delivery environment run alike; ADR-0216) and the
containment: dev runs inside the sandbox (its tools, its network); production joins the sandbox's
production holder (its own network, no host loopback, and its own Docker engine; ADR-0208) on the
host's read-only `/usr`, `/etc` and `/opt`, so its scripts install the tools they need into
`$YULIUS_CD_CACHE`. Files: `<data>/cd/<id>/source.git` (the verified
mirror), `<data>/cd/<id>/environments.git` (the mirror delivery environment runs take their commits
from, ADR-0175), `<data>/cd/<id>/cache/<name>` (`$YULIUS_CD_CACHE`; `cache/<stage>.<name>` for an
environment other than `dev` and `prod`, so a name moved to the other stage gets a fresh cache),
and `<data>/runs/cd-<run>/work` or `<data>/runs/env-<run>/work` (the commit's tree, `HOME` and log, removed
after each run; a production run's are in `<data>/cd/<id>/runs/cd-<run>/work` or
`…/env-<run>/work`, which its Docker engine binds); none is visible to agents. Runs are kept
14 days, 50 per environment, and each environment's last passed run always.

- **Secrets.** A dev run gets the real value of each secret its table names once a person allowed
  it outside the proxy on the Secrets page (the same allowance as copying it into `.env`); a
  production run gets a secret agents set up ("Let agents set it up", the default, ADR-0171) or
  one that someone who manages it chose "Allow for production deploys" for, and never a personal
  secret (ADR-0186). That choice is not offered for a secret agents use (bound hosts or `.env` copies): a production
  secret is exclusive, so production needs a separate secret with its own name (ADR-0109). Output is
  redacted, but scripts hold the values while they run, so allow only what a deploy needs.
- **Agent states** ([ADR-0207](decisions/0207-cd-runs-restore-and-save-agent-states-under-one-hold.md)).
  A table's `states = ["NAME"]` makes the run hold those agent states (a second run naming one waits,
  its output says for which run), restore the vault's newest version of each into
  `<run files>/work/states/<NAME>` (`$YULIUS_STATE_<NAME>`, never a task's copy, never
  inside `HOME`) and save each changed folder back as a new version after the deploy and the
  verify, failed ones included. A failed save makes the run an `error`. Each restore and save is
  audited `vault.state.restore`/`vault.state.save` as `cd:<environment>`. While a run holds a state,
  a task's `secret_state_save` and a person's send are refused; a restart during a run loses what
  it wrote to its states. The wait for the hold has its own bound (the job timeout), so a run that
  waited can last up to twice that. Production restores the newest version whoever wrote it (a
  dev run or a task included): a deploy tool treats its state as data and never runs code from it.
- **Variables.** Dev runs get all the project's variables in their environment (Yulius's own and
  the secrets win); production runs get only those their table names (`variables = ["NAME"]`),
  since a person with write access sets them and an administrator deploys production. A named
  variable the project lacks refuses the deploy.
- **Docker.** Dev runs use the sandbox's engine, as tasks do (`docker buildx`, `docker load`).
  Production runs use their production holder's engine (SPEC-279, ADR-0208): `dockerd` of the
  sandbox base (the host's Docker, which `yulius host setup` installs, with buildx), started on
  the first `docker` command, named `<slug>-prod`, its images and build cache in
  `<data>/sysroots/<id>.prod/docker` and kept between deploys, out of every agent's reach. The
  run's own files can be mounted (`docker run -v "$PWD":…`); nothing of the sandbox can. The
  `docker` CLI and its buildx plugin are the host's. Its storage counts toward the sandbox's
  disk quota (but never refuses a production deploy by itself) and is pruned after a production
  run that leaves the sandbox under disk pressure (90 % of its quota); the holder stops when idle and goes with the sandbox. Without
  a `dockerd` in the base, `docker` answers why (run `sudo yulius host setup`), and
  `yulius doctor --docker-smoke` proves it on the host.
- **SSH and other hosts.** Runs use their holder's network (dev the sandbox's, production its
  production holder's): every Internet host over TCP, port 22 included, but not the host's
  loopback. Yulius keeps no host list; a project names its hosts and
  pins their keys in its own state or repository (`StrictHostKeyChecking=yes` with its own
  `UserKnownHostsFile`), since each run's `HOME` is fresh.
- **Egress off.** With `YULIUS_SANDBOX_EGRESS=none` sandboxes have no network, and production
  deploys (whose production holder has no `slirp4netns` then either) have none.
- **Production.** Only an administrator deploys it, on the CD page, typing the project's slug. With
  `promote = true` the deploy first promotes under that person's GitHub identity; a promotion pull
  request whose checks still run refuses the deploy until they pass.
- **Restarts.** A run that was running when the server stopped is recorded as `error` ("outcome
  unknown") at the next start, its process host stopped and its files removed, and is never started
  again by itself: check the environment and deploy again.

## 6. Onboarding a repository and your Claude sessions (`yulius onboard`)

On a developer machine with the `yulius` binary:

```sh
yulius login --server https://yulius.example.com          # browser approval (device flow)
cd ~/code/product && yulius onboard                       # --server once signed in to one server
yulius onboard --status                                   # the sandbox's onboarding state
```

`yulius import` is the same command. The repository must be on GitHub under the instance owner;
adding it as a sandbox needs admin or maintain on GitHub, adding sessions to an existing sandbox
needs write. The command shows four steps (repository, Yulius, Claude Code sessions, upload),
asks which of your sessions of this repository to send (all, recent, or picked; `--sessions
all|recent|pick|none`, `--since 14d`), redacts secrets on your computer and lists what it
redacted (`--show-redactions` for every key name), and sends only sessions the server does not hold
yet or that changed. `--dry-run` uploads nothing; `--sessions-only` leaves out unpublished
branches and uncommitted changes; `--yes` asks nothing; colours follow `NO_COLOR` (and
`--no-color`). Uploads resume after interruptions: run the same command again.

Each teammate runs it for their own sessions; sessions are attributed to whoever uploaded them.
They are stored read-only under `<data>/sandboxes/<sid>/sessions/` (at most 128 MiB per
transcript, 10 000 sessions and 8 GiB per sandbox), listed on the sandbox's Sessions page, and
searched by agents (`sessions_search`, `session_read`). The uploader or an admin deletes one on
its page; deletions are audited (`session.deleted`). The command only uploads: a new sandbox's
onboarding task (adapting the repository to the Yulius standards) waits until someone clicks
**Start onboarding** on the sandbox page, or the person's own agent runs `yulius project onboard`
with an `operate` token ([remote-control.md](remote-control.md)); a `yulius login` token cannot
start it (SPEC-051, ADR-0038, ADR-0188). New tasks wait until onboarding is ready (or an admin skips it); `yulius onboard
--status` and the sandbox page say what is pending.

A checkout that is not on GitHub (no remote, or another host) goes to a playground instead
(SPEC-171): `yulius onboard --playground [path] [--name <name>]` sends every local branch and tag,
the uncommitted work and the chosen sessions to a playground of yours named after the checkout
directory (or `--name`). The first run makes it, with the uncommitted work as one commit on the
checked-out branch (a detached `HEAD` is refused); later runs land in it under `import/<login>/`.
Nothing is pushed anywhere and GitHub is never called; it counts toward your 10 playgrounds.

Removing any sandbox asks what happens to its files on this server, with no default (SPEC-276,
ADR-0205). **Delete its files permanently** (`POST /api/sandboxes/{sid}/purge`; the superadmin can
also purge an already removed one, from Settings → Instance → Leftovers → Kept files of removed
sandboxes) deletes everything of it: `<data>/sandboxes/<id>/` (clone, home with the sessions,
worktrees with any uncommitted work, CI, browser), `<data>/sysroots/<id>`,
`<data>/runtime/sandboxes/<id>`, `<data>/cd/<id>`, `<data>/displays/<id>`, its runs, transcripts,
the removal's export and a playground's `<data>/playgrounds/<id>.git`, as soon as no agent run,
shell, browser, CI run, deploy or other process of it is left and its holder stopped: tried at
once, every 15 seconds for 10 minutes, then with every disk round. The GitHub repository, task
records and the audit (`sandbox.purge_requested`, `leftovers.delete`, `sandbox.purged`) stay.
**Keep its files** keeps what PRJ-009 keeps (below), listed with its size until someone deletes it.

### Importing secrets (`yulius secrets import`)

Keys held on a computer go into the vault from the CLI, signed in with `yulius login`, or with
`yulius remote login` and a token from Settings → Connect your local agent (SPEC-205, ADR-0163):

```sh
yulius secrets import --to agent .env      # agent secrets of this checkout's sandbox
yulius secrets import --to agent --sandbox acme/shop keys.env
yulius secrets import --to team shared.env                 # team secrets of this checkout's sandbox
yulius secrets import --to team --instance shared.env      # team-wide team secrets
gpg -d keys.env.gpg | yulius secrets import --to team -   # a dotenv text on stdin
yulius secrets import --to agent                           # type each name and its hidden value
```

The file is dotenv: `NAME=value` lines (optionally `export`), values bare, in single quotes
(literal) or double quotes (`\n`, `\t`, `\"`, `\\` read), quoted values may span lines, `${…}` is
kept as written, and the `#` comment lines directly above a key become its description (agent)
or notes (team), except commented-out assignments (`# NAME=old-key`), which are never kept. A
malformed file is refused before anything is asked, by line number. The command lists the
server, the target and every name as `new` (marked `with description (agents read it)` or
`with notes` when a comment goes along), `exists` (skipped: nothing is overwritten) or skipped
with the reason (agent names are `[A-Z][A-Z0-9_]*`, values at least 8 characters on one line; a
PEM or OpenSSH key is kept as a key file, but its first line alone is refused). It then sends
nothing until `yes` is typed at the terminal, except with `--yes` and the `operate` token that
`yulius remote login` stored (refused with any other token; ADR-0194). `--from-file NAME=PATH`
imports a whole file, such as a key, as one secret, and `--env-files` creates agent secrets
already allowed into `.env` files ([remote-control.md](remote-control.md#secrets)). Team and agent
secrets go to the sandbox (`--sandbox`, else the checkout's GitHub repository); `--instance` makes
team secrets team-wide, and agent secrets global (instance administrators), switched on for that
sandbox ([ADR-0186](decisions/0186-agent-secrets-are-project-global-or-personal.md)). Agent secrets start bound to no host and, as on the Secrets page, agents set them
up: they bind the hosts they need and production deploys get them, which the list says before
`yes`; `--confirm-hosts` makes a person allow each host and production deploys instead (SPEC-216).

Agent secrets are masked as `[secret:NAME]` wherever their value appears, so the import skips a
value that is ordinary text, naming why: a configuration word (`production`, `staging`, `main`,
`true`, …), a cloud region, time zone, locale or path, a host name or address, an e-mail
address, or a URL without credentials (no user information, no key-named parameter, no
token-like host label or path segment); if one is a credential after all, a person adds it on
the Secrets page, never in the hand-over. Run in a checkout of the
project, it also looks for each new value in the tracked files (up to 1 MiB each, 256 MiB in all;
the value never leaves memory) and asks about one found there (a no is counted as declined, not as a failure; `--yes` skips
and lists it); elsewhere
it says the repository was not searched (SPEC-274, ADR-0204). A secret imported by mistake is
deleted by name with `yulius secrets delete NAME` (the operate token of `yulius remote login`;
`-p owner/name`, else the checkout's project).

Put keys that span lines in a file. The hidden prompt reads one line: a pasted key arrives cut
to its first line, which is refused, and the terminal usually discards the rest; if part of it
still shows at the next `Name:` prompt, press Ctrl-C (nothing has been sent).

Values never appear: no option takes one, the output holds names only, nothing is written to disk,
and the server logs no request bodies; the vault audit records who added which name. The command
reminds you that the source file still holds the values. Exit 0 when every secret was imported,
already there or declined at the question about the repository, 1 otherwise (skipped as invalid, refused, cancelled, or not sent after an expired
sign-in, a 403 or an unreachable server; creates are never retried, and a second run skips the
imported ones as existing).

### Dropped files

Files users drag onto the editor are written into the repository or a task worktree; files
dropped or pasted onto a terminal are stored as attachments in
`<data>/sandboxes/<sid>/attachments/{tasks,shells}/<id>/`, outside every worktree (never in
`git status` or a pull request; the sandbox's processes can read and change them like its other
files). They count towards the
data directory's disk: at most 1 GiB each (`YULIUS_UPLOAD_MAX_MB`), 500 per task or terminal,
8 uploads at once; an upload that would bring the data disk to its critical level (95 % used) is
refused before it starts (ADR-0219). A deleted task's attachments stay while another task's request or an automation's prompt names
them, and otherwise wait 7 days in `attachments/trash/<secs>-<number>-<id>/` (SPEC-396;
`attachments_trash` restores them into a task), a terminal's go when it ends
unless a task's request or an automation's prompt names its folder (others are removed at the next
start), and composer drafts wait in `attachments/drafts/<user>/` and go after a week unless named
(SPEC-413, ADR-0259); archiving a task keeps them. Uploads are audited
(`audit_log`: `file.uploaded`, `attachment.uploaded`).

## 7. Backup and restore

```sh
sudo -u yulius yulius backup --data-dir /var/lib/yulius /var/backups/yulius-$(date +%F).db
```

`backup` writes a consistent SQLite copy (`VACUUM INTO`). Also back up `/var/lib/yulius/master.key`
(it encrypts every stored secret; without it the database's credentials are unreadable) and, if
wanted, `sandboxes/` (clones and worktrees can be re-created from GitHub, uncommitted work cannot).
Restore: stop the service, put the database copy at `yulius.db`, restore `master.key`, start.
The database copy holds the secrets vault and the egress CA key as ciphertext only; keep
`master.key` apart from database backups (whoever has both can read every secret). For a copy of
everything, files included, see "Copy or move the whole instance" below; to move to another host,
use the next section.

### Move the instance to another host

`yulius instance migrate` (SPEC-460, ADR-0297; the warm copy: SPEC-465, ADR-0303) moves the whole
instance with one command. Run it as root on the old host over SSH (not from a Yulius terminal: it
freezes the service), or let a superadmin ask root for it from the page
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

It prints one checklist, a line per step as `host setup` does (`ok`, `changed`, `manual`, `FAIL`),
and ends with exit 0 when nothing failed, 1 when a step failed and 2 when it was refused before
anything changed. Run the same command again to continue a move that stopped: every step finds out
what is already done. The new host needs only SSH for root and the same architecture; it must not
hold a Yulius instance. One run at a time (a lock in the state directory); a unit that is already
frozen (an earlier run that was killed, an export in progress) is refused until it is thawed, and
so is a unit that does not run: start it first, so the move can freeze everything it runs (an
instance that cannot start is copied with `yulius instance export`, below).
`--public-url` is `https://host` without a port.

**Plan** (nothing is changed but the flow's own files in `/var/lib/yulius-migrate` on both hosts,
and nothing is frozen). Everything a person has to do is asked for here:

- The SSH sign-in to the new host, once: the flow makes a one-time key and adds it to root's
  `authorized_keys` there through your own sign-in (`ssh` asks for the password or second factor,
  and offers root's keys as any `ssh` of root does). When that sign-in does not work it prints the
  line to add and stops; add it and run the command again. From then on it uses only that key.
  With `--no-sign-in` no sign-in is tried at all and root's ssh configuration is not read: the
  line to add is printed, and every connection offers the one-time key alone. The move helper
  always runs the command so (a move asked from Settings → Instance is nobody's sign-in).
- The new host: release, architecture, systemd, CPUs and memory; refused when it runs or holds an
  instance or another program holds the ports of the proxy and of Yulius.
- Disk space where the data directory goes. When it is too small the blank disks of the new host
  are listed: name one with `--data-disk`, or two for a mirror. Only disks without a partition
  table, filesystem, holder or mount are taken.
- The link's rate, how long the copy will take at it, and a first estimate of the freeze. The
  instance runs during the copy; it is frozen only for the last changes, the proof and the start.
  For the estimate the plan reads a sample of the data directory twice (two seconds at most the
  first time) and takes how many entries a second the second reading looks at: the freeze counts
  every entry four times at about that rate. The passes measure it again, and the line
  `downtime` after the copy is the one that counts (**Copy**, below). With a staged copy of an
  earlier run on the new host the plan says so: only what changed since is sent.
- The new public URL: `--public-url`, else the old URL, else for an address-made URL
  (`1-2-3-4.sslip.io`) the same form for the new address. A name that does not lead to the new
  host yet is named: change its DNS record when the checklist says the new instance runs.
- When the URL changes, the GitHub App. The command asks the running instance, which asks GitHub
  whether it accepts the new URL's callback: `ok` when it does; else `manual` with the link to the
  App's settings and the two values: add the callback URL `<new URL>/api/auth/github/callback`
  and set the homepage, before the cutover (an App accepts several callback URLs, so the old one
  keeps working). GitHub has no API for this and Yulius never uses a person's GitHub session. Until
  it is done GitHub sign-in fails at the new URL; password sign-in works. When the instance cannot
  be asked (an older server is running, GitHub does not answer) the line says why and where the
  settings are in general ([the App after the public URL changes](#the-app-after-the-public-url-changes)).
- What is not carried: another reverse proxy than Caddy, another firewall than ufw, packages
  without a candidate on the new release.

Then it asks "Move this instance to …?" unless `--yes`. Without a terminal and without `--yes` it
ends after the plan.

**Prepare** (the old instance still runs). On the new host: the data disk (`--data-disk`: ext4 on
the disk, or on an mdadm RAID1 of two, mounted at the data directory by UUID from `fstab` with
`nofail`, the array in `mdadm.conf` and the initramfs, the unit drop-in `RequiresMountsFor=`, the
mount point given to the service user); the old host's third-party apt sources with their keys
and the release's codename replaced (a source without that suite keeps the old one; one that still
fails is removed and named); the old host's `apt-mark showmanual` packages in one transaction (a
library whose name carries another version there, `libicu74` → `libicu78`, is renamed; kernel,
boot and network packages are left to the machine); coreutils of the old host's flavour (GNU or
uutils); `/usr/local`, the entries of `/opt` no package owns and root's Rust toolchain, copied as
they are; `host setup`'s host items; the build at the unit's path; the unit file, its drop-ins,
its environment and key files outside the data directory, the update helper's units and its
record of the installed commit, the move helper's units when the old host has them
([the move helper](#the-move-helper-moves-from-settings--instance)). In the unit file, its drop-ins and the Caddyfile the old public
URL and host name are replaced; environment and key files travel as they are, and the plan names
one that mentions the old host, for you to edit; Caddy's directory
and drop-ins for the new host name, validated and started, so its certificate is there before the
cutover; ufw's rules (enabled at the very end); and `host setup` up to the sandbox base. Limits pinned in the unit travel unchanged and
are named; the new instance re-sizes those that no longer fit (SPEC-459).

**Copy** (the old instance still runs). The data directory is copied into a staged copy on the
new host in passes, compressed with `zstd` when both hosts have it. The first pass sends
everything; every later pass sends only what the new host does not hold as it is now. The
instance stays usable; the copy reads the whole data directory once, so its disk is busier than
usual.

- What changed is never judged by size and time. For every entry the new host has taken in and
  synced, the old host keeps a receipt in `/var/lib/yulius-migrate/copy`: a digest of the entry's
  name, kind, size, owner, mode, time and attributes, of its inode number and of its inode's
  change time, which no program can set. An entry is left out only while its receipt matches. An
  entry that changed within two seconds of a pass, or while it was read, earns no receipt and is
  sent again; so is every file on another filesystem mounted inside the data directory. A file
  that changed is sent whole.
- A pass removes nothing on the new host: what no longer exists goes during the freeze.
- After every pass that sent only changes, the line `downtime` gives the estimated freeze: the
  entries are counted four times (twice on each host), what changes meanwhile is sent, the new
  host syncs, and the start is allowed half a minute. Without `--max-downtime` the move copies
  again while the changes alone would take more than a minute to send, at most five times, and
  then freezes. With `--max-downtime <minutes>` it freezes only when the estimate is at most
  that; after five passes above it the move ends with exit 1, nothing was frozen, and the staged
  copy stays: run the command again at a quieter time or with a higher bound; it sends only
  what changed since.
- A pass that was cut (a broken link, Ctrl-C, a killed command) costs only what the new host had
  not yet taken in: run the same command again. A link that goes silent ends the pass after
  about two minutes (`Timeout, server … not responding`). Ctrl-C during the copy ends the
  command at once; nothing is frozen yet. The new host's data directory holds the staged copy's marker
  (`.yulius-import-incomplete`); when the copy there was removed, or is not the one the old host
  knows, the copy starts from the beginning. When the receiving side died in the middle of a
  pass (the new host was restarted), everything is sent into the staged copy again.
- The receipts hold only in the boot they were written in and while the old host's clock is not
  set back. After a restart of the old host (after a crash a file may be newer than its change
  time says), or when its clock was set back by a second and a half or more, the move says so
  and sends everything into the staged copy again; when that happens just before the freeze,
  nothing is copied frozen, the old instance is thawed and the same command copies again while
  it runs. The clock's own pace (NTP) does not count, only a step.
- The copy reads as root while the sandboxes run. It never follows a link, and it stays in the
  directories it opened: a directory that is renamed or replaced by a link while a pass walks
  it is left out of that pass and sent by the next.
- The line `downtime` names what every pass sends again, when that takes a second or more:
  entries that change all the time, files on another filesystem, and a file that took the
  place of a directory (the new host removes a directory only during the freeze). They are
  sent during the freeze too, in full.
- Do not update Yulius on the old host during the copy. An instance that reports another build
  before the freeze than in the plan is not frozen; run the command again with the build it runs
  (the staged copy is kept).

**Cutover.** The old unit is frozen, and with it its sessions unit `yulius-sandboxes.service`
where the host has one ([Where sessions live](#where-sessions-live)); every thaw below is of
both. One last pass sends what changed since the passes before, with the list of every path the
old host holds; the new host installs it and removes what is no longer there. Then both hosts
count the whole copy: entries and bytes for each top-level folder
and a fingerprint over every entry's name, kind, size, owner, mode, time and restorable
attributes. Only when they are equal is the copy marked and the new instance started
(`host setup`'s remaining items, `systemctl enable` and `start`, then it must report the old
host's build). Until that start any failure, Ctrl-C or SIGTERM thaws the old instance, which runs
on as before; the staged copy stays, and the next run sends what changed since (a copy whose
count differs is dropped, and the next run copies everything again); an instance that systemd
does not report frozen as a whole right after the freeze is thawed too, before anything is
copied frozen, and the line says what systemd said (the server's unit does not run, a unit of
it runs unfrozen, or no answer). Once the start is ordered nothing thaws the old instance by
itself, not a signal
and not a lost terminal. If the new instance does not come up, it is stopped and disabled, and
only then is the old one thawed; the copy there was started and must be removed (`rm -rf` the
data directory on the new host) before the next run. If the new host stops answering at that
point, the old instance stays frozen: run the command again when it answers. Once the new
instance reports the build it has taken over: the old unit stays frozen and is disabled, and
whatever fails after that (the helper, the Docker smoke) is a line to act on, never a rollback.

**After.** The flow waits for `https://<new host>/healthz`, reads what the new instance did with
the tasks that were live (its first start puts them back in the queue, whoever owns them; tasks a
person had paused or was stopping stay stopped), and replaces what the old host's Caddy site
proxied to Yulius by a temporary redirect (307) to the same path at the new URL, with the header
`X-Yulius-Moved-To`; the site keeps its name and TLS settings, and other sites in the file stay.
It enables the move helper's path unit on the new host when it was enabled on the old one (a
helper that is installed and switched off stays off there), leaves the finished checklist for the
new instance's page (`/var/lib/yulius-migrate/arrived.json` there; remove the file to drop it),
then removes its key from the new host, enables ufw there with the old host's rules (first
admitting the old host itself on the SSH port the move used; make sure the carried rules let
you in on the new host's SSH port), deletes the key on the old host and lists what is left for
people: the DNS record if still open; the GitHub App, which the new instance is asked about once
more (`ok` when GitHub accepts the callback by now, else the link and the values again); laptops
(`yulius remote` follows the old URL's redirect by itself and keeps its sign-in under the new URL,
[remote control](remote-control.md#2-connect-your-agent); Claude Code's own connection needs
`claude mcp remove yulius` and `claude mcp add` with the new URL); and that the old host
still holds a frozen copy with `master.key` and every secret. On the new instance, Settings →
Instance → Address then shows where it came from, when, what its first start did with the tasks,
and whether GitHub accepts the new address (the note about the move is read from
`<data>/moved.json`; remove the file to drop it). A later run that finds the key already deleted
on the old host (the finish was done and only its record was lost) records the finish and says
so, whether or not the new host answers; it asks the new host nothing more, so the GitHub App is
named with its link and values as in the plan.

**Abort.** `sudo yulius instance migrate --abort` on the old host stops and disables the new
instance (through the key while the move is not finished; afterwards the key is gone: stop it
there first with `systemctl disable --now yulius.service; systemctl stop
yulius-sandboxes.service`, since its sessions live in their own unit; the abort refuses while the new URL
still answers and otherwise asks you to confirm that it is stopped, `--yes` answers for you), puts
the old proxy configuration back, thaws and enables the old unit and removes the key and the
state, the receipts of a staged copy included. The copy on the new host, staged or started, with
`master.key` and every secret, stays there until you remove it.
It never leaves both running. Work done on the new host after the move is not copied back.
Laptops whose `yulius remote` had already followed the move point at the stopped instance: their
commands say where the sign-in came from, and `yulius remote login <old URL>` signs in there
again, with a token ([remote control](remote-control.md#2-connect-your-agent)).

The run that ordered the start ignores Ctrl-C, SIGTERM and a lost terminal from then on, on
purpose (it must not thaw the old instance beside a new one). If it hangs, end it with `kill -9`,
then run the same command again or `--abort`: a start still in flight on the new host finds the
abort's stop and starts nothing. A run that goes on from a recorded start watches no signal:
Ctrl-C ends it at once and thaws nothing (also when it had just frozen a unit of the old
instance again), and the same command goes on again.

The same command after a run that ended there plans and copies nothing; the new instance's state
decides. A new host that cannot be reached is waited for: the run fails saying so, touches
nothing and asks for no key again (only a key that is missing on the old host or refused by the
new one is added anew). With the old instance still frozen as a whole (the server's unit and its
sessions unit) the run orders the start again and goes on as above. With a unit of it no longer
frozen it starts nothing and only asks the new instance. When that reports the build, the move
stands: whatever unit of the old instance runs (thawed or started by hand) is frozen again, to
stay, the server's unit is disabled, and the checklist names the units that ran beside the new
instance (what their agents did meanwhile is on the old host only). If a unit cannot be frozen,
that is a `FAIL` line with the reason, the two commands to run and, for a freeze that keeps
failing, the commands that stop the old instance for good (`systemctl stop` and `systemctl kill
--kill-whom=all --signal=SIGKILL` of both units); nothing is thawed then either, and a freeze
that did not complete within a minute is left on its way. A server's unit that has stopped
(`inactive` or `failed`) is not frozen by systemd: its sessions unit is frozen where it still
runs, and both are named for you to make sure nothing of the old instance is left. A server's
unit that systemd reports on its way up or down is not taken for stopped (a sessions unit that
is being stopped counts as ended). A unit that was frozen is named also when the other could not
be. When the new instance answers
and does not run, `--abort` is the way back; when the new host does not answer the question,
nothing is touched and the same command asks again once it does.

Not part of a move: packages installed inside sandboxes keep their files; on another release the
checklist names those sandboxes. The sandbox base is built for the new host while preparing, so
a host that cannot run sandboxes is found before anything freezes.

### Move from Settings → Instance

A superadmin moves the instance from **Settings → Instance → Move to another host** (SPEC-470,
ADR-0305). Root runs the same command for it: the page asks through a request file, as updates do,
and shows the command's own checklist. It needs [the move helper](#the-move-helper-moves-from-settings--instance)
on the old host; without it the card says so and names the command above.

1. **Show the plan.** The form takes the new host (`root@new-host`, a name or an address that leads
   there by itself: root's ssh configuration is not read), its SSH port, a new public URL (empty:
   as the command decides) and one or two blank disks. The plan changes nothing but the move's own
   files and freezes nothing. A move asked from the page never signs in as root does (root's own
   keys, its agent, its ssh configuration, a password), so the first plan ends with **the key line
   to add**: copy it into root's `~/.ssh/authorized_keys` on the new host and show the plan again.
   That is on purpose: whoever can ask root's helper would otherwise reach every host that trusts
   root here, and so only a host where a person added the move's key is moved to. The lines a
   person has to act on (`manual`: a DNS record, the GitHub App, what is not carried) and the
   failed ones are repeated above the checklist; a plan with a failed line offers no move.
2. **Run migration…** names the host that root will give the instance's data to (every repository,
   secret and key) and the freeze the plan estimated, and is confirmed by typing the host's name.
   It is the plan that was shown: the same host, port, URL and disks, shown at most an hour ago
   (else show it again). The card then shows the phases and the checklist as it grows: the new
   host is prepared and the data copied while the instance keeps working (the phases Prepare and
   Copy), and only then it freezes. The page asks for no bound on the freeze: the move decides
   as the command does without `--max-downtime` (it copies again while that shortens the
   freeze). Use the command to set one.
3. **While the instance is frozen** for the last changes, the proof of the copy and the start on
   the new host, nothing answers, the page included: the card says so with the plan's estimate (the
   copy measures it again a moment before the freeze; that line is in the checklist once something
   answers), keeps the last checklist and keeps trying. With the same public URL the new instance
   answers the page once the name leads to it, and the card shows **the finished checklist**. With
   another URL the old address redirects: the card links to the same page at the new address, where
   you sign in again (a sign-in belongs to one address) and find the finished checklist. If the
   move did not complete, the old instance answers again and the card says where it stopped, with
   **Continue the move…** (the same values, the same confirmation) and **Abort…**.
4. **Abort…** says what it does first: a run in flight is stopped (before the new instance's start
   is ordered that leaves everything running here; after it the command finishes by itself), then
   `--abort` runs as above, without `--yes`: when it needs a person's word that the new instance is
   stopped, the checklist names the command to run on this host. Data that was already copied stays
   on the new host with `master.key` and every secret: the abort's checklist says to remove it
   there. After a finished move the old instance is frozen and cannot be asked: go back with `sudo
   yulius instance migrate --abort` on the old host.

Only the superadmin sees the card (403 for everyone else); every plan, move and abort is in the
audit log (`migration.planned`, `migration.started`, `migration.aborted`: who, the host, the
values). While a move's request waits or its run is in flight, **Update now** and **Restart…**
are refused, and nothing is asked of a move while an update or restart runs. A move started with
the command on the host shows on the page as a recorded checklist; whether it still runs is known
only for runs the page started. On the new instance the card shows the move that brought it
there until the next move does, or until `/var/lib/yulius-migrate/arrived.json` is removed.

### Copy or move the whole instance

`yulius instance export` and `yulius instance import` (SPEC-329, ADR-0222) copy everything: the
database, `master.key`, every sandbox (clones, worktrees with uncommitted work, homes with the
Claude sessions), installed packages and Docker images and volumes (`sysroots/`), transcripts and
runs. Both run as root.

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

**Export.** It freezes `yulius.service` and `yulius-sandboxes.service` (`systemctl freeze`; a
stopped server's sessions unit alone): the server, every agent, terminal,
browser, CI job, deploy and sandbox Docker engine pause, so nothing changes while it reads. The web
UI does not answer during the export. It then prints the exact size ("Exporting 123.4 GiB
(1,234,567 entries)"), streams an uncompressed tar of that exact length to standard output
(refused when standard output is a terminal), shows progress on stderr and thaws the service
(also when it fails, the reader goes away or it gets Ctrl-C, SIGTERM or SIGHUP, even while it
waits on a stalled connection). If it is killed
with SIGKILL, thaw by hand with the command it printed: `sudo systemctl thaw
yulius-sandboxes.service yulius.service`. `--move` keeps the old instance
frozen and disables its unit after a successful export, so it never runs beside the new one (both
would run the same agents against the same repositories); undo with `sudo systemctl thaw
yulius.service yulius-sandboxes.service && sudo systemctl enable yulius.service`. Left out, because the new host makes its
own: `system/` (the sandbox base), `runtime/`, `update/`, `claude-code/` (Claude Code for this host's platform, downloaded again), `yulius.db-shm`, sockets and FIFOs, and a
`.yulius-limits-adapt` the instance still holds from an import of its own (every import writes
that mark anew, below). It is
refused (exit 2, nothing frozen) while the update helper runs (checked again once frozen and
before the copy's checksum), from a Yulius terminal (it would freeze itself), and while the
service is stopped but its agents still run (start it first).
`--service` and `--update-service` name other units. The copy is uncompressed so its size is known
first; pipe it through `zstd` yourself if the link is slow.

**Import.** On the new VM with the same or a newer `yulius` installed (§2, before `host setup`):
it reads the manifest first and refuses (exit 2, nothing written) a copy of a newer database
schema, a copy exported from another path than `--data-dir` (default `/var/lib/yulius`, as
`YULIUS_DATA`; Git worktrees and the database hold absolute paths, and the copy never chooses
where it goes), a data directory that exists and holds anything but `lost+found` and dotfiles,
belongs to another user or to root (a new disk: it prints the `useradd` and `chown` to run), and
too little free space or inodes. A subordinate id range smaller than the exported one is refused
after the service user is made, before anything of the copy is written. It makes the service user and its subordinate ids (as
`host setup` does), extracts as the stream arrives, remaps the old service's ids and subordinate
range to the new ones, and checks the copy's SHA-256 and length. A file of another owner (root
included) becomes the service user's, without setuid. File capabilities and ACLs are not restored
(it says how many). A cut or damaged stream fails (exit 1) and leaves the data directory marked
incomplete: remove it (`sudo rm -rf /var/lib/yulius`) and import again. Then run `host setup`
with the public URL the import prints (it writes the unit, builds the sandbox base for this host,
checks Docker and starts Yulius; on a host where bwrap cannot create user namespaces yet, the
import says that this step fixes it), copy other `Environment=` lines you changed in the old unit
(keys; leave the resource limits behind), and point the reverse proxy and DNS at the new VM. The
import leaves a mark (`<data>/.yulius-limits-adapt`): the first start on the new host replaces
every [resource limit](#resource-limits) pinned for the old one by its host-sized value, also
one copied into the unit later. Tasks that were running come back as after a
reboot.

### Leftovers: what is kept, what is cleaned and when

Yulius deletes what nothing needs any more by itself (SPEC-215, ADR-0170): at every start (two
minutes in, so after every update) and every hour. Each deletion checks its rule again right
before it removes anything: a running agent, shell, browser, CI run or deploy, a live holder, or a
task that started working again keeps its files. Every deletion is audited as `leftovers.delete`
(path, size, reason), and every failure as `leftovers.failed`.

| What | When it goes | By itself? |
| --- | --- | --- |
| A removed sandbox's `worktrees/`, `ci/`, `browser/`, `<data>/import-worktrees/<id>`, `<data>/cd/<id>`, `<data>/displays/<id>` and its agent runs' `<data>/runs/<attempt>` | 10 minutes after the removal, once nothing of it runs | yes |
| A removed sandbox's `<data>/sysroots/<id>` (installed packages, Docker images and volumes) and `<data>/runtime/sandboxes/<id>` | once its holder has stopped (a minute after the removal, SPEC-206); removed as root of a mapped namespace | yes |
| Everything else of a removed sandbox: its clone, home with the Claude sessions, attachments, `<data>/transcripts/<task>`, a playground's `origin`, `<data>/exports/sandbox-<slug>-<ms>.json` | removed with **Delete its files permanently**: as soon as nothing of it runs; removed with **Keep**: kept (PRJ-009), listed under Kept files of removed sandboxes; the database records always stay | on request |
| A task's rebuildable caches (`target`, `node_modules`, `[disk] caches`, …) | closed, stopped or failed: after 10 minutes; **archived**: at once; **done**: after 30 quiet minutes; waiting: after 4 hours; any idle task at 90 % of the quota or with the data disk critical | yes (the worktree and uncommitted work stay) |
| Units of a Cargo `target` (worktrees and CI checkouts) | unused for 48 hours (6 h with the disk low, 1 h critical), then the least recently used incremental ones above 5 % of the disk (at least 8 GB; half when low, a quarter when critical); never while Cargo compiles there (its locks); compiled artifacts only in a CI checkout with no CI run and in the worktree of a task that neither works nor runs background work while no Shell of the sandbox is open (otherwise only incremental state goes) | yes, hourly, and every 5-minute round while the disk is low (audited `disk.sweep`) |
| A closed or cancelled task's clean worktree | after 10 minutes (its branch keeps the work) | yes |
| Git's record of a worktree inside the sandbox directory whose directory is gone (`.git/worktrees/<name>`), not locked | always (that record only; a worktree an agent keeps in its own `/tmp` keeps its record) | yes |
| A CI checkout of a job or lane no run used for 14 days, and its cache (`ci/cache/<job>[.<n>][@<lane>]`); an entry of `ci/cache` named like no job (what the cache every job shared held before SPEC-351) | while no CI run of that sandbox runs | yes |
| A draft attachment older than 7 days that no task's request or automation's prompt names (SPEC-413) | always | yes |
| `<data>/runs/<name>` that no attempt record, live run, active CI job, running deploy or running delivery environment run owns, untouched for a day (an ended task's last run, with its final terminal screen, stays) | always | yes |
| Transcripts of a deleted task; a Docker smoke test's sandbox whose process is gone | always | yes |
| An unused sandbox base (not current, no live holder mounts it) | always | yes |
| A worktree no task owns; a snapshot import's checkout an interrupted import left in `<data>/import-worktrees/<id>/` (after an hour, ADR-0213); a directory under `sandboxes/`, `sysroots/`, `runtime/sandboxes/`, `cd/`, `displays/` or `import-worktrees/` that no sandbox record owns | listed | **only a person** |
| The host's `/var/lib/docker` and `/var/lib/containerd` (docker-ce; Yulius's host daemons are off) | listed, never deleted: remove them on the host if nothing else uses them | report only |
| Update backups under `/var/lib/yulius-update/backups/` | the helper keeps the last 3 | yes (not listed) |

**When the disk runs low.** The upkeep reads the data disk every minute. From 85 % used every page
shows a "Disk 88% full" pill, each 5-minute round sweeps the build directories harder and the safe
leftovers round runs (at most every 10 minutes); from 95 % ("Disk almost full") idle tasks' and
CI checkouts' caches go too until the disk is back at 85 %. Each rise is audited `disk.pressure`.
If it stays low, delete kept files of removed sandboxes in the Leftovers card.

**Settings → Instance → Leftovers** (superadmin) lists every item with its path, kind, size, owner,
why it is a leftover and whether Yulius deletes it by itself, with the disk's usage and the
reclaimable total, when the automatic cleanup last ran and what it freed, and the kept files of
removed sandboxes with their size and **Delete files**. **Refresh** rescans, and **Delete** (the
selected items) and **Delete all safe** ask for confirmation with the size, then run in the
background. `yulius leftovers list` also prints the last automatic cleanup. The same from SSH, as root or the
service user while the server runs:

```sh
sudo -u yulius yulius leftovers list                 # rescans; id, size, safe/needs you, kind, path
sudo -u yulius yulius leftovers delete --all-safe    # shows them and the total, then asks for yes
sudo -u yulius yulius leftovers delete 3f9c0a1b2c3d4e5f 0a1b2c3d4e5f6a7b --yes
```

Nothing reached through a link is listed or deleted: a sandbox can put links in its own directory,
and Yulius never follows one. So if you moved part of the data directory (`sandboxes/`, `runs/`,
`sysroots/`, …) to another disk with a link, its leftovers are not listed, and the server logs a
warning at each scan. Bind-mount such a directory instead.

The command talks to the loopback listener (`--url`, default `http://127.0.0.1:8081`) with the key
the server writes to `<data>/leftovers-cli.key` (0600, new at every start; `--data-dir` if the data
directory is elsewhere). Exit 0: done, 1: a deletion failed, 2: not run (no key, server
unreachable, no confirmation).

### Secrets vault (agent secrets, ADR-20)

Nothing to configure. On first start the server creates the egress CA (its key sealed in the
database) and serves the agents' egress proxy on the internal listener (`YULIUS_INTERNAL_BIND`):
agent runs get `HTTPS_PROXY` pointing there, so whatever makes that address reachable from a
sandbox (it is loopback today; network isolation must keep it reachable) carries the agents'
HTTP(S). Claude Code's own API traffic (`*.anthropic.com`, `claude.ai`) and loopback bypass it.
Runs trust the CA through `<run>/egress-ca.pem`, built from the host's CA roots
(`/etc/ssl/certs/ca-certificates.crt` on Ubuntu, from the `ca-certificates` package); if the
server logs that the roots were not found, install `ca-certificates`. Every Bash command of an
agent runs through `<run>/scrub-exec` (which runs `yulius --scrub-exec`) and every tool result
through the `PostToolUse` hook.
When a command is not run, or a tool result is withheld, for want of the run's agent-secret
fingerprints, the message says why (SPEC-353): **Yulius no longer accepts this session** means its
run ended (its `scrub.json` is then not read), so continue the task (or open the Superagent again)
to start a fresh run; **Yulius could not be reached** while the server is up, with no usable
`scrub.json`, usually means the sandbox's connections to it are used up (next section).

### Leftover agent processes

A process an agent's command leaves behind once its parent exits (a detached test job, a
`(server &)`, a headless browser) is adopted by its session's sandbox init (`bwrap`). No shell of
the agent owns it, so Claude Code neither shows nor stops it. On 2026-10-30, about 60 test jobs of
one project each polled `sleep 0.05` there for days (SPEC-353). Yulius handles them by itself
([ADR-0240](decisions/0240-a-run-s-strays-are-reaped-after-a-bound-and-a-sandbox-gets-half-the-proxy.md)):

- When a run ends, is stopped, or its task is deleted or archived, its whole PID namespace ends
  with it: leftovers included.
- While an idle task's session lives, each probe of its process tree (every few seconds and
  at each turn's end) lists them. The first sighting logs `leftover processes in the run`
  (task, processes, the first pid of each) as a warning; 2 hours later Yulius kills that
  process tree and logs `reaped leftover processes` (with the same pids). A working task's
  processes and the agent's own shells (under `claude`) are never touched.
- The Superagent's session is probed the same way every minute while its conversation is idle
  (SPEC-355): `leftover processes in the Superagent's session` (shell, sandbox, pids) when first
  seen, `reaped the Superagent's leftover processes` 2 hours later. A prompt written meanwhile
  keeps them. A terminal of the Shell panel is a person's own and is not probed.
- One sandbox gets at most half of the egress proxy's 256 connections, so its leftovers cannot
  take every other sandbox's downloads. Within a sandbox, one run's processes cannot lock the
  others out (SPEC-357,
  [ADR-0243](decisions/0243-the-busiest-run-gives-way-at-a-full-proxy-share.md)). At a full share
  with every connection active, the oldest connection of the run holding the most gives way to
  a run holding at least 2 fewer, and at the full proxy the busiest sandbox's does. A plain
  request waiting for its answer is never taken, but it waits at most 5 minutes with no progress
  for its upstream's response head, then gets `504` (`Yulius proxy: <host> sent no response for
  300 s`) and frees its place
  ([ADR-0250](decisions/0250-a-plain-http-request-waits-a-bounded-time-for-its-answer.md)). This is logged as `egress proxy: a sandbox's share
  is full of active connections; the busiest run's oldest connection was closed for another run`
  (sandbox, run, how many it held) or `egress proxy: every connection is taken and active; the
  busiest sandbox's oldest connection was closed for another sandbox`, each at most once a minute
  per sandbox. At a full share, a `503`
  goes only to a run when no other run of the sandbox has 2 more to give up (requests waiting for
  their answer are never taken), and it says how many
  the asking run holds (`this run holds 120 of them, the most of the sandbox's runs`). A run reads
  it in its own commands' output, so the task whose processes hold the places sees it. The
  journal names the run by its run (attempt) id, a CI job by its job id and a Superagent by its
  shell id. At the proxy's own limit, a sandbox gets `too many connections; try again` when no
  other sandbox has 2 more to give up. A request the proxy refuses anyway (a bad target, a local
  destination, a protocol upgrade, a policy refusal) takes no place, so it closes nothing. On
  2026-10-31, quartio-v2's tasks got `503` for every host for 90 minutes, while their Claude
  traffic, which bypasses the proxy (`NO_PROXY`), kept working.
- A full relay gives way instead of refusing (SPEC-355,
  [ADR-0241](decisions/0241-a-full-relay-closes-its-longest-idle-connection-and-the-superagent-restarts-in-place.md)).
  A sandbox reaches Yulius on one port (8081: internal API, MCP, egress proxy) through two relays,
  the holder's forward inside the sandbox and the server's `out.sock`, 256 connections each. At
  its limit a relay, and the proxy at a sandbox's share or at its own limit, closes its
  longest-idle connection (no byte for at least 5 minutes, longer than any call to Yulius waits)
  for the new one. Only when every connection was active within those 5 minutes is the new one
  refused (by the proxy, only once no busier run or sandbox can give one up, as above). A client
  that a relay refused sees
  `ECONNRESET`, as the Superagent's `/mcp` reconnect did on 2026-10-31. These events are logged as
  warnings, at most once a minute, in the journal: `egress proxy: at a limit, the longest-idle
  connection was closed for a new one`, the two busiest-run lines above, `egress proxy: a sandbox
  holds its share of the connections` (with the sandbox, the run and how many it holds), `egress
  proxy: every connection is taken` (with the asking sandbox and the total); these two refusal
  lines also carry `holders`, the busiest three with their counts (runs of the sandbox at a share,
  `holders=a1 (120), a2 (8)`; sandboxes at the proxy's limit; SPEC-366), and `sandbox relay:
  …` for the server's side of a sandbox's relay. The relay inside the sandbox writes its `sandbox
  relay: at the limit, the longest-idle connection was closed …` and `sandbox relay: a connection
  was dropped at the limit` lines to the sandbox's `holder.log` (and the Docker front's to
  `docker-front.log`), both in `<data>/runtime/sandboxes/<sandbox-id>/`.
- A refusal that remains shows on the panel (SPEC-361,
  [ADR-0246](decisions/0246-a-full-proxy-share-or-relay-shows-on-the-panel-once-per-trouble.md)).
  A sandbox at its proxy share, a sandbox whose `out.sock` dropped a connection, and the proxy at
  its own limit each begin a *connection trouble*: the journal says `connection trouble began`
  (limit, sandbox, held of max, the busiest runs or sandboxes by run id), and the people who can
  view the sandbox (everyone, for the proxy's own) get a warning pill in the tab row and one
  notification, naming the task whose run holds the most ("Proxy share full: quartio-v2", title
  "task #12's run holds 120 of them"). Refusals within it only update its counts. It ends, with
  `connection trouble ended`, once its limit falls to seven eighths (112 of a share of 128, 224 of
  256). `GET /api/connections` lists the open ones. To free the share, end the processes of the
  named task's run (its panel lists its background processes; the `nsenter … ss` recipe below shows
  their connections), or stop the task. The holder's forward inside the sandbox is not reported
  there: read its `holder.log`.
- Every minute Yulius tests each live Superagent's link from inside its sandbox, through the
  holder's forward. Two failed tests in a row log `the Superagent's sandbox cannot reach Yulius`, and its
  panel says so and offers **Restart Superagent**, which starts it again in place, resuming its
  conversation. Claude Code keeps a failed MCP server disconnected, so the panel also offers the
  restart once the link works again (`the Superagent's sandbox reaches Yulius again`).

- A Yulius restart or update needs nothing from the tasks (SPEC-358,
  [ADR-0244](decisions/0244-task-runs-reach-yulius-through-a-stdio-bridge-and-are-woken-after-an-outage.md)).
  - Their Yulius tools go through `yulius mcp`, a stdio bridge that holds a call while Yulius is
    down: its stderr in Claude Code's MCP log says `Yulius cannot be reached …; holding calls
    until it is back`, then `Yulius is back`.
  - `yulius wait` waits on until its deadline.
  - Yulius records the time it was last alive in `<data>/alive-at`, and in each run's
    `stop-answered` when it last answered the run's Stop. From 90 s after a start's runs are
    reattached until about 10 minutes after that, it logs `woken after Yulius's outage` for each
    idle task it told "Yulius is back; continue". `<data>/outage-pending` holds the outage being
    dealt with, so a second restart within that window still covers the first.
  - Sessions started before this release still have the HTTP config: if one says "MCP server
    yulius is not connected" after a restart, `/mcp` in its terminal (or continuing the task)
    brings it back.

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

#### A saturated sandbox relay

Signs: a Superagent panel that says it cannot reach Yulius; `the Superagent's sandbox cannot reach
Yulius` or `sandbox relay: a connection was dropped at the limit` in the journal or the sandbox's
`holder.log`; agents' calls to `127.0.0.1:8081` reset. Count the sandbox's connections to Yulius
with the `nsenter … ss` recipe above: about 256 in one sandbox means its relay is full, and the
pids show whose they are.

To free it without restarting Yulius:

1. End the leftover processes that hold the connections (the `kill` recipe above). Their
   connections close with them, and the relay has room again at once.
2. If the Superagent's Yulius tools stay disconnected, press **Restart Superagent** in its panel
   (or close and open it). It resumes the same conversation.

A Yulius restart does not free the relay. The holder (and its relay inside the sandbox) keeps
running, and it keeps each connection whose server end closed until its client closes it, which
a leftover process never does. The holder runs the relay it started with. If it started before the Yulius update that shipped
SPEC-355 (compare `ps -o lstart= -p "$H"` with the update's time), it still refuses when full, and
its Superagents' panels show no link state. In that case, end every process in the sandbox (stop
its tasks, close its Superagents and terminals, kill the leftovers) and the holder stops once it
has been idle for `YULIUS_SANDBOX_IDLE_MINUTES`. The next run starts a holder with the new relay.

Rotation: replace a value on the Secrets page after rotating it at its provider (the page marks
values older than 180 days). If `master.key` and a database copy may have leaked together,
rotate every team and agent secret at its source.

## 8. Upgrades

### Updates from main (the update helper)

**Restart without updating** (SPEC-432): **Settings → Instance → Restart…** (refused while a
deploy runs: a restart has no build to wait through) pauses every task as step 2 below does, asks the helper to `restart` (stop and start `yulius.service`, then wait until
`/internal/version` reports the same commit; nothing is backed up, installed or rolled back), and
resumes the tasks one by one. It needs the update helper; prefer it to `systemctl restart`, which
does not pause tasks. Restart… and Update now are refused, before a task pauses, on a host where
the server could not start again while a session runs
([Where sessions live](#where-sessions-live)); the answer names `sudo yulius host setup`. A task session whose Yulius MCP was dropped by Claude Code is reconnected by
Yulius after 3 silent minutes (it types `/mcp reconnect yulius` into the idle session), but only
when the drop is certain: its bridge had pinged and stopped, or a session this server spawned never
connected. A
sandbox keeps the `yulius` binary of the base it started on until it restarts, so after an update
its bridges may be an older build that does not ping; those are left alone (ADR-0280).

Yulius updates itself from `main` of **Quartio-Studio/yulius** (SPEC-001 OPS-006..OPS-009,
SPEC-002 ADR-18). It knows the commit it was built from (`yulius version`, and
`GET /internal/version` on the internal listener) and checks `main` every hour and whenever a
superadmin opens **Settings → Instance** (SPEC-149). A failed check says whether GitHub did not
answer in time, answered with an error (HTTP n, as during a GitHub outage) or could not be
reached at all; the next check then comes 5 minutes later (an hour after a rate limit), and an
open page shows its result without a reload (SPEC-443). Superadmins get a
notification and a banner, "Update available: …", and **Settings → Instance** lists the new
commits. Nothing updates by itself: a superadmin chooses **Update now** and confirms. Then:

1. **Build** (tasks keep working): the root helper fetches `main`, checks that the commit is on it
   and not older than the commit it installed last, and runs `cargo build --release --locked -p
   yulius` with `YULIUS_BUILD_COMMIT` set. Several minutes; the page shows the progress.
   Once built, a deploy (a CD run or a delivery environment run) still running is waited for
   before anything pauses: the page says "Built. Waiting for N deploy(s) to finish before pausing
   tasks" and links each one. From then until the install ended, new deploys are refused ("nothing
   ran: Yulius is being updated; …"), also an agent's `cd_deploy`. A deploy still running past the
   longest a deploy can last (twice the CI job timeout or the job timeout plus 35 minutes of
   preparation, whichever is longer, plus 5 minutes: 70 minutes by default) cancels the update
   (SPEC-342). A crash or a manual restart still ends a running deploy as `error`.
2. **Pause tasks**: every task's agent is frozen at once with the cgroup v2 freezer (its process
   tree moves into `…/sandboxes/<sandbox>/agents/run-<attempt>` and that group is frozen; the
   ptyhost, shells and browsers keep running) and new starts queue. The agents are told nothing.
   This needs the delegated cgroups of §3 (`Delegate=yes`, `DelegateSubgroup=server`, and the
   sessions unit where the host has it); a run that
   cannot be frozen pauses at its next turn boundary instead (its Stop or `UserPromptSubmit` hook
   is held, as for a capacity pause), and while such a run is still in a turn nothing is frozen:
   the others freeze once it is held (or at once on **Continue without waiting**). A task still in a turn after 30 minutes cancels the update
   (nothing changes); the superadmin can also **Continue without waiting** or **Cancel**. A cancel
   while building stops the build: within about a second once it compiles (the helper sees its
   request gone and kills cargo with every compiler it started), or before compiling starts when
   it is still fetching; nothing is staged (ADR-0283). A cancel that lands while rustup installs a
   new toolchain for the build can leave it half written: `rustup toolchain uninstall <version>`
   (as root) repairs it, and the next build installs it again. To check
   a freeze on a host: `cat /sys/fs/cgroup/yulius.slice/yulius-sandboxes.service/sandboxes/*/agents/run-*/cgroup.events`
   (`sandboxes/*/run-*` without the `cpu` controller) shows `frozen 1` while the update restarts.
   A server that starts thaws every run group left frozen, but those of tasks a person paused
   (SPEC-191: it freezes them again; `SELECT number, paused_by FROM tasks WHERE paused_by IS NOT
   NULL` lists them); to thaw them by hand (a binary without
   this, SPEC-160): `for f in /sys/fs/cgroup/yulius.slice/yulius-sandboxes.service/sandboxes/*/{,agents/}run-*/cgroup.freeze; do echo 0 > "$f"; done`. A response streaming from the model provider at the
   restart is cut (the egress proxy restarts with the server) and Claude Code retries it.
3. **Restart**: the helper first checks that the unit would start again
   ([Where sessions live](#where-sessions-live)) and fails the request with nothing stopped
   otherwise. It then stops `yulius.service` (agent sessions and
   terminals keep running in `yulius-sandboxes.service`), copies the binary and `yulius.db` (with its WAL) to
   `/var/lib/yulius-update/backups/<run>/`, installs the new binary, starts the service and waits
   up to 2 minutes until `/internal/version` reports the new commit. If it does not, it stops the
   service, restores the previous binary **and database**, and starts them again ("rolled back").
   Once the new version is healthy, the helper runs **its** `yulius host check --json` (at most 2
   minutes; SPEC-203). That re-checks what sandboxes need for Docker, with only safe, idempotent
   fixes: the needrestart rule, the host's own Docker units, the modules file and loading, the
   binary's owner. It also starts bwrap in a user namespace ("bwrap namespaces") and reports,
   never fixes, a host where that fails (after a release upgrade, for example). A failed item, or a "sandbox Docker" that does not hold, is a regression. The
   update's outcome says it in one line ("Sandboxes may have lost Docker with this update" when
   the report's "sandbox Docker" does not hold; "Sandboxes still get Docker; the host check after
   it left N items to fix" when only other items failed), and the Sandbox Docker card lists the
   items of the latest root check (naming `sudo yulius host setup` when Docker held), never only
   in a log (SPEC-227).
   A helper unit installed before SPEC-203 keeps `/etc` read-only, so its re-check fails the
   needrestart rule, the modules file and "update helper" until `sudo yulius host setup` runs once.
   A regression does not roll the update back. The report is kept in
   `/var/lib/yulius-update/host-check.json`.
4. **Resume**: the server that started releases and thaws the tasks one by one; they go on as if
   nothing happened, except the tasks a person paused, which stay paused until they resume them.
   A server that starts with no update open thaws any run group left frozen but those.

The service user cannot replace `/usr/local/bin/yulius`, and it restarts only through the helper.
A root-owned **path unit** starts the helper when the server writes
`/var/lib/yulius/update/request.json`; the request can only say "build" or "install" one commit,
or "restart" the installed one (SPEC-432: the service can have root stop and start it, nothing
more). Everything else (repository, paths, service) is in the root-owned unit. The helper refuses a request that is not a regular file owned by the
service user, larger than 16 KiB, malformed, older than 15 minutes or already handled. It reports in
`/var/lib/yulius-update/status.json`, which the server reads.

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
# The re-check after an install (`yulius host check`, SPEC-203) keeps the kernel modules list
# and the needrestart rule.
ReadWritePaths=/etc/modules-load.d -/etc/needrestart/conf.d
UMask=0022
```

A helper unit installed before SPEC-203 lacks the second `ReadWritePaths` line. `yulius host
setup` adds it with a drop-in (`yulius-update.service.d/10-yulius-host-check.conf`), so the unit's
local edits stay.

No sudo rule or polkit rule is needed: the path unit is the only way in, and it starts a fixed
command. The build needs Rust through rustup for root (the checkout's `rust-toolchain.toml` picks
the version), `git`, a C linker, and network access to GitHub and crates.io.

**Reading the repository.** The repository is private. Yulius reads it, and hands the helper a
short-lived token for its fetch, with the GitHub App's installation token when the App is
installed on Quartio-Studio with access to `yulius` (Contents: read). Otherwise put a token that
may read the repository (a fine-grained personal access token with Contents: read on that one
repository) in a file:

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

**Logs and recovery.** `journalctl -u yulius-update` shows each step. A failed rollback (the
helper says "restore by hand") leaves the previous binary and database in
`/var/lib/yulius-update/backups/<run>/`: stop `yulius`, copy `yulius` to `/usr/local/bin/yulius`
and `yulius.db` (and `yulius.db-wal`, if present) to `/var/lib/yulius/` owned by `yulius`, mode
0600, remove `yulius.db-shm`, start `yulius`. The helper keeps the last 3 backups. A manual build
should set the commit it is built from: `YULIUS_BUILD_COMMIT=$(git rev-parse HEAD) cargo build
--release --locked -p yulius` (a checkout does it by itself; a tarball has no Git).

### The move helper (moves from Settings → Instance)

**Settings → Instance → Move to another host** ([Move from Settings → Instance](#move-from-settings--instance),
SPEC-470, ADR-0305) asks root for `yulius instance migrate` the way updates ask for an install:
the server writes `/var/lib/yulius/migrate/request.json`, a root-owned **path unit** starts
`yulius migrate-helper`, and the helper runs the command. The request can only say `plan`,
`move` or `abort` and name the new host, its SSH port, a public URL and data disks; there is no
field for anything else, and nothing secret is in it. Everything else (the data directory, the
units, the state directory) comes from the root-owned unit.

**What enabling it allows.** With the path unit enabled, the service user can have root move the
instance to a host where a person has added the move's one-time key to root's `authorized_keys`,
and to no other: the helper runs the command with `--no-sign-in`, so nothing root has (its keys,
its agent, the keys and proxies its ssh configuration names, a ticket) is offered to the new host
or read on the way. The service user already reads all of the instance's data; what a takeover of
the server process gains through the helper is the host's files a move carries (the unit's
environment and key files, the proxy's configuration, `/usr/local`, root's Rust toolchain) and the
stop of this instance, on a host whose root let the move in. So `yulius host setup` does not
install the helper: root does, on purpose. Two ways to limit it further: enable the path unit when
you want to move from the page and disable it afterwards (`systemctl disable --now
yulius-migrate.path`), or pin the hosts a request may name with `YULIUS_MIGRATE_TARGETS` in the
service unit. A move carries the units and enables the path unit on the new host only when it was
enabled on the old one.

**Install the helper** (once, as root; the unit files are in the repository under
`deploy/systemd/`; a second instance names them after its own unit, `<unit>-migrate`):

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

The unit has no start timeout (a copy takes as long as the data needs) and no file system
sandboxing: the command reads the whole host, writes the proxy's configuration and freezes units,
as it does from a root shell. A stop of the unit, or a shutdown, reaches the helper alone
(`KillMode=mixed`), which tells the command with SIGTERM: before the new instance's start is
ordered it thaws the old instance and ends; after that it finishes the cutover, for which it has
ten minutes. What is left of the unit's processes then is killed. `StartLimitIntervalSec=0` keeps the helper
listening when requests come quickly: with systemd's default (5 starts in 10 s) a sixth request
fails the path unit as well, and a failed path unit takes no request until root runs `systemctl
reset-failed yulius-migrate.service yulius-migrate.path` and starts it again.

**What the helper checks.** The request must be a regular file (not a link) owned by the data
directory's owner, who is not root, at most 16 KiB, with exactly the fields above, at most 15
minutes old, and an id it has not acted on: it keeps every id it handled in `handled` until that
request would be refused as stale, and when it cannot read or write that file it runs nothing and
the unit fails. Every value is checked again with the rules the server used: the host as `ssh`
takes it (`root@host` or `host`, nothing that reads as an option), a port from 1 to 65535, a public
URL that is `https://host` and nothing more, at most two different disks under `/dev/`; an abort
names none of them. It refuses while the update helper's unit runs, and a host outside
`YULIUS_MIGRATE_TARGETS` when that is set. A refused request changes nothing; the page shows the
reason, which never repeats the request's content. An accepted one runs `yulius instance migrate
--no-sign-in --request-id=<the request's id>` with each value as one argument (no shell): the plan
without `--yes`, the move with it, the abort as `--abort` and never with `--yes`. While the command
runs, an abort request ends it with SIGTERM and is then handled. The helper does not touch the
command's checklist: a run the command refuses (a move to another host is recorded, another run
holds its lock) changes nothing, and the page shows the recorded move's checklist as that move's:
the command writes the request's id into its checklist, so the page knows which run a checklist is
of.

**Files.** In `/var/lib/yulius-migrate` (root's, mode 0711): `helper.json` (0644: the request it
took, `running` with a heartbeat every 5 s, then `ended` with the command's exit code, or `refused`
with the reason), `handled` (0600: the ids it acted on), the command's `status.json` (0644: the
phase and the checklist, with the key line while a person has to add it, the new URL once decided,
and the id of the request the run answers), `state.json` and the one-time key (0600), and on a host
a move arrived at `arrived.json` (0644: that move's finished checklist). None holds a secret value.
The server reads them and stores nothing about a move: its database travels with the copy.

| Setting (flag / environment) | Default | Meaning |
| --- | --- | --- |
| `--migrate-state-dir` / `YULIUS_MIGRATE_STATE` (server and helper) | `/var/lib/yulius-migrate` | Where the helper and the command report, and where a move to this host left its checklist. |
| `YULIUS_MIGRATE_TARGETS` (helper's unit) | unset | The only hosts a request may name, separated by commas; unset or empty: any host. |
| `YULIUS_MIGRATE_SERVICE`, `YULIUS_MIGRATE_UPDATE_SERVICE` (helper's unit) | `yulius.service`, `yulius-update.service` | The server's unit and the update helper's, as the command takes them. |

**Logs.** `journalctl -u yulius-migrate` shows each request (its action, host and id, or why it
was refused) and the command's own output. "The move helper did not take the request" on the page
means the path unit is not enabled or could not start the service: `systemctl status
yulius-migrate.path yulius-migrate.service`. The path unit starts the helper on a change of the
request file; a request written in the moment the helper ends another run starts nothing, so the
server writes an untaken request again from 5 s on while the page waits for it. A helper that
stopped without a word (it was killed, the host restarted) is said on the page after a minute; the
move is continued or aborted from there, or with the command.

### Notes for specific upgrades

- Upgrading to flags that say why (SPEC-472, ADR-0306, 2026-11-12): migration 0053 adds
  `tasks.attention`. Nothing to do. A task that was flagged "Needs your action" before the update
  has no reason kept: its bar reads "The agent needs your attention. Look at its terminal below."
  until that flag ends (a typed message, or the turn's end), and a flag that Claude Code's own
  question raised before the update is not yet lifted by the agent's work. Flags raised from then
  on say why, and keep saying it across later updates.
- Upgrading to sessions in their own unit (SPEC-458, ADR-0298, 2026-11-11). **On a host with
  systemd 258 or later (Ubuntu 26.04: `systemctl --version`), do the first step before Update
  now.** On systemd 257 and earlier (Ubuntu 24.04) the update needs nothing first; run `sudo
  yulius host setup` after it, at any time.

  Why: the update is installed by the helper of the version before it. It stops `yulius.service`
  while the sessions are still in that unit's group, and systemd 258 and later cannot start such
  a unit again (the outage of 2026-11-11). With the three units of
  [Where sessions live](#where-sessions-live) in place, the start that follows the install first
  runs `yulius-adopt.service` with the new binary, which moves the sessions out.

  1. As root, before the update. This restarts nothing and ends nothing:

     ```sh
     # From a checkout of the new version; or write the three files from the texts of §3.
     install -m 0644 deploy/systemd/yulius-sandboxes.service deploy/systemd/yulius-adopt.service /etc/systemd/system/
     install -D -m 0644 deploy/systemd/yulius.service.d/20-yulius-sessions.conf /etc/systemd/system/yulius.service.d/20-yulius-sessions.conf
     systemctl daemon-reload
     systemctl start yulius-sandboxes.service
     systemctl show yulius.service --property=Wants    # names yulius-adopt.service
     ```

     The files are for `yulius.service`, the user `yulius` and `/usr/local/bin/yulius`, which
     must be a regular file only root may write, in directories that are root's (the adopt
     unit runs it as root). With other values, let a binary of the new version write them:
     `sudo <that binary> host setup`. Without the `daemon-reload` systemd does not know the
     files, and Restart… stays refused; `sudo yulius host setup` of the new version does it.

     **Between this step and the update the host is not safe yet**: the installed binary is
     still the old one, which has no adopt step, so `yulius-adopt.service` fails with it and a
     restart, a crash or Restart… still leaves the server down while sessions run, as before.
     Do the update next; the way back below applies until then.
  2. **Update now**, as always. `journalctl -u yulius-adopt` then says "moved N processes in M
     groups from /sys/fs/cgroup/system.slice/yulius.service to
     /sys/fs/cgroup/yulius.slice/yulius-sandboxes.service", and `yulius doctor` says "restart
     with sessions: sessions live in yulius-sandboxes.service".
  3. `sudo yulius host setup` once, afterwards: it brings the files to what the installed
     version writes and names the sessions unit in the needrestart rule.

  What changes with it:

  - Every sandbox Docker engine is killed once, at that first start, with its containers. They
    get no shutdown (as at a power loss: stop a container that must close its files first); the
    engine starts again on demand. Sessions, terminals and browsers go on.
  - cgroup paths in this guide are those of the sessions unit. On a host without the units they
    start with `/sys/fs/cgroup/system.slice/yulius.service/` instead.
  - Limits that bound all sandboxes together are the sessions unit's now. `TasksMax=`,
    `MemoryMax=` or `CPUQuota=` set on `yulius.service` for the sessions' sake belong on
    `yulius-sandboxes.service`: `sudo systemctl set-property yulius-sandboxes.service
    TasksMax=…` applies at once (never restart that unit for it), and the server reads the task
    limit at its next start. Both units get systemd's default `TasksMax`, so nothing changes
    without such a setting. A move (`yulius instance migrate`) carries drop-ins you wrote under
    `/etc/systemd/system/yulius-sandboxes.service.d/`, not what `set-property` wrote: set those
    again on the new host.
  - **Rolling back to an older binary** leaves the units in place. Sessions already moved stay
    in `yulius-sandboxes.service`; the older binary starts new ones in `yulius.service`'s group
    again, and `yulius-adopt.service` fails with it ("unrecognized subcommand"), which the server
    survives. On systemd 258 and later do not restart that binary while sessions run: update
    again.

  **The way back** when the update, or a `systemctl restart yulius`, was done without the units
  on systemd 258 or later: the server does not start (`Failed to spawn executor: Device or
  resource busy`, in a loop), the helper reports "Could not start yulius.service … The rollback
  failed too … Restore by hand from /var/lib/yulius-update/backups/…", and the sessions are
  still running. Nothing needs restoring: the helper put the previous binary and database back
  before that last start. As root, do step 1, then run the adopt step of a binary of the new
  version by hand (the helper keeps the build it staged) and start the server:

  ```sh
  /var/lib/yulius-update/staged/yulius host adopt-sessions --service yulius.service
  systemctl reset-failed yulius.service; systemctl start yulius.service
  ```

  The version that was installed before runs again with its sessions. Then Update now once more.
- Upgrading to host setup's bwrap rule (SPEC-456, ADR-0296, 2026-11-11): nothing to do on an
  Ubuntu 24.04 or Debian host whose sandboxes work. A hand-written `/etc/apparmor.d/bwrap` keeps
  working and is reported as adopted; `sudo yulius host setup` changes nothing there. On Ubuntu
  26.04 run `sudo yulius host setup` once even when sandboxes work with a hand-written `bwrap`
  profile: the package's `bwrap-userns-restrict` declares the same profile name and whichever
  loads last wins, so until setup has disabled the shipped one `yulius host check` and Settings →
  Instance report the item as failed. A host whose sandboxes fail with `setting up uid map:
  Permission denied` or a denied mount now says so in `yulius host check`, Settings → Instance
  and `yulius doctor --contained`: run `sudo yulius host setup`. No migration.
- Upgrading to host-sized resource limits (SPEC-459, ADR-0294, 2026-11-11): migration 0051 adds
  the limits record. A limit the unit does not set is now sized from the host instead of the
  fixed 8 sessions, 4 CI jobs, 2 lanes, 100 GB and 20 GB (the same numbers on a 12-CPU host with
  a data disk up to 400 GB; more on a bigger one). A value the unit sets keeps working and is
  recorded for the host of the first start; remove it from the unit to follow the host. See
  [Resource limits](#resource-limits).
- Upgrading to 1 GiB uploads (SPEC-325, ADR-0219, 2026-10-26): the default upload limit is now
  1 GiB. Raise the reverse proxy's request body limit to 1100 MB (§4: Caddy `max_size 1100MB`,
  nginx `client_max_body_size 1100m`) and reload it, or files over the old proxy limit still fail
  with 413 ("larger than the server accepts"). To keep the old limit, set
  `YULIUS_UPLOAD_MAX_MB=25`. No migration.
- Upgrading to named CD environments (SPEC-257, ADR-0195, 2026-10-21): migration 0041 gives each
  CD run its `stage`; runs of `dev` and `prod` keep theirs. `[cd.dev]` and `[cd.prod]` work as
  before; nothing to configure.
- Upgrading to remote control (SPEC-247, ADR-0188, 2026-10-20): migration 0040 makes every
  existing API token a `cli` token: the REST API as before, never the remote MCP. A person's
  laptop agent needs a new `operate` or `read` token from Settings → Connect your local agent
  ([remote-control.md](remote-control.md)).
- Upgrading to agent secret scopes (SPEC-245, ADR-0186, 2026-10-20): migration 0039 turns each
  team-wide agent secret into a global one, switched on for every live sandbox, so no run loses
  one. A project added later gets a global secret only when an administrator switches it on
  (Secrets → Global). Project and team secrets are unchanged.
- Upgrading to MCP server origins (SPEC-218, 2026-10-18): migration 0037 marks the servers the
  audit log records as imported from a repository's `.mcp.json`; their Test never runs on the
  host. A person who changes such a server's command takes it over.
- Upgrading to agents setting secrets up (SPEC-216, ADR-0171, 2026-10-17): migration 0035 keeps
  every existing agent secret on a person's confirmation (`auto_allow` off; new ones default to
  on), and the first start copies each team-wide team secret into every sandbox (audited
  `vault.team.assign`; [architecture](architecture.md#agent-secret-hosts)).
- Upgrading to leftovers cleanup (SPEC-215, task #202, ADR-0170): no migration. The first start
  of the new version deletes the existing safe leftovers two minutes in, the removed product
  sandbox's included (its worktrees, CI checkouts and system layer, more than 100 GB). Nothing is
  needed on the host. On the reference VM, after promotion to `main` and **Update now**:

  ```sh
  curl -s http://127.0.0.1:8081/internal/version         # the new commit
  df -h /var/lib/yulius                                  # before: 81 % (312 of 386 GiB)
  sudo -u yulius yulius leftovers list                   # what is left after the first round
  ```

  Then:
  1. Settings → Instance → Leftovers: note the reclaimable total, then **Refresh** a few minutes
     after the start. The product sandbox's `worktrees`, `ci` and `sysroots/6b76ccc7-…` are no
     longer listed, and its "Everything else of a removed sandbox" item stays until you delete it.
  2. `df -h /var/lib/yulius` is well below 81 %, and `ls /var/lib/yulius/sysroots` no longer shows
     `6b76ccc7-869e-4e5e-871e-c944824fe373`.
  3. The audit has `leftovers.delete` entries by `yulius`, and no `leftovers.failed` entries.
  4. Done tasks (#191–#194) keep their worktrees but not their `target/` (half an hour after they
     last moved).
  5. `/var/lib/docker` and `/var/lib/containerd` are listed as report only. Remove them as root
     only if nothing else on the host uses Docker.

- Upgrading to the host setup (SPEC-203, task #188, ADR-0162): no migration. The helper that
  installs this version is the previous one, so this update is not re-checked; the next ones are.
  On the reference VM, after promotion to `main` and **Update now**, as root:

  ```sh
  curl -s http://127.0.0.1:8081/internal/version     # the new commit
  yulius host setup --dry-run                        # what it would change
  yulius host setup
  ```

  Expected: `ok` for Docker Engine (docker-ce 29.9.1 from `docker.list`), the kernel modules
  file, the service user, its subordinate ids (set by hand) and the binary. Expected
  `changed`: the needrestart rule (new), the update helper's drop-in (new), and the host Docker
  units if `docker.socket` is still active (`Disabling 'docker.service', but its triggering units
  are still active` on 2026-10-16). Also `changed`: the sandbox base, if the host changed since
  `base-9`. Then "sandbox Docker" `ok`, the contained doctor, and the smoke test's lines ending
  `sandboxes get Docker on this host`. A second `yulius host setup` prints `nothing to change`.
  Then:
  1. Settings → Instance → Sandbox Docker: "sandboxes get Docker". The product sandbox is listed
     on its own `docker.io` with the way back (it ran 29.1.3).
  2. **Run smoke check** on that card: each check appears, and it ends "Smoke check passed".
  3. Reboot once (a kernel upgrade is pending), then `lsmod | grep -E '^(overlay|veth|xt_MASQUERADE)'`
     lists them; `systemctl is-enabled docker.socket docker.service containerd.service` says
     `disabled`; `yulius host setup` prints `nothing to change`; and in a sandbox,
     `docker run --rm hello-world` works.

- Upgrading to CD (deployment) (SPEC-134, task #113): migration 0027 adds `cd_runs` and
  `tasks.needs_you`. Nothing deploys until a project defines `[cd.dev]` or `[cd.prod]`. The agent
  tool `delivery_promote` is gone: production, with its promotion, is a person's on the CD page.

- Upgrading to CI jobs with CPU priority (SPEC-053, #283, 2026-10-13): no migration and no
  configuration. The next time a sandbox is prepared, its cgroup gets `system/`, `agents/` and
  `ci/`. Processes still in the sandbox's group (a holder and runs from before the update) move
  to `agents/`, and the `cpu` controller then passes down to the leaves. Check it with
  `cat /sys/fs/cgroup/yulius.slice/yulius-sandboxes.service/sandboxes/<sandbox>/ci/cpu.weight` (400).
  A holder that was already running keeps its `slirp4netns` in `agents/` until the holder stops
  when idle. **Rolling back to an older binary:** a server that stops gracefully undoes the split
  (`-pids`, then `-cpu`), so the older binary can join the sandbox groups again. After a crash or a
  forced stop, undo it by hand before starting the older binary. Otherwise its holders and runs
  fail with "Device or resource busy":
  `for f in /sys/fs/cgroup/yulius.slice/yulius-sandboxes.service/sandboxes/*/cgroup.subtree_control; do echo -pids > "$f"; echo -cpu > "$f"; done`.
- Upgrading to host-sized task limits (SPEC-268, 2026-10-22): no migration. At its next start the
  server gives each sandbox a quarter of the host's task table, at most three quarters of
  the `pids.max` of the unit the sandboxes live in (`yulius-sandboxes.service` since ADR-0298,
  `yulius.service` before; systemd's `DefaultTasksMax` is 15 % of the table unless the
  unit sets `TasksMax=`; raise it there for bigger sandboxes), and `ci/` and `engine/` 3/8 of a
  sandbox's;
  the log line "per-sandbox limits on" shows the figure. An explicit `YULIUS_SANDBOX_PIDS` stays.
- Upgrading to a common task limit for all sandboxes (SPEC-272, task #260, 2026-10-22): no
  migration. At every start the server writes `<unit>/sandboxes/pids.max`: three quarters of
  that unit's own `pids.max` (43 320 under the VM's default 57 763, so the server kept
  14 443 while it shared the unit; since ADR-0298 the unit is `yulius-sandboxes.service`), or
  `max` when the unit has `TasksMax=infinity`. Check it with
  `cat /sys/fs/cgroup/yulius.slice/yulius-sandboxes.service/sandboxes/pids.max`. An explicit
  `YULIUS_SANDBOX_PIDS` above it is not capped, but the common limit binds each sandbox too: the
  server logs a warning naming both. **Rolling back to an older binary:** it leaves
  `sandboxes/pids.max` as it is. That only keeps the server its share, but the older binary does
  not count a hit there on a 6.13+ kernel (a session that dies at it reads "The agent stopped
  unexpectedly") and does not follow a later change of `TasksMax=`. On such a kernel, or before
  changing `TasksMax=` under the older binary, clear it:
  `echo max > /sys/fs/cgroup/yulius.slice/yulius-sandboxes.service/sandboxes/pids.max`.
- Upgrading to tasks acting as their creator on GitHub (GIT-003, #188, 2026-10-11): no migration
  and no configuration. The agents' Git switches to the creator's token at its next credential
  request; a run gets `gh` logged in as the creator when it is started or continued (`gh` itself
  is a host prerequisite, §1). Task worktrees commit as
  `{id}+{login}@users.noreply.github.com` from their next run. Someone whose GitHub sign-in can no
  longer be refreshed signs in with GitHub again; until then their tasks act as the App, and the
  task's terminal says why.
- Upgrading to sessions as documents (#137, 2026-10-10): migration 0011 adds `session_docs`,
  `session_chunks` and the FTS5 index `session_fts` (SQLite's bundled FTS5). Existing data is kept:
  tasks imported earlier as archived tasks stay tasks (their sessions appear on the Sessions page as
  task sessions). Task sessions are indexed lazily, 32 MiB of transcript per list or search, so the
  first searches after the upgrade on a busy sandbox index while they answer. No configuration.
- Upgrading to sandbox isolation (ADR-22, 2026-10-10): install the packages, add the subordinate
  range and the two unit lines, and build the base as in [Sandbox isolation](#sandbox-isolation),
  then restart. Sessions and shells started before the upgrade keep running as they are (their
  launch records have no holder) and are adopted as usual. New sessions in the same sandbox join
  its holder. A dev server an old session still runs keeps its host port until it ends; the
  relay skips that port meanwhile. Processes left in the unit's cgroup from before delegation are
  moved to `legacy/`. Files written in either mode belong to the service user, so repositories,
  worktrees and homes need no migration. Each sandbox's system layer starts empty. No database
  migration.
- Upgrading to the instance Vidoc key (SPEC-195, 2026-10-16): migration 0034 adds the instance's
  sealed Vidoc key. Without a key nothing changes. A project's agent secret `VIDOC_API_KEY` keeps
  working, and wins over the instance key.
- Upgrading to updates from main and the Jev key in settings (ADR-18, 2026-10-10): migration 0008
  adds `update_runs` and the instance's sealed Jev key. Install the update helper (above). A
  configured `YULIUS_JEV_API_KEY_FILE` keeps working ("from config"); Settings → Instance → **Save
  it here** moves the key into settings, then the variable can go.
- Upgrading to the secrets vault (ADR-20, 2026-10-10): migration 0009 adds the vault tables. New
  agent runs route HTTP(S) through the egress proxy and redact agent secrets; runs started earlier
  keep their environment until they are continued. Make sure `ca-certificates` is installed.
- Upgrading to default workflows (AUT-006, #136, 2026-10-10): the default workflows now ship with
  the binary (`catalog/workflows/`) and are read-only. On the first start the "Feature" and "Quick
  task" rows seeded by earlier releases are retired (audited as `workflow.retired`); an edited
  "Feature" is kept as "Feature (custom)", an edited "Quick task" stays as it is. Automations that
  used "Quick task" continue without a workflow. No migration and no configuration.
- Upgrading to dropped files (EDT-006, UI-048, 2026-10-10): raise the reverse proxy's request body
  limit to 32 MB (§4) or uploads between 16 and 25 MiB fail at the proxy with 413. No migration.
- Upgrading to usage from the sessions and session-end tracking (F 2026-10-10 items 1(a)/(c)): no
  migration and no configuration. Every run started or continued after the upgrade gets Claude
  Code's `statusLine` and a `SessionEnd` hook in its settings; sessions already running keep their
  settings until they are continued, so their subscription's usage appears once a new run starts
  on it. The old "the provider refused this account's token for usage (HTTP 403)" note disappears
  within the first background usage round (2 minutes after the start). A task can now also end
  as `stopped_killed` (its process was killed outside Yulius).
- Upgrading to the subscription pool (ADR-17, 2026-10-10): migration 0005 drops the accounts'
  priority and enabled switch, so **a disabled account joins the pool**: remove accounts you do not
  want used before restarting. Tasks that were waiting for capacity are handed to the new resume
  loop. Sessions started before the upgrade keep their old hook settings (15 s timeout): the CLI
  ends a held hook after 15 s and the agent goes on until the CLI reports a limit, where it pauses;
  sessions started or continued after the upgrade get the long timeout. No configuration is needed.

- Upgrading to check fallbacks (#111, 2026-10-10): new Apps request Commit statuses: read. An App
  created earlier lacks it, so its pull requests' checks come from check runs only, and the Git page
  says so to instance administrators. Add the permission as described in
  [GitHub App permissions](#github-app-permissions). No data migration.
- Upgrading to the streamed browser (ADR-15, 2026-10-10): install `xvfb`, `ffmpeg` and the fonts
  (§1) before restarting; `yulius doctor` must show them. No data migration: logins in the
  sandbox Chrome profiles stay; displays and cookies live in `<data>/displays/`. Running
  browsers restart headful on next use. The preview DNS and TLS are now optional (§4).
- Upgrading to the live browser panel (ADR-14, 2026-10-09): over HTTPS the session cookie is
  now `__Host-yulius_session`, so everyone signs in once more after the upgrade. Add the preview
  DNS and TLS configuration (§4) for live pages.

By hand (without the helper): replace the binary and `systemctl restart yulius`. Live sessions keep running in their detached
hosts; the new server reconciles them on start (reattaches live runs, marks dead ones stopped,
never respawns). Users' terminals keep running too and are adopted again before the server
serves; a terminal whose host is gone (a machine reboot) or whose sandbox was removed is cleaned
up. Queued tasks start when capacity allows.

## 9. Security notes

- The data directory is owner-only (0700); secrets are sealed with `master.key` (XChaCha20-Poly1305);
  vault secrets under envelope encryption (per-secret data keys wrapped by a key derived from
  `master.key`), with re-authenticated, audited reveals (ADR-20).
- Agent secrets are never given to agents: the egress proxy adds them for their bound hosts and
  every channel an agent reads is redacted. Remaining threats (a bound host echoing a credential
  in another form, misuse on its hosts, weak values): SPEC-002 ADR-20.
- Agents receive per-run task tokens (MCP, hooks) and, through the credential helper and `gh`,
  their task creator's GitHub token (8 h, capped by that person's access within the App's
  installation; an agent could read it and act as them there until it expires), or without one a
  token restricted to their sandbox repository; provider credentials reach the agent process
  only through its environment (SPEC-002 §8, R-08).
- Known open risks are listed in SPEC-002 §12 (R-07, Chromium's unauthenticated DevTools port, is
  closed by ADR-0191: the sandbox browser speaks DevTools over a pipe and listens on no port; R-09,
  environment commands outside containment, is closed by ADR-0175).
- Isolated sandboxes cannot reach the host's loopback services (Yulius's public listener, Caddy's
  admin API, databases bound to 127.0.0.1). Services bound to the host's public
  addresses remain reachable like any internet host.
- Password sign-in is limited per client only with the proxy's shared secret configured
  (`YULIUS_PROXY_SECRET_FILE`, §4 "Per-client sign-in limits"); without it every client behind the
  proxy shares one limit.
