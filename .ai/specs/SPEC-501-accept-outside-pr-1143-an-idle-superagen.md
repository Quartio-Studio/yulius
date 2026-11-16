---
yulius-task: bf07c2f9-6e2f-4408-b169-bfc3b3b9d328
status: in-progress
---

# SPEC-501: An idle Superagent's strays are dated by the probe that found them (#1110)

Status: in-progress
Date: 2026-11-16
Owner: Yulius task #483
Scope authorization: outside pull request #1143 by studenciak00owl, accepted by MrFishPL,
brought onto current `dev` in one pull request (`Fixes #1110`).

This spec is the contributor's SPEC-449 (written in their fork against `dev` 91a0cec5), moved to
this task's number: `dev` has since given SPEC-449 to another spec
(`SPEC-449-fix-this-kind-of-issues-text-outside-the.md`). Its analysis, requirements and
decisions are theirs; Context notes what changed on `dev` since, and Evidence holds both their
proof and this task's on current `dev`.

## Goal

`cockpit::an_idle_superagents_strays_are_reaped_after_the_bound` passes under the gate's load:
the strays an idle Superagent's probe finds count their bound from the moment that probe was
asked, so the time between the probe's answer and the Cockpit's reading of it (within the probe's
timeout) can no longer restart the bound.

## Context

- **Symptom (#1110, open).** In a full `scripts/dev ci` run (task #418, head 3ff6ac27) the test
  failed once with `timed out waiting for the strays to be reaped` after 21 s; alone it passed 5
  of 5. SPEC-436's Evidence records the same failure.
- **Still present on `dev` (c7348d04).** About 40 pull requests touched `cockpit.rs`, `finish.rs`
  or `tests/cockpit.rs` since #1143 was opened; none changed the stray path.
  `tend_cockpit_strays` (`crates/yulius-app/src/cockpit.rs`) still runs the probe, awaits it and
  only then calls `sort_strays` with a fresh `self.now()`; the watch still stamps
  `strays_probed` with the iteration's `now` every `STRAY_PROBE_EVERY_MS` (60 s); the fake host
  (`tests/support/gated.rs`, `GatedHost::background_probe`) still counts the probe inside the
  blocking closure; the two tests are unchanged (now at `tests/cockpit.rs` lines 5502 and 5521).
  The flaky-test audit (#465) and #1286 (a `pool_supervision` test) did not reach this code.
- **Code path.** The Cockpit's watch probes an idle Superagent at most every 60 s (test clock).
  A stray seen for the first time is dated when `sort_strays` runs and is due once
  `stray_reap_after_ms` (2 h) has passed since.
- **The test.** It waits until the fake host counted one probe (`GatedHost::probes`), moves the
  test clock by the bound and waits for a reap.
- **Root cause (the race).** The probe is counted before the Cockpit takes its answer: between the
  count and `sort_strays` lie the blocking thread's return and the watch task's next poll. When
  the test's poll sees the count and moves the clock inside that window (a starved runner widens
  it), the strays are first seen at the moved time, the bound starts again, and the next probe
  (due, as 60 s passed since `strays_probed`) finds them not due. Nothing moves the clock again,
  so the wait times out. `a_superagent_turn_that_starts_during_the_probe_keeps_its_strays` has
  the same window before its first clock move.
- **Product effect.** The dating is up to `SHELL_PROBE_TIMEOUT` (3 s) late against a 2 h bound:
  harmless in production, but it makes the first sighting depend on scheduling. The task path
  (`finish.rs`, `tend_strays`) probes the same way; its tests (`strays.rs`) await the probe
  before moving the clock, so they have no such window, and this change leaves it alone.
- **Known leftover (pre-existing).** A probe answer later than `SHELL_PROBE_TIMEOUT` is dropped,
  in production too (the next probe sees the strays). The fake counts the probe inside the
  blocking closure, so if the blocking pool started that closure more than 3 s late, the test
  could still move the clock before a sighting was kept. That needs a trivial closure to wait
  3 s for a thread, far rarer than the window fixed here.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| SDP-001 | An idle Superagent's strays are first seen at the time their probe was asked (read just before the probe starts, in the same step as the watch's `strays_probed` stamp), and are due once the bound has passed since; time that passes while the probe runs counts toward the bound. | `crates/yulius-app/src/cockpit.rs` |
| SDP-002 | A regression test moves the clock by the bound inside the first probe: it fails on the old dating (never reaped) and passes with SDP-001, reaped at the second probe. | `crates/yulius/tests/cockpit.rs` |
| SDP-003 | No sleeps, retries or longer waits added; the existing tests keep their conditions. | diff |

## Plan

- [x] Spec SPEC-501 brought from the contributor's SPEC-449 onto dev
- [x] Contributor's dating fix and regression test on current dev, author kept
- [x] Race reproduced with a probe delay on dev, fix proven, cockpit tests under load
- [x] Changelog entry and Evidence
- [ ] Independent review of the change, findings answered

## Acceptance criteria

- With a throwaway delay at the await point after the probe (before `sort_strays`), the unchanged
  `an_idle_superagents_strays_are_reaped_after_the_bound` fails with #1110's signature; with the
  fix and the same delay it passes.
