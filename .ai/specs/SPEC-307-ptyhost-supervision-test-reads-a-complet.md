---
yulius-task: cd7ebfb6-15d4-465c-b078-d2ac3a106147
status: in-progress
---

# SPEC-307: ptyhost supervision test reads a complete CHILD pid

Status: in-progress
Date: 2026-10-25
Owner: task #293
Scope authorization: issue #647 (found by task #202), verified on `dev` at f770e4ff by task #289.

## Goal

`yulius-ptyhost::supervision shutdown_escalates_past_ignored_signals_and_reaps_descendants` reads
the descendant's pid only from a finished `CHILD <pid>` line, so it passes whatever the load. Test
only; no product code changes.

## Context

- Symptom (#647): under a full `scripts/dev ci` the test failed once after 0.059 s with
  `ParseIntError { kind: InvalidDigit }`; it passed 5 of 5 alone.
- Code path: the test waits for `"CHILD "` in the output (`wait_for` matches as soon as those six
  bytes arrive), types `ping\r`, waits for `ECHO: ping`, then parses the rest of the screen row
  that starts with `CHILD `.
- Suspected cause (task #289, to confirm): the PTY keeps the kernel defaults (ICANON, ECHO,
  ONLCR). The fake agent writes `CHILD <pid>\n` in one `write`, but the line discipline turns the
  `\n` into `\r\n` as a separate output step, so the master can read `CHILD <pid>` before its line
  end. If the test types `ping` in that window, the kernel's echo of `ping` lands on the same row
  (`CHILD 1234ping`), and `parse()` fails.
- Why the tests missed it: the window is microseconds wide and only opens when the agent is
  descheduled mid-write, which needs a loaded machine.
- What else the cause could break: any test that types after matching a partial line and then
  reads that row from the screen. The other `screen_line` readers are checked in the review.

## Requirements

- CPL-001: the test takes the pid from a `CHILD ` row that is finished (the cursor moved below it)
  before it types anything, and uses that pid after `ECHO: ping`.
- CPL-002: no retry and no sleep is added; the wait is on the condition.
- CPL-003: the reproduction (the fake agent pausing between `CHILD <pid>` and its line end) makes
  the old test fail and the new test pass.

## Plan

- [ ] Spec for the CHILD pid flake written and valid
- [ ] Flake reproduced with a split CHILD line and cause named
- [ ] Test waits for a finished CHILD line before typing
- [ ] Independent review answered and gates green
- [ ] PR merged into dev with evidence on #647

## Acceptance criteria

- With the probe, the test on `dev` fails with `InvalidDigit` every time; with the fix it passes.
- The test passes without the probe, alone and in `scripts/dev ci`.

## Test plan

- Probe: the fake agent writes `CHILD <pid>`, flushes, sleeps, then writes `\n` (temporary).
- Run the test 10 times with the probe before and after the fix, and under CPU load without it.

## Decisions

- Read the pid from the screen row above the cursor, not from the live output: the CHILD line can
  already be in the connection's snapshot, which carries no live bytes.
