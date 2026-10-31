---
yulius-task: e43c3936-2d99-4851-bfd3-837e875359be
status: done
---

# SPEC-361: A full proxy share or relay shows on the panel, with whose connections they are

Status: implemented
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
  open, read at start, on `connections.updated`, on `resync` and when the event stream reconnects.
  Its text says what is full and for which sandbox; its title says the reason, since when, whose
  the connections are ("task #12's run holds 120 of them"), and what to do. With one trouble whose
  busiest holder is a task, it leads to that task. The pill goes away when the trouble ends.
- **CHR-006** Each trouble that begins adds one entry to the notification center (and the
  desktop, when turned on), keyed by the trouble, so refusals within it never add another. The
  browser keeps the ids it notified apart from the list, so clearing the list or reloading does
  not notify an open trouble again.
- **CHR-007** The journal warnings of SPEC-353, SPEC-355 and SPEC-357 stay as they are; a
  trouble's beginning and end are logged once each (`connection trouble began`, `… ended`).

## Plan

- [x] Spec and ADR for connection troubles
- [x] The domain keeps connection troubles with their holders
- [x] The egress proxy reports a full share and a full proxy, and their end
- [x] The server's sandbox relay reports a full relay, and its end
- [x] The API lists troubles a person may see, named by task, and publishes their changes
- [x] The panel shows a pill and notifies once per trouble
- [x] Operations guide, architecture, changes
- [x] Independent review round 1, findings fixed test-first
- [x] Independent review round 2, findings fixed test-first
- [x] Independent review round 3 approves the head
- [x] Gates green, pull request merged into dev on green CI

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

## Review

Independent reviews in fresh sessions (skill `yulius-review`; this repository has no
`om-code-review`), read-only over `origin/dev...HEAD`:

- Round 1 on 97025309 (request changes). All own code, all fixed test-first:
  - Major: a trouble could stay open for good. `slot()` and `Places::admit` reported a refusal
    after releasing the table, and a close checked for an open trouble before taking it, so the
    closes that freed the limit could all slip between the refusal and its report. Both now report
    and check under the table's lock (the registry never calls back). Red first:
    `relay::tests::a_trouble_begun_while_every_place_closes_still_ends` ("the trouble never ends").
  - Minor: a lost `connections.updated` (the server sends `resync`) left the pill stale; it now
    reads again on `resync` (e2e red first).
  - Minor: the title's refusal count looked live while nothing publishes refusals within a
    trouble; it now says since when.
  - Minor: Clear (or 100 newer entries) made an open trouble notify again; the notified ids are
    kept apart, pruned to the open troubles (e2e red first: Clear and a reload notified again).
  - Nits fixed: the pill leads to a task only with one trouble; an event whose sandbox id does not
    parse reaches nobody rather than everyone; a long line in the product summary. Nit kept: the
    proxy and the relay each read the wall clock (`wall_ms`), one line each in their own crate.
- Round 2 on f00e5afa (request changes): round 1's fixes verified, no deadlock. Own code, fixed:
  - Minor: the e2e reload step depended on timing: the reseeded mock answered the first read with
    no troubles, which pruned the notified ids. The mock now keeps its troubles across a reload,
    as the server would list them, and the step reloads the page. A mutation that ignores the
    notified ids fails it.
  - Minor: the proxy half of round 1's race had no test. Its locked admission and report are now
    `Slots::admit`, and `proxy::tests::a_trouble_begun_while_every_slot_closes_still_ends`
    covers it (a mutation that reports after releasing the table fails it: "the trouble never
    ends").
  - Nit: `on_change` runs under the reporter's table lock; its doc now says it must be quick,
    never block and never call back.
  - Nit, accepted with a comment: a sandbox relay replaced while its old connections live may end
    the new relay's trouble early with its old count; the next refusal begins another.

- Round 3 on acbabf14: approve, no critical, major or minor findings; the mock's sessionStorage
  does not leak between tests (a new browser context per attempt). Two nits fixed after it: the
  proxy keeps one share for its refusals and its troubles (`Slots::share`), and the callback's
  doc says "a log line and an event sent at most" instead of "never block".

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

## Evidence

- **Acceptance (task #344's done criterion).**
  `connection_health::a_full_share_is_listed_with_the_runs_task_until_its_tunnels_end`
  (`crates/yulius/tests`) goes through the real egress proxy wired to the app. A seeded task
  run fills its sandbox's share (4) with `CONNECT` tunnels and is refused twice (`503`).
  - `GET /api/connections` then lists one `share` trouble for `s1` ("app"): held 4 of 4, refused
    2, its holder `task` t1 with the task's number, title and count 4.
  - A person who cannot view the sandbox sees none.
  - Once the tunnels end, the API lists none.
  - Exactly two `connections.updated` events were published (begin, end), scoped to `s1`.
  - Red first without the route.
- **Red before each fix:**
  - Proxy: `a_full_share_is_a_trouble_naming_the_run_until_its_tunnels_end` and
    `a_full_proxy_is_a_trouble_naming_the_sandboxes_until_it_has_room`, red at 8721b786 with
    the field only.
  - Relay: `a_full_out_sock_is_a_relay_trouble_until_its_connections_end`, red with its report
    off.
  - Round 1:
    - `a_trouble_begun_while_every_place_closes_still_ends`, red at d70cda13 ("the trouble never
      ends").
    - The e2e for resync, Clear and the single-trouble link, red before bcf7a955.
    - The e2e "notifies once" step, red with `once: false`.
  - Round 2: `proxy::tests::a_trouble_begun_while_every_slot_closes_still_ends`, and the e2e
    reload step, each red under a mutation (report after releasing the table; notified ids
    ignored).
- **Gate.** `scripts/dev ci` on 7445e68f exited 0:
  - doctor, the repository check, script tests, `cargo check`, fmt and clippy `-D warnings`
    (Linux and macOS);
  - nextest: 2915 passed, 16 skipped;
  - doctests;
  - `cargo deny`: advisories, bans, licenses and sources ok;
  - `cargo shear` and docs.

  After 7445e68f came a clean merge of dev (#990, `cargo check --workspace --all-targets` clean)
  and the round 3 nits (egress and domain tests: 436 passed; clippy clean). Yulius CI runs the
  gate on the delivered head.
- **e2e.** `npx e2e run` on 7445e68f: 794 passed, 1 skipped (71 files).
  - An earlier full run at load average ~30 failed `automations.e2e.ts` ("the shipped automations
    are the defaults…", 66 s) and `filedrop.e2e.ts` once each. Both files passed 3 of 3 runs
    alone, and the next full run passed.
  - These files neither touch nor depend on this change. They were timeouts under the host's
    load, not a regression.
- **Security.** `yulius security-scan` exited 2 (not run): Vidoc refused the instance's key
  (HTTP 401), and 24 changed files were not scanned. This is unrun, not passed. Yulius CI's
  `security` job scans the pull request head.
- **Not testable here:**
  - The production host's proxy under quartio-v2's real load.
  - A real bwrap holder's forward, which is not covered by design.
  - Desktop notifications from the operating system: the e2e checks the notification center's
    entries, not the OS popup.

