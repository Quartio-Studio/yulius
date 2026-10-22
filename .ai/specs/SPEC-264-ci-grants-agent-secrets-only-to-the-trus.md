---
yulius-task: 136815f5-cee3-4578-a2dd-d76813ddc960
status: in-progress
---

# SPEC-264: CI grants agent secrets only to the trusted job command (#659)

Status: in-progress
Date: 2026-10-22
Owner: Yulius task #250
Scope authorization: task #250, started by the nightly "Work through open issues" run: solve
issue #659 in one pull request into `dev`. Fixes #659. Amends
[SPEC-142](SPEC-142-fix-466-yulius-ci-jobs-opt-in-to-named-a.md) CSE-002.

## Goal

A CI job gets the agent secrets the integration branch grants a job only when it runs the command
the integration branch grants them to. A pull request that changes the CI and reuses a granted
job's name for another command runs that command without any secret.

## Context

- Symptom: a pull request that changes `yulius.toml` is tested with its own `[[ci.jobs]]`
  (`GateSource::PullRequest`, `crates/yulius-app/src/ci/mod.rs`). If one of its jobs is named like
  a job the integration branch grants a secret (e.g. `deploy`, granted `DEPLOY_TOKEN`), it gets the
  placeholder and the proxy credential, and the proxy replaces the placeholder toward the secret's
  bound hosts in requests made by the pull request's own command.
- Root cause: `granted(gate, key)` in `crates/yulius-app/src/ci/exec.rs` finds the trusted entry by
  `job.key == key` only and never compares the command, though `ci_job_secrets` launches the
  job's own `command` (the pull request's text). SPEC-142 CSE-002 grants "by the entry of the same
  name" and never considered a same-named job with another command.
- Task #160 (SPEC-217) closed this for the scanning jobs only: `dom::keep_security_jobs` replaces
  a pull request's job keyed like a trusted scanning job with the trusted one. Every other granted
  key stayed open.
- Why tests missed it: `local_ci::ci_jobs_get_the_secrets_the_integration_branch_grants_them`
  asserted the defect as intended (the pull request's `granted` job ran `report("own ")` and was
  expected to get `key=yulius-secret-API_KEY ... proxy=set`); the unit test of `granted` used one
  command for every job.
- Matrix values: `dom::expand_jobs` expands a definition into one job per value, each with the
  definition's command, so a matrix job matches its definition's command.
- Commands on both sides come from the same parser (`validate_gate`), so equal configured text is
  equal command text.

## Requirements

- CSG-001: A job is granted the secrets of the integration branch's `[[ci.jobs]]` entry whose
  name is the job's definition name **and** whose command is the job's command, byte for byte
  (amends SPEC-142 CSE-002). Each matrix value of such an entry shares its command and its grant.
- CSG-002: A job whose name matches a granted entry but whose command differs gets no
  placeholder, no proxy and no CA bundle, and its output says why: `[yulius] agent secrets: none;
  this job's command is not the one the integration branch grants them to (if that branch
  changed it after this run was queued, run it again)`.
- CSG-003: A pull request's own job keeping the trusted name and command still gets the grant;
  other job settings (resources, matrix values, its own `secrets` list) neither grant nor withhold.
- CSG-004: The scanning jobs keep their grant: `keep_security_jobs` puts the trusted job, with the
  trusted command, in the pull request's run.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Spec SPEC-264 and the ADR on granting by key and command
- [x] Regression tests: a PR's own job with a granted key and another command gets no secret, the trusted command keeps its grant
- [x] Grant by key and command, with a job-log note when the command differs
- [x] SPEC-142 CSE-002, architecture and changes docs say so
- [ ] Independent review answered, gates green, merged into dev on green CI

## Acceptance criteria

- `local_ci::ci_jobs_get_the_secrets_the_integration_branch_grants_them`: the pull request's own
  `granted` job with a different command gets `key=none other=none proxy= ca=none` and the
  CSG-002 note; a pull request whose own `granted` job keeps the trusted command still gets
  `key=yulius-secret-API_KEY ... proxy=set`. The first part fails without the fix.
- The unit test of `granted` refuses a same-named entry with another command and grants the
  same name and command.

## Test plan

- Unit (`crates/yulius-app/src/ci/exec.rs`): `granted` by name and command; another command, an
  unknown name, a profile or pull-request gate and no gate grant nothing.
- Integration (`crates/yulius/tests/local_ci.rs`, real ptyhost and proxy wiring): the existing
  CSE test with the pull request's changed command flipped to no secret, plus a pull request that
  keeps the trusted command and changes another job, which keeps its grant.
- Gates: `scripts/dev ci`, `scripts/dev security`.

## Decisions

- Grant by name and command (ADR-0196) rather than replacing the pull request's job with the
  trusted one as `keep_security_jobs` does: a pull request that changes a granted job's command
  must be able to test its new command (that is why its own jobs run); it does so without the
  secret, and the secret follows the command once it is merged. Replacing would hide the change
  from its own test and make every CI-changing pull request run the old command.
- Compare the command text exactly, without normalising whitespace: any difference is a
  different command; normalising would be one more place where two texts could run differently.
- Known limit (unchanged, ADR-0116): the trusted command runs the pull request's checkout, so a
  command calling a script runs the pull request's version of the script. The grant still bounds
  the secret to its bound hosts through the proxy, as for every pull request head since SPEC-142.
- Known limit (new, fails safe; review round 1): a run is planned with the jobs in force when it
  is queued and checked against the integration head when each job starts. A run queued before
  the integration branch changed a granted job's command runs the old command without the secret
  (the `security` scan then ends skipped, unrun), and its output says to run it again in that case. Granting
  the old command instead would let a command the integration branch dropped keep its secret.
