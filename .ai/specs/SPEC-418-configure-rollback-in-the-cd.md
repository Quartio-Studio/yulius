---
yulius-task: 9c9c5334-b957-480f-9e3a-6ef66d8ae613
status: in-progress
---

# SPEC-418: Rollback in Yulius's own CD, and environments that deploy nothing need none

Status: in-progress
Date: 2026-11-04
Owner: Yulius task #400
Scope authorization: task #400, asked by the project's owner right after promoting #1055 to
production: "configure rollback in the CD". Decision record:
[ADR-0264](../../docs/decisions/0264-yulius-own-production-rolls-back-through-its-update-not-a-cd-command.md).

## Goal

Yulius's own CD says truthfully how Yulius rolls back, and no environment that deploys nothing is
shown as missing a rollback.

## Context

- SPEC-415 (ADR-0263) added `rollback`/`migrations` to `[cd.<name>]`, the releases, the dev drill
  and the production gate. Only a production table with a `deploy` command is gated (RBK-032).
- Yulius's `yulius.toml`: `[cd.dev]` is empty ("No dev machine yet"); `[cd.prod]` has
  `promote = true` and no `deploy`: promoting `dev` into `main` is the release, and each instance
  installs `main` when a superadmin chooses Update now.
- After promoting #1055, `cd_status` showed for both environments "rollback: not configured (add
  `rollback` and `migrations` to its table; production deploys need a proven rollback)", and the
  CD page a "Rollback not configured" warning with "Add rollback … to [cd.prod]".
- The update helper (`crates/yulius/src/update_helper.rs`, ADR-0013) backs up the binary and the
  database and restores them when a new build fails to start (its health check); it refuses to
  install an older commit; the store refuses a database newer than its build.

## Requirements

- RCF-001. An environment needs no rollback when its table was read, has no `rollback` and no
  `deploy` command, and no production-stage environment with a deploy command drills its rollback
  on it (that production waits for the drill). The server marks it once per CD overview with the
  reason (`not_needed` of its rollback): for a production that promotes, "promoting is the release:
  nothing deployed here can be redeployed; undo a release by reverting it on <integration branch>
  and promoting again"; otherwise "it runs no deploy command; add rollback and migrations with
  one". `cd_status` prints `rollback: not needed (<reason>)`.
- RCF-002. The CD page shows such an environment's rollback with a neutral "Not needed" badge and
  the same reason, never the "Rollback not configured" warning nor the "Add rollback" hint. An
  environment without a deploy command that a deploying production drills on shows "Rollback not
  configured" and says production deploys wait for a rollback drill there.
- RCF-003. An environment with a `deploy` command and no `rollback` keeps "not configured" and,
  in production, its gate (RBK-031) unchanged; a table with `rollback` is shown as before; a table
  that could not be read is never said to need no rollback. A rollback (`cd_rollback`, the CD API)
  of an environment without `rollback` and `deploy` is refused with "nothing is deployed to <env>
  that a rollback could redeploy".
- RCF-004. Yulius's `[cd.prod]` `about` says how Yulius rolls back (the update restores itself
  when the new build fails to start; otherwise revert on dev and promote); `docs/product.md` and
  `docs/architecture.md` say the same.

## Plan

- [x] Spec and ADR: how Yulius's own CD rolls back
- [x] Environments that deploy nothing say rollback is not needed (cd_status), tests first
- [x] CD page shows rollback not needed for them, with e2e
- [x] yulius.toml and docs say how Yulius's production rolls back
- [ ] Independent review answered
- [ ] Gates green and evidence recorded

## Acceptance criteria

- AC1. `cd_status` for a project whose `[cd.dev]` is empty and `[cd.prod]` only promotes prints
  "rollback: not needed" for both, with the reasons of RCF-001, and never "production deploys need
  a proven rollback".
- AC2. `cd_status` for an environment with a `deploy` and no `rollback` still prints "rollback: not
  configured".
- AC3. On the CD page (mock), a promotion-only production and an empty dev show "Not needed" and
  their reason, no "Rollback not configured" badge; a deploying environment without `rollback`
  still shows it, and so does an empty dev that a deploying production drills on.
- AC4. Yulius's `yulius.toml` parses, and its `[cd.prod]` `about` names both ways Yulius rolls
  back.

## Test plan

- Unit tests of `mark_rollback_not_needed` and `rollback_text`
  (`crates/yulius-app/src/ci/rollback.rs`): not needed for an empty environment and for a promotion-only production; needed for a deploying
  one, for an empty dev a deploying production drills on, and for an unreadable table.
- Integration (`crates/yulius/tests/cd.rs`): `cd_status` and the overview over a project with an
  empty dev and a promotion-only production say "not needed"; `cd_rollback` of the dev is refused
  with "nothing is deployed".
- e2e (`e2e/tests/cd.e2e.ts`): the mock's promotion-only production and empty dev.
- The repository's own `yulius.toml` is parsed by the existing config test of the repository.

## Decisions

- No `rollback` command for Yulius's own CD; see ADR-0264 for the rejected ones (a tree commit
  pushed to `main`, a force push, a helper downgrade with a database restore, a placeholder).
- "Not needed" is derived from the tables (no `deploy`, no `rollback`, not drilled on by a
  deploying production), not from a new key: nothing to configure, and every promotion-only project
  gets it. The server computes the reason (`CdRollback.not_needed`) so that `cd_status` and the page
  cannot drift (review L2).
- Review round 1 (request changes): M1 an empty dev drilled on by a deploying production was "Not
  needed" while production waited for its drill: fixed, needed then, page says why. L1 an
  unreadable table was "Not needed": fixed. L2 wording drift between spec, Rust and page: one
  server-made reason naming the integration branch. L3 `cd_rollback` on such an environment said
  "not configured": refused with "nothing is deployed". L4 "fails its check" was ambiguous (a
  failed host check is reported, not rolled back): "fails to start (its health check)". N1 the test
  helper had taken another test's doc comment: moved. N2 (the product mock's dev is `not_defined`):
  no change, `seedCd` makes it `empty`, and the M1 case sets `empty` again. N3 long lines and
  `docs/api.md`: fixed.
- Review round 2 (approve, three non-blocking findings, all fixed): a production whose table could
  not be read counts as drilling on its drill environment (it may deploy); `cd_status` tells an
  agent that a drilled environment without a deploy command needs one too; the mock refuses a
  rollback of an environment that deploys nothing as the server does.

## Evidence

To be recorded when the gates have run.
