---
yulius-task: 9cf72682-e015-4edc-bb21-428e3493be18
status: in-progress
---

# SPEC-592 Instance move: Caddy takes the carried configuration and answers HTTPS

## Goal

The move's reverse-proxy step leaves the new host's Caddy serving the carried Caddyfile, and
proves it answers HTTPS on port 443 for the instance's public host before it reports the
proxy as done. A Caddy that still serves an older or default configuration never passes the
step unnoticed.

## Context

The 2026-11-21 move of the Yulius instance to the staging VM (task #575) found that on the
new host Caddy had started before the carried Caddyfile was in place (the package's postinst
starts it with its default `:80` site) and was never reloaded. It served only port 80, and
the move waited on HTTPS after the start until a person ran `systemctl reload caddy`.

`proxy()` in `crates/yulius/src/instance/migrate/target.rs` (SPEC-460 MIG-027) validated the
Caddyfile and, when Caddy was active and enabled and the carried files were reported
unchanged in this run (`was && !changed`), answered "Caddy runs with the carried
configuration" without a reload. "Unchanged in this run" proves nothing about the running
Caddy: an earlier, interrupted pass may have written the files after Caddy started, and the
package starts Caddy before the files are copied.

The step runs in the preparation, before the freeze and the start: the instance does not
answer yet (Caddy answers 502 for it), and the public host name may still lead to the old
host, so a certificate from a public CA may not be obtainable yet. The end-to-end HTTPS wait
after the start (`after()` in `migrate.rs`, MIG-039) stays as it is.

## Requirements

- **CKS-001** After `caddy validate` accepts the carried Caddyfile, the step always makes the
  running Caddy load it: `systemctl daemon-reload`, `systemctl enable caddy` and
  `systemctl reload-or-restart caddy`, whatever the files' state in this run. When the reload
  fails (for example `admin off` in the Caddyfile, which `caddy reload` needs), it restarts
  Caddy (`systemctl restart caddy`). When both fail, the step fails naming both reasons and
  the fix (`journalctl -u caddy`).
- **CKS-002** When the instance's public URL (the new one, else the old host's) is `https://`,
  the step then asks the new host's own port 443 for that host name
  (`curl --insecure --resolve <host>:443:127.0.0.1 https://<host>/`, bounded retries on a
  refused connection). Any HTTP answer, whatever its status, is HTTPS answering. A refused or
  timed-out connection means Caddy does not serve the carried site: the step restarts Caddy
  once and asks again, and then fails with the fix (`systemctl restart caddy`,
  `journalctl -u caddy`) instead of reporting the proxy as done.
- **CKS-003** A TLS handshake that fails while port 443 accepts connections (Caddy has no
  certificate for the host yet, as when the host name still leads to the old host) does not
  fail the step: its line says that Caddy listens on 443 and gets the certificate once the
  host leads here. Without an `https://` URL the step makes no HTTPS check and says so.
- **CKS-004** The line is `changed` when the files changed in this run or Caddy was not active
  and enabled, `ok` otherwise (a rerun of a finished step stays `ok`), and its detail says
  that Caddy was reloaded and what port 443 answered.
- **CKS-005** `Request::Proxy` carries the host name to ask as `https: Option<String>`
  (`#[serde(default)]`); a name that is not a plain DNS name (letters, digits, `.` and `-`) is
  not asked and the step says why.

## Plan

- [x] Read the move's proxy step, its caller and tests, and write the spec
- [ ] Failing fake-host tests: unchanged files still reload; reload fails; HTTPS refused
- [ ] Proxy step reloads Caddy and checks HTTPS on 443 on the new host
- [ ] The old host passes the public host name; flow tests updated
- [ ] ADR, docs/operations.md and docs/changes.md updated
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Evidence recorded and the spec finished for green-CI delivery

## Acceptance criteria

- **AC-1** Caddy active and enabled, files unchanged in this run: the step runs `caddy
  validate`, then `systemctl reload-or-restart caddy`, then the HTTPS check, and its line is
  `ok`. (CKS-001, CKS-004)
- **AC-2** `systemctl reload-or-restart caddy` fails and `systemctl restart caddy` succeeds:
  the step passes; both fail: the step fails naming both reasons, and no HTTPS check runs.
  (CKS-001)
- **AC-3** Port 443 refuses (curl exit 7) after the reload and after the restart: the step
  fails with `systemctl restart caddy` and `journalctl -u caddy` in its detail; refuses once
  and answers after the restart: the step passes. (CKS-002)
- **AC-4** The TLS handshake fails (curl exit 35): the step passes and its line says that the
  certificate comes once the host leads here. No `https://` URL: no curl runs. (CKS-003)
- **AC-5** The old host's move sends the new public URL's host in `Request::Proxy`; a host
  name with other characters is not asked. (CKS-005)
- **AC-6** `caddy validate` refusing the configuration still fails before any `systemctl`
  command. (CKS-001)

## Test plan

Fake-host unit tests in `crates/yulius/src/instance/migrate/tests/target.rs` script
`systemctl` and `curl` answers (exit codes) and assert the commands run, their order and the
line's outcome and detail; they are written first and fail against the old `proxy()`. The
flow tests (`tests/flow.rs`) assert the request the old host sends. A real Caddy is not
started: the step's commands are the package's own (`systemctl reload` runs `caddy reload
--force`), and the 2026-11-21 move proved that a reload fixes the symptom.

## Decisions

- Reload always instead of proving the loaded configuration through Caddy's admin API: a
  reload of an unchanged configuration costs nothing before the cutover, while the admin API
  can be off (`admin off`) or moved, and comparing its JSON with `caddy adapt` is fragile.
  [ADR-0431](../../docs/decisions/0431-the-move-reloads-caddy-and-asks-its-own-port-443.md)
- Ask the new host's own port 443 with `curl` (argv, no shell) rather than from the old host:
  before the cutover the public name may lead to the old host, and the new host's loopback
  is what the carried site must answer on. A missing certificate is not a failure at this
  point (CKS-003); the post-start HTTPS wait still reports it.
