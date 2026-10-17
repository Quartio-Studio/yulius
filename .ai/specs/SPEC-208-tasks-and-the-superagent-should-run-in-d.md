---
yulius-task: 57829d06-aedf-45d3-8017-1930f1d0ce8f
status: in-progress
---

# SPEC-208: Tasks and the Superagent run with Claude Code's approval bypass, not auto mode

Status: in-progress
Date: 2026-10-17
Owner: task #195
Scope authorization: task #195 (requested by MrFishPL): "tasks and the superagent should run in
dangerously skip permissions mode, not the automode", with a screenshot of the Superagent panel
showing Claude Code's "Teach auto mode about your environment?" dialog and the footer
"⏵⏵ auto mode on (shift+tab to cycle)". Amends SPEC-021 CKP-002 ("no
`--dangerously-skip-permissions`") and decision 0020.

## Goal

On a contained server every Claude Code session Yulius starts, task runs and the Superagent alike,
runs in bypass-permissions mode (`--dangerously-skip-permissions`): no auto-mode classifier
blocking commands, no auto-mode onboarding dialog, no approval prompts for ordinary tool calls.

## Context

- **Tasks already do.** `plan_launch` (`crates/yulius-harness/src/launch.rs`) adds
  `--dangerously-skip-permissions` to every run of a contained server, and the supervisor sets
  `IS_SANDBOX=1` for a run as root of an isolated sandbox (SPEC-002 ADR-22); the adapter records
  `bypassPermissionsModeAccepted` in the sandbox's `.claude.json`. Measured on this instance on
  2026-10-17: the 13,614 `permissionMode` records in the task transcripts of this project are all
  `bypassPermissions` (one `default`, from a run before containment). Covered by
  `crates/yulius-harness/tests/launch.rs` (the contained argv).
- **The Superagent does not.** `plan_cockpit` deliberately starts "plain" Claude Code (SPEC-021
  CKP-002, decision 0020: "no `--dangerously-skip-permissions` … the user switches modes with
  Shift+Tab"), and the adapter settles the user's `.claude.json` with `bypass_permissions = false`.
  Claude Code 2.1.292 then starts in its default mode for the account, which is now auto mode: the
  screenshot shows "auto mode on" and the auto-mode onboarding dialog in the Superagent panel, and
  auto mode's safety check blocks commands the person asked for (the kind of block SPEC-157 met).
- A contained Superagent runs inside its sandbox's bubblewrap as uid 0 (`user_space`: `Identity
  { uid: 0 }`, the person's login mapped to root). Claude Code refuses
  `--dangerously-skip-permissions` as root unless `IS_SANDBOX=1` is set; nothing sets it for the
  Superagent today.
- A running Superagent restarts by itself, once its conversation is idle, when its launch digest
  changes (SPEC-186 SAV-006, `COCKPIT_LAUNCH_REVISION`); its conversation continues.
- An uncontained server (development, `YULIUS_CONTAINED` unset) runs sessions as the service user
  on the host. ADR-22 keeps the bypass for contained runs only; this task does not change that.

## Requirements

| ID | Requirement |
| --- | --- |
| BYP-001 | On a contained server the Superagent starts with `--dangerously-skip-permissions` and `IS_SANDBOX=1` (it runs as root of its sandbox), so Claude Code starts in bypass-permissions mode, not auto mode. |
| BYP-002 | The user's Claude config for that Superagent records `bypassPermissionsModeAccepted: true`, so Claude Code shows no bypass warning to accept. |
| BYP-003 | A Superagent already running when Yulius is updated restarts into the new launch once its conversation is idle (SPEC-186 SAV-006); its conversation continues. |
| BYP-004 | On an uncontained server neither the Superagent nor a task run gets the bypass (unchanged, ADR-22): the session runs on the host as the service user. |
| BYP-005 | Task runs keep `--dangerously-skip-permissions` on a contained server (unchanged; regression-covered). |

## Plan

- [ ] Spec and decision record: Superagent bypasses approvals on contained servers
- [ ] Superagent launch plan carries --dangerously-skip-permissions and IS_SANDBOX when contained
- [ ] Contained Superagents start with bypass accepted, and running ones restart into it
- [ ] Docs (architecture, API, ADR index, changes) describe the bypass
- [ ] Gates green, independent review answered, merged into dev

## Acceptance criteria

- AC-BYP-1: `plan_cockpit` with `bypass_permissions: true` puts `--dangerously-skip-permissions`
  in argv and `IS_SANDBOX=1` in the environment; with `false`, neither (harness test).
- AC-BYP-2: A contained server's Superagent start asks the harness for a bypassing launch; an
  uncontained one does not (integration test with the recording harness and the live fixture).
- AC-BYP-3: The adapter's settled `.claude.json` has `bypassPermissionsModeAccepted: true` for a
  bypassing Superagent and none otherwise.
- AC-BYP-4: `COCKPIT_LAUNCH_REVISION` is bumped, so Superagents started by the previous Yulius
  are due to restart (the digest test of SPEC-186 covers the mechanism).
- AC-BYP-5: The contained task argv still has `--dangerously-skip-permissions` (existing test).

## Test plan

- Harness (`crates/yulius-harness/tests/launch.rs`): the Superagent plan with and without
  `bypass_permissions` (argv flag, `IS_SANDBOX`, nothing else changes).
- Adapter (`crates/yulius/tests/cockpit.rs`): the uncontained live fixture keeps the plain launch
  and no bypass acceptance in `.claude.json`; the contained recording test sees
  `contained: true` in the requested launch; an adapter-level test plans a contained Superagent
  and reads `bypassPermissionsModeAccepted: true` from the settled config.
- Existing: the contained task argv test (BYP-005), the SPEC-186 refresh tests (BYP-003).

## Decisions

- **Bypass for the Superagent on contained servers** (decision 0165, amends 0020 and SPEC-021
  CKP-002). The person asked for it; auto mode's classifier blocked commands they had asked for
  and its dialog interrupts the panel. Containment, not approval prompts, is Yulius's boundary
  (ADR-22): the Superagent runs in the same bubblewrap as its sandbox's tasks. The person can still
  switch modes with Shift+Tab for the rest of a session.
- **Same rule as tasks: contained only.** On an uncontained server a bypassing session would act
  on the host as the service user with no boundary; the flag follows `config.contained`, as a
  task's does.
- **`IS_SANDBOX=1` whenever it bypasses.** The contained Superagent is always uid 0 inside its
  bubblewrap, also on a legacy (non-isolated) sandbox, so the variable goes with the flag in the
  harness plan rather than with the isolation join as for tasks.
- **The read-only `--allowedTools` list stays.** It is redundant under bypass but keeps a session
  the person switched to default mode from asking about read-only Yulius tools.
- **Restart running Superagents** by bumping `COCKPIT_LAUNCH_REVISION` (SPEC-186 SAV-006) rather
  than waiting for the next idle stop: the change is what the person asked to see.
- **Tasks: no code change.** They already bypass on a contained server; the request's "tasks"
  part is verified (Context) and kept by the existing regression test.
