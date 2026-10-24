# 0212. A turn whose Stop Yulius never answered settles at its record

## Status

Accepted. Specified in [SPEC-290](../../.ai/specs/SPEC-290-fix-status-was-working-while-the-goal-wa.md);
task #276, issue #877. Refines the finish rule (SPEC-002 §6.1, TSK-014) and
[ADR-0115](0115-a-received-hook-is-applied-to-its-end.md)'s hook handling.

## Context

The hook client fails open: when Yulius answers a hook with an error, or cannot be reached, the
client prints nothing and Claude Code goes on. A Stop hook that failed open ended the turn, but
Yulius never settled it. The goal verdict that followed (`met: true`) found the task Working, and
on the live path only the Stop hook's verdict slot settles a Working task. Task #272 read
"working 11h 15m" after "Goal achieved", and its chains and session slot waited the whole time.
The replay after a restart already settles the same ending, because the record that closes a turn
ends it there.

## Decision

- Yulius remembers when it last answered a Stop hook of the run (`stop_answered_ms`), whatever it
  decided: settle, block, hold or switch. It also remembers when the last turn began
  (`turn_began_ms`): the prompt's transcript time, the UserPromptSubmit hook, or work that resumed
  a stopped turn.
- When the record that closes a turn (`turn_duration`) arrives, the run has hooks, and no Stop was
  answered since the turn began, the turn settles as that clean Stop would have: Done when the goal
  is achieved, waiting otherwise. The automatic pull request, chains and queue follow as after a
  clean Stop. This applies only to a Working task, and not while it recovers a full context, sits
  at a usage limit or has an open pause, since those paths decide how the task goes on.

## Alternatives

- Settle on the `met: true` record alone: a Stop that Yulius answered with a gate block may come
  with an achieved goal, and the agent works on. Done would start chains too early.
- Make the hook client fail closed: a Yulius outage would block every agent at its turn end.
- Retry the Stop hook in the client: an error answer is usually not transient, and the turn has
  already ended in Claude Code.

## Consequences

- An achieved goal reads Done within one transcript poll of the turn's record, even when the Stop
  hook failed. A failed Stop is logged as a warning ("the turn closed with no Stop Yulius
  answered").
- A turn that began before a restart and whose Stop failed while Yulius was down is still settled
  by the replay, as before.
