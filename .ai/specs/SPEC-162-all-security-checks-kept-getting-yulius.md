---
yulius-task: 9d64844f-957b-49eb-8bb7-bcb40e400f53
status: draft
---

# SPEC-162: A CI job that could not run ends skipped, and the security job gets its key

Status: draft
Date: 2026-10-15
Owner: Yulius task #153, issue #530 (part of #466)
Scope authorization: task #153: "All security checks kept getting: […] Security scan not run: No
Vidoc API key […] [exit code 0] but tests were marked as passed. Vidoc API key is set in agent's
secrets. Fix this. Do not run security hardening at the moment but just fix, i'll run it later."
Added before the start: "If Vidoc API key is not set, default behavious should be "skipped" so
user knows exactly".

## Goal

A Yulius CI job can end **skipped**, a state of its own that the CI page, `ci_status` and the
commit status show as such (`yulius wait` reports run states, and a run is never skipped): it does not block a merge, and it is never counted as passed.
This repository's `security` job ends skipped when the Vidoc scan could not run, and it gets
`VIDOC_API_KEY` (as a placeholder through the egress proxy) so that it can run.

## Context

- **Symptom.** Every pull request's `security` job prints `Security scan not run: No Vidoc API
  key …`, then `[yulius] the security scan did not run (exit 2 …)` and exits 0, so the job and the
  run show **Passed** (reproduced in every PR run since PR #496).
- **Root cause 1: no state for "did not run".** A job's state comes only from its exit status in
  `crates/yulius-app/src/ci/exec.rs` (`Ended::Exit`): 0 is `RunState::Passed`, anything else
  `Failed`. A job that wants to say "nothing was checked, and that must not block" has only exit 0,
  so the `security` command in `yulius.toml` maps `scripts/dev security`'s exit 2 to 0 (SPEC-142,
  ADR-0116) and the job reads Passed.
- **Root cause 2: no key in the job.** The grant line `secrets = "VIDOC_API_KEY"` on the `security`
  job was held back until the running instance knows `secrets` (ADR-0116: after `dev` is promoted
  to `main`). `main` carries #496 since promotion #517, but the line was never added, so the job has
  no key and the scan exits 2 with "No Vidoc API key". The grant is read from the integration
  branch's `yulius.toml` only, so it takes effect for runs after this change merges into `dev`.
- **Why the tests missed it.** The SPEC-142 tests assert the shell mapping of exit 2 to 0 as
  intended behavior; nothing asserted how a not-run job is shown.
- `RunState` (`crates/yulius-domain/src/ci/mod.rs`) is the state of CI runs and jobs (and of CD
  runs). It is stored as text (`ci_runs.state`, `ci_jobs.state`, no constraint), so a new value
  needs no migration.

## Requirements

- **SKP-001.** Yulius gives every CI job `YULIUS_CI_SKIP_CODE=78`. A job whose process exits with
  that code ends `skipped`; its exit code is kept. Any other non-zero exit is still `failed`.
- **SKP-002.** A run whose jobs ended passed or skipped is `passed`: the merge gate treats it as
  success. Failed, error and cancelled still win over skipped. A result copied to another head (a
  merge with the same tree, a merge train's verdict) keeps each skipped job skipped.
- **SKP-003.** Skipped shows as Skipped: the CI page (a badge and the run's job line), `ci_status`,
  and the commit status (`Passed: 3 jobs, 1 skipped in …`).
- **SKP-004.** The `security` job maps `scripts/dev security`'s exit 2 to `${YULIUS_CI_SKIP_CODE:-0}`
  and says `skipped`, and exits the same way on heads it does not scan (dev, main, merge trains), so an instance that does not yet know the code still passes it (no pull
  request is blocked by what only a person can fix). Findings (exit 1) still fail it.
- **SKP-005.** The `security` job names `secrets = "VIDOC_API_KEY"`.

## Plan

- [ ] Jobs that exit with `YULIUS_CI_SKIP_CODE` end skipped, and runs of passed and skipped jobs pass
- [ ] Skipped jobs show as Skipped in the commit status, `ci_status` and the CI page
- [ ] The security job ends skipped when the scan could not run and gets VIDOC_API_KEY
- [ ] Docs, ADR-0133 and changes say how a job is skipped; review and gates green

## Acceptance criteria

- A job exiting 78 is recorded `skipped` with exit code 78; its run is `passed`; the commit status
  says how many jobs were skipped.
- A job exiting 1 or 2 is still `failed`.
- The CI page and `ci_status` show Skipped for such a job.
- `yulius.toml`'s `security` job has `secrets = "VIDOC_API_KEY"` and exits
  `${YULIUS_CI_SKIP_CODE:-0}` on scan exit 2.

## Test plan

- Domain unit tests: `RunState` parses and names `skipped`; `aggregate` of passed + skipped is
  passed, skipped + failed is failed; `commit_status` counts skipped jobs; the gate treats a passed
  run with skipped jobs as success.
- Integration test (`crates/yulius/tests/local_ci.rs`): a job `exit "$YULIUS_CI_SKIP_CODE"` ends
  skipped, its run passes; written first and seen failing.
- `scripts/test_*` or the repository check: the `security` command's exit mapping (shell run with
  a stubbed `scripts/dev`) for exit 0, 1, 2 with and without `YULIUS_CI_SKIP_CODE`.
- The Vidoc scan itself is not run (the person asked not to run security hardening now).

## Decisions

- **A reserved exit code, given to the job in the environment** (ADR-0133), not a `yulius.toml`
  key per job (an older instance rejects unknown keys and the whole file; F 2026-10-15) and not a
  marker line in the output (a test could print it by accident). 78 is `EX_CONFIG` from
  `sysexits.h`, the code Drone uses for "skip"; a job reading it from `$YULIUS_CI_SKIP_CODE`
  stays correct if the code ever changes and can fall back to 0 on an older instance.
- **Skipped is a job state, not a run state.** A run of passed and skipped jobs is passed, and its
  description says how many were skipped: the merge gate stays one rule, and the person sees the
  skip on the job and in the status.
- **Every "could not run" exit of the scan is skipped**, not just "no key": a rejected key or an
  unbound host is equally "nothing was scanned", and the job's output names the reason.
