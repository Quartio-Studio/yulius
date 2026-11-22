---
yulius-task: fdcbd029-1d40-4d9d-9248-e5e26d3a7109
status: in-progress
---

# SPEC-621 Finish SPEC-558 on Claude Code: parked Codex tasks

## Goal

Deliver the last four steps of task #540 (SPEC-558) to dev, so that its original "Done when" holds: a parked task's page never shows Codex's exit text, resume continues the same session, unfinished Codex goal work is continued instead of parked, and Done stays true.

## Context

Task #540 ran on Codex and was stopped on 22 November 2026 to move its work to Claude Code. SPEC-558 stays the product spec; this spec tracks the takeover. On 22 November dev at `2a6e07423` already holds:

- Step 48, test shards unpacked on disk instead of the 1 GiB tmpfs: PR #1679 (`8855f295b`). It is not on main (`aabf37091` promotes `b03f426dc`), so main's failed CI run `21d8a791-8380-4290-899a-b4243928c5fa` needs the next promotion, not a rerun.
- Step 49, live owners hold their session slot while their PR decision needs verification: PR #1683 (`92d4e5a0b`). Its reviewed commit `34f395357` reverse-applies cleanly on dev, and the branch's spec and documents equal dev's.

Open on dev: steps 50 to 53 of SPEC-558. #540 left prepared patches in its handover folder: `step42-ported-current-legacy.patch` (step 53), `step41-ported-current-goal.patch` (step 52), the dropped commit `d04bfe295` on its branch (step 50) and `candidate-server-d33.patch` with `step40-mounted-candidate-status.patch` (step 51). They are reviewed here as code, re-tested on current dev and delivered one pull request each.

## Requirements

- FIN-001 (SPEC-558 step 53) A task parked before SPEC-558 has only its run's final screen. Its page and `task_screen` keep the real answer, drop only a trailing Codex exit block (token usage, `codex resume <id>`, the selection hint), and add one parking notice with the idle time when the recorded transition gives it. A genuine `codex resume` line inside an answer stays.
- FIN-002 (step 52) A Codex goal-mode turn that ends with open steps or failing PR checks receives the continue prompt through the Stop hook within the existing retry bound, and is flagged for its person when the bound runs out.
- FIN-003 (step 50) A live owner whose achieved goal has an open delivery releases a dependent task, satisfies a start rule or is closed (archive, removed subscription) only while its delivery is verified settled, holding the delivery read guard through the release or close.
- FIN-004 (step 51) A live owner with a failed integration candidate still pending verification is shown as waiting on checks or merge, not Done, in the terminal, the sidebar and Task Info.

## Plan

- [x] Write the takeover spec and confirm steps 48 and 49 on dev
- [ ] Recover screens from tasks parked before the upgrade
- [ ] Review and deliver the pre-upgrade screen recovery
- [ ] Port bounded Codex goal continuation onto current dev
- [ ] Prove the #325 shape and the retry bound for Codex goals
- [ ] Review and deliver bounded Codex goal continuation
- [ ] Reproduce a dependent task released by an unverified live owner
- [ ] Hold the live owner's delivery through chain release, start rules and close
- [ ] Review and deliver live-owner verification for dependents and closes
- [ ] Reproduce a live owner shown Done while its integration candidate is pending
- [ ] Render verified live integration status in the terminal, sidebar and Task Info
- [ ] Review and deliver the live integration status
- [ ] Close SPEC-558 with evidence and report the main CI rerun

## Acceptance criteria

- AC-1 A parked Codex run without a parked snapshot whose final screen ends with token usage and resume instructions shows the answer and one parking notice with its idle time in `task_screen`.
- AC-2 The #325 shape (5 of 7 steps, red PR) on a Codex goal Stop produces a continuation; the bound flags the task.
- AC-3 A start rule or chain naming a live owner whose delivery cannot be verified does not release the dependent task; close and archive of such an owner are refused until verified.
- AC-4 A live owner with `candidate_pending` reads "Waiting for checks or merge" in all three views and returns to Done once the candidate is resolved.

## Test plan

Each slice starts with a regression that fails on dev: the cockpit `task_screen` fixture for FIN-001, the agent-hook Codex goal fixtures for FIN-002, the parking candidate fixtures for FIN-003 and FIN-004, and Chromium e2e for FIN-004's views. Run each touched test binary's scope, `scripts/dev quick`, Clippy on touched crates, and Yulius CI on every head.

## Decisions

- Keep SPEC-558 as the product spec and tick its Plan as each slice merges; this spec only records the takeover, so the two plans name the same work.
- Deliver step 53 first: its patch applies cleanly and is independent of the capacity work. Then step 52, step 50 and step 51, each its own pull request with a fresh review, merging dev before each review because tasks #596 and #597 change `supervisor.rs` and `pr_watch.rs` in parallel.
- Old parks are recovered from the run's final screen instead of being rewritten on disk: the read path is the only place that knows whether the run was parked (the recorded transition), and leaving the files alone keeps the change reversible.

## Evidence

- `git apply --check -R` of `34f395357` (crates only) succeeds on `2a6e07423`; `git merge-base --is-ancestor 8855f295b origin/main` fails.
