---
yulius-task: f3ac9ff7-41e6-4a4c-b161-7101b6275855
status: done
---

# SPEC-432: A session whose Yulius MCP dropped is reconnected, and a superadmin restarts Yulius

Status: implemented
Date: 2026-11-06
Owner: Yulius task #414
Scope authorization: task #414 ("fix so this never ever happen again. Also add 'restart' button in
superadmin that works as on update but without installing update (stop tasks + restart +
resume)"). Builds on [SPEC-358](SPEC-358-sessions-keep-yulius-mcp-waits-and-deliv.md)
(ADR-0244, the stdio bridge) and SPEC-160 (ADR-0131, an update freezes every task).

## Goal

A task session never stays cut off from its Yulius tools. When Claude Code drops the session's
Yulius MCP server, whatever the cause, Yulius notices and types `/mcp reconnect yulius` into the
idle session, then tells the agent to continue. Until then, and mid-turn, the agent can call any
Yulius tool from its shell with `yulius tool`. A superadmin can restart Yulius from the Instance
page: tasks are frozen, the service restarts with the same build, and the tasks are thawed.

## Context

### What happened

On 2026-11-06, in the FW-N2-Device sandbox, a task's session lost its Yulius MCP server partway
through a 3-hour `/goal` run: `delivery_merge`, `step_update` and `ci_status` were gone, while the
`yulius` CLI (hooks, `yulius wait`) kept working. The agent could not merge PR #48 (CI green,
review approved). Claude Code's `/goal` Stop hook blocked the turn 9 times, the goal paused, and
the agent told the person to run `/mcp`.

ADR-0244 made a Yulius outage a slow call: the session's `yulius` MCP server is `yulius mcp`, a
stdio bridge that holds calls until Yulius is back. It does not cover Claude Code dropping the
server itself. Claude Code 2.1.311 (installed by Yulius on 2026-11-05, ADR-0266) shows how it
can:

