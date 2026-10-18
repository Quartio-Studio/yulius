---
yulius-task: 5df93cb6-8c5d-4ff1-9fae-71e0922f6476
status: in-progress
---

# SPEC-219: delivery environment runs run contained, from a mirror only Yulius writes

Status: in-progress
Date: 2026-10-18
Owner: Yulius task #205
Scope authorization: issue [#656](https://github.com/Quartio-Studio/yulius/issues/656) (high, found
by the Vidoc audit of task #160, SPEC-217; it is #54 part B again for environment runs): "Environment
runs work like CD dev runs (SPEC-134): they check out from the Yulius-only mirror into a run
directory; they run under the sandbox's containment, with a fresh HOME; repository-local git config
never runs on the host." The task adds: "make sure the installation token is never sent through
repository-local config (`url.insteadOf`)" and "regression test first".

## Goal

An agent cannot run code on the host through a delivery environment run. What an environment run
executes, and the Git that prepares it, never read anything an agent can write, and the command runs
inside the sandbox's containment like a CD dev run.

## Context

- Symptom (reproduced by the regression test on `dev` at 34944a6d): an agent writes
  `[filter "x"] smudge = …` into the sandbox clone's `.git/config` and `* filter=x` into
  `.git/info/attributes`, then calls `delivery_deploy` for a command-verified integration
  environment. `dlv_environment_steps` (`crates/yulius-app/src/delivery.rs`) runs
  `git checkout --force` in that clone on the host: hooks, fsmonitor and global/system config are
  neutralised, the repository-local filter is not, so its smudge command runs as the service user.
  It then runs `/bin/sh -c <command>` on the host in the clone, with `HOME` the sandbox's home.
- Root cause: environment runs predate SPEC-134. CD runs already take their commit into a bare
  mirror only Yulius writes (`<data>/cd/<sandbox>/source.git`), export its tree into a private run
  directory (`export_tree`: `read-tree` + `checkout-index` in the mirror) and run through the process
  host under `cd_containment`. Environment runs never moved to that path, and their tests ran a
  test-local Git with no planted configuration, so nothing noticed.
- Token: `dlv_refresh_clone` → `sync_clone` fetches in the agent-writable clone with the
  installation token in an `http.https://github.com/.extraheader`. A repository-local
  `url.insteadOf` cannot carry that header off GitHub (the header is matched against the rewritten
  URL), but other local settings (`http.proxy` with `http.sslVerify=false`, `http.curloptResolve`,
  `include.path`) can. The other callers of `dlv_refresh_clone` are #54 part B (ADR-0114,
  "network Git from a mirror only Yulius writes"), outside this change; environment runs stop using
  it.
- Agents reach environment runs only through `delivery_deploy`, for integration environments with a
  `command` and `verify = "command"`; production environments are a person's.

## Requirements

