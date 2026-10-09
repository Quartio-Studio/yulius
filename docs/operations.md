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
| Chromium / Google Chrome | sandbox browser | optional; found on `PATH` or set `--chromium`. |

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

`KillMode=process` is required: each agent session runs in its own detached `yulius ptyhost`
process so that sessions survive a restart or upgrade of the server (NFR-001). With the default
`KillMode=control-group`, stopping the service would kill every running agent.

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
Environment=PATH=/usr/local/bin:/usr/bin:/bin
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
chunks are up to 8 MiB. Let the proxy compress responses: Yulius serves its web UI uncompressed,
and compression cuts the first load from about 1 MB to about 300 KB (the editor's first open from
4.6 MB to about 1.2 MB), which matters most on phones.

Caddy:

```caddyfile
yulius.example.com {
    request_body {
        max_size 16MB
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
    client_max_body_size 16m;
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

`YULIUS_PUBLIC_URL` must be the exact external origin: cookies are `Secure` for `https` URLs and
WebSocket origins are checked against it.

Yulius itself sends `X-Frame-Options: SAMEORIGIN`, `Content-Security-Policy: frame-ancestors
'self'`, `X-Content-Type-Options: nosniff` and `Referrer-Policy: same-origin` on every response
(stricter route policies are kept). HSTS is the TLS proxy's job, as in the examples above; add it
once the domain is meant to stay on HTTPS.

## 5. First run

1. Start the service. On the first start it writes a one-time setup token to
   `/var/lib/yulius/setup.token` (0600) and logs where it is.
2. Open the public URL, enter the token, choose the superadmin password (twice) and confirm both
   warnings: **the password cannot be recovered** (SPEC-001 ACC-002).
3. Administration → GitHub: create the GitHub App with the manifest flow for the owner account or
   organization, then install it on the repositories Yulius may use. The App's callback URLs point
   at `YULIUS_PUBLIC_URL`.
4. Administration → Users: admit GitHub logins; they sign in with GitHub. Repository access in
   Yulius mirrors their GitHub permissions (read / write / admin). The superadmin owns the
   instance and sees every sandbox with admin access without GitHub (SPEC-001 ACC-012); pull
   requests it opens are authored by the GitHub App, and merging or releasing, which act as a
   person on GitHub, are done on GitHub or after signing in with GitHub as an admitted user.
5. Administration → Accounts: add Claude accounts; each login runs Claude's own sign-in in a
   terminal in the browser. Yulius rotates between them automatically.
6. Import or create projects (sandboxes). Product and Studio are detected and get the `product` and
   `studio` delivery presets.

## 6. Importing local work (`yulius import`)

On a developer machine with the `yulius` binary:

```sh
yulius login --server https://yulius.example.com          # browser approval (device flow)
cd ~/code/product && yulius import --server https://yulius.example.com --since 14d
```

The repository must be on GitHub under the instance owner. Uploads resume after interruptions;
re-running imports only new sessions.

## 7. Backup and restore

```sh
sudo -u yulius yulius backup --data-dir /var/lib/yulius /var/backups/yulius-$(date +%F).db
```

`backup` writes a consistent SQLite copy (`VACUUM INTO`). Also back up `/var/lib/yulius/master.key`
(it encrypts every stored secret; without it the database's credentials are unreadable) and, if
wanted, `sandboxes/` (clones and worktrees can be re-created from GitHub, uncommitted work cannot).
Restore: stop the service, put the database copy at `yulius.db`, restore `master.key`, start.

## 8. Upgrades

Replace the binary and `systemctl restart yulius`. Live sessions keep running in their detached
hosts; the new server reconciles them on start (reattaches live runs, marks dead ones stopped,
never respawns). Queued tasks start when capacity allows.

## 9. Security notes

- The data directory is owner-only (0700); secrets are sealed with `master.key` (XChaCha20-Poly1305).
- Agents receive per-run task tokens (MCP, hooks) and GitHub tokens restricted to their sandbox
  repository; provider credentials reach the agent process only through its environment.
- Known open risks are listed in SPEC-002 §12 (notably R-07: Chromium's DevTools port is
  unauthenticated on loopback; R-09: environment commands run outside containment).