- A stdio server gets 30 s to connect (`MCP_TIMEOUT`, default `30000`), and 2.1.311 first sends a
  `server/discover` version probe with its own bound. A session started or resumed while Yulius is
  down or slow (an update's resume, a restart, a crash recovery) has its `initialize` held by the
  bridge past that bound, and the server is marked failed. The bridge is then closed (SIGINT,
  SIGTERM, SIGKILL) and never restarted.
- A failed server stays failed for the rest of the session. Nothing a hook returns reconnects it.
- Claude Code accepts `/mcp reconnect <server>` typed as input ("Usage: /mcp
  [reconnect|enable|disable [<server>|all]]"), which respawns a stdio server without a panel.

The server has no record of which sessions still have a bridge: an idle bridge sends nothing.

### Restart today

The update run (`yulius-app::updates`) already does "stop tasks + restart + resume": `draining`
freezes every task's agent (cgroup freeze, ADR-0131), `installing` asks the root update helper to
stop the service, install, start and check it, and `resuming` thaws the tasks one by one. The
helper accepts only `build` and `install`; `install` needs a staged build of a newer commit.
There is no way to restart without an update, so a superadmin restarts with `systemctl` by hand,
which does not freeze the tasks.

## Requirements

- **MCR-001 The bridge says it is alive.** `yulius mcp` sends a `ping` to `POST /mcp` every 60 s,
  whatever Claude Code sends (Claude Code's own `initialize` reaches Yulius at the start). Its answer is never written to stdout. A ping
  that fails is dropped (the next one tries again).
- **MCR-002 Yulius records each task's last bridge contact.** Every request with an id of the
  session's MCP client that reaches `POST /mcp` with a task token sets the task's runtime
  `mcp_seen_ms`, and a bridge ping `mcp_pinged_ms`; a `yulius tool` call (id `yulius-tool`) sets
  neither. A new or reattached run starts afresh.
- **MCR-003 A silent bridge is reconnected.** Every 30 s, for each live task, Yulius types
  `/mcp reconnect yulius` into the session when all hold:
  - the task is Running with a session that started at least 3 minutes ago, and the server has
    been up at least 3 minutes;
  - no Yulius request from its bridge for 3 minutes;
  - no turn is open, no update holds tasks, the task is not paused, and no person typed in the
    last minute;
  - Yulius has not typed a reconnect into it in the last 10 minutes (60 minutes once 3 went
    unanswered in a row); silence also counts from the end of a hold, since a frozen bridge pings
    nothing.
- **MCR-004 The agent is told to continue.** When the bridge is heard from again after a
  reconnect, and the task's last turn closed while it was surely cut off (90 s after its last
  ping, or after the spawn of a run this server started that never pinged), Yulius types "Yulius's
  tools are reconnected; continue" once, unless its goal is achieved or it waits for a person
  (`needs_you`). A reattached run that never pinged here is never told to continue.
- **MCR-005 `yulius tool` calls a Yulius tool from the shell.** `yulius tool <name> [<json
  arguments>]` sends `tools/call` to the session's `POST /mcp` with its task token and prints the
  tool's text. Exit 0 on success, 1 when the tool answers an error, 2 for a usage or environment
  error, 3 when Yulius cannot be reached. The briefing's "Yulius tools" section names it.
- **RST-001 The helper restarts.** A helper request `restart` of commit C stops the service, starts
  it and waits until it reports C. It is refused when the helper installed another commit. It
  reports `done` (`restarted`) or `failed`; it backs up, installs and rolls back nothing.
- **RST-002 A superadmin restarts Yulius.** `POST /api/admin/restart` (superadmin, `confirm`)
  opens an update run whose target is the running build, straight in `draining`. It is refused
  without the helper, without a known build, or while a run is open. The run then goes the
  update's way: tasks frozen, the helper asked to `restart`, tasks thawed. It is refused while a
  deploy runs. The audit log records `restart.started`, and its later steps as `restart.*`.
- **RST-003 The Instance page offers it.** The Instance page's update card shows a "Restart"
  button to superadmins when no run is open, with a confirmation that says tasks pause and resume.
  A restart run reads as a restart ("Restarting", "Restarted"), not as an update.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The MCP bridge tells Yulius it is alive, and Yulius records each task's last contact
- [x] Yulius reconnects a session whose Yulius MCP went silent, then tells it to continue
- [x] `yulius tool` calls any Yulius tool from the shell
- [x] The update helper restarts the service without installing
- [x] A superadmin restarts Yulius from the Instance page: tasks freeze, restart, thaw
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- A bridge's pings reach `POST /mcp` on its interval, and its stdout carries only
  the answers to Claude Code's own messages.
- A live, idle, Running task whose bridge has been silent for 3 minutes (server and session up
  for 3 minutes) gets `/mcp reconnect yulius` typed once; a busy, paused, update-held, recently
  typed or just-started one does not; a fourth unanswered reconnect is never typed.
- After the bridge is heard again, a task whose turn closed while silent gets the continue text
  once; one with `needs_you` or an achieved goal does not.
- `yulius tool steps_get` in a session prints the tool's text; an unknown tool exits 1; a missing
  token exits 2; an unreachable Yulius exits 3.
- The helper's `restart` of the installed commit stops and starts the service and reports
  `done`; of another commit it is refused and touches nothing; a server that does not come back
  reports `failed`.
- `POST /api/admin/restart` by a superadmin opens a draining run whose target is the build; by
  others it is forbidden; while a run is open it conflicts. Once drained, the helper gets a
  `restart` request, and the helper's `done` resumes the tasks with outcome `succeeded`.
- The Instance page shows "Restart" to a superadmin with no open run, and a restart run as
  "Restarting".

## Test plan

- Unit (`yulius::mcp_bridge`): pings on a short test interval reach a fake Yulius;
  their answers never reach stdout; a failed ping does not end the bridge.
- Unit (`yulius-app::mcp_watch`, the pure rule): each condition of MCR-003 and MCR-004 on its
  own, the 10-minute spacing and the cap of 3.
- Integration (`yulius/tests`): an agent `ping` to `POST /mcp` sets `mcp_seen_ms`; a silent task's
  session gets `/mcp reconnect yulius` typed (read from the fake agent's input), and the continue
  text after a contact.
- Unit (`yulius::tool_cli`): against a fake Yulius, a result's text and exit 0, a tool error exit
  1, a missing token exit 2, an unreachable server exit 3, invalid JSON arguments exit 2.
- Unit (`yulius::update_helper`, with the fake `systemctl` and health server of its tests): a
  restart of the installed commit, of another commit, and one whose server does not come back.
- Integration (`yulius/tests` update tests): a restart run by a superadmin, refused for others and
  while a run is open, the `restart` request after the drain, and `succeeded` after `done`.
- e2e (`e2e/tests/update.e2e.ts`, the mock): the Restart button, its confirmation, and the
  "Restarting" state.

## Decisions

- **Reconnect by typing `/mcp reconnect yulius`, detected by a ping** (ADR-0276). Alternatives
  were raising `MCP_TIMEOUT` (covers only the slow-connect cause and delays every session's other
  servers), having the bridge answer `initialize` itself (rejected by ADR-0244: a stale copy of
  the server's tools) and restarting the session (cuts the turn, ADR-0244). The ping makes the
  rule independent of why Claude Code dropped the server.
- **A shell fallback for every tool** (`yulius tool`), because a reconnect needs an idle session
  and the CLI kept working in every outage seen so far.
- **The restart is an update run whose target is the running build**, so draining, freezing,
  thawing, cancel, the page's progress and the audit all stay one code path. A run is a restart
  when `from_commit` equals `target_commit` (no schema change).
- **The root helper restarts, not the server exiting under `Restart=on-failure`**: the helper
  already reports each step and checks that the server came back, and a server that exits on
  purpose would count against systemd's start limit.
- **The briefing names `yulius tool` in one short sentence** (" Unavailable? Run `yulius tool
  <name> '<json>'`."). The briefing has no spare bytes: the long-goal tests' reference budget
  moves from 1 700 to 1 650 bytes under the default, the sentence's size, so they still check that
  every section fits at a budget with nothing to spare. A hook that tells the agent only while its
  MCP is silent was the alternative; Claude Code's `PreToolUse` context is not a documented
  surface Yulius relies on yet.
- Both changes ship in one pull request because the person asked for them together; each is a
  separate commit series.

## Review

Independent reviewer in a fresh session (read-only, `om-code-review`), on this branch's own
commits.

- **Round 1** (`ac0d69ad`): request changes. Own code, all fixed test-first:
  - M1: the continue text would have reached tasks idle long before the silence, at this release's
    rollout (an older bridge never pings) and for a turn closing within a ping. Now pings are
    recorded apart, the cut-off is the last ping plus 90 s (or the spawn of a run this server
    started), and a reattached run that never pinged is never told to continue.
  - M2: a restart during a deploy froze every task up to 30 minutes, then cancelled. Now it is
    refused while a deploy runs.
  - Minors: hourly backoff instead of giving up after 3; state reset per run; `yulius tool` calls
    are no contact; silence counts from a hold's end; refusals log at debug; `restart.*` audits and
    restart wording; failure-path tests; operations and contracts docs; `-32001` exits 2.
  - Answered, not changed: `yulius tool` writes to stdout as `yulius wait` does; SPEC-028 is
    amended through the ADR as ADR-0244 did. Whether real Claude Code takes a pasted
    `/mcp reconnect`: verified (Evidence).
- **Round 2** (`bf0d50c5`): approve, with nits. Fixed: a run heard before its first ping is cut
  off from that contact (test); the operations sentence and the ADR index wording. Left: the gate
  may reopen in the milliseconds between a restart's deploy count and its run's insert; a deploy
  starting then is waited for, never cut.

## Evidence

- Real Claude Code 2.1.311 in a PTY (no model call), with a fake stdio MCP server that exits
  before `initialize`: Claude Code retried once, then left it failed (no spawn for 15 s). Typing
  the bracketed paste `/mcp reconnect fakey` and Enter respawned it: "Successfully reconnected to
  fakey". A healthy server is respawned the same way.
- `./target/debug/yulius tool steps_get` in this session printed the live plan (exit 0);
  `yulius tool no_such_tool '{}'` printed "unknown tool: no_such_tool" (exit 1).
- Unit: `mcp_bridge` 11 passed (pings on the interval, answers kept from stdout, a failed ping
  survived); `mcp_watch` 10 passed; `tool_cli` 4 passed; `update_helper` 20 passed (restart of the
  installed build, of another build refused untouched, a server that stays down fails).
- Integration: `restart::a_task_whose_yulius_mcp_went_silent_is_reconnected_then_continues`;
  `mcp_coordination::an_agent_request_records_its_tasks_mcp_contact` (a shell call is no
  contact); `self_update` 31 passed, with the restart run frozen and thawed, refused during a
  deploy, cancelled and failed.
- `cd e2e && npx e2e run tests/update.e2e.ts`: 21 passed, with "a superadmin restarts the server
  from the instance page".
- `scripts/dev ci` exit 0 on `ac0d69ad` (3182 tests passed) and on `bf0d50c5` (3187 passed). The
  last commits (one rule line, one unit test, docs) are covered by the targeted runs above and by
  Yulius CI on the pull request head.
- Yulius CI on the first pull request head (`49a951bd`) failed on the known flaky
  `pool_supervision::personal_mcp_servers_reach_only_their_holders_runs` (#1084, not caused by
  this change, which it blocked). Fixed here in its own commit: the attempt is recorded before
  the spawn writes `mcp.json` and `launch.json`, and the test read them after waiting only for the
  attempt. A 300 ms probe delay before the `launch.json` write reproduced the NotFound every time;
  waiting for the launch files passes with the probe, and 20 consecutive runs of
  `pool_supervision::` passed without it.
- Security scan: Vidoc is turned off for this project in Yulius (`yulius security-scan` reports
  it disabled); not run, not passed.
- Not tested: an actual production restart through systemd and the root helper (the helper's
  `restart` is tested with fake `systemctl` and health), and a real session whose Yulius MCP was
  dropped by Claude Code mid-goal being reconnected by a running Yulius (its parts are tested: the
  watch with the fake agent, the typed reconnect with real Claude Code).

