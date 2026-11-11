# 0299. A transcript prompt and its turn's clean Stop share the turn order

## Status

Accepted. Specified in
[SPEC-453](../../.ai/specs/SPEC-453-task-done-a-goal-the-tui-dropped-reaches.md) (TPO-001..003);
task #435, #1066. Extends
[ADR-0234](0234-a-prompt-hook-sent-before-its-turn-ended-changes-nothing.md).

## Context

ADR-0234 ordered a prompt *hook* with a Stop or goal verdict of the same task (`turn_order`,
`turn_end_ms`). A turn's prompt reaches Yulius a second way: the transcript watcher reads its
record (`Observation::UserTurn`), the fallback for a prompt hook that never arrived. That path
took no part in the order. It checked its own mark ("a prompt of a turn whose clean Stop already
came is not a new turn", `last_stop_ms`), awaited the store, then started a turn: it cleared the
verdict wait and set Working. A clean Stop of the same turn, handled on another task, stamped
`last_stop_ms` without the lock.

When that Stop landed between the watcher's check and its writes:

- with a goal open, the watcher cleared the verdict wait the Stop had just opened. Claude Code's
  "goal met" then found no waited turn and left the task Working with the agent idle.
  `task_done::a_goal_the_tui_dropped_is_submitted_again` timed out waiting for Done that way
  (#1066, its third report after #243 and #921);
- without a goal, the Stop settled from the copy of the task it read on arrival. When that copy
  already read Your turn (its prompt hook was lost, or the turn was Claude Code's own wake-up),
  it wrote nothing after the watcher set Working.

Unloaded, the watcher's 1 s poll reads a turn's records hundreds of ms after its Stop was answered
(about 11 ms). Load moves the poll and stretches the Stop's handling. With 100 ms of delay at both
points and a shifted poll, 2 of 48 runs of that test failed with the issue's signature.

## Decision

- The watcher's prompt takes the task's `turn_order` before its `last_stop_ms` check and holds it
  until Working is set: across its own store writes only, as the prompt hook does.
- A clean Stop takes `turn_order` to stamp `last_stop_ms`. It then waits for a prompt that passed
  the check, and a prompt applied after the stamp is not a new turn.
- After the stamp the clean Stop reads the task again and settles it as it is (the copy it came
  with if the store fails), as `hook_event` does after its own lock.
- The lock is released before the Stop opens its verdict wait or settles: once the time is
  stamped, neither prompt path of that turn changes the task any more.

## Alternatives

- **Checking `last_stop_ms` again after the await.** The store write of Working still follows the
  second check, so a Stop settling in between is overwritten. ADR-0234 rejected the same repair.
- **Checking `turn_end_ms` instead of `last_stop_ms`.** It needs no change in the clean Stop, but
  it also moves for Stops that did not end the turn (a gate block, a held Stop), whose turn's
  prompt must still start the turn for the watcher.
- **Holding the lock across the Stop's settle.** Not needed once the stamp orders the two, and it
  would hold the lock across background probes and the pull request.
- **Not clearing the verdict wait in the watcher's prompt.** A turn the agent went on with
  (BGW-005) must drop the old wait, or a later verdict settles the wrong turn.
- **Quarantining the test.** The race is in the product, and it reproduces.

## Consequences

- A turn whose clean Stop arrives while the watcher applies its prompt ends: Done once the goal is
  met, Your turn otherwise.
- A clean Stop can wait for the watcher's store writes of one prompt (milliseconds), and the
  watcher for a Stop's stamp. Neither holds the lock across a hold, a scope check or a hook answer.
- A clean Stop reads the task once more.
- The rule trusts the host's clock, as ADR-0234 does: the transcript's time of a prompt is
  compared with the server's time of the Stop, both on the same host.
- Not covered, as before: a prompt record without a time always starts a turn; the verdict wait's
  20 s expiry can land on the next turn's prompt (it heals with the agent's next work line); a
  failed Stop's settle and a late Stop (ADR-0115) against the next turn's prompt; the opening Stop
  of a resumed goal session stamps no time.
