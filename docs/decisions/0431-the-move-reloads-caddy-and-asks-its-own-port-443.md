# 0431. The move reloads Caddy and asks its own port 443

## Status

Accepted for [SPEC-592](../../.ai/specs/SPEC-592-instance-move-caddy-keeps-serving-its-pr.md).
Refines the reverse-proxy step of [SPEC-460](../../.ai/specs/SPEC-460-run-migration-one-command-moves-the-whol.md) (MIG-027).

## Context

On the 2026-11-21 move to the staging VM, Caddy on the new host had been started by its package
with the default `:80` site before the carried Caddyfile arrived. The move's reverse-proxy step
skipped the reload because Caddy was active and enabled and the files were reported unchanged in
that pass (an earlier pass had written them). Caddy served only port 80, and the move waited on
HTTPS after the start until a person reloaded it by hand. Whether the files changed in a pass says
nothing about what the running Caddy loaded.

## Decision

After `caddy validate` accepts the carried Caddyfile, the step always runs `systemctl
reload-or-restart caddy`, and `systemctl restart caddy` when that fails (`caddy reload` needs the
admin endpoint). It then asks the new host's own port 443 for the public host with `curl
--insecure --resolve <host>:443:127.0.0.1`. Any HTTP answer passes: the instance does not run yet,
so a 502 is expected. A failed TLS handshake on an open port passes with a line saying the
certificate comes once the name leads to the new host. A refused or timed-out port gets one more
restart and then fails the step with the fix.

## Alternatives

- Compare the configuration Caddy loaded (its admin API) with `caddy adapt` of the carried file and
  reload only on a difference. The admin endpoint can be off or moved, and comparing adapted JSON
  is fragile. A reload of an unchanged configuration costs nothing before the cutover.
- Check HTTPS from the old host. Before the cutover the public name may still lead to the old host,
  and a public CA cannot issue the certificate yet, so that check cannot tell a missing site from a
  missing DNS change.
- Require a valid certificate at this step. That would stop every move whose DNS record changes
  after the start, which is the documented order.

## Consequences

A Caddy that keeps a stale configuration is caught before the freeze, with the command that fixes
it. Every pass reloads Caddy on the new host, which serves nobody yet. The step needs `curl` on the
new host (present on Ubuntu and Debian server images); without it the step fails and says to
install it. The certificate itself is still proven only by the HTTPS wait after the start.
