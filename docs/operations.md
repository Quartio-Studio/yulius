# Operating Yulius

Yulius is one Rust binary, `yulius`, that serves the web UI and API, supervises agent sessions and
provides the CLI (`yulius import`, `yulius login`). This guide covers a single Linux VM with a TLS
reverse proxy in front. It satisfies SPEC-001 AC-OPS-1 as documentation only: **a deployment has not
been performed or verified** as part of the implementation work.

## 1. Prerequisites

| Component | Needed for | Notes |
| --- | --- | --- |
| Linux x86_64 or arm64, systemd | the service | macOS works for development (runs are "not contained"). |
| `git` ≥ 2.40 | sandboxes, worktrees, delivery | on `PATH` of the service. |
| Claude Code (`claude`) | Claude tasks and Claude account login | verified with 2.1.292. |
| bubblewrap (`bwrap`) | containment (`--contained`) | strongly recommended in production; without it runs are marked "not contained". |
| Chromium / Google Chrome | sandbox browser | optional; found on `PATH` or set `--chromium`. Google Chrome (`google-chrome`) is recommended: it decodes and plays H.264 pages and is what captchas expect. |
| `xvfb` | the sandbox browser's virtual display (SPEC-002 ADR-15) | `apt install xvfb`. Without it (or ffmpeg) the browser runs headless: JPEG frames, no real input, captchas fail. `xauth` is not needed (Yulius writes the cookie itself); `x11-utils` (`xdpyinfo`) only for diagnostics. |
| `ffmpeg` with libx264 and x11grab | the browser video | `apt install ffmpeg` (Ubuntu's build has both; `yulius doctor` checks). |
| Fonts | pages in the sandbox browser | at least `fonts-noto-core fonts-noto-color-emoji fonts-liberation` (Ubuntu 24.04 names); add `fonts-noto-cjk` for CJK pages. |

Inside containment an agent sees only `/usr`, `/bin`, `/lib`, `/lib64`, `/etc`, `/opt`, its own
sandbox directory and the `yulius` binary. Install Claude Code system-wide under `/usr/local` or
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
provider accounts are added inside Yulius (Administration → Accounts), not with the CLIs' own
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
# Yulius-run CI (see "CI on this machine" below): jobs at once, minutes per job, seconds between checks.
Environment=YULIUS_CI_MAX_JOBS=4
Environment=YULIUS_CI_JOB_TIMEOUT_MIN=30
Environment=YULIUS_CI_POLL_SECS=60
ExecStart=/usr/local/bin/yulius serve
KillMode=process
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
3. Administration → GitHub: create the GitHub App with the manifest flow for the owner account or
   organization, then install it on the repositories Yulius may use. The App's callback URLs point
   at `YULIUS_PUBLIC_URL`. The manifest requests the permissions below.
4. Administration → Users: admit GitHub logins; they sign in with GitHub. Repository access in
   Yulius mirrors their GitHub permissions (read / write / admin). The superadmin owns the
   instance and sees every sandbox with admin access without GitHub (SPEC-001 ACC-012); pull
   requests it opens are authored by the GitHub App, and merging or releasing, which act as a
   person on GitHub, are done on GitHub or after signing in with GitHub as an admitted user.
5. Administration → Accounts: add Claude subscriptions; each login runs Claude's own sign-in in a
   terminal in the browser. They form one pool with nothing to configure: tasks are
   subscription-agnostic; Yulius places them on the capacity that expires soonest, moves them at
   98 % at a turn boundary, pauses them (without telling the agent) only while no subscription has
   capacity left and resumes them by itself. The capacity panel there and on the Dashboard says
   when to add subscriptions (SPEC-002 ADR-17). Claude's sign-in here (`claude setup-token`) may
   not read Claude's usage endpoint (it lacks the `user:profile` scope), so a subscription's usage
   shows once a task runs on it: the running sessions report it through Claude Code's status line.
6. Import or create projects (sandboxes). Product and Studio are detected and get the `product` and
   `studio` delivery presets.

### GitHub App permissions

Yulius uses one GitHub App (SPEC-002 ADR-08). The App's tokens can do only what its permissions
allow: the server's installation tokens, the repository-restricted tokens agents get, and people's
sign-in (user-to-server) tokens, which are also capped by the person's own repository access. The
manifest flow requests these repository permissions:

| Permission | Access | What Yulius does with it |
| --- | --- | --- |
| Contents | Read and write | Clone and fetch repositories; push task branches (the server, and agents through their repository-restricted token); merge pull requests; fast-forward promotions; compare branches. |
| Pull requests | Read and write | Open, list and read pull requests. |
| Checks | Read | Read check runs on pull request heads: GitHub Actions and every other Checks API app. |
| Commit statuses | Read and write | Publish the result of Yulius's own CI as the status `yulius/ci` on each tested commit (write, since 2026-10-11), and read statuses of projects without Yulius CI. |
| Workflows | Read and write | Push the onboarding commit that deletes `.github/workflows/` (Yulius is the only CI). Requested since 2026-10-11. |
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
  - `jobs/<job>`: one persistent checkout per job; ignored build outputs such as `target/` and
    `node_modules/` are its cache;
  - `home`: `HOME` for jobs, with `~/.cargo`, `~/.npm` and `~/.cache`;
  - `cache`: `YULIUS_CI_CACHE`.
- Budget the disk: a Rust workspace's `target/` per job.
- Removing `ci/` (with no run active) resets a sandbox's caches. The next run rebuilds them.
- Size `YULIUS_CI_MAX_JOBS` to the cores left after the agents. Each job gets
  `YULIUS_CI_JOB_TIMEOUT_MIN`.

**Moving a repository off GitHub Actions** (the onboarding step; agents use the `ci_setup` tool):

1. Translate each workflow job's checks into `[[ci.jobs]]`.
   - Use matrices for splits.
   - Drop `${{ }}` expressions, Actions secrets and services.
   - Install what `uses:` steps installed on the machine or in the job's command.
2. `git rm -r .github/workflows` in the same commit, then open the pull request. Yulius tests it with
   the CI it adds, because the integration branch has none yet.
3. Merge once it passes. The App needs **Workflows: Read and write** to push step 2.
4. Turn GitHub Actions off for the repository (see "GitHub App permissions").
5. Rulesets that require Actions checks must switch to `yulius/ci`, or go. Yulius's own merge gate
   enforces the delivery profile either way.

**Hygiene.** After a merge through Yulius the head branch is deleted. Every 6 hours each sandbox is
scanned for stale branches and worktrees, shown on its CI page. People clean them up there, agents
with `repo_cleanup`.

## 6. Importing local work (`yulius import`)

On a developer machine with the `yulius` binary:

```sh
yulius login --server https://yulius.example.com          # browser approval (device flow)
cd ~/code/product && yulius import --server https://yulius.example.com --since 14d
```

The repository must be on GitHub under the instance owner. Uploads resume after interruptions;
re-running imports only new sessions.

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
`GET /internal/version` on the internal listener) and checks `main` every hour. Superadmins get a
notification and a banner, "Update available: …", and **Settings → Instance** lists the new
commits. Nothing updates by itself: a superadmin chooses **Update now** and confirms. Then:

