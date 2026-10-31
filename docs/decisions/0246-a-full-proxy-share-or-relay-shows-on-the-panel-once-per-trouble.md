# 0246. A full proxy share or relay shows on the panel, once per trouble

## Status

Accepted. Specified in
[SPEC-361](../../.ai/specs/SPEC-361-proxy-and-mcp-health-on-the-panel-with-t.md); task #344.
Builds on [ADR-0240](0240-a-run-s-strays-are-reaped-after-a-bound-and-a-sandbox-gets-half-the-proxy.md),
[ADR-0241](0241-a-full-relay-closes-its-longest-idle-connection-and-the-superagent-restarts-in-place.md)
and [ADR-0243](0243-the-busiest-run-gives-way-at-a-full-proxy-share.md).

## Context

On 2026-10-31 the egress proxy refused quartio-v2 at its 128-connection share for more than 90
minutes. The only trace was a journal warning, so the sandbox's tasks found out one by one, from
their own commands' `503`s, and its people did not find out at all. The proxy and the server's
`out.sock` relay know at the moment of a refusal what is full, for which sandbox and whose
connections fill it (ADR-0243), but they keep it only for the minute of their warning's rate
limit.

## Decision

- **One registry of connection troubles,** in the domain (`connection_health`), shared by the
  egress proxy, the server's relays and the app. A trouble is a limit refusing: a sandbox's proxy
  share, a sandbox's `out.sock` relay, or the proxy's own total. It begins at the first refusal and
  keeps when it began, the counts and, for the share and the total, the three busiest holders (runs,
  or sandboxes).
- **A trouble ends with room, not with the first free place:** once the connections held fall to
  seven eighths of the limit. A busy sandbox stays near its limit, and ending at the first free
  place would show and hide the notice with each connection.
- **Only a beginning and an end are published** (`connections.updated`, to the sandbox's viewers;
  the proxy's own to everyone). Refusals within a trouble update its counts silently.
- **The panel shows a pill in the tab row** (as the disk's, ADR-0205) read from
  `GET /api/connections`, naming the run's task, and the notification center gets one entry per
  trouble, keyed by its id, which reaches the desktop when the person turned that on.

## Alternatives

- **The app polls the proxy's table.** Rejected: a poll is late or busy, and the notice should come
  at the refusal.
- **Reading the journal's warnings.** Rejected: they are rate-limited text, written once a minute
  at most, and the journal is not the app's to read.
- **A notification by mail or chat.** Rejected: Yulius has no such channel today; the notification
  center already reaches the desktop.
- **Persisting troubles.** Rejected: a restart empties the proxy's table, and with it the trouble.
- **The holder's forward inside the sandbox reporting to the server.** Deferred: it needs a new
  channel from the holder. A full forward usually fills `out.sock` too (one connection each), and
  its `holder.log` and the Superagent's link test still tell it.

## Consequences

- A sandbox's people see a full proxy share within a refusal, with the task whose run holds the
  most, and once per trouble in their notifications.
- The registry is in memory and bounded by the sandboxes that have a trouble open.
- A trouble may stay open while its limit hovers between seven eighths and full without refusing;
  its counts are those of its last refusal.
- The processes holding the connections are not named: the proxy sees a run's credential, not
  its processes. The notice leads to the task, and the operations guide has the recipe.
