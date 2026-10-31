---
yulius-task: 4cfd3777-a8cf-412d-896a-2dd86ddcb606
status: in-progress
---

# SPEC-358: Sessions keep Yulius MCP, waits and delivery working across Yulius restarts

Status: in progress
Date: 2026-10-31
Owner: Yulius task #343
Scope authorization: task #343, follow-up 2 of task #342 (SPEC-357). Builds on SPEC-355 (the
Superagent's link test and restart in place) and SPEC-290 (a Stop Yulius did not answer).

## Goal

A Yulius restart is invisible to a task. A task session's Yulius tools work again once Yulius is
back, with no `/mcp`. A `yulius wait` running across the restart keeps waiting. A task that
went idle while Yulius was down gets "Yulius is back; continue" once it is back. An armed merge
whose checks already passed merges right after the restart.

## Context

### What happened

On 2026-10-31, after each Yulius outage in the quartio-v2 sandbox (`7c46cdac-…`), task sessions
kept a dead Yulius MCP connection: "MCP server yulius is not connected", or `ECONNRESET` at
startup or on reconnect. They could not call `ci_status`, `ci_rerun` or `delivery_merge` until a
person ran `/mcp`, even after Yulius was back. `yulius wait` gave up after its loop, and nothing
woke the tasks.

### How Claude Code treats a failed HTTP MCP server (2.1.292, read from the shipped CLI)

- A task run gets the Yulius MCP as `{"type": "http", "url": "<YULIUS_URL>/mcp"}` with the task
  token as bearer (`yulius-harness::launch::plan_claude`).
