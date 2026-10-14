---
yulius-task: 0d502300-4de1-4061-84fe-d2f03da16e49
status: done
---

# SPEC-088: `yulius doctor` warns about a GitHub CLI older than 2.82.1

Status: implemented
Date: 2026-10-14
Owner: Yulius task #73
Scope authorization: task request: the doctor's GitHub CLI check warns below 2.82.1, stays OK for
current or unrecognised version lines, points to GitHub's package repository, and the GitHub CLI
row of `docs/operations.md` says the same.

## Goal

An operator running `yulius doctor` on a host whose `gh` is older than 2.82.1 is told so, why it
matters and where to get a current one, before agents in every sandbox hit the failure.

## Context

- GitHub retired Projects (classic). Since then, `gh pr edit` and `gh issue view --comments` fail
  with a GraphQL error on GitHub CLI versions before 2.82.1, which no longer ask for the classic
  project fields. Ubuntu 24.04 ships gh 2.45 (this host: `gh version 2.45.0 (2025-07-18 Ubuntu
  2.45.0-1ubuntu0.3)`); agents hit the error about 35 times on 2026-10-12/13.
- Sandboxes copy the host system (SPEC-002 ADR-22), so the host's `gh` is every sandbox's `gh`
  (SPEC-004). The base must be rebuilt after the host's `gh` is upgraded.
- `crates/yulius/src/doctor.rs` checks `gh --version` and reports its first line as OK, or warns
  when `gh` is missing and suggests `apt-get install gh`, which installs exactly the old version.
- `gh --version` prints `gh version X.Y.Z (date)` and then a release URL.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| GHV-001 | The GitHub CLI check warns when `gh --version`'s first line is `gh version X.Y.Z…` with X.Y.Z below 2.82.1; the detail shows the version line, says `gh pr edit` and `gh issue view --comments` fail on it, and points to GitHub's package repository (`https://github.com/cli/cli/blob/trunk/docs/install_linux.md`) and the base rebuild. | unit tests in `doctor.rs` |
| GHV-002 | The check stays OK at 2.82.1 and above, and for a version line it does not recognise (it never warns on a guess). | unit tests in `doctor.rs` |
| GHV-003 | A missing `gh` still warns, and points to GitHub's package repository instead of `apt-get install gh`. | unit test in `doctor.rs` |
| GHV-004 | `docs/operations.md`'s GitHub CLI row names the minimum version, the package repository and the doctor's warning. | doc review |

## Plan

- [x] Spec SPEC-088 written and validated
- [x] yulius doctor warns about a gh older than 2.82.1, with tests
- [x] Operations doc and changes describe the minimum gh
- [x] Independent review in a fresh session, findings fixed
- [x] Every gate green with dev merged

## Acceptance criteria

- AC-1: `gh version 2.45.0 (2025-07-18 Ubuntu 2.45.0-1ubuntu0.3)` and `gh version 2.82.0 (…)`
  give Warn, whose detail contains the version line, `2.82.1`, `gh pr edit` and the install URL.
- AC-2: `gh version 2.82.1 (2025-10-22)`, `gh version 2.83.0`, `gh version 3.0.0` give Ok.
- AC-3: unrecognised lines (`gh version DEV`, `something else`, `gh version 2`) give Ok with the
  line as the detail.
- AC-4: no `gh` gives Warn with the install URL and without `apt-get install gh`.
- `scripts/dev ci` passes.

## Test plan

- Unit tests in `crates/yulius/src/doctor.rs` on a pure function from the `gh --version` first
  line (or none) to the check: AC-1 to AC-4, written first and failing against the old check.
- Manual: `yulius doctor` on this host (gh 2.45.0) shows the warning.

## Decisions

- Parse only `gh version <digits>.<digits>.<digits>` and compare numerically; anything else is
  OK. A doctor that warns on an unknown format would cry wolf on a build or packaging we have not
  seen; the cost of missing a warning there is the status quo.
- Pre-release suffixes (`2.82.1-pre.0`) count as their release: the patch's leading digits are
  taken. A pre-release of 2.82.1 is close enough not to warn.
- Warn, not Fail: Yulius itself works with an old `gh`; only two of its subcommands break for
  agents, and `yulius doctor` fails only when Yulius cannot work.
- The threshold is a constant in `doctor.rs` next to the check, not configuration: it describes
  GitHub's API, not the deployment. No ADR: this is a local check, recorded here.

## Evidence

- Red first: `doctor::tests::a_github_cli_older_than_2_82_1_warns_and_points_to_the_package_repository`
  (2.45.0 was Ok) and `a_missing_github_cli_points_to_the_package_repository` (detail said
  `apt-get install gh`) failed against the old check; `a_current_or_unrecognised_github_cli_is_ok`
  passed before and after. All three pass with the change (`cargo test -p yulius --lib doctor::`:
  6 passed).
- Manual, this host (gh 2.45.0): `yulius doctor` prints `warn  GitHub CLI  gh version 2.45.0
  (2025-07-18 Ubuntu 2.45.0-1ubuntu0.3) is older than 2.82.1: `gh pr edit` and `gh issue view
  --comments` fail …; install it from GitHub's package repository
  (https://github.com/cli/cli/blob/trunk/docs/install_linux.md), then rebuild the sandbox base`.
- Review round 1 (fresh read-only session) at f4a314d7: APPROVE, no blockers or majors. Minors
  fixed: the warning says the commands "fail here and in every sandbox" (they also fail on the
  host), test-first (red against "fail in every sandbox"); the warning test also pins
  `gh issue view --comments` and the missing-`gh` test the base rebuild hint, text that was
  already there (these two assertions guard against regressions, they were not red).
- Review round 2 (fresh read-only session) at 3b6684a7: APPROVE. Fixed: the record above (only
  the wording assertion was red), "install a current `gh`" instead of an ambiguous "it", and the
  changes entry says the commands fail on the host too. A `+` sign accepted by the number
  parser (`gh version +2.45.0`) is left as it is: not a real `gh` output.
- `scripts/dev ci` at f4a314d7 (dev up to date): exit 0; nextest 1505 run, 1505 passed,
  16 skipped; repository check, check, fmt, clippy, doctests, deny, shear, doc green. After the
  review fixes: `cargo fmt --check`, `cargo clippy -p yulius --all-targets --all-features -D
  warnings` and the doctor tests green.
- Not verified here: that 2.82.1 is exactly the first fixed release (taken from the task request;
  the check is a constant, `GH_MIN_VERSION`, if it needs moving). The host's `gh` was not upgraded.
