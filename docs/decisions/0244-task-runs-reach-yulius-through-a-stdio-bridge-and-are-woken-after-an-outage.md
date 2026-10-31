# 0244. Task runs reach Yulius through a stdio bridge and are woken after an outage

## Status

Accepted. Specified in
[SPEC-358](../../.ai/specs/SPEC-358-sessions-keep-yulius-mcp-waits-and-deliv.md); task #343.
Builds on
[ADR-0241](0241-a-full-relay-closes-its-longest-idle-connection-and-the-superagent-restarts-in-place.md),
where the Superagent restarts in place. Amends SPEC-028 MSR-004 ("nothing is typed into an
idle session") for one case: Yulius's own outage.

## Context

On 2026-10-31 every Yulius restart left the task sessions of the quartio-v2 sandbox with a dead
Yulius MCP connection. A session could not call `ci_status`, `ci_rerun` or `delivery_merge` until
a person ran `/mcp`.

Claude Code 2.1.292 retries a closed HTTP MCP transport 5 times, about a minute in all. It retries
a failed first connect 3 times, and only for some errors. Then it leaves the server failed. No
hook output or setting reconnects it. A Yulius restart or update takes longer than that, so every
outage cut the sessions off for good.

`yulius wait` gave up after about 50 seconds. Its exit woke the agent while Yulius was still down,
so the agent ended its turn. That turn's Stop hook failed open, so Yulius never pushed it, and
nothing woke the task when Yulius came back.

## Decision

- **A stdio bridge.** A task run's `yulius` MCP server is `yulius mcp`, a stdio process started by
  Claude Code. It gets the run's `YULIUS_URL` and `YULIUS_TASK_TOKEN` as `${VAR}` references. It
  forwards each JSON-RPC line to `POST /mcp` and writes each answer back as one line. The process
  never ends while the session runs, so Claude Code never sees the server fail. An outage
  becomes a slow call.
- **Resend only what is safe.**
  - The bridge sends each message with `Expect: 100-continue`. A connection that fails before
    Yulius asks for the body never reached a handler, so the message is sent again with a
    backoff, for up to 10 minutes. Past that bound, the call is answered "not made".
  - A failure after the body was sent may have taken effect. The bridge then resends only
    `initialize`, `ping`, `tools/list` and notifications. A `tools/call` is answered an error
    saying that it may have taken effect.
  - A call Claude Code cancels while it is held is dropped.
- **`yulius wait` rides out an outage** until its own deadline, with a backoff capped at
  10 seconds.
- **Yulius is back; continue.**
  - Yulius writes `<data>/alive-at` every 30 seconds.
  - Once a start has reattached the runs, it waits 90 seconds so that they replay their
    transcripts. Then, every 30 seconds for the first 10 minutes, it types "Yulius is back;
    continue" once into each reattached, idle, Running task whose last turn closed between
    `alive-at` and the start, and whose last Stop it never answered. It answered none since the
    turn's last prompt or answer: it keeps that time in `<run>/stop-answered`, because `alive-at`
    is up to 30 seconds old, and a Stop it blocked comes before the turn's next answer. If the
    server restarts again before those wakes ran, `<data>/outage-pending` keeps the first outage
    (for at most a day). That covers a wait that died during the outage, the turn
    the wait's exit woke, and a turn whose Stop Yulius never answered. The woken turn ends with a
    Stop that Yulius answers, which pushes the branch and opens or arms delivery as usual.
  - These tasks are left alone: a task whose goal is achieved, one with an open `needs_you` or
    pause, and one where a person typed within the last minute.
- **Armed merges are tried right after their restore**, not at the delivery loop's first tick.

## Alternatives

- **A hook, or the `yulius` wrapper, asks Claude Code to reconnect.** There is no such interface.
  Hooks can block, add context or change tool input, but they cannot touch MCP clients.
- **Restart the task's session in place, as ADR-0241 does for the Superagent.** Rejected:
  - It cuts a turn in progress, losing the agent's in-flight tool calls.
  - The server cannot see that a session's client is dead. It would have to guess from the
    transcript.
  - It is also heavier than the problem: the session is fine, and only one connection is not.
- **The bridge answers `initialize` and `tools/list` itself while Yulius is down.** Rejected: the
  tools would be a stale copy, and the bridge would have to know the server's tool set.
- **Resend every call after a restart.** Rejected: `task_create`, `issue_create`, `record_number`
  and `automation_create` are not idempotent. The server keeps no call ids across a restart.
- **Record the open waits in the database, and wake those cut by the outage.** Rejected: every way
  a wait ends during an outage wakes the agent through Claude Code, and that turn closes while
  Yulius is down. So the turn's close time against `alive-at` finds the same tasks. It also finds
  turns whose Stop went unanswered, with one persisted time instead of a table.

## Consequences

- Every task session runs one more small process, `yulius mcp`, which mostly waits on stdin.
- During an outage a Yulius tool call blocks (bounded) instead of failing. An agent that
  needs Yulius waits for it.
- A bridge started by an older Yulius keeps its code until the session restarts. Its protocol is
  only `POST /mcp`, which does not change with the server.
- The first start of a release with this change finds no `alive-at`, so it wakes nobody. Sessions
  started before it keep the HTTP config until they start again.
- The Superagent keeps its HTTP config and ADR-0241's restart in place.
