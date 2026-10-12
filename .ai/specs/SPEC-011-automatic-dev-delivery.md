---
yulius-task: 7f2dac7e-b5cd-454e-b06b-370a2a983dae
status: in-progress
---

# SPEC-011: merging into dev is automatic and test-driven; no person confirms dev

Status: in progress
Date: 2026-10-12
Owner: Yulius task #10
Scope authorization: user request ("Dev deployment/merge should be automatic, test-driven, and not wait for an explicit human approval")

## Goal

In the `standard` delivery profile a pull request that has its green checks on the exact head is
merged into `dev` by its agent, and nothing then waits for a person: the passing tests are the
verification of `dev`. Finished features are on `dev` and their branches and worktrees are gone.

## Context

`DeliveryProfile::standard()` carries an environment `dev` (after integration) with no command and
`verify = manual`. After the merge the task stays at `merged_integration`; `delivery_deploy`
refuses the environment ("verified by a person") and `delivery_promote` is not offered until a
person confirms it in the UI. Every agent therefore ended its turn waiting, although `AGENTS.md`
and the briefing say delivery needs no human acceptance step. Stored profiles of existing sandboxes
hold the same manual `dev` checkpoint.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| DLV-009 | The `standard` preset has no environment: a merge into `dev` on green checks (DLV-005) is the whole integration stage, and the next action after the merge is promotion. Its test gate (`require_checks`) stays on. | `delivery::standard_*`, `tests/delivery.rs` |
| DLV-010 | A stored `standard` profile that still holds only the former manual `dev` checkpoint (no command) reads as the new `standard` profile, so existing sandboxes need no edit. A profile with any other environment is untouched; `product` keeps its manual checkpoints. | `delivery::legacy_standard_*` |

## Design

- `DeliveryProfile::standard()` returns an empty `environments`.
- `yulius.toml` parsing and `profile_from_contract` upgrade a `standard` profile whose environments are exactly the manual
  `dev`/integration/no-command checkpoint to `DeliveryProfile::standard()` before validation.
- Docs (`docs/engineering/git.md`, SPEC-001 DLV-002, the changes log) say the
  merge on green is the dev delivery.

## Acceptance criteria

1. With the standard profile, after the merge `next_actions` is `Promote`, not `ConfirmEnvironment`.
2. A stored legacy standard profile is read as the new one; a custom `dev` command environment stays.
3. `delivery_merge` then `delivery_promote` completes with no person in the loop.

## Test plan

- Domain: standard has no environments and goes `merged_integration` → promote; legacy upgrade
  cases (legacy, with command, product untouched).
- App/integration: existing delivery tests that confirmed the `dev` checkpoint now promote at once.
- Not automated: GitHub rulesets of real repositories.

## Plan

- [x] Standard profile has no manual dev checkpoint and legacy profiles read as it
- [x] Docs and changes log say the merge on green is the dev delivery; every gate green
- [ ] Finished tasks are on dev and their branches and worktrees are cleared

## Decisions

- Remove the checkpoint instead of auto-confirming it: a confirmation nobody can refuse verifies
  nothing; the green gate on the exact head and the post-merge CI run are the verification.
- Promotion to `main` keeps its own gate (merge-commit PR with green checks); only the dev wait goes.

## Evidence

- `cargo test -p yulius-domain -p yulius-app --lib`: 141 + 215 passed (new `legacy_standard_checkpoint_reads_as_the_new_standard`; standard flow goes merge → promote).
- `cargo test -p yulius --test delivery`: 15 passed (the dev checkpoint steps removed; promotion follows the merge).
- `scripts/dev ci` (build dir outside the worktree after a disk prune removed `target` mid-run): exit 0.
- Not run: a real GitHub repository's rulesets.
