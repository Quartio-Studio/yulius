---
yulius-task: f297e672-6e37-4678-8dd4-2ccf47909b04
status: done
---

# SPEC-642: Repair the integration gate: flaky parking::candidate::a_candidate_refresh_waits_for_the_launch_to_reserve_its_verified_slot

## Goal

Make `yulius::main parking::candidate::a_candidate_refresh_waits_for_the_launch_to_reserve_its_verified_slot`
deterministic. The full integration gate on dev `28ab10c50` (Yulius CI run
`b9cc8d29-a9c0-472d-8cf1-eeedf88d8aa5`, job `test (2/2)`) failed at
`assert_eq!(retry.len(), 1, "{retry:?}")` (`candidate.rs:614`) with `retry == []`. The merged
PR #1739 changed no Rust, so the test is flaky.

## Context

- The test came with #1683/#1720 (SPEC-621 step 49). SPEC-621 already met a flake in its retry
  part: "stopping the peer wakes the queue, whose capacity check briefly holds the owner's
  delivery guard, and a watch pass meeting it defers the hosted read (`observe_candidate_pull`'s
  `try_write`), as designed". Its fix loops passes until `remote.pull_reads` passes `during`,
  then takes that pass as the answer.
- `observe_candidate_pull` (`pr_watch.rs`) takes the owner's `arm_writer` with `try_write`
  twice: before the hosted read (to start a new observation generation) and after it (to record
  the answer). A typing pass observes twice: in `pr_facts` (deferring through
  `observed_only()`) and in `armed_notice_recipient`. Any of these `try_write`s fails with
  `Conflict` while anyone holds the guard: the pass types nothing and the notice stays unheard
  for the next pass.
- Capacity readers hold the owner's guard as a read guard for the length of their checks
  (`hold_live_delivery`, through `working_capacity`): the queue's launch, `status`,
  `capacity_view`, account views, notice producers, start rules and `close`. Several run on
  their own clocks.
- Probe count: `FakeGitHub::pull_reads` counts every `get_pull` and `list_pulls`, from any
  caller.

## Root cause

The test's wait condition is not the condition it asserts. It stops at the first pass after
which a hosted read was counted and asserts that pass typed the notice. A pass can read the pull
request and still defer: when a capacity reader takes the owner's guard during the read, the
second `try_write` fails, and the pass types nothing. The next pass types the notice. The gate's
log holds only the panic; that a capacity check landed inside the read there is inferred, and
the reproduction shows it is sufficient: the read was counted, the pass typed `[]`.

The product is right. A deferred pass records nothing and leaves the notice unheard. The next
pass verifies the pull request again and types it (SPEC-621: deferring is designed). Waiting
for the guard after a hosted read could instead hold the watch behind a launch's slot
reservation, which this test's first half forbids.

## Requirements

- FLK-001: The test reproduces the interleaving every run: a retry pass is held inside its
  hosted read (`pull_read_hold`) while a capacity reader (`status`) holds the owner's delivery
  guard. A new test pause point, `LiveDeliveryHeld`, is reached in `hold_live_delivery` once
  the read guard is taken (keyed by task id; does nothing in production builds). The held pass
  reads and types nothing.
- FLK-002: The test then waits for the condition it asserts: passes run until one types, each
  earlier pass typed nothing, bounded by a timeout. The typing pass types exactly the one
  failure notice (`shard 2`, `292.1 ms`), and the next pass types nothing.
- FLK-003: Waits are on conditions (notifies, typed outcomes), with no sleeps and no retried
  test runs.

## Plan

- [x] Root cause: a pass can defer after its hosted read
- [x] Regression check: a capacity reader inside the held read makes the old wait fail every run
- [x] Fix: wait for the pass that types; candidate tests in full and under stress
- [x] Independent review in a fresh session, findings answered

## Acceptance criteria

- With the regression check and the old wait rule, the test fails every run with `[]`.
- With the fix, the test passes under `--stress-count`, also beside CPU-bound busy loops.
- Every `parking::` test of the `yulius::main` binary passes.

## Test plan

- `scripts/cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/a_candidate_refresh_waits/)' --stress-count 20`
  before and after the fix.
- The same beside CPU-bound busy loops.
- `scripts/cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^parking::/)'`.

## Decisions

- Fix the test, not the product: deferring after a read costs one more hosted read on the next
  pass and loses nothing. Blocking on the guard after the read would let a launch's
  reservation stall the watch.
- The pause point is a test hook like the others in `race.rs` (ADR-0174), so the interleaving
  is certain in every run, not left to timing.

## Evidence

- Regression check (`dc4621278`: the held pass under a `status` capacity reader, judged by the
  old rule "the pass that read is the answer"): `--stress-count 5 --no-fail-fast`: 5/5 fail with
  `` assertion `left == right` failed: [] `` (left 0, right 1), the gate's panic.
- Fixed (the held pass typed nothing; passes run until one types):
  `--stress-count 20`: 20/20 pass; `--stress-count 20` beside 128 CPU-bound busy loops (load
  average 4 → 110): 20/20 pass.
- All `parking::` tests of `yulius::main`: 165/165 pass.
- `cargo clippy --locked -p yulius -p yulius-app --all-targets --all-features -- -D warnings`
  and `cargo fmt --all -- --check` pass.
- Independent review in a fresh session (Fable 5.2), round 1 at `0022fc831`: approve, no
  blocker, major or minor. Nits answered: the root-cause wording now says the gate's interleaving
  is inferred and names both observation sites. `pull_reads > during` stays a lower bound, not
  `during + 1`: `status` and the other readers may list pull requests too, so an exact count
  would add a new timing dependency. If `status` needed over 5 s to reach the hook, the
  product's read timeout would end the held read instead of the guard; the test still passes,
  it just would not exercise the guard on that run.
- Yulius CI run on PR #1742 at `0022fc831`: passed.
