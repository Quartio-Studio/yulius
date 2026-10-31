---
yulius-task: e43c3936-2d99-4851-bfd3-837e875359be
status: in-progress
---

# SPEC-361: A full proxy share or relay shows on the panel, with whose connections they are

Status: in progress
Date: 2026-10-31
Owner: Yulius task #344
Scope authorization: task #344, follow-up 3 of task #342 (SPEC-357, ADR-0243). Builds on SPEC-353
(LAP-002, the share and its warnings), SPEC-355 (SLR-001, SLR-002, the relays' give-way and
warnings) and SPEC-357 (ESR-004, ESR-005, whose the connections are).

## Goal

When the egress proxy refuses a sandbox at its share, when the proxy itself is full, or when a
sandbox's relay toward Yulius drops connections at its limit, the people of that sandbox see it on
the panel at once, with the reason and whose connections they are ("quartio-v2 is at its proxy
share: task #12's run holds 120 of its 128 connections"). They are notified once per episode, not
on every refusal, and the notice goes away once places are free again.

## Context

- On 2026-10-31, quartio-v2's tasks found out one by one that the proxy answered `503` for every
  host. The cause, a sandbox at its 128-connection share, was visible only in the journal
  (`egress proxy: a sandbox holds its share …`, SPEC-353 LAP-002). Nobody looking at Yulius saw
  it.
- The proxy already knows everything the notice needs at a refusal (SPEC-357): the sandbox, the
  asking run (`EgressClient::use_key`), `Connections::count_of` and `most_of_others`. At its own
  limit it knows the sandboxes' counts. It logs these as warnings at most once a minute per kind
  and sandbox (`warn_due`), and keeps nothing once the minute is over.
- The server's `out.sock` relay of each sandbox (`relay::serve_outbound`) warns
  `sandbox relay: a connection was dropped at the limit` (SPEC-353, SPEC-355). It is the server's
  half of the sandbox's relay toward Yulius. The holder's forward, inside the sandbox, writes its
  warnings to `holder.log` only; its link is tested from the server only for a Superagent
  (SPEC-355 SLR-005).
- The web app shows instance conditions as pills in the tab row (`diskalert.js`, SPEC-276
  RDA-007: `GET /api/disk` read again on `disk.updated`) and keeps one notification per subject
  in its notification center (`notifications.js`, `key`), sent to the desktop when the person
  turned that on. That is how Yulius notifies people today; this spec uses it.
- Related, not repeated: the Superagent's lost-link notice (SPEC-355 SLR-006) stays as it is.

## Requirements

- **CHR-001** Yulius keeps the open *connection troubles*: one per sandbox at its proxy share
  (`share`), one per sandbox whose `out.sock` relay dropped a connection at its limit (`relay`),
  and one for the proxy at its own limit (`proxy`). A trouble begins at the first refusal there,
  records when it began, how many connections were held of how many, how many refusals it has
  seen, and, at the share and the proxy's limit, the busiest holders with their counts (runs of
  the sandbox; sandboxes of the proxy), busiest first, at most 3, updated at each refusal.
- **CHR-002** A trouble ends once its limit has room again: when the connections held there fall
  to at most seven eighths of the limit (112 of a share of 128, 224 of 256). Ending at the first
  free place would show and hide the notice with every connection while a busy sandbox stays at
  its limit. A refusal after the end begins a new trouble.
- **CHR-003** Beginning and ending a trouble is published at once as `connections.updated`, to the
  people who can view its sandbox (the proxy's own, to everyone signed in). A refusal within an
  open trouble updates its counts but publishes nothing.
- **CHR-004** `GET /api/connections` lists the open troubles a person may see: those of the
  sandboxes they can view, and the proxy's own. Each names its sandbox, its limit, since when,
  the counts and its holders. A run is named by its task (number, title, id) when it is an agent
  run, as "CI job" for a CI job, and as "Superagent" for a Superagent's shell. At the proxy's
  limit a sandbox the person cannot view is named "another sandbox".
- **CHR-005** The panel shows a warning pill in the tab row while a trouble the person may see is
  open, read at start, on `connections.updated` and when the event stream reconnects. Its text
  says what is full and for which sandbox; its title says the reason and whose the connections
  are ("task #12's run holds 120 of them"), and what to do. With one trouble whose busiest holder
  is a task, it leads to that task. The pill goes away when the trouble ends.
- **CHR-006** Each trouble that begins adds one entry to the notification center (and the
  desktop, when turned on), keyed by the trouble, so refusals within it never add another.
- **CHR-007** The journal warnings of SPEC-353, SPEC-355 and SPEC-357 stay as they are; a
  trouble's beginning and end are logged once each (`connection trouble began`, `… ended`).

## Plan

- [x] Spec and ADR for connection troubles
- [ ] The domain keeps connection troubles with their holders
- [ ] The egress proxy reports a full share and a full proxy, and their end
- [ ] The server's sandbox relay reports a full relay, and its end
- [ ] The API lists troubles a person may see, named by task, and publishes their changes
- [ ] The panel shows a pill and notifies once per trouble
- [ ] Operations guide, architecture, changes
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Gates green, pull request merged into dev on green CI

## Acceptance criteria

- One run fills its sandbox's share (tunnels active within the floor) and asks once more: the
  request gets `503`, and `GET /api/connections` lists a `share` trouble of that sandbox naming
  the run's task and its count. A second refusal adds no event. Once the run's tunnels close, the
  trouble ends and the API lists none.
- A full `out.sock` lists a `relay` trouble of its sandbox, which ends once its connections end.
- A person who cannot view the sandbox does not see its trouble, and is not sent its event.
- The panel shows the pill with the reason and the task, and one notification for the trouble,
  and hides the pill once it ends.

## Test plan

- Unit (`yulius-domain::connection_health`): begin, update without a change notice, end at the
  threshold and not before, a new trouble after the end, holders sorted and bounded.
- Integration (`yulius-egress/tests/proxy.rs`): real `CONNECT` tunnels fill a sandbox's share;
  the registry holds the trouble with the run and its count, then ends it once they close. Same
  for the proxy's limit.
- Unit (`yulius-sandbox::relay`): a full `out.sock` begins a `relay` trouble, which ends once its
  connections end.
- Integration (`yulius/tests`): through the HTTP API, a trouble reported to the app's registry for
  a seeded agent run is listed with the task's number and title, hidden from a person who cannot
  view the sandbox, and gone once it ends; `connections.updated` reaches the event stream.
- e2e (`e2e/tests`): the pill, its title and one notification, then the pill gone.

## Decisions

- ADR-0246: connection troubles are kept in one registry in the domain, which the proxy and the
  server's relays write and the app reads, and are shown as a pill and one notification per
  trouble. Polling the proxy from the app was rejected: the notice must come at the refusal.
  Parsing the journal was rejected: it is rate-limited text. A new notification channel (mail,
  chat) was rejected: Yulius has none, and its notification center already reaches the desktop.
- The holder's forward inside the sandbox is not covered: it runs in another process, and the
  server would need a new channel to hear it. Its `holder.log` and, for a Superagent, the link
  test of SPEC-355 still report it. Each forward connection opens one `out.sock` connection, so
  a full forward usually shows as a full `out.sock` too.
- The processes holding a run's connections are not listed: the proxy sees the run's credential,
  not the process (connections arrive through the relay), and the sandbox's loopback port 8081
  serves the internal API as well. The notice leads to the task, whose background processes the
  task's panel already lists, and the operations guide keeps the `nsenter … ss` recipe.
- The state is in memory: a server restart empties the proxy's table and the troubles with it.
