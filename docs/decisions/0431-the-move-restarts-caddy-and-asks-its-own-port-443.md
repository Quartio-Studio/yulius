# 0431. The move restarts Caddy and asks its own port 443

## Status

Accepted for [SPEC-592](../../.ai/specs/SPEC-592-instance-move-caddy-keeps-serving-its-pr.md).
Refines the reverse-proxy step of [SPEC-460](../../.ai/specs/SPEC-460-run-migration-one-command-moves-the-whol.md) (MIG-027).

## Context

On the 2026-11-21 move to the staging VM, Caddy on the new host had been started by its package
with the default `:80` site before the carried Caddyfile arrived. The move's reverse-proxy step
skipped the reload because Caddy was active and enabled and the files were reported unchanged in
that pass (an earlier pass had written them). Caddy served only port 80, and the move waited on
HTTPS after the start until a person reloaded it by hand. Whether the files changed in a pass says
nothing about what the running Caddy loaded. The review also showed that even the reload the step
did on a changed pass never gave a running Caddy the environment of the carried
`caddy.service.d` drop-in (the proxy secret): systemd reads it only when Caddy starts.

## Decision

After `caddy validate` accepts the carried Caddyfile, the step runs `systemctl daemon-reload`,
`systemctl enable caddy` and `systemctl restart caddy` on every pass. The step runs only before the
cutover (or after a rolled-back one), when the new host serves nobody, so a restart costs nothing.
With an `https://` public URL it then asks the new host's own port 443 for the public host with
`curl --disable --noproxy '*' --insecure --resolve <host>:443:127.0.0.1`. Any HTTP answer passes:
the instance does not run yet, so a 502 is expected. A failed TLS handshake on an open port passes
with a line saying the certificate comes once the name leads to the new host. A refused or
timed-out port fails the step, pointing to `systemctl status caddy` and `journalctl -u caddy`; a
curl that cannot run fails it saying to install curl.

## Alternatives

- Reload, and restart only when the reload fails. A reload loads the Caddyfile but not the unit's
  environment, so a carried drop-in's secret would stay missing while the check passes.
- Compare the configuration Caddy loaded (its admin API) with `caddy adapt` of the carried file and
  restart only on a difference. The admin endpoint can be off or moved, comparing adapted JSON is
  fragile, and it would not see the unit's environment either.
- Check HTTPS from the old host. Before the cutover the public name may still lead to the old host,
  and a public CA cannot issue the certificate yet, so that check cannot tell a missing site from a
  missing DNS change.
- Require a valid certificate at this step. That would stop every move whose DNS record changes
  after the start, which is the documented order.

## Consequences

The restart is what guarantees that Caddy runs the carried configuration and environment. The
port-443 check only catches a Caddy with nothing open there, as in the incident: a TLS failure
looks the same for a name the running configuration does not serve. Every pass restarts Caddy on
the new host. The step needs `curl` there (present on Ubuntu and Debian server images). A Caddy
with no site on port 443, with TLS ending in front of it, cannot be moved with an `https://` public
URL; the failure says so. The certificate itself is still proven only by the HTTPS wait after the
start.
