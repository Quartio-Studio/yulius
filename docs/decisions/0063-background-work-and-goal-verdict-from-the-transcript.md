# 0063. Background launches and the goal verdict come from the transcript; the hook does not wait

## Status

Accepted. Extends [0044](0044-task-status-follows-claude-codes-own-records.md); specified in
[SPEC-079](../../.ai/specs/SPEC-079-fix-305-264-265-271-an-idle-task-s-backg.md) (#305, #264,
#265, #271).

## Context

Decision 0044 derives a task's activity from Claude Code's own records. It left four gaps:

- Background agents are counted only from `turn_duration`, which Claude Code writes after its
  Stop hooks. At the Stop hook, an agent launched in the turn that ends is unknown, so a
  proactive switch restarted the session and ended the agent.
- Monitor streams wake the session like shells and agents do. `turn_duration` does not count
  them. A command monitor runs in Claude Code's Bash wrapper, so the process probe counts it, but
  a WebSocket monitor has no process at all.
- Claude Code's goal evaluator is a Stop hook that runs after Yulius's own. Yulius made the task
  idle at its hook, and the verdict ("not met") took that back a second or more later. The
  sidebar, notifications and chains saw a state that never really held.
- When the last background work of a Done task ended, chains advanced but the queue waited for
  the next tick.

## Decision

1. **Launches are read from the transcript.** The watcher observes the launches: a Monitor start
   (`toolUseResult` `{taskId, timeoutMs}`) and a background agent launch
   (`{status: "async_launched", agentId}`). It also observes the `<task-notification>` that ends
   either one, as a prompt or a queued command. The runtime keeps the live ones in a bounded map.
   - A Monitor counts as background work until it ends or its `timeoutMs` passes.
   - An agent counts until the turn's own record, whose count then takes over.
2. **The Stop hook reads before it switches.** When the pool decides a switch at a Stop, the hook
   first waits (bounded) for the watcher to read the lines written before it. Background work,
   launches included, defers the switch to a later Stop.
3. **The hook never waits for the verdict.** While a goal is open, the clean Stop records its time
   in one slot and returns, and the task stays Working. Whoever comes first takes the slot (one
   compare-and-swap) and settles the turn:
   - the verdict: "met" makes the task Done, "failed" makes it wait, "not met" keeps it working;
   - agent work after the Stop, a prompt or an interrupt: the task keeps working;
   - the turn record, or the 20 s timeout: the turn settles as before.
4. **Every path to Done drains the queue.** The last background work ending drains it, and so does
   a Stop that settles the task Done.

## Alternatives

- **Wait for the verdict inside the Stop hook.** Claude Code runs the evaluator after the hook
  returns, so the hook would wait for itself.
- **Count Monitor processes only.** WebSocket monitors would stay invisible, and a monitor's
  expiry would not bound it.
- **Count agents ourselves instead of from `pendingBackgroundAgentCount`.** The CLI's count is
  exact once the turn closes, and survives restarts and absorbed notifications. Our own launch
  tracking covers only the window before that record.
- **Have the Stop hook parse the transcript file itself.** That would be a second reader with its
  own offsets. One more watcher poll costs at most about two seconds, and only when a switch was
  decided.

## Consequences

- A goal task reads Working for the second or two the evaluator takes, then Done or waiting. A
  run whose evaluator never answers settles at its turn record, or after 20 s.
- The launch map lives in memory. After a restart a WebSocket monitor is not counted again; it
  expires within 30 minutes anyway. Command monitors stay counted by the process probe.
- A notification that names an id Yulius never saw (a shell's) changes nothing.
