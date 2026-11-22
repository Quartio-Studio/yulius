# 0453. A failure naming its own change's file makes no test flaky

## Status

Accepted. Specified in [SPEC-618](../../.ai/specs/SPEC-618-make-the-quarantined-fixture-hygiene-tes.md)
(task #602, issue #1545). Amends
[ADR-0356](0356-a-test-failing-on-two-pull-requests-is-quarantined-with-an-issue.md).

## Context

ADR-0356 quarantines a test that failed in the runs of two different pull requests within 48
hours while the integration branch did not fail it, standing for "fails on pull requests that did
not touch it". On 20 Nov 2026 it quarantined `fixture_hygiene::test_fixtures_hold_no_provider_shaped_credential`
(issue #1545). That test is deterministic: it reads the tracked files under `crates/yulius/tests`
and fails naming each provider-shaped literal. PR #1470 failed it with
`tests/subscription_moves.rs:404` and PR #1543 with `tests/needs_you_messages.rs:541` and
`tests/telegram.rs:2604`. Each literal was at the pull request's head and absent from its merge
base, and each pull request passed once it built its literal with `concat!`. Both failures were
their own change's. A test that checks the repository's content (fixture_hygiene, `ui_contract`,
the documentation and catalog readers of ADR-0448) fails correctly in every pull request that
breaks its rule. Two such pull requests in 48 hours quarantined it, and the gate then stopped
checking for pasted tokens. ADR-0356 named the case as a consequence ended by closing the issue,
but nothing prevented it.

## Decision

- **A pull request's failure that names a file it changed is its own**: when a pull request's job
  fails a test whose kept failing block (`yulius_domain::ci::failures::failed_test_blocks`) names a
  file that pull request changed against its merge base with the integration branch
  (`App::ci_changed_paths`, the files that also pick its jobs), that failure is not recorded in
  `ci_test_failures`, so it makes no test flaky. The job's output says so: "`<test>` names
  `<file>`, which this change touched: its failure is taken as the change's own and makes no test
  flaky", for a name a quarantine could hold.
- **Named** (`yulius_domain::ci::flaky::named_changed_file`): a block names a repository path
  whole, a nested one also after any `/` (an absolute path of the checkout), or as a relative path
  of at least two components that ends it after a character no path holds or `./` (a test running
  in its crate prints `tests/x.rs` for `crates/app/tests/x.rs`). A top-level file is named only
  whole and not after a `/`. A match continued by a path character (`x.rs.bak`, `x.rsx`, `x.rs/`)
  or inside a word does not count. A suffix of another path does: changed `src/lib.rs` is named by
  `crates/other/src/lib.rs` or a toolchain's `/rustc/…/src/lib.rs`, which only delays.
- **Everything else counts as before**: a failure naming no changed file, any failure when the
  changed files cannot be told (too many, a head the clone lacks), and every branch's failure. A
  job failing more than 10 tests still quarantines none, counting its own failures.

## Alternatives

- **Move fixture_hygiene into `scripts/check_repository.py`**: fixes one test. Every other
  repository reader stays exposed, and ADR-0258 chose a test.
- **A project list of tests never quarantined**: manual and forgotten for the next reader, and a
  listed test that turns flaky is never quarantined.
- **Count only pull requests that do not touch the test's own file**: both pull requests here
  touched other files, which the test reads.
- **Run the failing test again on the integration branch to see**: ADR-0356 forbids judging by a
  rerun.
- **Record the failure with a flag instead of leaving it out**: `ci_test_failures` is read only for
  flakiness, so leaving it out needs no migration.

## Consequences

- A content check that names the file it refuses (fixture_hygiene) fails the pull request that
  breaks it and stays in the gate, however many pull requests break it. A check whose output names
  another file than the one changed (`ui_contract` naming the UI call when a route was removed in
  Rust) still counts as before.
- A flaky test whose block happens to name a file the pull request changed (its own test file
  edited, since a panic prints its location; a path both print; a top-level file named like a word
  of the runner's, such as `test`) is not counted for that pull request. In often edited test files
  that delays real flake detection. Its quarantine waits for two other pull requests; it is never
  made wrongly.
- The quarantine of #1545 was made before this rule. Closing the issue ends it and forgets its
  failures.