1. **Build** (tasks keep working): the root helper fetches `main`, checks that the commit is on it
   and not older than the commit it installed last, and runs `cargo build --release --locked -p
   yulius` with `YULIUS_BUILD_COMMIT` set. Several minutes; the page shows the progress.
2. **Pause tasks**: each task pauses at its next turn boundary (its Stop or `UserPromptSubmit`
   hook is held, as for a capacity pause) and new starts queue. The agents are told nothing. A
   task still in a turn after 30 minutes cancels the update (nothing changes); the superadmin can
   also **Continue without waiting** or **Cancel**.
3. **Restart**: the helper stops `yulius.service` (with `KillMode=process` agent sessions and
   terminals keep running), copies the binary and `yulius.db` (with its WAL) to
   `/var/lib/yulius-update/backups/<run>/`, installs the new binary, starts the service and waits
   up to 2 minutes until `/internal/version` reports the new commit. If it does not, it stops the
   service, restores the previous binary **and database**, and starts them again ("rolled back").
4. **Resume**: the server that started releases the held tasks one by one; they go on as if
   nothing happened.

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
| `--update-check-minutes` / `YULIUS_UPDATE_CHECK_MINUTES` | 60 | Minutes between checks of `main` (0: only "Check now"). |

**Logs and recovery.** `journalctl -u yulius-update` shows each step. A failed rollback (the
helper says "restore by hand") leaves the previous binary and database in
`/var/lib/yulius-update/backups/<run>/`: stop `yulius`, copy `yulius` to `/usr/local/bin/yulius`
and `yulius.db` (and `yulius.db-wal`, if present) to `/var/lib/yulius/` owned by `yulius`, mode
0600, remove `yulius.db-shm`, start `yulius`. The helper keeps the last 3 backups. A manual build
should set the commit it is built from: `YULIUS_BUILD_COMMIT=$(git rev-parse HEAD) cargo build
--release --locked -p yulius` (a checkout does it by itself; a tarball has no Git).

### Notes for specific upgrades

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
- Agents receive per-run task tokens (MCP, hooks) and GitHub tokens restricted to their sandbox
  repository; provider credentials reach the agent process only through its environment.
- Known open risks are listed in SPEC-002 §12 (notably R-07: Chromium's DevTools port is
  unauthenticated on loopback; R-09: environment commands run outside containment).
