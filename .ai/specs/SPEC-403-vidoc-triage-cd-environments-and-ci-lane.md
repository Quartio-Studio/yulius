---
yulius-task: 103e3870-8384-4b50-9f9e-08314022b36f
status: in-progress
---

# SPEC-403: Vidoc triage of the CD environment and CI lane findings (#784 #718 #716 #715 #738)

Status: in-progress
Date: 2026-11-03
Owner: Yulius task #384
Scope authorization: task #384 (a person asked that each of the five issues end closed). All five
are false positives on `dev` at ffbc2e91. They are dismissed with file:line evidence, a narrow
`[[ignore]]` entry per wording, and tests that pin the two guards nothing pinned yet.

## Goal

Each of the five Vidoc findings about CD environments, CD run environments and CI lanes is
checked against `dev` (ffbc2e91). Each one ends closed. A repeat scan does not reopen it.

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
- VCE-004 (#715, false positive). `contained = false` is the documented development mode
  (docs/operations.md lines 13, 18 and 335; SPEC-002 NFR-007). In that mode every agent run and
  shell already runs on the host as the service user, which can read the data directory and
  `master.key`. So an uncontained production run gets no capability that the agents lack.
  Runs are marked "not contained". With containment, every CD run is contained
  (`cd_containment`, `ci/cd.rs:1331-1380`).
- VCE-005 (#738, false positive). There is no window between the two locks. The first lock
  (`ci/mod.rs:1246-1250`) only skips the disk-room query when no lane is free. The reservation
  and the `executing` registration happen under one lock scope (`ci/mod.rs:1256-1267`).
  `take_lane` checks again there, and on `None` the loop `continue`s before anything is
  registered. Only one dispatcher exists (`start_ci`'s `swap`, `ci/mod.rs:1151-1159`).
  `ci_claim` and the lanes share the same `busy` mutex. The test
  `a_sandbox_runs_as_many_runs_as_it_has_lanes_and_never_while_claimed` pins that a claim holds
  every lane.

## Plan

- [ ] Triage the five findings against dev with file:line evidence
- [ ] Pin the agent deploy's environment name check (#784) with a test
- [ ] Accept the five false positives in .config/vidoc.toml
- [ ] Independent review and gates green
- [ ] Comment and close the five issues as not planned

## Acceptance criteria

- `cd::named_environments_deploy_by_their_stage` refuses the agent tool's `cd_deploy` of
  malformed names with the unknown-environment message. It fails when `tool_cd_deploy` skips
  `dom::env_name` (mutation-checked).
- `host_setup::tests` asserts that the written unit pins `PATH=/usr/local/bin:/usr/bin:/bin`.
- `.config/vidoc.toml` has one narrow entry per dismissed wording, each with its reason.
- Each of the five issues ends closed as not planned, with a comment giving the evidence. #718
  is labelled `duplicate` of #780.

## Test plan

- Additions to `crates/yulius/tests/cd.rs` and `crates/yulius/src/host_setup/tests.rs`.
- Mutation check of the #784 test, done by removing the validation in `tool_cd_deploy`.
- `scripts/dev ci` (the full gate). `yulius security-scan` reports the scan as disabled for this
  project (Vidoc is off in Yulius), never as passed.

## Decisions

- No code change for any finding. Each guard Vidoc asks for already exists. Moving
  `dom::env_name` into `cd_start` (#784) would only duplicate a check that both entry points
  already make. A hard-coded PATH (#716) would ignore an operator's deliberate choice, and the
  installer already writes the minimal PATH. Refusing `contained = false` for production (#715)
  would break the documented development mode, where it protects nothing.
- Each dismissal gets an `[[ignore]]` entry, so that Yulius does not reopen the closed issue when
  Vidoc is switched on again and repeats the finding.
- Two related open findings on the inherited PATH, #755 (`supervisor.rs`) and #806
  (`accounts.rs`), belong to other triage groups. They are left to those groups and mentioned in
  the pull request.
