# 0470. A hook's send time is read on Yulius's clock

## Status

Accepted for SPEC-636 (#1615). Refines ADR-0234.

## Context

`yulius hook` stamps each request with its wall-clock send time (`x-yulius-hook-sent-ms`), and
`hook_event` orders hooks by it, clamped so that it is never later than the server's now
(ADR-0234). A prompt hook sent before its turn's Stop then changes nothing. Integration tests run
Yulius on a `ShiftedClock`, the system clock minus some minutes that the test moves forward. On
that clock every stamp lies ahead of now, so the clamp turned each send time into the hook's
arrival. Under host load a submit reached Yulius more than 5 s late, after its client had failed
open and the agent had answered and sent its Stop. It read as sent after the Stop, set Working
and replaced the held-message flag (`task_message::a_draft_the_screen_shows_holds_until_it_is_cleared`
failed once, #1615). Production reads one wall clock in client and server, so it was not affected.

## Decision

The `Clock` port gets `of_wall(wall)`: this clock's time at a wall-clock instant another process
of the host stamped. Its default returns the stamp unchanged (the system clock and the fixed test
clocks). `hook_event` reads the send time through it, then clamps it to now as before.
`ShiftedClock` keeps the shifts a test made (`shift`, `advance`) with the wall time each took
effect, and maps a stamp with the shift in force at that instant, so a hook that straddles a move
of the clock keeps its order. A test-support pause point at the entry of `hook_event`
(`HookArrived`, keyed by the hook's session and event) holds one hook before anything of it is
read, which reproduces the late arrival without load.

## Alternatives

- Waiting in the test for the opening submit's answer before going on: the late submit still sets
  Working (its send time is still its arrival), and the task stays Working with an idle agent.
- Mapping with the shift at the hook's arrival: a hook sent before a move and handled after it
  reads as sent after the move, the same failure once the test moves its clock.
- Shifting the hook client's stamp from the test (an environment offset in `yulius hook`): puts a
  test-only knob into the product client and still needs every spawned process to learn moves.
- A guard on the Stop for a turn its record settled (the cause first suspected): no such order
  happens in this test, whose fake agent writes no transcript the watcher reads.

## Consequences

Production behaviour is unchanged. Tests on a shifted clock order late hooks by their sending,
as the shared host clock does in production. Transcript record times are still wall-clock stamps
compared with a shifted clock; no test depends on that order yet, and they can use `of_wall`
when one does. Tests move a `ShiftedClock` with `shift`/`advance` instead of its atomic.
