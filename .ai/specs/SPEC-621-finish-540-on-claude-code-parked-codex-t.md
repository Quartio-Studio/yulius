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
- [x] Recover screens from tasks parked before the upgrade
- [x] Review and deliver the pre-upgrade screen recovery
- [x] Port bounded Codex goal continuation onto current dev
- [x] Prove the #325 shape and the retry bound for Codex goals
- [x] Review and deliver bounded Codex goal continuation
- [x] Close the round-2 notes on the Codex goal continuation
- [x] Reproduce a dependent task released by an unverified live owner
- [x] Hold the live owner's delivery through chain release, start rules and close
- [x] Review and deliver live-owner verification for dependents and closes
- [x] Reproduce a live owner shown Done while its integration candidate is pending
- [x] Render verified live integration status in the terminal, sidebar and Task Info
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
- Step 52 is not ported from `step41-ported-current-goal.patch`: dev's Stop gate (PR #1616, ADR-0428) already continues goal plans and specs for every harness, carries the retry count across moves and keeps it across capacity holds. Only the red-PR condition was missing. Review round 1 of #1700 refused deciding it from the task list's display read of GitHub (ADR-0447, ADR-0409), so the gate reads local evidence only: the task's latest Yulius CI run for its open PR, failed at the worktree's `HEAD`, after the PR watcher has told the agent (SPEC-578). There is no "could not be read" continuation. Codex only: Claude Code's native goal evaluator owns its goal's end. [ADR-0464](../../docs/decisions/0464-a-codex-goal-continues-on-its-own-red-yulius-ci-run.md)
- Step 50 is ported from #540's dropped commit `d04bfe295` onto dev's read-write `arm_writer` (read guards, as step 49's capacity check takes). The hold is taken where the owner is released or closed, after the Done check (which can record a merge it observed), not inside `task_is_done`/`shows_done`, whose callers all take it themselves.
- Step 51 reads the PR watch's last answered observation of the owner's failed decision (`local_candidate_shown`) rather than #540's `candidate-server-d33.patch`, which read the hosted PR from the list path, and rather than capacity's `local_candidate_blocks`: capacity must re-verify (its observation expires and is cleared while a re-read runs), but a view showing "pending" during every 20 s re-read would flicker and was the round-1 finding. The view answer is kept per decision (span and branch), so a new failed decision reads pending at once; it is in memory, so a restart shows pending until the watch's first pass. `candidate_pending` is an attention fact like `merge_armed`, so the server's groups, Home and the tools agree with the browser, and a pending owner is not parked because its group is Working, as capacity keeps its session.
- Old parks are recovered from the run's final screen instead of being rewritten on disk: the read path is the only place that knows whether the run was parked (the recorded transition), and leaving the files alone keeps the change reversible.

## Evidence

