---
yulius-task: a0010f70-991c-43b6-95e6-f397d7431c84
status: done
---

# SPEC-379: Hygiene tests count GitHub pull-request lookups against MAX_PULL_LOOKUPS (#847)

Status: done
Date: 2026-11-02
Owner: Yulius task #361
Scope authorization: the open-issues automation (task #360). Fix GitHub issue #847 in one pull
request into `dev` (`Fixes #847`). Create no new GitHub issue; comment progress and leftovers on
#847.

## Goal

The repository hygiene scan's promise of at most `MAX_PULL_LOOKUPS` GitHub pull-request lookups
per scan is held by tests, not by code reading alone, and the leftover wording nits of SPEC-269's
last review are gone.

## Context

- **The bound.** `hygiene_remote` (`crates/yulius-app/src/ci/hygiene.rs`) looks up a branch's
  pull requests with `GitHub::branch_pulls`, one request each, at most `MAX_PULL_LOOKUPS` (50) per
  scan. SPEC-269 (task #255, review rounds 4 and 5) split it: unmerged branches first, branches
  already in the integration or production branch get only what is left
  (`merged_budget = MAX_PULL_LOOKUPS.saturating_sub(unmerged)`), and an open pull request's branch
  takes none.
- **The gap.** The test fake's `branch_pulls` (`crates/yulius/tests/support/mod.rs`) neither
  counts nor records calls, so `local_ci::merged_branches_leave_the_lookups_to_unmerged_ones` and
  `local_ci::the_lookup_bound_counts_only_lookups_and_says_what_it_left` check verdicts and notes
  only: a change that looked up every merged branch would still pass both (51 lookups, verdicts
  unchanged).
- **Worktree names.** `task_worktree_path` accepts only `worktrees/<one plain name>`; its
  `name == format!("worktrees/{leaf}")` check rejects the forms `Path::components` normalises
  away (`worktrees/./x`, `worktrees//x`, `worktrees/x/`). The unit test
  `only_a_worktree_directly_under_worktrees_is_removable` lists none of them, so it passes with
  that check removed (core-team comment on #847).
- **Wording.** The card says a `merged` branch's item "…: no merged pull request was found for
  it, so Yulius leaves it to you" (`pages/ci.js`), but the comment above it and the e2e test
  title (`e2e/tests/ci.e2e.ts`) still say "a branch no pull request had". The scan's note about
  merged branches whose lookups the bound skipped, and the `merged` reason itself, say "already in
  the integration branch", though `merged` also means contained only in the production branch.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| HLC-001 | The GitHub test fake records every `branch_pulls` call (the branch looked up). | `crates/yulius/tests/support/mod.rs` |
| HLC-002 | Both bound tests assert that one scan made at most `MAX_PULL_LOOKUPS` `branch_pulls` calls, and that an open pull request's branch took none; each assertion fails with the guard it protects removed. | `crates/yulius/tests/local_ci.rs` |
| HLC-003 | `only_a_worktree_directly_under_worktrees_is_removable` rejects `worktrees/./x`, `worktrees//x` and `worktrees/x/`, and fails with the `name == format!(...)` check removed. | `crates/yulius-app/src/ci/hygiene.rs` |
| HLC-004 | The bound's note and the `merged` reason say "already in the integration or production branch"; a test scans a branch contained only in the production branch. | `hygiene.rs`, `crates/yulius-domain/src/ci/mod.rs`, `local_ci.rs` |
| HLC-005 | The e2e test title and the `ci.js` comment use the card's wording ("no merged pull request was found for it"). | `e2e/tests/ci.e2e.ts`, `pages/ci.js` |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The GitHub fake counts lookups and both bound tests assert the bound
- [x] The worktree-name test rejects the three normalised names
- [x] The merged note and reason name the integration or production branch
- [x] The e2e title and the ci.js comment use the card's wording
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- With `merged_budget` ignored (every merged branch looked up), `merged_branches_leave_the_lookups_to_unmerged_ones`
  fails on the lookup count (51 > 50).
- With an open pull request's branch given a lookup, `the_lookup_bound_counts_only_lookups_and_says_what_it_left`
  fails on the lookup of `feat/one`; with merged branches looked up past the bound, it fails on the count.
- With the `name == format!(...)` check removed, the worktree-name unit test fails.
- The note test fails on the old "already in the integration branch" wording.
- `scripts/dev ci` passes; the `ci.e2e.ts` file passes.

## Test plan

- Mutation checks, one at a time on a committed tree: remove each guard, run the test that
  protects it, see it fail, restore with `git checkout`.
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^local_ci::/)'`, the
  `yulius-app` and `yulius-domain` unit tests, `npx e2e run tests/ci.e2e.ts`.
- `scripts/dev ci`; `yulius security-scan` (Vidoc is switched off for this project: reported as
  disabled).

## Decisions

- Record branch names (`Mutex<Vec<String>>`) rather than a bare counter: the count is its
  length, and a test can also say which branch took a lookup it should not have (an open pull
  request's). Counted per scan as the difference around the scan call, so other reads cannot
  blur it.
- The `merged` reason's wording changes with the note: both describe the same fact
  (`is_ancestor` of the integration or production branch); fixing one would leave the card saying
  the narrower thing. No ADR: test-only hardening and a wording fix, no lasting design choice.

## Evidence

- **Mutation checks** (each on a committed tree, restored with `git checkout`):
  - `merged_budget` ignored (every merged branch looked up): both bound tests fail, "51 lookups,
    bound 50".
  - An open pull request's branch given a lookup (`in_use` forced false): test 2 fails on the
    `feat/one` assertion, and test 1 fails with 51 lookups, `feat/one` among them.
  - `name == format!(...)` removed from `task_worktree_path`: the worktree-name unit test fails.
  - Old note wording, with `release/1` only on `main`: the note test failed, showing "1 branch already
    in the integration branch". It passes with the new wording.
- `cargo nextest` over `local_ci::`, `yulius-domain` and the hygiene tests: 480 passed.
- `npx e2e run tests/ci.e2e.ts`: 19 passed, including the renamed test.
- `scripts/dev ci` at 42ceec7b: exit 0, including nextest (2951 passed, 16 skipped), check, fmt,
  clippy (Linux and macOS), deny, shear and doc.
- `yulius security-scan`: "Security scan disabled". Vidoc is turned off for this project, so this
  is reported as disabled, not passed.
- **Independent review** in a fresh session (om-code-review, read-only).
  - Round 1, REQUEST CHANGES:
    - Test 1 lacked the open-PR assertion (own code): added, mutation-checked.
    - The `classify_remote_branch` comment was half reworded (own code): fixed.
    - Old wording remained in the `repo_cleanup` MCP description and two `hygiene.rs` comments
      (pre-existing, inside this goal): reworded.
    - A `docs/api.md` line was too long (own code): re-wrapped.
  - Round 2: APPROVE at 42ceec7b. Its remaining wrap nit was fixed in a docs-only commit.
