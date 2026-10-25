---
yulius-task: cd7ebfb6-15d4-465c-b078-d2ac3a106147
status: done
---

# SPEC-307: ptyhost supervision test reads a complete CHILD pid

Status: done
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
- Root cause (suspected by task #289, confirmed by the probe in Evidence): the PTY keeps the
  kernel defaults (ICANON, ECHO, ONLCR). The fake agent writes `CHILD <pid>\n` in one `write`, but the line discipline turns the
  `\n` into `\r\n` as a separate output step, so the master can read `CHILD <pid>` before its line
  end. If the test types `ping` in that window, the kernel's echo of `ping` lands on the same row
  (`CHILD 1234ping`), and `parse()` fails.
- Why the tests missed it: the window is microseconds wide and only opens when the agent is
  descheduled mid-write, which needs a loaded machine.
- What else the cause could break: any test that types after matching a partial line and then
  reads that row from the screen. `screen_line` had no other caller; the other tests that type
  right after `FAKE-AGENT READY` only match `ECHO: …` in live output, so an echo joining the READY
  row cannot fail them (review finding 2, pre-existing, no change).

## Requirements

- CPL-001: the test takes the pid from a `CHILD ` row that is finished (the cursor moved below it)
  before it types anything, and uses that pid after `ECHO: ping`.
- CPL-002: no retry and no sleep is added; the wait is on the condition.
- CPL-003: the reproduction (the fake agent pausing between `CHILD <pid>` and its line end) makes
  the old test fail and the new test pass.

## Plan

- [x] Spec for the CHILD pid flake written and valid
- [x] Flake reproduced with a split CHILD line and cause named
- [x] Test waits for a finished CHILD line before typing
- [x] Independent review answered and gates green
- [x] PR merged into dev with evidence on #647

## Acceptance criteria

- With the probe, the test on `dev` fails with `InvalidDigit` every time; with the fix it passes.
- The test passes without the probe, alone and in `scripts/dev ci`.

## Test plan

- Probe: the fake agent writes `CHILD <pid>`, flushes, sleeps, then writes `\n` (temporary).
- Run the test 10 times with the probe before and after the fix, and under CPU load without it.

## Decisions

- Read the pid from the screen row above the cursor, not from the live output: the CHILD line can
  already be in the connection's snapshot, which carries no live bytes.
- Do not make the descendant's `SLEEPING` a suspect: the agent writes `CHILD <pid>\n` in one
  `write(2)` (line-buffered stdout) and the tty write lock keeps other writers out of it; only the
  echo, which takes the output lock alone, gets between the text and its line end.

## Evidence

- Reproduction (temporary probe, not committed): the fake agent's fork-child path writes
  `CHILD <pid>`, flushes, sleeps 200 ms, then writes `\n`; the descendant delays `SLEEPING` 1 s.
  On `dev` f770e4ff: `cargo nextest run -p yulius-ptyhost -E
  'test(shutdown_escalates_past_ignored_signals_and_reaps_descendants)'` failed 10 of 10 with
  `Error: ParseIntError { kind: InvalidDigit }`; a debug print showed the row `CHILD 4688ping` and
  the live text `ping\r\n\r\nECHO: ping\r\n` (the CHILD line came in the snapshot). A first probe
  without the descendant delay gave `4412SLEEPING`, an artifact of splitting the line into two
  writes.
- Fix with the probe: the same command passed 10 of 10.
- Without the probe: `cargo nextest run -p yulius-ptyhost` 49 of 49 passed; the test binary run
  100 times with `--exact` beside 36 busy loops on 12 cores (load average 43) passed 100 of 100.
- `scripts/dev ci` on 7d3b50a9: 2610 of 2611 tests passed; the one failure was
  `yulius::main leftovers::kept_files_of_a_removed_sandbox_are_listed_and_deleted_on_request`
  (`kept[0]["bytes"]` was null, `crates/yulius/tests/leftovers.rs:989`), a flake outside this
  change (no leftovers code touched; 5 of 5 passed alone), reported on #647. The gate is rerun on
  the final head.
- Independent review (fresh session, `om-code-review`): approved 7d3b50a9; finding 1 (this spec's
  status, plan and wording) fixed here; findings 2 and 3 pre-existing, no change.
- `scripts/dev security`: run by Yulius CI's `security` job on the pull request head.
