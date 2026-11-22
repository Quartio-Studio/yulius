# 0356. A test failing on two pull requests is quarantined with an issue

## Status

Accepted. Specified in [SPEC-497](../../.ai/specs/SPEC-497-an-armed-pull-request-merges-in-about-on.md)
slice B (task #479), OFC-002. Amended by
[ADR-0453](0453-a-failure-naming-its-own-changed-file-makes-no-test-flaky.md): a pull request's
failure that names a file it changed is not recorded. Complements
[ADR-0354](0354-a-job-that-fails-for-the-infrastructure-runs-again-once-by-itself.md): a failure
there is the infrastructure's, here it is a test's that is not the change's.

## Context

On 16 Nov 2026, quartio-v2's `quality` jobs failed again and again on the same tests in pull
requests that did not touch them, for example timing tests under load. Each such failure cost the
pull request a cycle, and if the pull request was a merge candidate's member, it dissolved the
candidate. The workflow's rule allows two answers: reproduce the failure and fix it, or quarantine
the test with an issue. It forbids the third: running the test again until it passes. Nothing
told a test that fails on other people's changes apart from one the change broke, and nothing
quarantined one.

## Decision

- **Flaky** (`yulius_domain::ci::flaky::is_flaky`): a test is flaky if it failed in the same job
  in the runs of 2 different pull requests within 48 hours, at heads that share no commit outside
  the integration branch, and the integration branch did not fail it in that window. A test the
  integration branch fails is broken there, not flaky. A chained pull request carries its
  predecessor's commits, and a pull request recreated from the same branch carries its own: a
  break in those fails both, so they count as one change: every merge base of the two heads
  (`merge-base --all`; heads that both merged a newer integration commit have it and the shared
  commit as bases) must be on the integration branch, and a pair Git cannot answer (a head the
  clone lacks) counts as one change. Tests are named as
  their failing blocks name them (`crate tests::name` for nextest, `tests::name` for libtest).
  Only names of at most 200 bytes (the issue's title stays within GitHub's 256) of letters,
  digits, `_ : - . /` and one space are quarantined: they go into a filter,
  an issue's text and the environment, where a backtick, an `@` or a control character could do
  harm.
- **Kept**: when a job fails, its kept failing tests are recorded (`ci_test_failures`, migration
  0065, 7 days, with the run's reference and commit) for every reference. Only a pull request's
  failure can quarantine a test. A job with more than 10 failing tests, those kept and those only
  counted past the kept output's bound, quarantines none: it is broken, not flaky, and each
  quarantine files an issue.
- **Quarantined at once, with one issue**: the job whose failure makes the test flaky files an
  issue in the project's repository through the GitHub App. The issue is in the Definition of
  Ready's form (labels `type: bug`, `priority: high`, `area: ci`, `flaky test`) and names the
  pull requests. The test goes into `ci_quarantine`, keyed by sandbox and test. One quarantine of
  a sandbox is decided at a time (a slow GitHub holds only that project), and the list is read
  again under that lock, so two jobs that fail the test at
  once file one issue; the second job's output names it. The job's output and reason say so
  ("quarantined as a flaky test (issue #N)"), and an audit entry `ci.test.quarantined` is
  written. If the issue cannot be filed, nothing is quarantined and the output says why (the
  reason stays the job's own). A crash between filing the issue and recording the quarantine
  leaves that issue without a quarantine; the next failure files another.
- **The project is told in every later job's environment**, never in a file:
  - `YULIUS_CI_QUARANTINE` holds the names, one per line.
  - `YULIUS_CI_QUARANTINE_NEXTEST` holds a nextest filter of their test paths
    (`test(=tests::name) | …`), used as
    `cargo nextest run ${YULIUS_CI_QUARANTINE_NEXTEST:+-E "not ($YULIUS_CI_QUARANTINE_NEXTEST)"}`.
    The binary is left out: nextest refuses a filter naming a binary the head does not build (a
    binary new on the integration branch, one a pull request renames), and a gate must never fail
    on the quarantine. A test of the same path in another binary is left out with it.

  Both are empty when nothing is quarantined. A file in the job's home could have been replaced
  by a link the job planted. A project's gate leaves the quarantined tests out. This repository's
  does: `cargo xtask test` reads the names, lists the binaries the head builds
  (`cargo nextest list --list-type binaries-only`), leaves each test out in its own binary
  (`binary_id(=crate) & test(=…)`), names none whose binary is missing, and prints what it left
  out.
- **Released when the issue is closed**: the hourly prune asks GitHub about each quarantine's
  issue. A closed or deleted issue ends the quarantine (`ci.test.released`) and forgets the test's
  recorded failures, so the failures that made it flaky never count again: one more failure after
  the fix is not a quarantine. An issue that cannot be read keeps it. A sandbox that is gone or
  removed loses its quarantines.
- **Never run again until green**: nothing here runs a job or a test again. The job that
  quarantined the test still ends failed. Its next run, made by a push or `ci_rerun`, leaves the
  test out.

## Alternatives

- **Yulius turns a failed job into a pass when only quarantined tests failed**: this needs every
  failure of the job read from its output. A build error, a second test with no block, or a
  runner's own failure would then pass a run, and a pass gates the merge. The project's gate
  knows what it ran.
- **Rerun the failing test to see whether it is flaky**: the rule forbids it, and a rerun proves
  little under the same load.
- **One pull request's repeated failures count**: the pull request may well have broken the test.
  Only a second, unrelated change shows that the failure is not the change's.
- **The integration branch's last run decides, not the window**: a failure on the branch hours
  ago that was fixed since would keep a flaky test unquarantined. The window is the same for
  both.
- **A file listing the quarantine in the checkout**: the job could have linked it elsewhere
  (see above). The environment is the job's own.

## Consequences

- A test that fails on unrelated pull requests stops failing them from the third one on, in
  projects whose gate reads the variables. A project whose gate does not read them (quartio-v2's
  today) is told so in slice E. Until then, it gets the issue and the line in the output.
- The pull request whose failure quarantined the test needs one more run (push or `ci_rerun`).
  That run leaves the test out, which is not a rerun of the test.
- A quarantine wrongly made (two unrelated pull requests that both broke the same test) is ended
  by closing its issue. The test runs again in the next job, and needs two new pull requests'
  failures to be quarantined again.
- Runs before this change kept no failing tests' names: the 48-hour window starts empty.
