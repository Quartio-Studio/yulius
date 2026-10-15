# 0115. A hook Yulius received is applied to its end, whatever its client does

## Status

Accepted. Specified in [SPEC-140](../../.ai/specs/SPEC-140-fix-404-464-353-443-task-done-supervisio.md)
(HKS-001); task #127, #404. Refines SPEC-002 §7.5.

## Context

Claude Code runs its hooks through the `yulius hook` client. The client waits 5 s for Yulius's
answer and then lets the agent go on (fail-open, SPEC-002 §7.5), closing its connection. The server
handled the hook inside its HTTP handler, and hyper drops a handler when its connection closes. A
Stop hook does a dozen store round trips on the single database thread and runs Git in the worktree
before its last step, which sets Done or Your turn and starts the probe of background work. It
took about 100 ms, and up to 2 s on a loaded test host. On the CI host under heavier load, a Stop
past 5 s was cut off before that step: the task stayed Working with nothing to wake it. Four
`task_done` tests hung in CI that way (#404). Shown deterministically: a hook whose client hangs up
while the server handles it was never applied.

## Decision

- The server runs every hook event (`hook_event`) in its own task; the HTTP handler only waits for
  its answer. A client that gave up loses the answer, never the event.
- The status line is left as it was: it only reads, and a dropped one changes nothing.
- The client's timeout and fail-open stay as they are.

## Alternatives

- **A longer client timeout.** It moves the threshold without removing it, and keeps every agent
  waiting longer at each boundary whenever Yulius is slow.
- **A server-side deadline on hook handling.** It would cut the same work off at a known point and
  still leave the state half-applied.
- **Making every hook step idempotent and replaying it later.** Nothing tells Yulius that a hook was
  cut off, so there is nothing to replay from.

## Consequences

- An event the agent acted on is always recorded, late at worst. A Stop the gate would have blocked
  (its answer lost to the client's timeout) still counts as a gate block, as the agent did stop.
- A hook's work can outlive its connection by the hook's own bounds (its store calls, its Git work);
  hold answers stay short, so no hook task waits for long.
