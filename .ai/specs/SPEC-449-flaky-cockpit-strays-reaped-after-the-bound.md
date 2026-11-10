---
yulius-task: 6e4c881a-2895-4dda-89ba-2d2468d42ed0
status: done
---

# SPEC-449: An idle Superagent's strays are dated by the probe that found them (#1110)

Status: done
Date: 2026-11-10
Owner: Yulius task #5 (fork studenciak00owl/yulius)
Scope authorization: fix GitHub issue #1110 in one pull request into `dev` (`Fixes #1110`).

## Goal

`cockpit::an_idle_superagents_strays_are_reaped_after_the_bound` passes under the gate's load:
the strays an idle Superagent's probe finds count their bound from the moment that probe was
asked, so the time between the probe's answer and the Cockpit's reading of it (within the probe's
timeout) can no longer restart the bound.

## Context

- **Symptom (#1110).** In a full `scripts/dev ci` run (task #418, head 3ff6ac27) the test failed
  once with `timed out waiting for the strays to be reaped` after 21 s; alone it passed 5 of 5.
- **Code path.** The Cockpit's watch (`crates/yulius-app/src/cockpit.rs`, `watch_cockpit`) probes
  an idle Superagent at most every `STRAY_PROBE_EVERY_MS` (60 s, test clock), stamping
  `strays_probed` with the iteration's `now`. `tend_cockpit_strays` runs the host's
  `background_probe` on a blocking thread, awaits it, and only then calls `sort_strays` with a
  fresh `self.now()`: a stray seen for the first time is dated then, and is due once
  `stray_reap_after_ms` (2 h) has passed since.
- **The test.** It waits until the fake host counted one probe (`GatedHost::probes`, counted
  inside `background_probe`), moves the test clock by the bound and waits for a reap.
- **Root cause (the race).** The probe is counted before the Cockpit takes its answer: between the
  count and `sort_strays` lie the blocking thread's return and the watch task's next poll. When
  the test's 50 ms poll sees the count and moves the clock inside that window (a starved runner
  widens it), the strays are first seen at the moved time, the bound starts again, and the next
  probe (due, as 60 s passed since `strays_probed`) finds them not due. Nothing moves the clock
  again, so the wait times out. `a_superagent_turn_that_starts_during_the_probe_keeps_its_strays`
  has the same window before its first clock move.
- **Product effect.** The dating is up to `SHELL_PROBE_TIMEOUT` (3 s) late against a 2 h bound:
  harmless in production, but it makes the first sighting depend on scheduling. The task path
  (`finish.rs`, `tend_strays`) probes the same way; its tests (`strays.rs`) await the probe
  before moving the clock, so they have no such window, and this change leaves it alone.
- **Known leftover (pre-existing).** A probe answer that comes later than `SHELL_PROBE_TIMEOUT`
  (3 s) is dropped, in production too (the next probe sees the strays). The fake counts the probe
  inside the blocking closure, so if the blocking pool started that closure more than 3 s late,
  the test could still move the clock before a sighting was kept. That needs a trivial closure to
  wait 3 s for a thread, far rarer than the window fixed here; it is noted in the pull request.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| SDP-001 | An idle Superagent's strays are first seen at the time their probe was asked (read just before the probe starts, in the same step as the watch's `strays_probed` stamp), and are due once the bound has passed since; time that passes while the probe runs counts toward the bound. | `crates/yulius-app/src/cockpit.rs` |
| SDP-002 | A regression test moves the clock by the bound inside the first probe: it fails on the old dating (never reaped) and passes with SDP-001, reaped at the second probe. | `crates/yulius/tests/cockpit.rs` |
| SDP-003 | No sleeps, retries or longer waits added; the existing tests keep their conditions. | diff |

## Plan

- [x] Race reproduced with a probe delay and named in this spec
- [x] Regression test that moves the clock inside the probe fails on the old dating
- [x] Strays dated by their probe's start; cockpit tests pass
- [x] Cockpit tests 20 times under load, gates and docs updated

## Acceptance criteria

- With a throwaway delay at the await point after the probe (before `sort_strays`), the unchanged
  `an_idle_superagents_strays_are_reaped_after_the_bound` fails with #1110's signature; with the
  fix and the same delay it passes.
- The regression test fails before the fix and passes after it.
- `cockpit::` passes 20 consecutive times while the machine runs a parallel load.

## Test plan

- Probe delay: a temporary `tokio::time::sleep` after the probe's answer in
  `tend_cockpit_strays`, the flaky test run before and after the fix (not committed).
- New `an_idle_superagents_strays_are_seen_when_their_probe_was_asked` in `cockpit.rs`, before
  and after the fix.
