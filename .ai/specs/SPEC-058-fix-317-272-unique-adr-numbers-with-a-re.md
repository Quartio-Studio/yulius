---
yulius-task: 8c9bb5a5-cd5a-4d26-b87c-f8b5a081afb2
status: in-progress
---

# SPEC-058: unique decision numbers with a repository check; onboarding CI-proof wording

Status: in progress
Date: 2026-10-13
Owner: Yulius task #49 (part of task #47)
Scope authorization: user request: "Fix GitHub issues #317 and #272 of Quartio-Studio/yulius,
following each issue's acceptance criteria".

## Goal

Every decision record has its own number, and the repository check stops two records from sharing
one again (#317). The onboarding prompt and the infra-bootstrap skill tell the agent everything it
needs to prove a CI job the way Yulius CI runs it: matrix values, recovering from an interrupted
proof, per-lane checkouts that share the CI home and cache, and system packages installed by
onboarding rather than by a job (#272).

## Context

- #317: four parallel tasks each took the next free number and merged within the hour, so
  `docs/decisions` has four `0034-*.md` records, and "ADR-0034" in the docs and specs names four
  different decisions. Nothing checks the numbers: `scripts/check_repository.py` checks links,
  skills, crates and UI contracts.
- #272: task #34 added the fresh-home proof to `ENVIRONMENT_SETUP`
  (`crates/yulius-app/src/onboarding.rs`) and infra-bootstrap step 2
  (`catalog/skills/yulius/infra-bootstrap/SKILL.md`). Its `env -i` drops `YULIUS_CI_MATRIX`, which
  `ci/exec.rs::ci_env` sets for matrix jobs, and a killed proof leaves `~/.ci-proof.*` and a stale
  worktree entry. Since #238 (CI lanes) a job's checkout is per lane (`<job>@<lane>`,
  `ci/exec.rs::checkout_name`) while up to `CiSettings::lanes` (2) runs of a sandbox share its CI
  `HOME` and cache; the `ci/exec.rs` module doc still says "each job keeps its own persistent
  checkout". Since #299 (#300: an `apt-get` in a job raced the other lane for the dpkg lock)
  system packages belong to the sandbox: CI jobs run inside its holder and see what it installed.
- SPEC-020 ONB-001 states what the prompt says, traced to
  `onboarding.rs::prompts_describe_the_persistent_ci_home_and_the_fresh_home_proof`.

## Requirements

- ADR-001: each decision record has a unique number: the earliest merged of the four 0034 records
  (ready-made automations, #297) keeps 0034; the Agent in the right panel becomes 0036 (a free gap),
  the personal two-factor vault 0040 and the task status from Claude Code's records 0041. Each
  record's H1, the index in `docs/decisions/README.md` (in numeric order) and every reference in
  the docs, specs and code comments follow.
- ADR-002: `scripts/check_repository.py` fails when two files in `docs/decisions` share a number,
  naming them, and when a record's H1 number differs from its file name's.
- ADR-003: the decisions README says the check refuses a duplicate number, so the second task to
  merge takes the next free number after merging `dev`.
- ONB-101: `ENVIRONMENT_SETUP` and infra-bootstrap step 2 say to add `YULIUS_CI_MATRIX=<value>` to
  the proof once per matrix value.
- ONB-102: both say to run the proof with a long timeout or in the background, and how to recover
  from an interrupted one: remove the `~/.ci-proof.*` left by proofs started over six hours ago
  (`find "$HOME" -maxdepth 1 -name '.ci-proof.*' -mmin +360 ...`; the agent home is shared with
  other tasks' running proofs) and run `git worktree prune`.
- ONB-103: both say a job's checkout is per lane while by default up to two runs at once share the
  CI `HOME` and cache, so a job's installs there must tolerate a concurrent run.
  `docs/operations.md` says the same of `ci/jobs/<job>@<lane>`, `home` and `cache`.
- ONB-104: both say system packages (`sudo apt-get`, a browser's system libraries) belong to the
  sandbox and are installed during onboarding, never in a CI job.
- ONB-105: the `ci/exec.rs` module doc describes per-lane checkouts; SPEC-020 ONB-001 states the
  new wording.

## Plan

- [ ] Decision records have unique numbers and every reference follows them
- [ ] The repository check refuses duplicate decision numbers
- [ ] The onboarding prompt covers matrix values, interrupted proofs, shared CI home and system packages
- [ ] The infra-bootstrap skill, the CI module doc and SPEC-020 match the prompt
- [ ] Gates green and an independent review approves the head
- [ ] Merged into dev on green CI and both issues closed

## Acceptance criteria

- `ls docs/decisions` shows no number twice; `git grep 'ADR-0034'` names only the automations
  catalog; every moved record's links resolve (the link check passes).
- `test_check_repository.py` has a test that fails without the duplicate-number check, and
  `scripts/dev ci` passes with it.
- `prompts_describe_the_persistent_ci_home_and_the_fresh_home_proof` asserts ONB-101..104 for both
  onboarding prompts and fails without the new wording.
- SPEC-020 ONB-001 and the infra-bootstrap skill say the same as the prompt.

## Test plan

- `python3 -m unittest scripts/test_check_repository.py` (via `scripts/dev ci`): a temporary
  decisions folder with two `0007-*.md` records and one H1 mismatch is refused; a clean one passes.
  Run before the check exists (must fail), then after.
- `cargo nextest run -p yulius-app -E 'test(prompts_describe_the_persistent_ci_home)'` with the new
  assertions before the prompt changes (must fail), then after.
- `python3 scripts/check_repository.py` on the renumbered tree (must pass), and on the tree before
  the renumbering (must fail with the four 0034 records).
- `scripts/dev ci` for every gate.

## Decisions

- **Which record keeps 0034:** the first merged (#297 at 04:47, ready-made automations), so the
  number keeps meaning what it meant first. The others take free numbers in merge order: 0036 (the
  gap left when SPEC-054 took 0037), then 0040 and 0041 after `dev`'s 0039. The issue's "0035 to
  0037" was written before 0035 and 0037 were taken.
- **Historical notes** in SPEC-035, SPEC-040 and SPEC-052 that tell how a number was chosen at the
  time stay as written; the ones that name the record get "now 00NN (#317)" so they are not
  ambiguous.
- **A check rather than a numbering service:** a duplicate cannot be prevented across parallel
  branches without coordination, but every task merges `dev` and runs the repository check before
  its merge, so the second one fails and renumbers. The H1 check catches a renumber that forgot its
  title. No ADR: the check is a repository contract, documented in the decisions README.
- **#272 wording only:** no CI behavior changes; the prompt grows by under 1 KB, far below
  `MAX_PROMPT_BYTES`.
