---
yulius-task: e8f48332-16c1-47db-a0d7-77469c08b52b
status: in-progress
---

# SPEC-335: The Superagent starts tasks in other sandboxes once its person approves each

Status: in-progress
Date: 2026-10-26
Owner: Yulius task #319
Scope authorization: task #319 (`/goal`): "Superagents should be able to talk to other sandboxes
(spawn task in them) but every action like this should require explicit user's acceptance."
Extends SPEC-037 MGT-006 (the Superagent's tools) and
[ADR-0029](../../docs/decisions/0029-cockpit-manages-through-mcp.md); decided in
[ADR-0227](../../docs/decisions/0227-the-superagent-starts-tasks-in-other-sandboxes-only-on-its-persons-approval.md).

## Goal

A person's Superagent in sandbox A can start a task in another sandbox B of the same person
("make a task in the website repo to update the API client"). Nothing reaches B until the person
approves that one request, in the Yulius web UI, beside the Superagent: every request is shown
in full and answered once, and a declined, unanswered or abandoned request creates nothing.

## Context

- The Superagent (one per person and sandbox, `cockpit.rs`) calls the Yulius MCP with a `ck_`
  token that `cockpit_caller` turns into a `ProjectCaller` pinned to its own sandbox. Every
  management tool (`task_create`, `task_message`, …) acts in that sandbox only.
- On a contained server the Superagent bypasses Claude Code's approvals (ADR-0165), so Claude
  Code's own permission prompt cannot be the acceptance: it is not shown, and when it is (an
  uncontained server) "always allow" ends it. The acceptance must be enforced by the server.
- The remote MCP (SPEC-247) already lets a person's *own* agent create tasks in any project with
  their operate token: that is the person acting. The Superagent crosses into another sandbox
  on text it reads (repository files, transcripts), so its reach is the risk here.
- Existing pieces reused: `App::tool_task_create` (New task's rules, the hourly bound, write
  access), `App::visible_sandboxes`, the remote MCP's project lookup (owner/name, slug or id),
  `AuthContext::is_browser`, `Events::publish_to_user`.
- The Superagent's panel (`web/assets/agent.js`) shows a note above its terminal; the person who
  talks to the Superagent is looking at that panel when it asks.

## Requirements

- XSR-001: the Superagent is served `sandboxes_list`: the other live sandboxes its person can
  work in (write access), each with its slug, repository (or playground) and onboarding state.
  It only reads, so it is pre-allowed on an uncontained server. Task agents are not served it.
- XSR-002: the Superagent is served `sandbox_task_create {sandbox, prompt, title?, workflow?,
  mode?, after_task?, start_rule?, supertask?, prepared?, start_at?, model?, effort?}`. `sandbox`
  is a slug, owner/name or id from `sandboxes_list`. Its own sandbox is refused (use
  `task_create`); a sandbox the person cannot work in is refused as not found; unknown arguments
  and an empty prompt are refused; nothing is asked in those cases. Task agents are not served it.
- XSR-003: a valid call asks the person: it records a request in memory and waits for the
  answer for at most 4 minutes (`CROSS_REQUEST_WAIT`). Approved, the task is created in the
  target sandbox through `tool_task_create` for the person, exactly as the request showed it
  (no rewritten prompt), and the call answers with the task (number, sandbox, status). Declined,
  it answers that the person declined and nothing was created. Unanswered, it answers that
  nobody answered and nothing was created, and the request is gone.
- XSR-004: a request ends with the call: when the wait ends, the call is dropped (the client went
  away), the Superagent's client cancels it (`notifications/cancelled` with its JSON-RPC id, as
  Claude Code sends on Esc while it may keep the HTTP request open; the call answers
  `cancelled`), or the server restarts, the request disappears and can no longer be approved. An
  approval already taken is completed even if the person's page goes away meanwhile. At most 3 requests wait per person and source sandbox; a fourth is refused.
- XSR-005: `GET /api/sandboxes/{sid}/superagent/requests` lists the caller's own waiting requests
  of their Superagent in `sid` (target sandbox, title, the full prompt, the start options, when
  it ends). Another person's requests are never listed.
- XSR-006: `POST /api/sandboxes/{sid}/superagent/requests/{id}` with `{"approve": true|false}`
  answers one request. Only from a browser session (a cookie with CSRF): an API token (the CLI,
  a script, a remote agent) is refused with 403, so the Superagent cannot answer its own request.
  Only the request's person; another person's id is not found. An answered or ended request is
  not found. Approving rechecks the person's write access in the target and New task's rules;
  their refusal is the answer of both the click and the waiting call. The answer of an approval
  is the created task.
- XSR-007: every change of the waiting set sends `superagent.requests {sandbox_id}` to that
  person's event streams only; the event names nothing else.
