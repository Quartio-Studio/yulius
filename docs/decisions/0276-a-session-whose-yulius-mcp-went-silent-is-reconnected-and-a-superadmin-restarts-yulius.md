# 0276. A session whose Yulius MCP went silent is reconnected, and a superadmin restarts Yulius

## Status

Accepted; its "a silent session is reconnected" is amended by
[0280](0280-only-a-yulius-mcp-known-to-be-gone-is-reconnected.md) (only a Yulius MCP known to be
gone is reconnected). Specified in
[SPEC-432](../../.ai/specs/SPEC-432-fix-so-this-never-ever-happen-again-also.md); task #414.
Builds on [0244](0244-task-runs-reach-yulius-through-a-stdio-bridge-and-are-woken-after-an-outage.md)
(the stdio bridge) and [0131](0131-an-update-freezes-every-task-at-once.md) (an update freezes
every task). Amends SPEC-028 MSR-004 ("nothing is typed into an idle session") for one more case.

## Context

On 2026-11-06 a task session in the FW-N2-Device sandbox lost its Yulius MCP server partway through
a 3-hour `/goal` run. `delivery_merge`, `step_update` and `ci_status` were gone, while the `yulius`
CLI kept working. The agent could not merge a green, approved pull request, and its `/goal` Stop
hook blocked 9 times before the goal paused, waiting for a person to run `/mcp`.

ADR-0244's bridge makes a Yulius outage a slow call. It does not stop Claude Code from dropping the
server: Claude Code 2.1.311 gives a stdio server 30 s to connect (`MCP_TIMEOUT`), after a version
probe with its own bound, so a session started or resumed while Yulius is down or slow loses it.
A failed server stays failed for the session, and no hook output reconnects it. Claude Code does
accept `/mcp reconnect <server>` typed as input. Yulius had no record of which sessions still have
a bridge: an idle bridge sends nothing.

Separately, a superadmin had no way to restart Yulius without an update. `systemctl restart` by
hand does not pause the tasks.

## Decision

- **The bridge pings.** `yulius mcp` sends `ping` (id `yulius-bridge-alive`) to `POST /mcp` every
  60 s and keeps the answers to itself. Every request of the session's MCP client to `POST /mcp`
  records the task's last contact in memory, and a ping its last ping; a `yulius tool` call (id
  `yulius-tool`) records nothing. A new or reattached run starts afresh.
- **A silent session is reconnected.** Every 30 s Yulius types `/mcp reconnect yulius` into a
  Running task's session when its last contact (or its spawn, if later) is 3 minutes old, the
  server has been up 3 minutes, no turn is open, no update holds tasks, the task is not paused and
  no person typed in the last minute. Silence counts from a hold's end too: a frozen bridge pings
  nothing. Reconnects of one task are 10 minutes apart, and hourly after 3 in a row with no
  contact (a sandbox that could not reach Yulius gets its MCP back once it can).
- **It is told to continue.** Once the bridge is heard after a reconnect, a task whose last turn
  closed while it was surely cut off gets "Yulius's tools are reconnected; continue" once, unless
  its goal is achieved or it waits for a person. Surely cut off means 90 s after its last ping,
  or since its last contact (else its spawn) for a run this server started that never pinged. A reattached run that never
  pinged here (an older bridge) is reconnected but never told to continue: when it was cut off is
  not known.
- **`yulius tool <name> [<json>]`** calls any Yulius tool from the shell through the same
  `POST /mcp`, for a turn that cannot wait for an idle moment. The briefing names it in one
  sentence.
- **A restart is an update run whose target is the running build.** `POST /api/admin/restart`
  (superadmin, confirmed) opens a run straight in `draining` with `from_commit` equal to
  `target_commit`, refused while a deploy runs (an update waits for deploys while it builds; a
  restart has nothing to build). Every task is frozen, the root helper gets a new action
  `restart` (stop, start, wait until the server reports that commit; refused for a build other
  than the one it installed; nothing backed up, installed or rolled back), and the tasks thaw one
  by one. The Instance page
  offers **Restart…** and shows the run as a restart.

## Alternatives

- **Raise `MCP_TIMEOUT` for task runs.** Covers only a slow connect, not any other way Claude Code
  drops a server, and delays every other MCP server of the session at start.
- **The bridge answers `initialize` itself** so a connect never times out. Rejected in 0244: the
  bridge would serve a stale copy of the server's tools.
- **Restart the session in place.** Cuts a turn in progress (0244).
- **Detect the drop from the transcript.** Claude Code records no stable line for a failed MCP
  server; the ping is independent of why the server was dropped.
- **Tell the agent only while its MCP is silent, through a `PreToolUse` hook's context.** Narrower
  than a briefing sentence, but Yulius does not rely on that surface yet.
- **The server exits on purpose and systemd restarts it (`Restart=on-failure`).** No step reports
  or checks that the server came back, and a deliberate exit counts against systemd's start limit.
- **A restart as its own kind of run.** Would duplicate draining, freezing, thawing, cancel and the
  page's progress.

## Consequences

- Each task session's bridge makes one small request a minute.
- A session started before this release has a bridge that does not ping: after the update it reads
  as silent once idle and gets one `/mcp reconnect yulius`, which starts the new bridge, and no
  continue text. (It did not: the sandbox's binary stayed the old one, so such sessions were
  reconnected every 3½ minutes; 0280 leaves a bridge that never pinged alone.)
- The unprivileged service can now have the root helper stop and start it at will; that is no more
  than the service stopping itself.
- `/mcp reconnect` is Claude Code's inline action; a Claude Code without it would show its usage
  line. Yulius keeps Claude Code current (0266), so runs use a version with it.
- A restart that does not come back is reported failed; the server stays as the helper left it, as
  for a failed install, and the run resumes the tasks once a server reads the result.
