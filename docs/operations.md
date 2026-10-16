# Operating Yulius

Yulius is one Rust binary, `yulius`, that serves the web UI and API, supervises agent sessions and
provides the CLI (`yulius onboard` alias `yulius import`, `yulius login`). This guide covers a single Linux VM with a TLS
reverse proxy in front. It satisfies SPEC-001 AC-OPS-1 as documentation only: **a deployment has not
been performed or verified** as part of the implementation work.

## 1. Prerequisites

| Component | Needed for | Notes |
| --- | --- | --- |
| Linux x86_64 or arm64, systemd | the service | macOS works for development (runs are "not contained"). |
| `git` ≥ 2.40 | sandboxes, worktrees, delivery | on `PATH` of the service. |
| Claude Code (`claude`) | Claude tasks and Claude account login | verified with 2.1.292. |
| GitHub CLI (`gh`) ≥ 2.82.1 | `gh` in every sandbox, logged in as the person (their shells) or the task's creator (agents) | install it on the host from [GitHub's package repository](https://github.com/cli/cli/blob/trunk/docs/install_linux.md), not the distribution's package: Ubuntu 24.04 ships 2.45, and since GitHub retired Projects (classic) `gh pr edit` and `gh issue view --comments` fail with a GraphQL error before 2.82.1. Sandboxes copy the host system, so rebuild the base afterwards (`yulius sandbox-base build`). `yulius doctor` warns without it or below 2.82.1. |
| bubblewrap (`bwrap`) | containment (`--contained`) | strongly recommended in production; without it runs are marked "not contained". |
| `uidmap` (`newuidmap`, `newgidmap`) and a subordinate id range for the service user | sandbox isolation (SPEC-002 ADR-22): `apt` in sandboxes, private networks | `/etc/subuid` and `/etc/subgid` lines of at least 65,536 ids (see [Sandbox isolation](#sandbox-isolation)). |
| `slirp4netns` | sandbox egress (isolation) | `apt install slirp4netns`. |
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

Run `yulius doctor --data-dir /var/lib/yulius [--contained]` to check all of these, the data directory
permissions and the session socket path budget (the data directory path must be short enough that
`<data>/logins/<uuid>/pty.sock` fits the 103-byte Unix socket path limit).

## 2. Build and install

```sh
cargo build --release -p yulius            # from a checkout; output: target/release/yulius
sudo install -m 0755 target/release/yulius /usr/local/bin/yulius
sudo useradd --system --create-home --home-dir /var/lib/yulius --shell /usr/sbin/nologin yulius
sudo -u yulius yulius doctor --data-dir /var/lib/yulius --contained
```

The agent CLIs must be installed for (and logged-in state is never taken from) the `yulius` user:
Claude subscriptions are added inside Yulius (user menu → Administration → Subscriptions), not with the CLIs' own
`login` on the VM.

## 3. systemd unit

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
Yulius's copy, like `master.key`. Keep Caddy's admin API unreachable from runs without
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
- a seccomp filter: no nested user namespaces, `io_uring`, `bpf`, keyrings, module or mount calls.

**Install once** (as root):

```sh
apt install uidmap slirp4netns bubblewrap gh
# A free subordinate range of 65,536 ids for the service user (check /etc/subuid and /etc/subgid
# for ranges already used; on the reference VM 100000–231071 are taken):
usermod --add-subuids 231072-296607 --add-subgids 231072-296607 yulius
# Delegate=yes and DelegateSubgroup=server in the unit (§3), then:
systemctl daemon-reload
# Build the first base before restarting, so new sessions are isolated at once (a few minutes,
# about 6 GB under <data>/system):
sudo -u yulius yulius sandbox-base build --data-dir /var/lib/yulius
sudo -u yulius yulius doctor --data-dir /var/lib/yulius --contained
systemctl restart yulius
```

**Settings** (environment of the unit):

| Variable | Default | Meaning |
| --- | --- | --- |
| `YULIUS_ISOLATION` | `auto` | `auto`: the holder when its prerequisites hold, else the previous containment (and a failing holder never blocks work); `namespaces`: refuse runs without it; `legacy`: the previous containment. |
| `YULIUS_SANDBOX_EGRESS` | `direct` | `none`: no egress, only the relayed endpoints (the internal listener, which also serves the secrets proxy, and `YULIUS_SANDBOX_FORWARD`). |
| `YULIUS_SANDBOX_MEMORY_GB` | the RAM but an eighth (at least 4 GiB kept for the host) | `memory.max` per sandbox. One sandbox runs all its agents and CI jobs: size it to the sessions, not to half the host. |
| `YULIUS_SANDBOX_CPUS` | none | `cpu.max` per sandbox. Without it the CPU is shared by weight: a quota of all CPUs but two throttled the sandbox in half of its busy periods while the two CPUs it left idled (SPEC-082). The sandbox's capacity for budgets is this limit, else the host's CPUs. |
| `YULIUS_SANDBOX_PIDS` | 8192 | `pids.max` per sandbox. |
| `YULIUS_SANDBOX_DISK_GB` | 100 | Soft quota: the sandbox directory plus its installed packages, measured every 10 minutes (again when a refusal would rest on a reading older than a minute); from 90 % Yulius prunes the build caches of idle tasks, ended tasks' caches and closed or cancelled tasks' worktrees always (every 5 minutes), see [SPEC-008](../.ai/specs/SPEC-008-sandbox-disk.md); above it, new sessions are refused (live ones continue). `0` turns it off. Persistent CI job checkouts are pruned the same way while no CI run executes, and one job's cache beyond a quarter of the quota at once. A project declares its own caches in `yulius.toml`: `[disk]` `caches = "target build/cache"`. |
| `YULIUS_SANDBOX_FORWARD` | — | Extra host loopback ports reachable inside every sandbox at the same port (for example a loopback MCP server), comma-separated. |
| `YULIUS_SANDBOX_IDLE_MINUTES` | 5 | A holder without runs is stopped after this long; the next run starts a fresh one on the newest base. |

**Where things live.**
- `<data>/system/base-<n>/`: the base (`current.json` names it). The server rebuilds it when the host's packages, `/usr/local` or `/opt` change; it waits until they have been unchanged for 10 minutes and checks every 30 minutes. A sandbox moves to the new base the next time its holder starts; the packages it installed stay.
- `<data>/sysroots/<sandbox>/`: a sandbox's installed packages (the overlay's upper layer).
- `<data>/runtime/sandboxes/<sandbox>/`: the holder's records and relay sockets.

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

### Task intake (Jev)

Optional (SPEC-002 ADR-16). With a TypeSafe Jev key, Yulius offers to split new tasks that mix
separate workstreams (the proposal comes from one headless `claude -p` run on a connected
account), it holds prompts typed into a running task that add, drop or replace its scope until
someone decides in the task view, and it chooses a new task's mode and workflow when the composer
leaves them on Auto (the default). Without a key the split and scope checks are off, and Auto
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
  question: for a split check or an Auto choice, the new task's prompt (the workflow question also
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

## 5. First run

1. Start the service. On the first start it writes a one-time setup token to
   `/var/lib/yulius/setup.token` (0600) and logs where it is.
2. Open the public URL, enter the token, choose the superadmin password (twice) and confirm both
   warnings: **the password cannot be recovered** (SPEC-001 ACC-002). Store the recovery code
   shown next. The last step asks for the Jev API key (optional; skip it to add it later in
   Settings → Instance).
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
| Administration | Read and write | Create repositories in an organization (New sandbox) and add branch rulesets to them. |
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

CD (deployment, SPEC-134) needs no configuration on the server: projects define `[cd.dev]` and
`[cd.prod]` in their `yulius.toml`. A run uses the CI job timeout (`--ci-job-timeout-min`, `YULIUS_CI_JOB_TIMEOUT_MIN`) and the
containment: dev runs inside the sandbox (its tools, its network); production joins only the
sandbox's network (no host loopback) on the host's read-only `/usr`, `/etc` and `/opt`, so its
scripts install the tools they need into `$YULIUS_CD_CACHE`. Files: `<data>/cd/<id>/source.git` (the verified
mirror), `<data>/cd/<id>/cache/<env>` (`$YULIUS_CD_CACHE`) and `<data>/runs/cd-<run>/work` (the
commit's tree, `HOME` and log, removed after each run); none is visible to agents. Runs are kept
14 days, 50 per environment, and each environment's last passed run always.

- **Secrets.** A dev run gets the real value of each secret its table names once a person allowed
  it outside the proxy on the Secrets page (the same allowance as copying it into `.env`); a
  production run only once someone who manages it chose "Allow for production deploys". Output is
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
**Start onboarding** on the sandbox page in the browser, which the CLI cannot do (SPEC-051,
ADR-0038). New tasks wait until onboarding is ready (or an admin skips it); `yulius onboard
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
UMask=0022
```

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
  (`-cpu`), so the older binary can join the sandbox groups again. After a crash or a forced stop,
  undo it by hand before starting the older binary. Otherwise its holders and runs fail with
  "Device or resource busy":
  `for f in /sys/fs/cgroup/system.slice/yulius.service/sandboxes/*/cgroup.subtree_control; do echo -cpu > "$f"; done`.
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
  in another form, misuse on its hosts, weak values, raw CDP): SPEC-002 ADR-20.
- Agents receive per-run task tokens (MCP, hooks) and, through the credential helper and `gh`,
  their task creator's GitHub token (8 h, capped by that person's access within the App's
  installation; an agent could read it and act as them there until it expires), or without one a
  token restricted to their sandbox repository; provider credentials reach the agent process
  only through its environment (SPEC-002 §8, R-08).
- Known open risks are listed in SPEC-002 §12 (notably R-07: Chromium's DevTools port is
  unauthenticated on loopback, which isolated sandboxes cannot reach; R-09: environment commands
  run outside containment).
- Isolated sandboxes cannot reach the host's loopback services (Yulius's public listener, Caddy's
  admin API, DevTools ports, databases bound to 127.0.0.1). Services bound to the host's public
  addresses remain reachable like any internet host.
- Password sign-in is limited per client only with the proxy's shared secret configured
  (`YULIUS_PROXY_SECRET_FILE`, §4 "Per-client sign-in limits"); without it every client behind the
  proxy shares one limit.
