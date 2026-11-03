---
yulius-task: 103e3870-8384-4b50-9f9e-08314022b36f
status: done
---

# SPEC-403: Vidoc triage of the CD environment and CI lane findings (#784 #718 #716 #715 #738)

Status: done
Date: 2026-11-03
Owner: Yulius task #384
Scope authorization: task #384 (a person asked that each of the five issues end closed). Fixes
#738. Its mechanism as Vidoc describes it is wrong, but the independent review found a real
double dispatch in the same reservation. #784, #718 (a duplicate of #780), #716 and #715 are false
positives on `dev` at ffbc2e91. Each is dismissed with file:line evidence and a narrow
`[[ignore]]` entry per wording. Tests pin the two guards nothing pinned yet.

## Goal

Each of the five Vidoc findings about CD environments, CD run environments and CI lanes is
checked against `dev` (ffbc2e91). Each one ends closed: fixed test-first, or dismissed so that a
repeat scan does not reopen it.

## Context

- `[[ignore]]` matches the path exactly and the title as a case-insensitive substring. There is
  one narrow entry per wording (SPEC-262, ADR-0202). `.config/vidoc.toml` already accepts the
  uncontained-production wording twice (lines 280 and 419), but not #715's.
- SPEC-401 (task #378) dismissed #780, the same confirmation finding as #718, and pinned it in
  `cd::production_is_deployed_by_an_administrator_who_types_the_slug`.
- Line numbers below are from `dev` at ffbc2e91.

## Requirements

- VCE-001 (#784, false positive). An agent's deploy validates the environment name before
  anything else. The agent tool `cd_deploy` (`tool_cd_deploy`, `ci/cd.rs:2089`) applies
  `dom::env_name` at `ci/cd.rs:2098`. It is the only caller of the `pub(crate)`
  `cd_deploy_for_task` (`ci/cd.rs:581`, called at `:2099`). The person's route applies the same
  validator (`ci/cd.rs:560`). A test pins it: malformed names are refused with the
  unknown-environment message, and no run starts.
- VCE-002 (#718, duplicate of #780). `ci/cd.rs:991-992` refuses a non-empty `expect` unless it
  holds at least 7 characters and prefixes the SHA. The value is lowercased before the match,
  and Git SHAs are lowercase hex. A shorter value fails the guard with 409. It is not let
  through. The test of #780 pins this.
- VCE-003 (#716, false positive). The PATH of a CD run is the server's own PATH
  (`ci/cd.rs:1482`). That PATH is set by the operator, a trusted party that no agent can
  influence. `yulius host setup` writes `Environment=PATH=/usr/local/bin:/usr/bin:/bin` into
  `yulius.service` (`host_setup.rs:256`, docs/operations.md §3), which is the fallback literal
  itself. In a production run those directories are the host's read-only system. `/home`,
  `/root` and `/var` are empty, and only the run's directory and its cache are bound in
  (`production_containment`, `ci/cd.rs:1856`). So nothing an agent installed is on that PATH. A
  test pins the unit's PATH.
- VCE-004 (#715, false positive). `contained = false` is the documented development mode:
  docs/operations.md, lines 13 and 18, says runs are then marked "not contained", and the web UI
  shows a "Not isolated" chip. In that mode, every agent run and shell already runs on the host
  as the service user. That user can read the data directory and `master.key` (operations.md
  line 335). So an uncontained production run gets no capability that the agents lack. A
  production deploy still needs an administrator who types the slug. With containment, every CD
  run is contained (`cd_containment`, `ci/cd.rs:1331-1380`).
- VCE-005 (#738, real; fixed). Vidoc's mechanism is wrong. The first lock
  (`ci/mod.rs:1246-1250`) only skips the disk-room query when no lane is free. `take_lane` checks
  again and reserves under the lock scope that registers the executor
  (`ci/mod.rs:1256-1267`). A failed take registers nothing, and only one dispatcher exists
  (`start_ci`, `ci/mod.rs:1151-1159`).
  - **The real race:** the dispatcher keeps every run whose state is `Queued`. A pending wake
    can start the next dispatch before the first executor's Queued→Running transition
    (`ci/exec.rs:340`) commits. With 2 or more lanes (the default), the same run then takes a
    second lane. Its `executing.insert` drops the first executor's cancellation sender. The
    second executor's transition fails, and it forgets its own entry. A later cancel then finds
    no executor and marks the run cancelled while its jobs keep running.
  - **Fix:** the dispatcher reserves through `reserve_run`, which refuses a run that already has
    an executor, all under the `busy` and `executing` locks. That lock order is already the
    dispatcher's own, and no other site holds `executing` while it takes `busy`.
  - **Disk requeue:** a run queued again for disk forgets its executor itself
    (`ci_forget_executor`), and the executor's end wakes the dispatcher, so such a run is
    dispatched again as before.

## Plan

- [x] Triage the five findings against dev with file:line evidence
- [x] Pin the agent deploy's environment name check (#784) with a test
- [x] Accept the five false positives in .config/vidoc.toml
- [x] Reserve a CI lane only for a run without an executor (#738)
- [x] Independent review and gates green
- [x] Close the four dismissed issues and link #738 to the pull request

## Acceptance criteria

- `cd::named_environments_deploy_by_their_stage` refuses the agent tool's `cd_deploy` of
  malformed names with the unknown-environment message. It fails when `tool_cd_deploy` skips
  `dom::env_name` (mutation-checked).
- `host_setup::tests` asserts that the written unit pins `PATH=/usr/local/bin:/usr/bin:/bin`.
- `ci::lane_tests::a_run_with_an_executor_is_not_reserved_again` fails on dev's reservation (a
  second lane, `Some(1)`) and passes with the fix. The CI, CD and disk integration tests pass
  unchanged.
- `.config/vidoc.toml` has one narrow entry per dismissed wording (#784, #718, #716 and #715),
  each with its reason.
- Each issue ends closed. #738 is closed by the merged pull request. The others are closed as
  not planned, each with a comment giving the evidence, and #718 is labelled `duplicate` of
  #780.

## Test plan

- Additions to `crates/yulius/tests/cd.rs`, `crates/yulius/src/host_setup/tests.rs` and
  `ci/mod.rs`'s `lane_tests`.
- Mutation check of the #784 test, done by removing the validation in `tool_cd_deploy`.
- `scripts/dev ci` (the full gate). `yulius security-scan` reports the scan as disabled for this
  project (Vidoc is off in Yulius), never as passed.

## Decisions

- #738 is fixed, not dismissed. The pure `reserve_run` helper makes the race testable without
  timing, where a test racing two dispatches would need a sleep. The executor check sits in the
  helper that the dispatcher must use.
- No code change for the other four. Each guard Vidoc asks for already exists. Moving
  `dom::env_name` into `cd_start` (#784) would only duplicate a check that both entry points
  already make. A hard-coded PATH (#716) would ignore an operator's deliberate choice, and the
  installer already writes the minimal PATH. Refusing `contained = false` for production (#715)
  would break the documented development mode, where it protects nothing.
- Each dismissal gets an `[[ignore]]` entry, so that Yulius does not reopen the closed issue when
  Vidoc is switched on again and repeats the finding.
- Two related open findings on the inherited PATH, #755 (`supervisor.rs`) and #806
  (`accounts.rs`), belong to other triage groups. They are left to those groups and mentioned in
  the pull request.

## Evidence

- `scripts/dev ci` at 688d777a passed with exit 0. It ran the repository contracts, fmt, clippy,
  the macOS check, deps, doctests and nextest. Nextest reported 3001 tests passed and 16
  skipped. The run before it, at 1ac54c97, failed only on `cargo fmt --check`, in the new lane
  test, which was then reformatted.
- `cargo nextest run -p yulius-app -E 'test(/lane_tests::/)'`:
  `a_run_with_an_executor_is_not_reserved_again` failed with dev's reservation (`left: Some(1)`,
  `right: None`, "the run already executes") and passes with the fix.
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^(ci|ci_[a-z_]+|local_ci|disk|cd|merge_train|ci_lanes)::/)'`
  with the fix passed: 125 of 125 tests.
- Mutation checks:
  - When `tool_cd_deploy` passes the name through without `dom::env_name`,
    `cd::named_environments_deploy_by_their_stage` fails (`tests/cd.rs:1776`).
  - When the unit's `Environment=PATH` line is removed,
    `host_setup::tests::a_fresh_host_gets_the_service_and_a_base_with_docker` fails.
  - Both tests pass on the real code.
- `yulius security-scan` reported "Security scan disabled: Vidoc is turned off in Yulius". The
  scan is disabled for this project. It did not run, and it is not reported as passed.
- Independent review, two rounds in fresh sessions:
  - The first round, at 02248a01, approved and agreed with the four dismissals and their
    `[[ignore]]` entries. It found the double dispatch next to #738, which was already on dev,
    plus two nits: a test comment and the spec's citations. Those are fixed in a22ff69a.
  - The second round, at 1ac54c97, approved the fix. It found no lock-order inversion, no guard
    held across an `.await`, and no stuck disk-requeued run.
- Not tested: the double dispatch with real timing (two dispatches racing an executor's
  transition). The helper test covers the reservation decision without sleeps.