- The regression test fails before the fix and passes after it.
- `cockpit::` passes repeatedly while the machine runs a parallel load.

## Test plan

- Probe delay: a temporary `tokio::time::sleep` after the probe's answer in
  `tend_cockpit_strays`, the stray tests run before and after the fix (not committed).
- New `an_idle_superagents_strays_are_seen_when_their_probe_was_asked` in `tests/cockpit.rs`,
  before and after the fix.
- `cockpit::` repeated under a parallel CPU load; `cargo fmt --check`, `cargo clippy
  --all-targets -- -D warnings`; Yulius CI runs the whole gate.

## Decisions

- **Fix the dating, not the test's wait** (the contributor's decision, kept). Options:
  - A longer wait or a retry in the test: the clock never moves again, so no wait can pass;
    a retry hides the race.
  - Let the test move the clock again until a reap: passes, but no longer proves the strays are
    reaped one bound after they were first seen.
  - Date the sighting by the probe's start (a `now` read just before it is asked, as the watch's
    `strays_probed` is): chosen. The first sighting no longer depends on when the watch task runs
    again, the tests' conditions stay exact, and the strays can be reaped at most one probe's
    duration (under 3 s) earlier against a 2 h bound. No ADR: it corrects which instant SLR-004's
    "first seen" names and changes no contract.
- **The task path keeps its dating** (kept). Changing `finish.rs` the same way would align the two
  paths (at most 3 s against 2 h), but it has no race and no test that could fail on it.
- **Bringing it onto `dev`** (this task). The contributor's code commit applies to current `dev`
  unchanged apart from line offsets, so it is cherry-picked with them as its author. Their spec
  file is renamed to this task's number (SPEC-449 is taken on `dev`), and their changelog entry
  moves under today's date. Nothing of #1143 is dropped.

## Evidence

### On current `dev` (this task)

On `feat/483-accept-outside-pr-1143-an-idle-superagen` from `dev` c7348d04, a 64-CPU Linux
sandbox shared with other tasks (load average 35–94 during the runs). Logs and the load script are
in the worktree's ignored `target/proof1110/`.

- **Cherry-pick.** `git cherry-pick -n bb33b431` (#1143's one commit) applied cleanly to
  `cockpit.rs` and `tests/cockpit.rs` (offsets only); its spec file is this spec, its changelog
  entry moved under 2026-11-16. Commit 5da234cc keeps studenciak00owl as author.
- **Fix in place.** `cargo nextest run -p yulius -E 'binary_id(yulius::main) &
  test(/^cockpit::.*strays/)'`: 3 of 3 passed (2.2–3.2 s).
- **Regression test, old dating (mutation M1, not committed):** `asked` back to `self.now().0`:
  `an_idle_superagents_strays_are_seen_when_their_probe_was_asked` failed, `timed out waiting for
  the strays to be reaped` (20.2 s); the other two passed.
- **Probe delay, old dating (M2, not committed):** M1 plus
  `tokio::time::sleep(200 ms)` after the probe's answer, before `sort_strays`: 0 of 3 passed;
  `an_idle_superagents_strays_are_reaped_after_the_bound` failed with #1110's signature
  (`timed out waiting for the strays to be reaped`), the turn test with `timed out waiting for the
  strays to be reaped after the turn`.
- **Probe delay, with the fix (M3, not committed):** the same delay on the fixed dating: 3 of 3
  passed. File restored with `git checkout` and touched.
- **20 runs under load.** `target/proof1110/loaded20.sh`: 6 `sha256sum /dev/zero` burners pinned
  to CPUs 0–3, and the `main` binary with `test(/^cockpit::/)` pinned to the same 4 CPUs, 20
  times in a row: 20 of 20 passed, 75 of 75 tests each (12–14 s per run; host load average
  35–79). This load did not reproduce #1110 by itself, so the probe delay above is the
  deterministic proof; the series shows the fix breaks none of the Cockpit's tests under load.
- **Gates.** `cargo fmt --all --check`: exit 0. `cargo clippy --locked -p yulius-app -p yulius
  --all-targets --all-features -- -D warnings`: exit 0. The whole gate is Yulius CI's on the
  pull request head.
- **Security.** `yulius security-scan`: exit 2, not run (Vidoc refused the key: HTTP 401); unrun,
  not passed. Yulius CI's security job scans the pull request head.

### In the contributor's fork (from #1143, on `dev` 91a0cec5)

Their SPEC-449 Evidence, summarized: with a throwaway 200 ms delay after the probe, 0 of 3 stray
tests passed on the old dating and 3 of 3 with the fix; the regression test failed on the old
dating (20.3 s) and passed with the fix; `cockpit::` passed 20 of 20 runs (51 tests each) under
a `cargo build -j 3` loop plus 6 CPU burners on 6 CPUs (load average 9–15); fmt and clippy exit
0. An earlier overloaded series (out of memory, load average 45) failed two other tests that
touch no stray code. Their independent review approved after its findings on their own code
were fixed; its pre-existing finding is the Known leftover in Context.
