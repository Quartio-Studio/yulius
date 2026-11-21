---
yulius-task: 9cf72682-e015-4edc-bb21-428e3493be18
status: done
---

# SPEC-592 Instance move: Caddy takes the carried configuration and answers HTTPS

## Goal

The move's reverse-proxy step leaves the new host's Caddy running the carried Caddyfile and
drop-ins, and checks that port 443 answers for the instance's public host before it reports
the proxy as done. The restart is what guarantees the carried configuration; the check catches
a Caddy with nothing open on port 443, as in the incident.

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
  `systemctl restart caddy`, whatever the files' state in this run. A restart, not a reload:
  only a start reads the carried `caddy.service.d` drop-ins' environment (the proxy secret),
  and the new host serves nobody before the cutover. A failed restart fails the step with its
  reason and where to look (`systemctl status caddy`, `journalctl -u caddy -n 50`).
- **CKS-002** When the new public URL is `https://`, the step then asks the new host's own
  port 443 for its host name (`curl --disable --noproxy '*' --insecure --resolve
  <host>:443:127.0.0.1 https://<host>/`, bounded retries, within a 60 s command bound). Any
  HTTP answer, whatever its status, is HTTPS answering. A refused or timed-out connection fails
  the step instead of reporting the proxy as done: Caddy is not running the carried site, or
  that site has no listener on 443 (TLS ending in front of Caddy, which the move does not
  carry); the detail names both and where to look. A curl that cannot run (exit 126, 127, or
  not started) fails the step saying to install curl.
- **CKS-003** A TLS handshake that fails while port 443 accepts connections (Caddy has no
  certificate for the host yet, as when the host name still leads to the old host) does not
  fail the step: its line says that port 443 takes connections and the certificate comes once
  the host leads here. Without an `https://` URL the step makes no HTTPS check and says so.
- **CKS-004** The line is `changed` when the files changed in this run or Caddy was not active
  and enabled, `ok` otherwise (a rerun of a finished step stays `ok`), and its detail says
  that Caddy was restarted and what port 443 answered.
- **CKS-005** `Request::Proxy` carries the host name to ask as `https: Option<String>`
  (`#[serde(default)]`); a name that is not a plain DNS name (letters, digits, `.` and `-`) is
  not asked: the step fails saying why, before it runs anything.

## Plan

- [x] Read the move's proxy step, its caller and tests, and write the spec
- [x] Failing fake-host tests: unchanged files still reload; reload fails; HTTPS refused
- [x] Proxy step reloads Caddy and checks HTTPS on 443 on the new host
- [x] The old host passes the public host name; flow tests updated
- [x] ADR, docs/operations.md and docs/changes.md updated
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Evidence recorded and the spec finished for green-CI delivery

## Acceptance criteria

- **AC-1** Caddy active and enabled, files unchanged in this run: the step runs `caddy
  validate`, then `systemctl restart caddy` (no reload), then the HTTPS check, and its line is
  `ok`. (CKS-001, CKS-004)
- **AC-2** `systemctl restart caddy` fails: the step fails with its reason and
  `journalctl -u caddy`, and no HTTPS check runs. (CKS-001)
- **AC-3** Port 443 refuses (curl exit 7) or times out (exit 28): the step fails with
  `systemctl status caddy`, `journalctl -u caddy` and the no-site-on-443 limit in its detail.
  curl exits 127 or does not start: the step fails saying to install curl. (CKS-002)
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
flow tests (`tests/flow.rs`) assert the request the old host sends, and that an `http://`
public URL asks nothing on port 443. No real Caddy runs in the tests; the reviewer probed the
step's curl command, with the port moved from 443 to 18443, against a real Caddy 2.11.9 for the
exit codes the step reads.

## Decisions

- Restart always, instead of a reload or proving the loaded configuration through Caddy's
  admin API: a reload never reads the unit's environment, the admin API can be off or moved,
  and a restart costs nothing before the cutover.
  [ADR-0431](../../docs/decisions/0431-the-move-restarts-caddy-and-asks-its-own-port-443.md)
- Ask the new host's own port 443 with `curl` (argv, no shell) rather than from the old host:
  before the cutover the public name may lead to the old host, and the new host's loopback
  is what the carried site must answer on. A missing certificate is not a failure at this
  point (CKS-003); the post-start HTTPS wait still reports it.

## Evidence

- Test-first: at `0c953e6ff` the five new fake-host proxy tests failed against the old `proxy()`.
  The clearest failure is `a_running_caddy_is_reloaded_although_the_files_did_not_change`: no
  reload ran on unchanged files. After round 1, the eight rewritten or new tests failed against
  the round-1 code before `1c95e1cdb`. These were the restart, missing-curl and failed-restart
  cases, and the flow test with an `http://` URL.
- `cargo nextest run -p yulius --lib -E 'test(/migrate::/)'`: 185 passed, 500 skipped, on
  `cd5246a4e`. `cargo clippy -p yulius --all-targets` was clean, and `scripts/dev quick` passed.
- Mutations: removing the old host's `https://` filter fails
  `an_http_public_url_asks_no_port_443`. Removing the `Unasked` arm fails
  `a_missing_curl_fails_saying_so`.
- Real Caddy probes (by the reviewer; Caddy 2.11.9, curl 8.18, port 18443 instead of 443, the
  sandbox's `https_proxy` set):
  - A default `:80`-only Caddy gives curl exit 7.
  - A site without a certificate gives exit 35.
  - `tls internal` with the upstream down answers exit 0 with `502`.
  - Without `--noproxy`, the same call failed with `CONNECT tunnel failed, response 403`.
  - The round-1 reviewer also reproduced the pre-existing defect: a reload left Caddy without
    the drop-in's `YULIUS_PROXY_SECRET`, and only a restart gave it the secret. That defect is
    fixed here by restarting.
- Independent review: round 1 requested changes (three majors, four minors, four nits), all
  answered in `1c95e1cdb` and `d671e95c1`. Round 2 approved `cd5246a4e` with two nits: the
  certificate wording and this Test plan's claim about the probe. Both are fixed after the
  approval.
- Yulius CI: the round-1 head `0f17e0496` passed its run. The merged head's run is in the
  task's delivery evidence.
- Security scan: `yulius security-scan` printed "Security scan disabled" (Vidoc is off for
  this project). It did not run and is not a pass.
- Not tested: a real move between two hosts with a public CA. The certificate is still proven
  only by the HTTPS wait after the start (MIG-039).
