# 0470. Wall-clock stamps are read on Yulius's clock

## Status

Accepted for SPEC-636 (#1615). Refines ADR-0234 (a hook's send time) and ADR-0299 (a transcript
prompt's time).

## Context

Two of Yulius's orderings compare a time another process of the host stamped on the wall clock
with Yulius's own clock:

- `yulius hook` stamps each request with its send time (`x-yulius-hook-sent-ms`), and
  `hook_event` orders hooks by it, clamped so that it is never later than now (ADR-0234). A
  prompt hook sent before its turn's Stop changes nothing.
- The transcript watcher compares a prompt record's time with the last clean Stop
  (`last_stop_ms`), so a record read after its turn's Stop is not a new turn (ADR-0299).

Integration tests run Yulius on a `ShiftedClock`: the system clock minus some minutes, which the
test moves forward. On that clock every wall-clock stamp lies ahead. The clamp turned each hook's
send time into its arrival, and every prompt record looked later than any Stop. A submit that
reached Yulius more than 5 s late, or a record the watcher read after the Stop (it reads once a
second), therefore read as a new turn. It set Working and replaced the held-message flag, and
`task_message::a_draft_the_screen_shows_holds_until_it_is_cleared` failed once under load
(#1615). Production reads one wall clock in client, agent and server, so it was not affected.

## Decision

The `Clock` port gets `of_wall(wall)`: this clock's time at a wall-clock instant that another
process of the host stamped. Its default returns the stamp unchanged (the system clock and the
fixed test clocks).

- `hook_event` reads the send time through it, then clamps it to now as before.
- The transcript watcher maps each observation's recorded time (`at`) through it right after its
  poll, so live lines and the replay's facts agree. Announced times, such as a limit's
  `resets_at`, are left as written.
- `ShiftedClock` keeps the shifts a test made (`shift`, `advance`) with the wall time each took
  effect. It maps a stamp with the shift in force at that instant, so a stamp made before a move
  keeps its order.

Two test-support pause points reproduce the late arrivals without load:

- `HookArrived`, at the entry of `hook_event`, keyed by the hook's session and event.
- `UserTurnRead`, where the watcher takes a prompt record, keyed by the task id.

## Alternatives

- Waiting in the test for the opening submit's answer before going on: the late submit still sets
  Working (its send time is still its arrival), and no wait orders a watcher read that comes
  later.
- Mapping with the shift at arrival: a stamp made before a move and read after it reads as made
  after the move, the same failure once the test moves its clock.
- Shifting the stamps at their source (an offset handed to `yulius hook` and the fake agent):
  puts a test-only knob into the product client, and every spawned process would have to learn
  each move.
- A guard on the Stop for a turn its record settled (the cause first suspected): the record that
  settles a turn without a Stop is written after the Stop hooks, so that order does not occur here.

## Consequences

Production behaviour is unchanged. Tests on a shifted clock order late hooks and late transcript
lines by when they were made, as the shared host clock does in production. Tests move a
`ShiftedClock` with `shift`/`advance` instead of its atomic. The submit-origin lookup still
compares queue record times with the run's start unmapped; no shifted-clock test reaches it.
