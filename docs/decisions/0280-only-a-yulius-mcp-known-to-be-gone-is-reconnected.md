# 0280. Only a Yulius MCP known to be gone is reconnected

## Status

Accepted. Specified in
[SPEC-436](../../.ai/specs/SPEC-436-previously-we-fixed-discnnection-error-b.md); task #418,
issue #1108. Amends [0276](0276-a-session-whose-yulius-mcp-went-silent-is-reconnected-and-a-superadmin-restarts-yulius.md)
("a silent session is reconnected").

## Context

ADR-0276 types `/mcp reconnect yulius` into an idle session when nothing reached `POST /mcp` from
it for 3 minutes, expecting each bridge to ping every minute. A running sandbox keeps the `yulius`
binary of the base it started on, so after the update that brought the watch its bridges were an
older build that never pings, and so was every bridge `/mcp reconnect` started. Each reconnect was
answered by that bridge's `initialize`, which reset the watch, and the session was reconnected
again 3½ minutes later: 129-130 times per idle session overnight on 2026-11-07, while Claude
Code's logs show the server itself never dropped. A person saw a server that kept disconnecting.

## Decision

The watch reconnects a session only when its Yulius MCP is known to be gone:

- its bridge pinged on this server and then went silent for 3 minutes (a bridge pings every
  minute whatever the session does), or
- the run was spawned by this server and its MCP client never reached it in 3 minutes (it never
  connected).

A session heard but never pinged, and a reattached run never heard, are left alone: their bridge
may be an older build that is alive. Spacing, backoff, holds and the continue text stay as in
ADR-0276.

## Alternatives

- **Give up after 3 reconnects answered by a non-pinging bridge.** Still restarts a healthy server
  three times per session after every server start.
- **Restart sandboxes on the new base after an update**, so every bridge pings. Cuts every running
  session to fix what is only a wrong inference.
- **The bridge pings at once on start.** Would make "heard, never pinged" certain to be an older
  bridge; it only narrows the gap of a new bridge dying in its first minute, so it is left out.
- **Remove the watch.** A real drop would again wait for a person, and the watch no longer touches
  a live server.

## Consequences

- No reconnect is typed into a session whose server may be connected; once every sandbox runs a
  pinging bridge the rule is the same as ADR-0276's for every real drop after the first ping.
- A bridge that dies before its first ping, and a run dropped before a server restart that
  reattached it, are not reconnected; `yulius tool` remains the fallback from the shell (once
  #1109 lets it through the session's wrapper).