- `cockpit::` 20 times in a row under a parallel CPU load; `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings`.

## Decisions

- **Fix the dating, not the test's wait.** Options:
  - A longer wait or a retry in the test: the clock never moves again, so no wait can pass;
    a retry hides the race.
  - Let the test move the clock again until a reap: passes, but no longer proves the strays are
    reaped one bound after they were first seen.
  - Date the sighting by the probe's start (a `now` read just before it is asked, as the watch's
    `strays_probed` is): chosen. The first sighting no longer depends on when the watch task runs
    again, the tests' conditions stay exact, and the strays can be reaped at most one probe's
    duration (under 3 s) earlier against a 2 h bound. No ADR: it corrects which instant SLR-004's
    "first seen" names and changes no contract.
- **The task path keeps its dating.** Changing `finish.rs` the same way would align the two paths
  (a difference of at most 3 s against 2 h), but it has no race and no test that could fail on it,
  so it stays out of this one-concern fix.

## Evidence

All on `fix/flaky-cockpit-strays-1110` from upstream `dev` 91a0cec5, on a 6-CPU, 12 GB Linux
sandbox shared with other agent sessions (its load average was about 12 before any load of ours).

- **Reproduction with a probe delay.** A throwaway `tokio::time::sleep(200 ms)` after the probe's
  answer in `tend_cockpit_strays`, before `sort_strays` (not committed), on the unchanged dating:
  `cargo test -p yulius --test main -- cockpit::an_idle_superagents_strays
  cockpit::a_superagent_turn_that_starts_during_the_probe cockpit::strays_are_seen`: 0 of 3
  passed; `an_idle_superagents_strays_are_reaped_after_the_bound` failed with #1110's signature
  (`timed out waiting for the strays to be reaped`), the turn test with `timed out waiting for the
  strays to be reaped after the turn`, the new test with the first signature. With the fix and the
  same delay: 3 of 3 passed (5.8 s).
- **Regression test, old dating, no delay.** The new test (then named
  `strays_are_seen_when_their_probe_was_asked`, renamed in review with the same body): failed,
  `timed out waiting for the strays to be reaped` (20.3 s). With the fix: passed.
- **20 runs under load.** `target/flaky1110/loaded20.sh` (not committed) starts a loop of
  `cargo build --workspace -j 3` into its own target directory on disk, cleaned after each build,
  plus 6 `sha256sum /dev/zero` CPU burners, waits 60 s, then runs the `main` test binary with the
  filter `cockpit::` 20 times in a row: 20 of 20 passed, 51 of 51 tests each (54–61 s per run;
  load average 9.0–15.0 on 6 CPUs, 1.6–4.9 GB memory available).
- **An earlier, overloaded series** (before review; `-j 6` build whose target directory sat on the
  RAM-backed `/tmp`, plus a concurrent clippy): runs 1–4 passed; run 5 took 1316 s while the
  machine ran out of memory (OOM kills, load average 45) and two other tests failed,
  `a_blocked_cockpit_start_delays_no_other_user_or_sandbox` ("alice's open in another sandbox
  waited for her start in s1") and
  `a_cockpit_start_whose_request_is_dropped_still_ends_as_the_one_cockpit`
  ("timed out waiting for the dropped start to register its Cockpit"); the three stray tests
  passed. They touch no stray code; memory exhaustion is outside this change. A second series
  (`-j 3` on disk) passed runs 1–3 before it was stopped to apply the review's fixes.
- **Gates.** `cargo fmt --all --check`: exit 0. `cargo clippy --locked --workspace --all-targets
  --all-features -- -D warnings` (`-j 2`: the shared machine's memory): exit 0. The full
  `scripts/dev ci` and the macOS cross-check were not run here (memory); Yulius CI runs the gate
  upstream.
- **Security.** `yulius security-scan`: exit 2, not run ("No Vidoc key"); unrun, not passed.
  Yulius CI's `security` job scans the pull request upstream.
- **Review round 1** (fresh read-only session, approved): (1) minor, pre-existing: a probe answer
  later than `SHELL_PROBE_TIMEOUT` is dropped after the fake counted it, so the test could still
  time out if the blocking pool starts the closure more than 3 s late; recorded as a known
  leftover in Context, not fixed here. (2) minor, own: the Goal and SDP-001 claimed more than the
  fix; reworded. (3) minor, own: the task path dates sightings by the answer; kept, see Decisions.
  (4) nit, own: status, Plan and Evidence; done. (5) nit, own: test name and comment; renamed to
  `an_idle_superagents_strays_are_seen_when_their_probe_was_asked`, comment shortened.