- ENV-001: an environment run takes the head of its stage's branch from the repository's own origin
  (GitHub with the installation token, a playground's origin by its path) into a bare mirror only
  Yulius writes, `<data>/cd/<sandbox>/environments.git`, with a depth-1 fetch that checks every
  object and ignores replacement refs. The token goes only through that command's environment; no
  Git of the run reads the sandbox clone.
- ENV-002: the commit's tree is written into the run's own directory
  (`<data>/runs/env-<run>/work/src`) from the mirror; the run's `HOME` is a fresh
  `work/home` there. The directory is removed when the run ends.
- ENV-003: the command runs through the process host with CD's containment for its stage: an
  integration environment as a CD dev run (the sandbox's containment), a production environment
  as a CD production run. It sees no host path outside them.
- ENV-004: the run keeps its contract: audited before it starts and at its end, one at a time per
  project, output bounded and streamed into the run's record, the commit recorded, the CI job
  timeout (as CD runs), and a run left by a restart ends `error` with its process host terminated.

## Plan

- [x] Spec SPEC-219 and its plan
- [x] Regression test: a planted smudge filter and `delivery_deploy` leave no host marker
- [x] Git takes a branch head from origin into a Yulius-only mirror with the token in the command's environment only
- [x] Environment runs export the mirror's commit into a run directory and run through the process host under containment
- [x] Contained test: the environment command cannot read a host-only path
- [x] The host runner is gone and a lost run's process host is terminated
- [x] ADR-0175, architecture, product and changes updated
- [ ] Gates green, independent review answered, delivered

## Acceptance criteria

- AC-1 (issue AC 1): `crates/yulius/tests/delivery.rs` plants a filter/attributes pair whose smudge
  touches a host marker, calls `delivery_deploy`, and the run passes with no marker.
- AC-2 (issue AC 2): with containment on (bubblewrap), the environment command cannot see a file
  outside the sandbox and the run's directory, and its `HOME` is the run's own.
- AC-3 (issue AC 3): an environment run never fetches with the token in the sandbox clone: a
  repository-local `url.insteadOf` and `http.proxy` planted in the clone change nothing, and the
  commit comes from origin even when the clone is stale.
- AC-4: the existing environment flows (product profile deploy, agents' `delivery_deploy`) pass
  unchanged through the process host.

## Test plan

- `delivery::an_agent_planted_git_filter_never_runs_on_the_host` (AC-1, AC-3): planted filter,
  attributes, `url.insteadOf` and `http.proxy` in the sandbox clone; the clone is not fetched; the
  run passes at origin's head, prints the file's content, and the marker does not exist.
- `delivery::contained_environment_runs_see_only_their_sandbox` (AC-2, Linux, skipped when bubblewrap
  cannot run): a host-only file outside the sandbox is not there; `HOME` is under the run directory.
- `delivery::a_contained_environment_run_starts_in_the_sandbox_containment` (AC-2 everywhere): a
  recording process host sees a bubblewrap containment rooted at the sandbox, only the run's
  directory writable, a fresh `HOME` there, the exported tree as `cwd`, no secrets.
- `delivery::an_environment_command_past_its_timeout_is_killed` and
  `delivery::a_restart_ends_a_running_environment_command` (ENV-004) through the real process host.
- `yulius-git` unit tests of the mirror fetch: depth 1, replacement refs ignored, a missing branch
  refused.
- Existing: `product_flow_deploys_on_confirmation_then_fast_forwards_and_rolls_out`,
  `agents_merge_only_through_gated_tools` (AC-4); CD and CI containment tests unchanged.

## Decisions

- **Fetch from origin into a separate shallow mirror** rather than CD's `cd_mirror_fetch` from the
  sandbox clone: that needs the clone fetched first, which is the token-in-agent-config fetch this
  issue names. A depth-1 fetch needs one tree, and a separate `environments.git` keeps CD's mirror
  (whose promotion checks walk history) unshallow. Recorded in ADR-0175.
- **Reuse CD's containment, process host and wrapper** instead of wrapping `run_bounded` in a
  hand-built `bwrap` argv: joining a sandbox's holder is the process host's job, and two
  implementations of containment would drift. The run uses the CI job timeout like CD runs
  (30 minutes by default, as before).
- **Stages are not recorded** for environment runs (no span subject exists for them); `ci_watch`
  takes them optionally.
- **Scope:** the other token fetches in the sandbox clone (`dlv_refresh_clone` for CI, merges,
  hygiene, CD) stay #54 part B; this change does not widen to them (one concern per pull request).

## Evidence

- Red first (commit 81fd4904, on `dev` 34944a6d's code): `an_agent_planted_git_filter_never_runs_on_the_host`
  failed with "the planted smudge filter ran on the host"; `an_environment_run_never_fetches_in_the_sandbox_clone`
  failed with state `error` ("reading quartio failed": the planted `url.insteadOf` redirected the
  clone's fetch). Both pass after the fix.
- `a_restart_ends_a_running_environment_command` fails without `environment_end_lost` in the
  startup reconciliation ("the run's directory is removed") and passes with it.
- `contained_environment_runs_see_only_their_sandbox` printed "SKIPPED … bubblewrap cannot run
  here" in this task's sandbox; the recording-host test covers the containment plan there.
- `scripts/dev ci` at e990fbb1: exit 0, 2217 tests passed, 18 skipped (lint, macOS clippy, docs,
  deps included).
- `scripts/dev security` at e990fbb1: exit 1 on one high finding at
  `crates/yulius-app/src/ci/exec.rs:914`, "OS command execution from pull request-defined CI jobs":
  pre-existing (the job launch, not this change's `ci_watch` edit), and by design (CI runs the pull
  request's code in the sandbox's containment without secrets, SPEC-142, ADR-0116). Recorded as an
  `[[ignore]]` in `.config/vidoc.toml`, subject to the independent reviewer's agreement. Four medium
  or low findings outside the change (cd.rs:1394, ci/mod.rs:723, wiring/git.rs:79, cd.rs:1071) do not
  fail it; Yulius CI files them. No low finding was reported in the change.
