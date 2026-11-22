---
yulius-task: 2a6cef0e-5b68-403c-8cea-fb84cb13271e
status: done
---

# SPEC-618 A test failure that names the change's own file makes no test flaky

## Goal

Part of #1545. Yulius CI quarantined `yulius::main
fixture_hygiene::test_fixtures_hold_no_provider_shaped_credential` (job `test (2/2)`) as flaky
after it failed in the runs of pull requests #1470 and #1543. Find why it failed on changes that
were not its own, fix the cause, prove it, and say what lifts the quarantine.

## Context

- The test (ADR-0258, SPEC-406) reads every file under `crates/yulius/tests` of the checkout and
  fails naming each provider-shaped literal by file and line. It reads tracked sources only; no
  test writes under that folder (the two `tests/…` writes in `delivery.rs` go to temporary
  repositories), so for one tree it always gives the same answer.
- PR #1470 (run `0f0d2921`, head `55817fe`) failed it with `tests/subscription_moves.rs:404:
  sk-ant- and 35 token characters`. PR #1543 (run `69a154f0`, head `b19ab9b85`) failed it with
  `tests/needs_you_messages.rs:541` and `tests/telegram.rs:2604` (`ghp_`). Each literal is at the
  pull request's head and absent at its merge base with `dev`; each pull request then assembled it
  with `concat!` (`cb7f9ad8a`, `c8a09ce6d`) and passed. Both failures were their own change's.
- So the test is not flaky; the quarantine is wrong. ADR-0356 defines flaky as "failed in the runs
  of 2 different pull requests within 48 hours … while the integration branch did not fail it",
  standing for its context's "failed … on the same tests in pull requests that did not touch
  them". A test that checks the repository's content fails, correctly, in every pull request that
  breaks the rule, and two such pull requests in 48 hours quarantine it. ADR-0356 names this
  ("two unrelated pull requests that both broke the same test") as a consequence ended by closing
  the issue, but nothing prevents it: fixture_hygiene failed this way in tasks #388, #484, #485,
  #521 and #562, and the same holds for every repository reader of SPEC-610 (`ui_contract`, the
  documentation and catalog readers).
- While quarantined, the gate leaves the check out, so a real token pasted into a test file passes
  CI.
- `App::ci_changed_paths` (SPEC-493) already tells the files a pull request's head changed since
  its merge base with the integration branch.

## Requirements

- FOC-001: When a pull request's job fails a test whose kept failure block names a file that pull
  request changed (against its merge base with the integration branch), that failure is the
  change's own: it is not recorded as a failure that can make the test flaky, and the job's output
  says so, naming the file.
- FOC-002: A file is named when the block holds its repository path (a nested one also after any
  `/`), or a suffix of it of at least two path components (a test running in its crate prints
  `tests/x.rs` for `crates/app/tests/x.rs`), not continued by a path character or inside a word. A
  one-component path is named only whole.
- FOC-003: A failure that names no changed file counts as before; so does every failure when the
  changed files cannot be told, and every branch's failure.
- FOC-004: The quarantined test is proven deterministic: it fails at both pull requests' heads,
  naming their own files, and passes at their merge bases and at this change's head, run
  explicitly (the gate leaves it out).

## Plan

- [x] Reproduce both quarantining failures at their own pull requests' heads
- [x] Tell each failing test's block and the changed files it names
- [x] Leave a pull request's own-file failures out of the flaky count
- [x] Record the rule in an ADR, the architecture and the changes
- [x] Run the quarantined test and every touched test binary
- [x] Independent review approves the head

## Acceptance criteria

- Two pull requests whose failing blocks each name a file their own change added quarantine
  nothing, and each job's output says the failure is its change's; the same failure naming no
  changed file still quarantines (existing tests).
- The probe of FOC-004 shows fail at `55817fe` and `b19ab9b85`, pass at their merge bases and at
  the head of this change.
- The pull request says what lifts the quarantine: closing #1545 after this change is deployed.

## Test plan

- Domain unit tests: blocks of nextest and libtest output per test; path naming by whole path,
  two-component suffix, bounded on both sides, a one-component path whole only.
- `local_ci` integration test with the real Git fixture: two pull requests fail a test whose block
  names the file each added: no issue, no quarantine, the output line; it fails without the
  change (an issue is filed).
- Probe: the built `main` test binary run against the `crates/yulius/tests` tree of each commit.
- Whole binaries: `yulius-domain` library, `yulius-app` library, `yulius --test main`.

## Decisions

- ADR-0453: the change's own failure is told by its failing block naming a file the change
  touched. Rejected: moving fixture_hygiene into `scripts/check_repository.py` (fixes one test,
  every other repository reader stays exposed and ADR-0258 chose a test); a project list of tests
  never quarantined (manual, forgotten for the next reader); counting only pull requests that do
  not touch the test's own file (both pull requests here touched other files); running the test
  again to see (ADR-0356 forbids it).
- The own failure is not recorded at all rather than recorded with a flag: the failures table is
  read only for flakiness, and no migration is needed.
- A coincidental match (a flaky test whose output names a file the pull request changed) only
  keeps that pull request's failure from counting: a quarantine is delayed, never made wrongly.

## Evidence

- Probe `target/probe-1545.sh` (the built `main` test binary run with `--exact
  fixture_hygiene::test_fixtures_hold_no_provider_shaped_credential` on each commit's
  `crates/yulius/tests` tree): `55817fe` rc=101 `tests/subscription_moves.rs:404: sk-ant- and 35
  token characters`; `8d05c25bf` (its merge base) rc=0; `b19ab9b85` rc=101
  `tests/needs_you_messages.rs:541` and `tests/telegram.rs:2604` (`ghp_`); `5addbfbb8` (its merge
  base) rc=0; `cb7f9ad8a`, `c8a09ce6d` (the fixes) and this head rc=0. `git show` confirmed each
  literal at the head and absent at the merge base.
- `local_ci::a_failure_naming_its_own_changed_file_makes_no_test_flaky` with
  `crates/yulius-app/src/ci/flaky.rs` from before the change: failed at "each pull request broke
  it" (an issue was filed for #1, #2); with the change: passed, as did the three other quarantine
  tests.
- Quarantined test run explicitly: in the full `cargo nextest run -p yulius --test main` (no
  quarantine filter) `fixture_hygiene::test_fixtures_hold_no_provider_shaped_credential` PASS;
  2594 passed, 24 skipped. `--stress-count 30` of it alone: 30 of 30 passed.
- `yulius-domain` and `yulius-app` libraries: 1307 passed; `yulius-domain` doctests passed;
  `cargo clippy -p yulius-domain -p yulius-app -p yulius --all-targets -D warnings` and
  `cargo doc -D warnings` for both libraries clean; `scripts/check_repository.py` passed.
- The Yulius instance runs `aabf37091` (main); the rule acts on CI once a build with it is
  deployed there. Vidoc: disabled for this project, nothing scanned.
- Review round 1 (yulius-reviewer) approved `f60ce3726` with three minors and three nits on own
  code. Answered in the next commit: the texts now say a nested changed path is also named after
  any `/` (pinned: `src/lib.rs` named by `crates/other/src/lib.rs`), a delay and never a wrong
  quarantine; `local_ci::a_broken_job_counts_its_own_failures_too` pins the over-ten rule (with
  `tests.len()` in place of `failing` it failed, "a broken job, not a flaky test"); the operations
  guide and both module docs state the exception; the output line is printed only for a
  quarantinable name and says "taken as the change's own"; ADR-0453's Consequences narrow the
  content-check claim and name the `test`-like top-level file case, kept as a documented delay.
  Afterwards: 1307 library tests, the full `yulius --test main` (2595 passed, fixture_hygiene
  included), clippy, rustdoc, doctests and the repository check passed.
