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
| Claude Code (`claude`) | Claude tasks and Claude account login | verified with 2.1.292. |
| GitHub CLI (`gh`) ≥ 2.82.1 | `gh` in every sandbox, logged in as the person (their shells) or the task's creator (agents) | install it on the host from [GitHub's package repository](https://github.com/cli/cli/blob/trunk/docs/install_linux.md), not the distribution's package: Ubuntu 24.04 ships 2.45, and since GitHub retired Projects (classic) `gh pr edit` and `gh issue view --comments` fail with a GraphQL error before 2.82.1. Sandboxes copy the host system, so rebuild the base afterwards (`yulius sandbox-base build`). `yulius doctor` warns without it or below 2.82.1. |
| bubblewrap (`bwrap`) | containment (`--contained`) | strongly recommended in production; without it runs are marked "not contained". `yulius host setup` installs it. |
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

Two commands on a fresh Ubuntu 24.04 (or Debian) host: install the binary, then run the host
setup as root (SPEC-203, [ADR-0162](decisions/0162-one-host-setup-command-brings-and-keeps-docker-in-sandboxes.md)).

```sh
cargo build --release -p yulius            # from a checkout; output: target/release/yulius
sudo install -m 0755 target/release/yulius /usr/local/bin/yulius
sudo yulius host setup --public-url https://yulius.example.com
```

`yulius host setup` prints one line per item, `ok` (left alone), `changed`, `manual` (with the
reason) or `FAIL`, then a summary; it exits 1 when an item failed. In order:

1. **needrestart**: `/etc/needrestart/conf.d/yulius.conf` keeps package installs from restarting
   `yulius.service`, written before any install. A restart outside an update pauses no task.
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
8. **yulius.service**: written from §3 and enabled when missing (`--public-url` is needed then).
   Otherwise, a drop-in `yulius.service.d/10-yulius-sandboxes.conf` adds `Delegate=yes`,
   `DelegateSubgroup=server` and `KillMode=process` if they are missing. The restart that applies
   it is manual: run it when it suits. A unit file systemd has not loaded is never overwritten.
9. **update helper**: when `yulius-update.service` is installed, a drop-in lets its re-check
   write `/etc/modules-load.d` and `/etc/needrestart/conf.d` (§8).
10. **yulius binary**: the running binary and the unit's, links resolved, are made root-owned,
    writable by root alone. A binary in a directory the service user or others may write fails
    instead: fixing the file would not stop it being replaced.
11. **sandbox base**: built now, as the service user, when there is none, when it lacks the
    host's `dockerd`, or when it no longer matches the host. Live sandboxes' bases are kept
    ([ADR-0161](decisions/0161-a-base-a-live-holder-mounts-is-never-collected.md)), so this is
    safe while sandboxes run.
12. A unit it wrote is started; then "sandbox Docker", the verdict `yulius doctor` gives; the
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
- Claude Code: it is installed system-wide, and its version is yours to choose (§1);
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
default `KillMode=control-group`, stopping the service would kill every running agent and terminal.

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
Environment=YULIUS_MAX_SESSIONS=8
Environment=YULIUS_CONTAINED=true
# Largest file dropped onto the editor or a terminal, in MiB (keep the proxy's body limit above it).
Environment=YULIUS_UPLOAD_MAX_MB=25
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
# Yulius-run CI (see "CI on this machine" below): jobs at once, runs at once per sandbox, minutes
# per job, seconds between checks.
Environment=YULIUS_CI_MAX_JOBS=4
Environment=YULIUS_CI_LANES=2
# CPUs an agent session, terminal or Cockpit plans for (YULIUS_CPUS; default a third of a
# sandbox's, at least 2). CI jobs get what they declare in yulius.toml (SPEC-082).
#Environment=YULIUS_AGENT_CPUS=4
Environment=YULIUS_CI_JOB_TIMEOUT_MIN=30
Environment=YULIUS_CI_POLL_SECS=60
# Sandbox isolation (SPEC-002 ADR-22): auto (default), namespaces or legacy; see "Sandbox
# isolation" below for limits and egress.
#Environment=YULIUS_ISOLATION=auto
ExecStart=/usr/local/bin/yulius serve
KillMode=process
# Per-sandbox CPU, memory and task limits: the server keeps itself in server/ and gives every
# sandbox its own cgroup under the unit (systemd 254 or later).
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

## 4. Reverse proxy (TLS, WebSockets)

Only the public listener (`YULIUS_BIND`) is proxied. The internal listener (`YULIUS_INTERNAL_BIND`)
serves agents' MCP, hooks and Git credentials; it must stay on loopback and is never proxied.
WebSockets are used for terminals, the event stream and the browser panel; import
chunks are up to 8 MiB and a dropped file up to the upload limit (`YULIUS_UPLOAD_MAX_MB`, 25 MiB
by default), so the proxy's body limit must be above it (32 MB below; raise both together). Let the proxy compress responses: Yulius serves its web UI uncompressed,
and compression cuts the first load from about 1 MB to about 300 KB (the editor's first open from
4.6 MB to about 1.2 MB), which matters most on phones.

Caddy:

```caddyfile
yulius.example.com {
    request_body {
        max_size 32MB
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
    client_max_body_size 32m;
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
([decision 0165](decisions/0165-the-superagent-bypasses-approvals-on-contained-servers.md)). Keep Caddy's admin API unreachable from runs without
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
  CI gets up to 80 % of the sandbox's CPU while agents build ([ADR-0039](decisions/0039-ci-jobs-get-the-cpu-first.md));
- a seccomp filter: no nested user namespaces, `io_uring`, `bpf`, keyrings, module or mount calls;
- its own **Docker engine** ([ADR-0151](decisions/0151-a-sandbox-runs-its-own-docker-engine-under-an-engine-seccomp-profile.md)),
  started on the first use of `/var/run/docker.sock` (`DOCKER_HOST`) by an agent, the
  Superagent, a Shell or a CI job, also in sessions started before, and stopped once idle. See
  "Docker in sandboxes" below.

**Install once** (as root): `sudo yulius host setup` (§2). It installs the packages, gives the
service user a subordinate range, gives the unit its delegation, and builds the first base (a
few minutes, about 6 GB under `<data>/system`). Then, if it added the drop-in, restart once:
`systemctl restart yulius`. Install `gh` from GitHub's repository (§1) before the setup, so
the base copies it.

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
| `YULIUS_SANDBOX_MEMORY_GB` | the RAM but an eighth (at least 4 GiB kept for the host) | `memory.max` per sandbox. One sandbox runs all its agents and CI jobs: size it to the sessions, not to half the host. |
| `YULIUS_SANDBOX_CPUS` | none | `cpu.max` per sandbox. Without it the CPU is shared by weight: a quota of all CPUs but two throttled the sandbox in half of its busy periods while the two CPUs it left idled (SPEC-082). The sandbox's capacity for budgets is this limit, else the host's CPUs. |
| `YULIUS_SANDBOX_PIDS` | a quarter of the host's `kernel.threads-max` or `kernel.pid_max`, whichever is smaller, at least 8192, at most three quarters of `yulius.service`'s own `pids.max` | `pids.max` per sandbox: processes and threads of all its tasks, CI jobs and containers together. With the CPU split, CI jobs (`ci/`) and the Docker engine with its containers (`engine/`) each get 3/8 of it, so agent sessions keep at least a quarter (ADR-0198). |
| `YULIUS_SANDBOX_DISK_GB` | 100 | Soft quota: the sandbox directory plus its installed packages, measured every 10 minutes (again when a refusal would rest on a reading older than a minute); from 90 % Yulius prunes the build caches of idle tasks, ended tasks' caches and closed or cancelled tasks' worktrees always (every 5 minutes), see [SPEC-008](../.ai/specs/SPEC-008-sandbox-disk.md); above it, new sessions are refused (live ones continue). `0` turns it off. Persistent CI job checkouts are pruned the same way while no CI run executes, and one job's cache beyond a quarter of the quota at once. A project declares its own caches in `yulius.toml`: `[disk]` `caches = "target build/cache"`. Caches are found and removed only with no link from the sandbox directory down, and on Linux only ([SPEC-263](../.ai/specs/SPEC-263-disk-upkeep-refuses-links-at-worktrees-a.md)). |
| `YULIUS_SANDBOX_FORWARD` | — | Extra host loopback ports reachable inside every sandbox at the same port (for example a loopback MCP server), comma-separated. |
| `YULIUS_SANDBOX_IDLE_MINUTES` | 5 | A holder without runs is stopped after this long; the next run starts a fresh one on the newest base. A sandbox's Docker engine stops after this long without a client and without a running container. |
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
already. **The scanned code is sent to Vidoc** (`https://model.vidocsecurity.com`). Without a key
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

The start log says `Vidoc is on (instance key saved in settings)` (or `… from
YULIUS_VIDOC_API_KEY_FILE`) or `Vidoc is off: no instance key`.

## 5. First run

1. Start the service. On the first start it writes a one-time setup token to
   `/var/lib/yulius/setup.token` (0600) and logs where it is.
2. Open the public URL, enter the token, choose the superadmin password (twice; at least 8
   characters, 12 or more recommended) and confirm both warnings: **the password can be recovered
   only with the recovery code** (SPEC-001 ACC-002, ACC-011). Store the recovery code shown next.
   The last two steps ask for the Jev API key and the Vidoc API key (both optional; skip them to
   add them later in Settings → Instance).
3. User menu → Administration → GitHub connection: create the GitHub App with the manifest flow for the owner account or
   organization, then install it on the repositories Yulius may use. The App's callback URLs point
   at `YULIUS_PUBLIC_URL`. The manifest requests the permissions below.
4. User menu → Administration → Users: admit GitHub logins; they sign in with GitHub. Repository access in
   Yulius mirrors their GitHub permissions (read / write / admin). The superadmin owns the
   instance and sees every sandbox with admin access without GitHub (SPEC-001 ACC-012); pull
   requests it opens are authored by the GitHub App, and merging or releasing, which act as a
   person on GitHub, are done on GitHub or after signing in with GitHub as an admitted user.
5. User menu → Administration → Subscriptions: add Claude subscriptions; each login runs Claude's own sign-in in a
   terminal in the browser. They form one pool with nothing to configure: tasks are
   subscription-agnostic; Yulius places them on the capacity that expires soonest, moves them at
   98% at a turn boundary, pauses them (without telling the agent) only while no subscription has
   capacity left and resumes them by itself. The capacity panel there and on the Dashboard says
   when to add subscriptions (SPEC-002 ADR-17). Claude's sign-in here (`claude setup-token`) may
   not read Claude's usage endpoint (it lacks the `user:profile` scope), so a subscription's usage
   shows once a task runs on it: the running sessions report it through Claude Code's status line.
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
once the ruleset exists.

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

### CI on this machine

Yulius is the projects' only CI (SPEC-002 ADR-21). No GitHub Actions or paid GitHub feature is
involved.

**What runs.** For each sandbox:

- The CI comes from `[[ci.jobs]]`, or `[commands] gate`, in `yulius.toml` on the integration
  branch (docs/api.md "CI").
- An administrator can set `gate` on the sandbox profile instead (`PATCH /api/sandboxes/{sid}`).
- Every open pull request into the integration branch and every new integration or production
  commit is tested once.
- The jobs run in parallel as the `yulius` user, inside the sandbox's containment when
  `YULIUS_CONTAINED=true`, without any token.
- Results appear on each sandbox's CI page and on GitHub as the status `yulius/ci`.
- `YULIUS_CI=off` stops all of it. Merges then fall back to GitHub's checks.

**The machine.** Jobs use the tools installed on the VM (`/usr`, `/opt` and
`YULIUS_CONTAINED_READONLY` paths are visible read-only inside containment).

- Install the toolchains the projects need system-wide, for example rustup's toolchains under
  `/opt`, Node and Python.
- Each sandbox keeps `<data>/sandboxes/<id>/ci/`:
  - `jobs/<job>` (and `jobs/<job>@<lane>` past the first lane): one persistent checkout per job
    and lane; ignored build outputs such as `target/` and `node_modules/` are its cache;
  - `home`: `HOME` for jobs, with `~/.cargo`, `~/.npm` and `~/.cache`;
  - `cache`: `YULIUS_CI_CACHE`.

  `home` and `cache` are shared by the sandbox's lanes, so a job's installs there must tolerate a
  concurrent run.
- Budget the disk: a Rust workspace's `target/` per job and lane.
- Removing `ci/` (with no run active) resets a sandbox's caches. The next run rebuilds them.
- CI jobs share the sandbox's CPUs and memory by budget (SPEC-082): a job starts once the CPUs
  and memory it declares (`cpus`, `memory_gb` in `[[ci.jobs]]`, else an equal share among its
  run's jobs) are free, so the jobs of all lanes never ask for more than the sandbox has. Each
  is told its budget in `YULIUS_CPUS` and `YULIUS_MEMORY_MB` and the repository's
  `[resources.env]`. `YULIUS_CI_MAX_JOBS` still bounds the number of jobs. Each job gets
  `YULIUS_CI_JOB_TIMEOUT_MIN`.
- Agent sessions, terminals and the Cockpit plan for `YULIUS_AGENT_CPUS` CPUs (default a third
  of the sandbox's, at least 2) and the memory in proportion: three of them testing at once fill
  the sandbox.
- `YULIUS_CI_LANES` (default 2) runs of one sandbox execute at once. Each lane past the first
  keeps its own job checkouts and caches (`ci/jobs/<job>@<lane>`), so budget its disk too.

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

**Hygiene.** After a merge through Yulius the head branch is deleted. Every 6 hours each sandbox is
scanned for stale branches and worktrees, shown on its CI page. People clean them up there, agents
with `repo_cleanup`.

### CD on this machine

CD (deployment, SPEC-134) needs no configuration on the server: projects define `[cd.dev]`, `[cd.prod]`
and any other `[cd.<name>]` with its `stage` in their `yulius.toml` ([API](api.md#cd-deployment)). A run uses the CI job timeout (`--ci-job-timeout-min`, `YULIUS_CI_JOB_TIMEOUT_MIN`) and the
containment: dev runs inside the sandbox (its tools, its network); production joins only the
sandbox's network (no host loopback) on the host's read-only `/usr`, `/etc` and `/opt`, so its
scripts install the tools they need into `$YULIUS_CD_CACHE`. Files: `<data>/cd/<id>/source.git` (the verified
mirror), `<data>/cd/<id>/environments.git` (the mirror delivery environment runs take their commits
from, ADR-0175), `<data>/cd/<id>/cache/<name>` (`$YULIUS_CD_CACHE`; `cache/<stage>.<name>` for an
environment other than `dev` and `prod`, so a name moved to the other stage gets a fresh cache),
and `<data>/runs/cd-<run>/work` or `<data>/runs/env-<run>/work` (the commit's tree, `HOME` and log, removed
after each run); none is visible to agents. Runs are kept
14 days, 50 per environment, and each environment's last passed run always.

- **Secrets.** A dev run gets the real value of each secret its table names once a person allowed
  it outside the proxy on the Secrets page (the same allowance as copying it into `.env`); a
  production run gets a secret agents set up ("Let agents set it up", the default, ADR-0171) or
  one that someone who manages it chose "Allow for production deploys" for, and never a personal
  secret (ADR-0186). That choice is not offered for a secret agents use (bound hosts or `.env` copies): a production
  secret is exclusive, so production needs a separate secret with its own name (ADR-0109). Output is
  redacted, but scripts hold the values while they run, so allow only what a deploy needs.
- **Egress off.** With `YULIUS_SANDBOX_EGRESS=none` sandboxes have no network, and production
  deploys (which use the sandbox's network) have none either.
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

Removing a playground can delete its code for good (the removal dialog's "Also delete its code
from this server for good", or `POST /api/sandboxes/{sid}/purge`; the superadmin can purge an
already removed one through the API). Its `<data>/playgrounds/<id>.git` and
`<data>/sandboxes/<id>/` are deleted once no agent run, shell, browser, CI run or other process of
it is left, within a disk upkeep round (5 minutes); task records, the removal's export under
`<data>/exports/` and the audit (`sandbox.purge_requested`, `sandbox.purged`) stay (ADR-0139).
Its system layer, holder state and CD files go with the leftovers of any removed sandbox (see
"Leftovers" below).

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

Put keys that span lines in a file. The hidden prompt reads one line: a pasted key arrives cut
to its first line, which is refused, and the terminal usually discards the rest; if part of it
still shows at the next `Name:` prompt, press Ctrl-C (nothing has been sent).

Values never appear: no option takes one, the output holds names only, nothing is written to disk,
and the server logs no request bodies; the vault audit records who added which name. The command
reminds you that the source file still holds the values. Exit 0 when every secret was imported or
already there, 1 otherwise (skipped as invalid, refused, cancelled, or not sent after an expired
sign-in, a 403 or an unreachable server; creates are never retried, and a second run skips the
imported ones as existing).

### Dropped files

Files users drag onto the editor are written into the repository or a task worktree; files
dropped or pasted onto a terminal are stored as attachments in
`<data>/sandboxes/<sid>/attachments/{tasks,shells}/<id>/`, outside every worktree (never in
`git status` or a pull request; the sandbox's processes can read and change them like its other
files). They count towards the
data directory's disk: at most 25 MiB each (`YULIUS_UPLOAD_MAX_MB`), 500 per task or terminal,
8 uploads at once. A task's attachments go when the task is deleted, a terminal's when it ends
(left-overs are removed at the next start); archiving a task keeps them. Uploads are audited
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
`master.key` apart from database backups (whoever has both can read every secret).

### Leftovers: what is kept, what is cleaned and when

Yulius deletes what nothing needs any more by itself (SPEC-215, ADR-0170): at every start (two
minutes in, so after every update) and every hour. Each deletion checks its rule again right
before it removes anything: a running agent, shell, browser, CI run or deploy, a live holder, or a
task that started working again keeps its files. Every deletion is audited as `leftovers.delete`
(path, size, reason), and every failure as `leftovers.failed`.

| What | When it goes | By itself? |
| --- | --- | --- |
| A removed sandbox's `worktrees/`, `ci/`, `browser/`, `<data>/cd/<id>`, `<data>/displays/<id>` and its agent runs' `<data>/runs/<attempt>` | 10 minutes after the removal, once nothing of it runs | yes |
| A removed sandbox's `<data>/sysroots/<id>` (installed packages, Docker images and volumes) and `<data>/runtime/sandboxes/<id>` | once its holder has stopped (a minute after the removal, SPEC-206); removed as root of a mapped namespace | yes |
| Everything else of a removed sandbox: its clone, home with the Claude sessions, attachments, `<data>/transcripts/<task>`, a playground's `origin`, `<data>/exports/sandbox-<slug>-*.json` | kept (PRJ-009); the database records always stay | **only a person** |
| A task's rebuildable caches (`target`, `node_modules`, `[disk] caches`, …) | closed, stopped or failed: after 10 minutes; **done**: after 30 quiet minutes; waiting: after 4 hours; any idle task at 90 % of the quota | yes (the worktree and uncommitted work stay) |
| A closed or cancelled task's clean worktree | after 10 minutes (its branch keeps the work) | yes |
| Git's record of a worktree inside the sandbox directory whose directory is gone (`.git/worktrees/<name>`), not locked | always (that record only; a worktree an agent keeps in its own `/tmp` keeps its record) | yes |
| A CI checkout of a job or lane no run used for 14 days | while no CI run of that sandbox runs | yes |
| A draft attachment older than 7 days | always | yes |
| `<data>/runs/<name>` that no attempt record, live run, active CI job, running deploy or running delivery environment run owns, untouched for a day (an ended task's last run, with its final terminal screen, stays) | always | yes |
| Transcripts of a deleted task; a Docker smoke test's sandbox whose process is gone | always | yes |
| An unused sandbox base (not current, no live holder mounts it) | always | yes |
| A worktree no task owns; a directory under `sandboxes/`, `sysroots/`, `runtime/sandboxes/`, `cd/` or `displays/` that no sandbox record owns | listed | **only a person** |
| The host's `/var/lib/docker` and `/var/lib/containerd` (docker-ce; Yulius's host daemons are off) | listed, never deleted: remove them on the host if nothing else uses them | report only |
| Update backups under `/var/lib/yulius-update/backups/` | the helper keeps the last 3 | yes (not listed) |

**Settings → Instance → Leftovers** (superadmin) lists every item with its path, kind, size, owner,
why it is a leftover and whether Yulius deletes it by itself, with the disk's usage and the
reclaimable total. **Refresh** rescans, and **Delete** (the selected items) and **Delete all safe**
ask for confirmation with the size, then run in the background. The same from SSH, as root or the
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
agent runs through `yulius --scrub-exec` and every tool result through the `PostToolUse` hook:
a command that prints `the agent-secret fingerprints of this run cannot be loaded` means the run
cannot reach Yulius and has no `scrub.json` — continue the task to start a fresh run.

Rotation: replace a value on the Secrets page after rotating it at its provider (the page marks
values older than 180 days). If `master.key` and a database copy may have leaked together,
rotate every team and agent secret at its source.

## 8. Upgrades

### Updates from main (the update helper)

Yulius updates itself from `main` of **Quartio-Studio/yulius** (SPEC-001 OPS-006..OPS-009,
SPEC-002 ADR-18). It knows the commit it was built from (`yulius version`, and
`GET /internal/version` on the internal listener) and checks `main` every hour and whenever a
superadmin opens **Settings → Instance** (SPEC-149). Superadmins get a
notification and a banner, "Update available: …", and **Settings → Instance** lists the new
commits. Nothing updates by itself: a superadmin chooses **Update now** and confirms. Then:

1. **Build** (tasks keep working): the root helper fetches `main`, checks that the commit is on it
   and not older than the commit it installed last, and runs `cargo build --release --locked -p
   yulius` with `YULIUS_BUILD_COMMIT` set. Several minutes; the page shows the progress.
2. **Pause tasks**: every task's agent is frozen at once with the cgroup v2 freezer (its process
   tree moves into `…/sandboxes/<sandbox>/agents/run-<attempt>` and that group is frozen; the
   ptyhost, shells and browsers keep running) and new starts queue. The agents are told nothing.
   This needs the delegated cgroup of §3 (`Delegate=yes`, `DelegateSubgroup=server`); a run that
   cannot be frozen pauses at its next turn boundary instead (its Stop or `UserPromptSubmit` hook
   is held, as for a capacity pause), and while such a run is still in a turn nothing is frozen:
   the others freeze once it is held (or at once on **Continue without waiting**). A task still in a turn after 30 minutes cancels the update
   (nothing changes); the superadmin can also **Continue without waiting** or **Cancel**. To check
   a freeze on a host: `cat /sys/fs/cgroup/system.slice/yulius.service/sandboxes/*/agents/run-*/cgroup.events`
   (`sandboxes/*/run-*` without the `cpu` controller) shows `frozen 1` while the update restarts.
   A server that starts thaws every run group left frozen, but those of tasks a person paused
   (SPEC-191: it freezes them again; `SELECT number, paused_by FROM tasks WHERE paused_by IS NOT
   NULL` lists them); to thaw them by hand (a binary without
   this, SPEC-160): `for f in /sys/fs/cgroup/system.slice/yulius.service/sandboxes/*/{,agents/}run-*/cgroup.freeze; do echo 0 > "$f"; done`. A response streaming from the model provider at the
   restart is cut (the egress proxy restarts with the server) and Claude Code retries it.
3. **Restart**: the helper stops `yulius.service` (with `KillMode=process` agent sessions and
   terminals keep running), copies the binary and `yulius.db` (with its WAL) to
   `/var/lib/yulius-update/backups/<run>/`, installs the new binary, starts the service and waits
   up to 2 minutes until `/internal/version` reports the new commit. If it does not, it stops the
   service, restores the previous binary **and database**, and starts them again ("rolled back").
   Once the new version is healthy, the helper runs **its** `yulius host check --json` (at most 2
   minutes; SPEC-203). That re-checks what sandboxes need for Docker, with only safe, idempotent
   fixes: the needrestart rule, the host's own Docker units, the modules file and loading, the
   binary's owner. A failed item, or a "sandbox Docker" that does not hold, is a regression. The
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

The service user cannot replace `/usr/local/bin/yulius` or restart itself. A root-owned **path
unit** starts the helper when the server writes `/var/lib/yulius/update/request.json`; the request
can only say "build" or "install" one commit, everything else (repository, paths, service) is in
the root-owned unit. The helper refuses a request that is not a regular file owned by the
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

### Notes for specific upgrades

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
  `cat /sys/fs/cgroup/system.slice/yulius.service/sandboxes/<sandbox>/ci/cpu.weight` (400).
  A holder that was already running keeps its `slirp4netns` in `agents/` until the holder stops
  when idle. **Rolling back to an older binary:** a server that stops gracefully undoes the split
  (`-pids`, then `-cpu`), so the older binary can join the sandbox groups again. After a crash or a
  forced stop, undo it by hand before starting the older binary. Otherwise its holders and runs
  fail with "Device or resource busy":
  `for f in /sys/fs/cgroup/system.slice/yulius.service/sandboxes/*/cgroup.subtree_control; do echo -pids > "$f"; echo -cpu > "$f"; done`.
- Upgrading to host-sized task limits (SPEC-268, 2026-10-22): no migration. At its next start the
  server gives each sandbox a quarter of the host's task table, at most three quarters of
  `yulius.service`'s own `pids.max` (systemd's `DefaultTasksMax` is 15 % of the table unless the
  unit sets `TasksMax=`; raise it there for bigger sandboxes), and `ci/` and `engine/` 3/8 of a
  sandbox's;
  the log line "per-sandbox limits on" shows the figure. An explicit `YULIUS_SANDBOX_PIDS` stays.
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
