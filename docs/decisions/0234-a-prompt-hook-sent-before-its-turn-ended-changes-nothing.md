# 0234. A prompt hook sent before its turn ended changes nothing

## Status

Accepted. Specified in [SPEC-346](../../.ai/specs/SPEC-346-goal-resubmit-after-a-late-watcher-leave.md)
(LPH-001..003); task #330, #921. Refines
[ADR-0115](0115-a-received-hook-is-applied-to-its-end.md).

## Context

Since ADR-0115 a hook Yulius received runs to its end, even after the `yulius hook` client gave up
waiting (5 s) and the agent went on. Its Consequences note that such a hook can land after the
agent's next hooks. For a Stop that heals with the agent's next work. For a `UserPromptSubmit` it
does not: on a loaded host a goal turn's prompt hook was handled after the turn's Stop and Claude
Code's "goal met". It reset the achieved goal and set Working, and nothing ended that turn again,
since the agent was idle. `task_done::a_goal_the_tui_dropped_is_submitted_again` timed out waiting
for Done that way (#921, also tasks #237 and #314). A test layer that holds that prompt hook past
the client's deadline reproduces it.

The server cannot tell such a prompt from the next turn's by when it handles it: the delay may come
before the handler starts (the database, the accept queue).

## Decision

- The hook client sends the time it sent each request in `x-yulius-hook-sent-ms` (Unix ms). The
  server clamps it to its own clock, and uses its own time for a request without it.
- A task's runtime keeps `turn_end_ms`, the latest of: the send time of a Stop that reached Yulius,
  and when Yulius applied a goal verdict (met, not met, failed).
- A `UserPromptSubmit` sent before `turn_end_ms` belongs to a turn that is over. It is still
  handled (holds, the scope check, a person's answered question), but it does not set Working,
  reset the achieved goal, the gate count or the turn start.
- A goal verdict "met" settles an idle task (Your turn) as Done also when a Stop waited for it.

## Alternatives

- **The server's arrival time.** It misses delays before the handler starts, and a test layer that
  delays the request defeats it.
- **A per-task counter of Stops.** It needs the task, so it is read only after the token lookup,
  which waits on the same busy database.
- **Dropping a hook handled past the client's deadline.** This reverses ADR-0115 and loses events
  the agent acted on.
- **Matching prompts to transcript records by text.** It costs a lot to keep in step, and prompts
  repeat.
- **Confirming a typed goal by its own prompt hook** (the triage's lead). It did not cause #921:
  random watcher lags of up to 15 s passed 29 of 29 runs.

## Consequences

- A late prompt no longer strands a task as Working or Your turn after its turn ended.
- The client and the server use the same clock: the client runs on the same host (loopback only).
  A forged time only affects the task's own bookkeeping, and a future one is clamped.
- If the watcher applies the previous turn's verdict after the next prompt was sent, that prompt's
  effects are skipped. Its transcript `UserTurn` still sets Working, so nothing is lost.
- An older client without the header is ordered by arrival, as before, at worst.