- `git apply --check -R` of `34f395357` (crates only) succeeds on `2a6e07423`; `git merge-base --is-ancestor 8855f295b origin/main` fails.
- Step 53: on dev the extended cockpit fixture `the_superagent_sees_a_crashs_last_screen_a_tasks_usage_and_its_checkout` failed at its first new assertion, showing `Token usage: total=389,557`, `To continue this session, run:` and `codex resume synthetic-rollout` under the answer. With the port it passes; `parking::tests` (9) and the `cockpit::`, `parking::`, `codex::` scope (287 tests) passed, Clippy on `yulius-app` and `yulius` passed, `scripts/dev quick` passed. The red run on dev is the mutation check for the whole recovery path.
- Review round 1 follow-up: `a_wrapped_codex_exit_block_under_an_answer_is_removed` (80 columns), and the cold queued-park fixture now also removes the snapshot and reads the final screen through the recorded transition: a Claude Code run keeps a Codex-looking footer, the same run marked Codex loses it. Hand mutations, each restored: removing the wrapped-row join failed the unit test; replacing the harness gate with `true` and dropping the transition recovery each failed `parking::a_switch_overtaken_by_a_park_parks_with_its_activity`.
- Step 52: `agent_hooks::codex_goal_stop_continues_the_325_shape_and_a_red_pr_alone` failed on dev (decision `null` once steps 6 and 7 were done with the PR red) and passes, with a held GitHub checks read that cannot delay it; `codex_goal_stop_ends_once_its_head_moved_past_the_red_run` and `a_red_pr_continues_no_other_harnesss_finished_goal` hold the head comparison and the Codex-only gate. Hand mutations, each restored: removing the harness gate, the head comparison, or mapping the failing arm to `Settled` each failed its test. Whole `yulius::main` 2,608 passed, Clippy and `scripts/dev quick` passed after the rework.
- Round 2 of #1700 follow-up: `codex_goal_stop_ends_after_its_red_pr_merged` and `codex_goal_stop_leaves_an_untold_failure_uncharged` added; the other-harness test asserts a null decision and no charged attempt. Hand mutations, each restored: matching any delivery stage instead of `PrOpen` failed the merged-PR test. (Round 2 of #1709 found the untold test no longer held the told check once the newest-run guard existed; see below.) The run is now read by commit (`ci_runs_for_sha`), not from the sandbox's newest 100 runs.
- Review of #1709 follow-up: `codex_goal_stop_finds_its_red_run_in_a_busy_sandbox` (100 newer runs of other pull requests), `codex_goal_stop_yields_to_a_newer_run_of_its_pr_elsewhere` and `codex_goal_stop_yields_to_a_pushed_head_past_its_red_commit` pin the by-commit read and the two supersessions. Hand mutations, each restored: reading the sandbox's newest 100 runs instead of the commit's failed the busy-sandbox and moved-past tests; dropping the newest-run guard and the pushed-head guard each failed its test.
- Round 2 of #1709: `codex_goal_stop_leaves_an_untold_failure_uncharged` now puts the untold red run outside the watcher's window (100 newer runs), so Stop does not hand over and no other guard settles it; the newest-run window is read in `stop_pull` and an unreadable window settles.
- Mutation sweep on the final `stop_pull` (each line undone alone, restored): harness gate → `a_red_pr_continues_no_other_harnesss_finished_goal`; `PrOpen` guard → `codex_goal_stop_ends_after_its_red_pr_merged`; by-commit read → `..._finds_its_red_run_in_a_busy_sandbox` and `..._ends_once_its_head_moved_past_the_red_run`; told check → `..._leaves_an_untold_failure_uncharged`; newest-run guard → `..._yields_to_a_newer_run_of_its_pr_elsewhere`; pushed-head guard → `..._yields_to_a_pushed_head_past_its_red_commit`. Each failed.
- Step 50: the four tests ported from `d04bfe295` (`after_all_tasks_includes_the_owner_of_the_current_failed_candidate`, `archive_keeps_the_owner_when_a_replacement_arm_arrives_at_close`, `a_replacement_merge_is_refused_while_the_reader_settles_a_close`, `a_compound_done_rule_rechecks_its_first_owner_after_reading_the_second`) each failed on dev for the expected reason (rule without the owner; archive closed; merge armed during the close; successor queued). Two needed the watch to observe the moved base first, since step 49 clears a failure only from that observation; the archive test asserts the close was decided. `a_chained_successor_waits_until_the_live_owners_failure_is_cleared` was added for the single-predecessor chain. Hand mutations, each restored: the compound recheck, the chain release hold, the settling half of the closing-arm refusal, the start-rule verification and the close hold each failed their test. Whole `yulius::main` 2,665 passed, `yulius-app` lib 718, Clippy passed.
- Inherited flake: `a_candidate_refresh_waits_for_the_launch_to_reserve_its_verified_slot` (step 49) failed 1 in 4 to 1 in 13 stress runs at the merge base: stopping the peer wakes the queue, whose capacity check briefly holds the owner's delivery guard, and a watch pass meeting it defers the hosted read (`observe_candidate_pull`'s `try_write`), as designed. The test now waits, bounded, for the read; 60 of 60 stress runs passed.
- Step 50 review round 1: `supervisor::tests::a_close_intent_or_a_settling_run_counts_as_closing` (unit), `delete_keeps_the_owner_when_a_replacement_arm_arrives_at_close` and `removing_the_subscription_moves_an_unverified_owner_instead_of_closing_it` added. Hand mutations, each restored: dropping the Close-intent half of `closing_or_settling`, passing no owner to delete's close, and passing none to the removed subscription's close each failed its test. A task that parks while archive or delete closes it closes as on dev.
- After that round: whole `yulius::main` 2,707 passed once `fake-agent` was rebuilt (dev's new `forks::` tests timed out on the stale binary), `yulius-app` lib 722, Clippy and `scripts/dev quick` passed.
- Step 50 review round 2 (approved `da5b65d10`) notes: delete and the onboarding archive now return the hold's own refusal ("…being verified…") while the owner still runs, asserted by the delete test; the removed-subscription test asserts the close was decided.
- Step 51: `a_live_owner_with_a_pending_candidate_is_shown_waiting_not_done` failed on dev (the owner's summary read `activity: done` with no pending flag) and passes: pending and `shows: working` while the failure applies, then a published `TaskUpdated` without it and `shows: done` once the watch observed the moved base. Domain group test for the fact. Five `task-done.e2e.ts` cases (counting rows, terminal and Task Info with the sidebar, the terminal's detail read clearing it, a resume found by the terminal bringing it to the row) failed before the web change, three of them shown red. Hand mutations, each restored: the fact never set, the group ignoring it, no publish on the flip, `statusInfo` ignoring it, and dropping it in the terminal's summary merge, `pickSummary` and the store's `lifecycleSummary` each failed a test. Full e2e 1,346 passed (1 skipped), whole `yulius::main` 2,724, `yulius-app`/`yulius-domain`/`yulius-contracts` libs 1,428, workspace Clippy and `scripts/dev quick` passed.
- Step 51 review round 1: the views no longer reuse capacity's predicate (it reads pending during each re-read and after a failed one); `local_candidate_shown` reads the watch's last answered observation per failed decision, the watch publishes whenever it changes, `task_get` names the re-check in `waits_for`, and the web settles the failure before failing checks and never on a parked row. The Rust case now also reads the summary with the re-read held at `CandidateObservationRead` and after a failed re-read, and checks a parked owner omits the field; the counting case adds a red PR and a parked row. Hand mutations, each restored: the views on capacity's predicate, the `live` guard, the publish on a changed answer, `asIdleRun` keeping the field and failing checks first each failed a test. A new failed decision over an older cleared one is covered by comparing the decision's span.
- After that round: whole `yulius::main` 2,735 passed, libs 1,428, workspace Clippy, `scripts/dev quick` and the full e2e suite (1,346 passed, 1 skipped) passed.
- Step 51 review round 2: the Rust case also checks that an unchanged answer publishes nothing and that a second decision (its span rewritten as `a_singleton_decided_on_a_new_base_during_the_host_probe_keeps_the_owner_live` does) reads pending at once and is published by the next pass. Hand mutations, each restored: comparing only the branch in `local_candidate_shown` and publishing on every pass each failed it. The web's two unreachable `candidate_pending` conditions after the early return were dropped.
