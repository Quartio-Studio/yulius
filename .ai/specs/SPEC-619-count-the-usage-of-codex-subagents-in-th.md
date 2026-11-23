---
yulius-task: 6b2bcd1a-f537-46c2-8615-8e3444bac1e1
status: done
---

# SPEC-619. Count Codex subagents' usage in their task's metrics

## Goal

A Codex task's run counts the responses of the subagents its thread spawned, once each, under
the run in which they were answered and priced at the model that answered them. The live watch
and the historical recount give the same figures. Fixes
[issue 1628](https://github.com/Quartio-Studio/yulius/issues/1628).

## Context

[SPEC-503](SPEC-503-accept-outside-pr-1146-codex-returns-as.md) CDX-004 counts a Codex run from
its rollout's `token_usage_record` lines, one per response. The watch
(`crates/yulius/src/wiring/harness.rs`) and the recount
([SPEC-091](SPEC-091-recompute-the-metrics-of-runs-recorded-b.md),
`crates/yulius-harness/src/recount.rs`) read the main rollout only. Codex 0.170 writes each
subagent to a rollout of its own in the sandbox's shared `codex/sessions/YYYY/MM/DD`. The
rollout's first line, `session_meta`, names the child's thread `id`, its `parent_thread_id` (also
under `source.subagent.thread_spawn`) and the root thread as `session_id`. Each
`token_usage_record` names the `thread_id` that made the request. A forked child copies its
parent's history into its own file, ahead of `subagent_history_start_ordinal`. Other tasks' and
people's sessions share the folder.

The sandbox's 662 retained rollouts (checked 2026-11-22) hold 582 children of 80 sessions. One
session spawned 234 children over two days. 196 children are in a later date folder than their
parent. 15 children ran `gpt-6.2-astra` under a `gpt-6.2-sol` parent. No response id appears in
two files. No copied history held a token record.

[SPEC-602](SPEC-602-it-doesnt-seem-real-verify-i-think-it-s.md) prices one model per run
([ADR-0440](../../docs/decisions/0440-estimate-codex-work-at-standard-and-ultrafast-api-prices.md))
and discloses "Separate Codex subagent sessions excluded". Claude Code's sub-agents already count
in their session's run (SPEC-087 MET-004).

## Requirements

- **CSA-1** A run of a Codex session counts the responses of every descendant thread of its
  session: rollouts whose `parent_thread_id` is the session or another descendant, at any depth,
  or that name the session as `session_id` and have a parent. A session with no parent (a
  person's fork, a `codex` run in the worktree, another task) is not counted.
- **CSA-2** A child's response counts once. It counts only in its own thread (`thread_id`). A
  response id read twice counts once: a poll again, a copied history or a duplicate line.
- **CSA-3** A child's response counts in the run whose window holds its time. The window is the
  one SPEC-087 MET-002 and SPEC-091 RCT-001 give the main rollout's lines, across resumes and
  account moves. It is priced at the model of the child's last `turn_context`, also when that
  line is before the window.
- **CSA-4** A run keeps the model of its own thread. Its tokens include its children's. The parts
  that ran on another model are stored by model. The pool and subscription estimates price each
  part at its own model's Standard price and Ultrafast scenario. A run with a part of an unpriced
  model counts as a run without a model price.
- **CSA-5** The live watch and the recount of the same window give the same figures.
- **CSA-6** Discovery and reads are bounded while other tasks write to the same folder. Only date
  folders from the day before the session's own are listed. Only files changed since 2 seconds
  before the window began are opened (file times may keep whole seconds). Each file's first line
  is read once, at most 1 MiB of it, and an incomplete one again only once the file grew. At most
  4,096 rollouts are examined and 1,024 children followed; the live watch logs once that a bound
  was reached or a rollout could not be read (a recount that cannot read one fails). A run keeps
  parts of at most 16 models, of names of at most 128 bytes. The live watch lists the folder at
  most every 2 seconds.
- **CSA-7** The migration queues the Codex runs that ended in the last 7 days for one recount, so
  the weekly estimate counts their children. A running run's watch reads its window again after
  the restart. The disclosure is removed.

## Plan

- [x] Write and validate the spec
- [x] Find a thread's child rollouts with bounded discovery
- [x] Count child responses with their model in the historical recount
- [x] Count child responses in the live watch, agreeing with the recount
- [x] Store and price the per-model parts of a run
- [x] Recount recent Codex runs and update the estimate's disclosure and docs
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- **AC-1** A session with a child, a nested grandchild, a forked child, a duplicate line and an
  unrelated session and fork in the same folder counts the session's and its descendants'
  responses once each.
- **AC-2** A child that answers in two runs of a resumed session, on different subscriptions,
  counts each response in its own run, at the child's model, when the model line is earlier.
- **AC-3** The watch, polled while children appear, and the recount of the same window agree on
  tokens and parts.
- **AC-4** A run whose child ran another model is priced per part in both estimates. An unpriced
  part makes it an unpriced run.
- **AC-5** Discovery skips date folders before the session's and files unchanged since the window
  began, and stops at its bounds.

## Test plan

Unit tests in `yulius-harness` build synthetic rollouts in Codex 0.170's shapes in a temporary
sessions folder: discovery, nesting, forks, duplicates, windows, bounds. A wiring test compares the
live watch with the recount. Domain tests price runs with parts, and a store test round-trips
them. Each new test fails without its change.

## Decisions

- Children are found by `parent_thread_id`, not by working directory or time: other sessions run
  in the same worktree. [Decision 0455](../../docs/decisions/0455-count-codex-subagents-under-their-sessions-run-by-model.md)
- A run stores the parts of its tokens that ran on other models. Splitting a child into a run of
  its own would change run counts and lifetimes.
- Only the last week's Codex runs are recounted. Recounting every one would reread gigabytes of
  rollouts at start, and older figures do not feed the estimate.

## Evidence

- New tests: `codex_children::tests` (5: descendants only, nested, fork copy, unrelated session and fork; windows across a resume; date-folder and mtime skips with a partial first line; bounds and the listing interval), `recount::tests::a_codex_run_counts_its_subagents_once_by_model`, `wiring::harness::tests::the_live_watch_and_the_recount_agree_on_a_codex_runs_subagents`, `pricing::tests::a_runs_parts_are_priced_at_their_own_models`, `capacity::tests::a_runs_subagent_parts_are_priced_at_their_models`, the parts case of `accounts::tests::a_subscriptions_cost_counts_only_priced_runs_with_usage`, `tests::codex_runs_of_the_last_week_are_recounted_and_parts_are_kept` (store) and the two capacity-panel checks in `subscriptions.e2e.ts`.
- Hand mutation checks: dropping the subagent loop of `count_codex_run` failed the recount test; dropping `observe_child` in the watch failed the agreement test (`output Some(1), not 11`); pricing every run with no parts failed the capacity and subscription-cost tests. Each passed again once restored.
- Test binaries run in full: `yulius-domain`, `yulius-store`, `yulius-harness` and `yulius-app` (1,724 tests); the `yulius` library tests with the `metrics_recount`, `analytics`, `subscription_pool`, `codex` and `capacity` integration files (775 tests); `subscriptions.e2e.ts` (50 tests). `scripts/dev quick` passed.
- Real rollouts (the sandbox's 662, read only, private data kept out of the repository): a throwaway probe built on `count_codex_run` matched an independent Python sum of distinct `token_usage_record`s per thread tree for the 8 sessions with most children. The largest, with 234 children, counts 4,277,946 output tokens instead of the main rollout's 3,293,469, read in 0.8 s. Two Sol sessions get an Astra part (4,848 and 5,438 output tokens).
- Not tested: a nested subagent (depth 2) written by a real Codex; no retained rollout has one, so its `session_id` is not known. Either link (parent or session) admits it.
- Review round 1 (Fable 5.2) requested changes: the session-only link, a resumed run's live window and the failure paths had no test that fails without them, plus minors. The new tests `a_child_whose_parent_is_not_followed_counts_by_its_session`, `a_modification_time_in_whole_seconds_still_reads_the_file`, `first_lines_are_read_within_their_bound`, `a_child_that_cannot_be_read_fails_the_poll_until_it_can`, `observe::tests::model_parts_are_bounded` and `a_resumed_codex_runs_live_watch_counts_its_own_window_of_subagents` cover them. Removing the session link, the 2-second modification slack and the watch's `since` each failed its test. A first line still being written is read again only once the file grew. Reaching a bound, or the first unreadable subagent rollout, is logged at warn. A run of an unpriced model without parts stays unpriced at zero tokens, as before.
- After the fixes, the four library crates passed 1,729 tests, and the `yulius` library plus its touched integration files passed 776. `hook_client an_antigravity_turn_ended_by_an_error_is_never_continued`, a test this change did not touch, failed once in an earlier run and then passed in four reruns. It is tracked in [issue 1693](https://github.com/Quartio-Studio/yulius/issues/1693).
- Review round 2 (same reviewer) approved `5422f8ea6`, with these minors: the 128-byte name cap and the pending-file rules had no test that fails without them, a file that could not be opened waited for its length to change, and CSA-6 claimed logging for the recount too. The fixes after the approval: `incomplete_first_lines_wait_for_the_file_to_grow` and a stronger `model_parts_are_bounded`, which fail when the pending skip, the pending files in the bound, the bound flag or the name cap is removed. A file that cannot be opened is retried at the next listing, and one removed since it was listed is skipped. Neither has a test: each needs a race between the listing and the open or stat, the walk lists regular files only, and the tests run as root, so file permissions refuse nothing. Round 3 approved `f66d8a4f5`. Afterwards `a_pending_file_that_grew_is_read_at_the_bound` pinned the bound's `continue`, and putting `break` back fails it. A bound skips only new files, and CSA-6 is reworded. The `yulius-harness` and `yulius-domain` tests (870) and the `yulius` library tests (692) passed, as did `scripts/dev quick` and clippy.
- Vidoc is switched off for this project in Yulius: no security scan ran.