- When a remote (`http`, `sse`) transport closes, Claude Code reconnects at most 5 times
  (`see=5`), with a backoff from 1 s up to 30 s. Then it gives up ("Max reconnection attempts (5)
  reached") and leaves the server `failed`.
- A failed initial connect is retried only 3 times (`W6=3`), and only for some errors (5xx,
  `CONNECT_TIMEOUT`, or any `sse` failure).
- A tool of a server that is not connected answers "MCP server yulius is not connected".
- No hook output and no setting asks Claude Code to reconnect a server. Only `/mcp` or a new
  process does that.
- So any outage longer than about a minute, which every Yulius restart and update is, leaves the
  session without Yulius tools for good. SPEC-355 hit the same wall for the Superagent and
  solved it with a restart in place.

### What a restart loses on the server today

- **`yulius wait`** (`crates/yulius/src/wait_cli.rs`) gives up after 5 failed requests, about
  50 s of retries. It exits 1 with "Yulius cannot be reached". Claude Code wakes the agent with
  that line while Yulius is still down. The agent can do nothing, so it ends its turn, and nothing
  wakes it once Yulius is back.
- **A turn that ends while Yulius is down.** Its Stop hook fails open, so Yulius never settles
  that turn. The turn's push and pull request (`auto_open_pr`, `finish.rs`), its arming and its
  chains wait for a next turn that nobody starts. SPEC-290 GDW-001 settles a Stop Yulius failed to
  answer only while Yulius runs (`hooks_seen` is memory and false after a restart).
- **An armed merge** survives (`armed-merges.json`, DLV-205). One whose checks passed while Yulius
  was down merges only at the delivery loop's first tick, about 3 minutes after the start.
- **What already survives:**
  - CI runs that were running are re-queued (`ci_recover`), and a `ci_rerun` is a queued run in the
    database.
  - Pushes made by a turn end that Yulius did answer have already happened.
  - Task runs are reattached (`reconcile`).
  - The egress proxy and the git credential helper take the run's task token from the database.
  - A CD or environment run cut by the restart reads `error` (SPEC-134, SPEC-347). A person or the
    agent starts it again. This is out of scope.

## Requirements

- **MSB-001** A task run reaches the Yulius MCP through a stdio bridge, `yulius mcp`, started by
  Claude Code. The bridge forwards each JSON-RPC message to the run's `POST /mcp`, with the task
  token as bearer. It writes each answer back as one line. The bridge never exits while Claude
  Code runs, so Claude Code never sees the Yulius MCP fail.
- **MSB-002** A message that Yulius never read is sent again until Yulius answers, for at most
  `OUTAGE_WAIT` (10 min). "Never read" means the connection failed before Yulius asked for the
  body (`Expect: 100-continue`). Past the bound, the call answers an error saying that Yulius could
  not be reached and the call was not made. Any later call is tried again.
- **MSB-003** A message that Yulius may have read is sent again only when it changes nothing:
  `initialize`, `ping`, `tools/list` or a notification. A `tools/call` that fails after its body
  was sent answers an error saying that Yulius restarted during the call and that it may have taken
  effect. Read its state before calling again.
- **MSB-004** A call that Claude Code cancels (`notifications/cancelled`) while the bridge holds
  it is never sent afterwards. At most `MAX_IN_FLIGHT` (64) messages are in flight. Beyond that a
  request is answered an error at once. Answers larger than 32 MiB, or lines longer than 4 MiB,
  are answered an error. Each answer is one line of JSON.
- **MSB-005** `yulius wait` rides out an outage. It keeps asking with a backoff of at most 10 s
  until its own deadline. It says once that Yulius cannot be reached and that it keeps waiting.
  When the deadline passes while Yulius is still down, it ends (exit 1) saying so, and that Yulius
  wakes the task once it is back. A refused token still ends it at once.
- **MSB-006** Yulius writes the time it was last alive to `<data>/alive-at` every `ALIVE_EVERY`
  (30 s). At the start, the time read from that file is when it went down.
- **MSB-007** After a restart, Yulius waits `WAKE_GRACE` (90 s) so that the reattached runs replay
  their transcripts. Then it types "Yulius is back; continue" once into each reattached task whose
  session is idle and whose last turn closed after Yulius went down. "Idle" means no turn open
  and the task Running. "Closed after" means the transcript's record that closes the turn is later
  than `alive-at`. This covers a wait that died, the turn its exit woke, and a turn whose Stop went
  unanswered. A task is not woken in these cases: its goal is achieved, it has an open `needs_you`
  or pause, a person typed within the last minute, or the start knows no `alive-at`. The text is
  typed with the guards of a typed message (SPEC-230 TMS-004), unlabelled, and is not a person's.
  It amends SPEC-028 MSR-004 for this single case.
- **MSB-008** Right after the armed merges are restored at the start, Yulius tries them once, as
  its delivery loop would. An armed merge whose checks passed during the outage merges then.

## Plan

- [x] Read SPEC-355 and SPEC-357, and find what a restart loses
- [ ] Spec and ADR for the stdio bridge and the wake-up
- [ ] `yulius mcp` bridges stdio to the Yulius MCP across a restart
- [ ] Task runs reach the Yulius MCP through the bridge
- [ ] `yulius wait` rides out an outage until its deadline
- [ ] Yulius wakes the tasks that went idle while it was down
- [ ] An armed merge whose checks passed merges right after a restart
- [ ] A restart test: a running session uses MCP, git and gh afterwards
- [ ] Architecture, operations guide and changes
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Gates green, pull request merged into dev on green CI

## Acceptance criteria

- A session's `yulius mcp` bridge is set up before a restart. While the server is down, a call is
  held. Once a new server listens on the same address, the call is answered, with no new process
  and no `/mcp`. A `tools/call` cut after Yulius read it is not sent twice.
- `yulius wait` started before a restart longer than its old 50 s of retries ends with the
  outcome after the restart, not with "Yulius cannot be reached".
- After a restart, an idle reattached task whose last turn closed after `alive-at` gets "Yulius is
  back; continue" once. A task idle since before the outage, and a task mid-turn, get nothing.
- An armed merge whose checks passed during the outage merges at the start, with no delivery
  tick.
- After the restart the run's token still opens the git credential helper, `gh-token` and the
  egress proxy.

## Test plan

- Unit (`yulius-git::credential`): `agent_call` tells "not sent" from "lost after sending" for a
  server that closes before `100 Continue` and one that closes after reading the body.
- Integration (`yulius/tests/mcp_bridge.rs`): the real `yulius mcp` binary is run against a real
  internal router. The test restarts the server on the same port, and the bridge's held call is
  answered. It also covers a cut `tools/call` that is not resent, a cancelled held call that is
  never sent, and a refused token. The restart test then asks the git credential, `gh-token` and
  the egress client with the run's token from the new server.
- Integration (`yulius/tests/delivery.rs`): `yulius wait` runs across a restart of the server, and
  an armed merge with passed checks merges at the restore.
- Integration (`yulius/tests/supervision.rs` or `task_message.rs`): the wake. The test covers the
  typed text for an idle task whose turn closed after `alive-at`, and nothing for one idle since
  before, one mid-turn, one with `needs_you`, and one without `alive-at`.
- Unit (`yulius-harness::launch`): the run's MCP config names `yulius mcp` with only `${VAR}`
  references.

## Decisions

- ADR-0244: a task run reaches the Yulius MCP through a stdio bridge that outlives Yulius
  restarts, and Yulius wakes the tasks that went idle while it was down. Alternatives considered:
  - A hook or the `yulius` wrapper triggering a reconnect: Claude Code has no such interface.
  - Restarting the task's session in place as SPEC-355 does for the Superagent: it cuts a turn in
    progress, and it needs a detection of the dead client that the server cannot see.
  - Answering `initialize` and `tools/list` in the bridge without Yulius: the tools would be a
    stale copy.
- The bridge resends only what Yulius never read (`Expect: 100-continue`), or what changes
  nothing. Tool calls are not all idempotent (`task_create`, `issue_create`, `record_number`), and
  the server keeps no call ids across a restart.
- The wake keys on the transcript's turn-closing record against `alive-at`, not on a record of
  open waits. Each way a wait ends during an outage (the old client's give-up, the new client's
  deadline) wakes the agent through Claude Code, and the turn this starts closes while Yulius is
  down. The same key also covers a turn whose Stop went unanswered. One persisted time replaces a
  table of waits.
- The Superagent keeps SPEC-355's restart in place: its config is inline JSON with its own bearer,
  and it is a person's session. Moving it onto the bridge is a later change.

## Evidence

Pending.
