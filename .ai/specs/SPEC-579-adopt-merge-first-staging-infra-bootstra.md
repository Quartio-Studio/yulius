---
yulius-task: 7e391e46-4f3d-4996-a717-5d2ba29db67f
status: done
---

# SPEC-579: Adopt merge-first staging (Infra bootstrap revision 1)

## Goal

Bring the `Quartio-Studio/yulius` repository to Infra bootstrap revision 1 (merge-first staging,
[SPEC-575](SPEC-575-make-task-local-testing-and-one-staging.md),
[ADR-0415](../../docs/decisions/0415-merge-first-is-the-bootstrap-default-and-reaches-onboarded-projects-through-a-task.md))
while preserving every existing project command, CI job, CD target, secret name, and vault grant,
and reconciling any repository guidance that conflicts with the new default.

## Context

Observed repository state at `dev` (`1d97b363c`):

- `yulius.toml` defines four CI jobs (`quality`, `test` with `matrix = "1/2 2/2"`, `e2e`, and
  `security`) and two CD tables:
  - `[cd.dev]`: empty (no `deploy`, `verify`, or `rollback`), with
    `about = "No dev machine yet: once a person adds its credentials to the vault, an agent writes scripts/cd/ and fills this in"`.
  - `[cd.prod]`: `promote = true` (promotion of `dev` to `main`, no `deploy` script; rollback via
    Yulius self-update, [ADR-0264](../../docs/decisions/0264-yulius-own-production-rolls-back-through-its-update-not-a-cd-command.md)).
- There is no `scripts/cd/` directory and no other `[cd.<name>]` table.
- `cd_status` confirms `dev` is empty (`deployed: never`) and `production` promotes `dev` to `main`.
- Therefore, Yulius's own repository has **no shared staging target** (a dev-stage `[cd.<name>]`
  deploying a machine people use before production). SPEC-575 itself notes this: Yulius keeps its
  full CI suite on every pull request and records that posture in `[cd.dev] about`.
- `AGENTS.md` (lines 23-28) and the comment above `[cd.dev]` in `yulius.toml` (lines 79-82) still
  summarize `[cd.dev]` only as "empty: no dev machine yet" and state "a merge deploys nothing"
  without referencing the revision 1 no-shared-target posture or merge-first staging (SPEC-575).
- `docs/product.md`, `docs/architecture.md`, `docs/api.md`, `docs/engineering/git.md`, and the
  bundled `infra-bootstrap` / `delivery-pipeline` skills already reflect SPEC-575 and contain no
  contradicting staging-by-hand or merge-deploys-staging guidance.

In scope:
- Recording the revision 1 no-shared-target posture in `yulius.toml` `[cd.dev] about` and updating
  the adjacent CD comment.
- Reconciling `AGENTS.md` CD summary wording with revision 1 and SPEC-575.
- Updating `docs/changes.md` and verifying repository contracts and `yulius.toml` parsing.

Out of scope:
- Modifying any `[[ci.jobs]]`, `[commands]`, `[cd.prod]`, secrets, or vault grants.
- Inventing a shared staging target, `scripts/cd/`, or a `[local]` deployment table for a project
  that has no shared staging target.