- XSR-008: the Superagent panel shows each waiting request above the terminal: "Your Superagent
  asks to start a task in <sandbox>", the title, the whole prompt (scrollable), the start options
  that are set, and **Create task** / **Decline**. Characters a person cannot see (controls,
  bidirectional and zero-width marks) show as `⟨U+XXXX⟩`, and runs of two or more empty lines (lines
  of only spaces of any kind, line and paragraph separators counted as line breaks) as
  `[N empty lines]`, so a prompt cannot hide or reorder words; the server already refuses a
  request whose texts (prompt, title, options) hold such characters (Unicode tag characters,
  bidirectional and zero-width marks, blank fillers, variation selectors, private use), so what
  the person reads is all that reaches the task. Create task takes a click only once
  the end of the prompt was on screen (not clipped by the prompt box, the list, which scrolls
  within at most 60 % of the panel so the session keeps room, or the window; until then a hint
  says so),
  both buttons only 600 ms after the card appeared or moved up because another card went (a
  click aimed at the terminal it pushed down, or the second click of a double-click), and neither
  while an answer is on its way or after it succeeded. After an answer the card goes; an approval
  says which task was created with a link to it; a refused approval says why. It refetches on the
  event and on a resync.
- XSR-009: answers are audited: `superagent.cross_task` with `approved`, `failed`, `declined`,
  `cancelled` or `expired`, the source and target sandbox, never the prompt.
- XSR-010: the Superagent's MCP instructions say to find the sandbox with `sandboxes_list`, ask
  with `sandbox_task_create`, tell the person the request waits in the panel above, and never
  retry a declined request unless the person asks.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The Superagent lists the other sandboxes its person can start work in
- [x] sandbox_task_create waits for the person's answer and creates the task only on approval
- [x] Only the person, in a browser session, approves or declines a request through the API
- [x] The Superagent panel shows each request with Create task and Decline
- [x] Instructions, ADR, architecture and product docs describe cross-sandbox requests
- [x] Current dev merged and the gate's style, format and allowlist failures fixed
- [x] Review round 1 fixed: cancellation, detached approval, race tests, card guards
- [x] Review round 2 fixed: invisible characters refused on the server and named in the card
- [x] Review rounds 3 and 4 fixed: the list scrolls, read means the prompt's end was on screen, moved cards arm again
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-XSR-1: a Superagent in A calls `sandbox_task_create` for B; while it waits, the person's
  `GET` lists the request with the full prompt; the person approves from a browser session; the
  call answers with B's new task, created for the person with exactly that prompt, and the
  request is gone.
- AC-XSR-2: declined → the call says so and B has no new task; an unanswered request ends after
  the wait bound with nothing created; a dropped call removes its request.
- AC-XSR-3: an API token's `POST` is refused with 403 and the request still waits; another
  person cannot list or answer it.
- AC-XSR-4: the own sandbox, a sandbox without write access, unknown arguments and a fourth
  waiting request are refused without asking.
- AC-XSR-5: task agents are served neither tool; `sandboxes_list` is pre-allowed and
  `sandbox_task_create` is not.
- AC-XSR-6 (e2e): the panel shows a request card with its prompt and options; Create task
  creates the task and links it; Decline removes the card.

## Test plan

- Integration (`crates/yulius/tests/cross_sandbox.rs`): calls `App::tool_sandbox_task_create` with
  a Superagent `ProjectCaller` while the test answers through the router (cookie session with
  CSRF, and a bearer token for the refusal), on a harness whose clock is real for the wait bound
  (a test-only short bound through the app config). Covers AC-XSR-1..4.
- Unit (`mcp.rs` tests, `manage_tools.rs`): the served tool sets and the read-only allowlist
  (AC-XSR-5).
- e2e (`e2e/tests/superagent-requests.e2e.ts`) against the mock: AC-XSR-6.

## Decisions

- **Server-enforced acceptance, not Claude Code's prompt.** The bypass (ADR-0165) hides it, and
  "always allow" would end it; the server is the only place every request passes.
- **The call waits for the answer (at most 4 minutes).** The person who talks to the Superagent
  is at its panel, and the Superagent learns the outcome in the same turn, without a status tool.
  4 minutes stays below the 300 s headers timeout of Node's HTTP client that MCP clients commonly
  run on. Longer waits are an alternative in ADR-0227.
- **In memory, ending with the call.** A request lives exactly as long as the call that would
  report it; a restart or a dropped call leaves no request someone could still approve later
  without the Superagent knowing. No migration.
- **Approval only from a browser session**, as only a person's click; tokens (which an agent in
  the sandbox could hold) are refused.
- **Only task creation crosses.** Messaging, stopping or reading other sandboxes' tasks stays in
  their own Superagent; a later task can add them behind the same approval.
