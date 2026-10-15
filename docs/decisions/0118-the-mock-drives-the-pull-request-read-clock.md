# 0118. The mock drives the pull request read clock

## Status

Accepted. Specified in [SPEC-144](../../.ai/specs/SPEC-144-fix-438-444-461-469-415-pulls-e2e-ts-wai.md);
task #133, issue #444. Builds on [ADR-0087](0087-pull-request-changes-are-announced.md) (announced
reads and their gap).

## Context

The browser store (`store.js`) plans its pull request reads in time:
- an announced change is read at least `pullGapMs` after the start of the last read (3 s);
- a view that watches again, or a tab shown again, reads once 15 s have passed;
- a watched sandbox is read every minute.

`pulls.e2e.ts` proved "announcements within the gap make one read" on real time, with a 1.5 s gap.
Under load the test reached the announcements after the gap. The first announcement then read at
once, and the next two asked for one more read after it (correct, PRL-005), so the count was one
too many (`expected 3 to be 2`). Taking `before` later, or counting only the reads the
announcements cause, still needs the host to reach each step within 1.5 s, which load breaks.

## Decision

- `createStore(api, { pullGapMs, clock })` takes the clock its pull request reads use (`now`,
  `setTimeout`, `clearTimeout`, `setInterval`, `clearInterval`) from the transport, beside
  `pullGapMs`. Without one, it uses the real clock, so production is unchanged.
- In mock mode, `pullclock=manual` gives a clock that only `__yuliusMock.advanceClock(ms)` moves.
  Moving it runs the timers that come due on the way, in order.
- Tests of how pull request reads are spaced in time use the manual clock, so they count reads
  without depending on how fast the host is.

## Alternatives

- **Take `before` once the first read has settled, or count only announcement reads**: the
  announcements still have to land within the gap on the host's clock. That is the timing that
  failed.
- **A longer gap (say 10 s)**: makes the flake rarer but not impossible, and makes the test wait
  the whole gap.
- **Fake timers for the whole page** (overriding `Date.now` and `setTimeout`): also stops the
  mock's latency, the sidebar's coalesced repaint and the event socket, which the test needs to
  run normally.

## Consequences

- One more seam in the store, used only by the mock.
- Read planning stays testable without a real wait: the 15 s spacing and the minute's read can be
  proven the same way.
- Other timers (counter reloads, repaints, the mock's latency) stay on real time, so a test on the
  manual clock still waits on conditions for them.