- Promoting `dev` to `main` (always a person's action).

## Requirements

- **IBR-001 Preserve existing CI and CD configuration.** Every existing `[[ci.jobs]]` entry
  (`quality`, `test`, `e2e`, `security`), `[commands]`, `[delivery]`, `[branch_policy]`,
  `[resources.env]`, `[cd.prod]`, and `[disk]` setting in `yulius.toml` remains unchanged; no
  `merge_first` or `after_merge` flag is added.
- **IBR-002 Declare no-shared-target posture in `[cd.dev] about`.** `[cd.dev] about` in
  `yulius.toml` explicitly includes `"no shared target: the full suite gates each pull request"`
  while retaining the note that no dev machine is configured yet.
- **IBR-003 Reconcile repository guidance.** `AGENTS.md` and the CD comment in `yulius.toml`
  consistently reflect that Yulius has no shared staging target (so the full suite gates each pull
  request), that a merge deploys nothing by itself, and that a shared staging target uses
  merge-first staging via `cd_deploy` ([SPEC-575](SPEC-575-make-task-local-testing-and-one-staging.md)).
- **IBR-004 Valid configuration and repository contracts.** `yulius.toml` remains valid under
  Yulius's own CI/CD/local parsers, all repository contract checks (`scripts/dev quick`) pass, and
  existing instruction contract tests in `yulius-app` pass.

## Plan

- [x] Write and validate SPEC-579 for Infra bootstrap revision 1
- [x] Record no-shared-target posture in yulius.toml and reconcile AGENTS.md
- [x] Verify config parsing, repository contracts, and update docs/changes.md
- [x] Complete independent review in a fresh session and address findings

## Acceptance criteria

- **AC-1 (IBR-001, IBR-002):** `git diff origin/dev -- yulius.toml` shows zero changes to
  `[[ci.jobs]]`, `[commands]`, `[delivery]`, `[branch_policy]`, `[resources.env]`, `[cd.prod]`,
  or `[disk]`, and `[cd.dev] about` states `"no shared target: the full suite gates each pull request"`.
- **AC-2 (IBR-003):** `AGENTS.md` accurately states `[cd.dev]` has no shared target (full suite
  gates each pull request) and aligns with SPEC-575 merge-first staging without contradicting
  existing instruction contract rules.
- **AC-3 (IBR-004):** `scripts/dev quick`, `cargo nextest` on the `yulius.toml` / CD / skills
  contract tests, and `spec_check` all pass. Negative check: parsing `yulius.toml` confirms zero
  `merge_first` environments and zero `after_merge` CI jobs.

## Test plan

- Extend `spec_format::config::tests::the_repositorys_own_cd_is_defined` in
  `crates/yulius-domain/src/spec_format/config.rs` to assert that Yulius's `yulius.toml` has
  `[cd.dev] about` stating `"no shared target: the full suite gates each pull request"`, no
  `merge_first` environment, no `after_merge` CI job, and no `[local]` table; verify the test
  fails against `origin/dev:yulius.toml` and passes with the updated file.
- Run `scripts/dev quick` (repository contracts including link checks, delivery instruction rules,
  skill drift, and `cargo fmt --check`).
- Run `PYTHONPATH=scripts python3 -m unittest test_check_repository test_vidoc_scan` (covers
  `AGENTS.md` instruction contracts and `yulius.toml` TOML parsing).
- Run `rustup run 1.99.0 cargo test --lib -p yulius-domain spec_format`,
  `rustup run 1.99.0 cargo test --lib -p yulius-app skills::tests`, and
  `rustup run 1.99.0 cargo clippy -p yulius-domain --all-targets -- -D warnings`.
- Note security scan status: Vidoc is turned off for this project in Yulius (reported as disabled,
  never passed).

## Decisions

- **No shared target path (step 3 of the upgrade prompt):** `Quartio-Studio/yulius` has an empty
  `[cd.dev]` and a promotion-only `[cd.prod]` with no `scripts/cd/` and no staging machine. Per
  SPEC-575, ADR-0415, and skill `infra-bootstrap` step 3, the project keeps its full CI suite on
  every pull request, adds no `merge_first`, `after_merge`, or `[local]` table, and records the
  reason in `[cd.dev] about`. Alternative rejected: fabricating a `[local]` deploy wrapper or
  splitting CI jobs without a shared staging target, which contradicts both the upgrade prompt
  (item 3) and SPEC-575.
- **No new ADR:** This task applies existing [ADR-0415](../../docs/decisions/0415-merge-first-is-the-bootstrap-default-and-reaches-onboarded-projects-through-a-task.md)
  and [ADR-0264](../../docs/decisions/0264-yulius-own-production-rolls-back-through-its-update-not-a-cd-command.md)
  without introducing a new architectural decision.

## Evidence

- `rustup run 1.99.0 cargo test --lib -p yulius-domain the_repositorys_own_cd_is_defined`:
  - Against `origin/dev:yulius.toml`: failed (exit 101, panicked on old `[cd.dev] about`).
  - Against updated `yulius.toml`: 1 passed, 0 failed.
- `rustup run 1.99.0 cargo test --lib -p yulius-domain spec_format`: 60 passed, 0 failed.
- `rustup run 1.99.0 cargo test --lib -p yulius-app skills::tests`: 22 passed, 0 failed (verifies `AGENTS.md` instruction contracts).
- `rustup run 1.99.0 cargo clippy -p yulius-domain --all-targets -- -D warnings`: passed cleanly.
- `scripts/dev quick`: passed (`Repository contracts passed: 16 crates, 7 skills, local Markdown links, no conflict markers, unique decision numbers, delivery instructions, UI design tokens, one product test binary`; `cargo fmt --all -- --check` clean).
- `PYTHONPATH=scripts python3 -m unittest test_check_repository test_vidoc_scan`: 135 tests passed (`OK`).
- `scripts/dev security`: `Security scan disabled: Vidoc is turned off in Yulius, for this project or the whole instance; nothing was scanned.` (disabled, not passed).
- `spec_check`: `docs: valid: true`, `spec: valid: true`.
- Independent review (fresh read-only session, Gemini 4 Neon (High)) of exact head `ee0617985917680d81b3d441e850dd097ac67d65`: verdict **approve**; 1 minor own-code doc finding (add `yulius-app` `skills::tests` to `Evidence`), addressed above (0 product lines changed after approval).
- Full gate (`quality`, `test` shards 1/2 & 2/2, `e2e`, `security` skipped): verified by Yulius CI on the pull request head before merge.
