---
yulius-task: 92636ff0-3e0c-43ab-a520-97e4666a2df5
status: done
---

# SPEC-565 Preserve verdicts when a reviewer reports again

## Goal

Fixes #1401. Count each reported review once when the author continues the same reviewer after fixes.

## Context

On dev `645eff207`, repeating the completion update moves the end time, and approving after requesting changes leaves only the approval. The reviewer start handler treats every known subagent as a retry. The stop handler updates every unmerged row for that subagent. Earlier hook tests continued an unfinished reviewer or started a different reviewer after a report.

[SPEC-493](SPEC-493-faster-from-start-to-merge-opus-writes-f.md) observed that continuing a Claude Code reviewer with `SendMessage` fires another `SubagentStart` with the same ID. Task #541 assigned this issue to task #547.

## Requirements

- **RVR-001** A stop closes its unfinished round once. Repeated delivery preserves its verdict and end time. A repeated start of an unfinished or cut-off reviewer keeps that round.
- **RVR-002** Starting a previously reported reviewer in the same open pull request opens the round after the last reported round. Record the current head, the same reviewer and model, and fresh timing and cut-off state. Count identical verdicts in successive rounds. Merged rounds stay closed.
- **RVR-003** The delivery records and merged review analytics expose the continued rounds in their existing format. Document the rule with SPEC-493's round records.

## Plan

- [x] Reproduce lost verdicts on current dev and specify report identity
- [x] Prove replay and continued review analytics with failing regressions
- [x] Preserve each distinct report and document round accounting
- [x] Protect cut-off recovery and accept reports during a slow head lookup
- [x] Order recovery against the continued round’s own cut-off
- [x] Attribute replayed transcript observations to the review round that ran
- [x] Preserve same-model answers and recovery during a delayed account write
- [x] Verify the fix and obtain independent approval for delivery

## Acceptance criteria

- **AC-1** A duplicate stop leaves the first report intact. A start after that report and a second stop produce two numbered verdicts. A duplicate start during a round creates no extra row.
- **AC-2** Request changes, approve and approve again with one reviewer. The merged analytics endpoint returns three rounds, one re-review after changes and one round after approval. A late hook for the merged reviewer leaves the next pull request alone.
- **AC-3** Replays with an earlier client send time cannot settle the next round. A continued round takes its own head at start and records its own transcript observations.
- **AC-4** An old hook preserves the continued reviewer’s pending recovery after a usage limit. A report arriving during its start-time Git lookup still closes the continued round.
- **AC-5** On restart, replayed model and usage-limit observations for a completed round retain their attribution. A delayed live limit from that round leaves the continued reviewer’s recovery alone.
- **AC-6** Consecutive rounds preserve the same observed model even when both answers arrive during restart replay. A limit being saved to the account can complete after the reviewer reports without scheduling a move for the next round.

## Test plan

Drive the authenticated reviewer hooks with synthetic starts/stops, inspect SQLite through the store, merge via the normal delivery transition and read `GET /api/analytics/reviews`. Run the review-round suite, repository quick checks, change security scan and mutation command. Independently review and deliver on Yulius CI.

## Decisions

Use the start after a report as the round boundary. Comparing verdicts or report text loses a second approval with identical wording. The existing schema holds several rows for one subagent. [ADR-0400](../../docs/decisions/0400-continuing-a-reviewer-after-its-report-opens-the-next-round.md) records the numbering and replay rule. Past overwritten verdicts have no recoverable value in those rows.

## Evidence

- On dev `645eff207`, `cargo test --locked -p yulius --test main review_rounds::continued_review` failed both new regressions. Repeating the stop moved the end time by 1 ms. The analytics endpoint returned median rounds 1, first rounds requesting changes 0, re-reviews after changes 0 and rounds after approval 0 for a three-report sequence.
- The existing `a_resumed_reviewer_and_other_subagents_change_no_round` test passed on the same base. It only resumes an unfinished reviewer.
- With the fix, both hook regressions passed. The API returned median rounds 3, first rounds requesting changes 1, re-reviews after changes 1 and rounds after approval 1. The store retained the first report's head, timestamp and model while the continued reviewer reported against a second real Git commit.
- `scripts/dev quick` passed the repository contracts and Rust formatting checks.
- The change security scan reported Vidoc disabled for the project or instance. It scanned no files.
- All 29 tests in `cargo test --locked -p yulius --test main review_rounds::` passed, including eight subprocess cases for cut-off reviewers, moves and resumption.
- Round 1 requested changes on `3fae70d80`. Two new deterministic regressions failed on its implementation. Replaying the older start and stop each erased the continued reviewer's pending resume and move. Delivering the stop while Git was held left round 2's verdict empty. Both findings concern this change.
- Round 1 findings were fixed in the same branch. The store reserves the continued row before Git I/O and the accepted start or stop controls recovery. The held-Git test retained the approval and the correct head. Both stale events preserved recovery while a genuine resume and report cleared it. Nextest ran all 31 `review_rounds::` tests with zero failures (run `0295f944-aa01-4e3b-9649-de325632e6a4`).
- Round 2 confirmed both earlier fixes and requested one further own-code change on `7773d7618`. Replaying round 2's original start after a later cut-off cleared recovery. The new nextest regression failed on that head with both the reviewer ID and move lost.
- First review's mutation run completed. `scripts/dev mutants` reported 30 listed, 5 caught, 0 missed, 0 timed out, 25 unviable and 0 not reached. Its unmutated baseline passed 3,209 tests.
- Round 2's finding was fixed by retaining the limit's observation time with the pending reviewer. The original start is ignored for recovery, and a genuine later start clears it even when it was sent before the watcher processed the limit. Nextest passed all 32 review-round tests and both usage-credit tests for live and restart transcript observations.
- Round 3 on `c1e2c6723` found an own-code defect in transcript attribution. The new fixture restarted a real supervisor over two rows sharing a reviewer ID. It lost the first round’s observed model during catch-up. A second regression showed that a dated limit from the completed round scheduled recovery for the continued round. Both failed on the reviewed implementation.
- After the transcript attribution fix, the restart fixture retained round 1’s Fable answer and cut-off, round 2’s Opus answer and its uninterrupted state. The live delay regression preserved round 2’s recovery state until its own limit arrived. Undated replay with multiple candidate rounds is left unattributed.
- Nextest run `33ffc8a8-3e80-40a0-94a0-e97a42f4eca8` passed 37 of 37 targeted tests covering authenticated hooks, the analytics endpoint, transcript parsing, live observation and restart.
- The first follow-up mutation run listed 23, caught 14, missed 1, timed out 0, found 8 unviable and reached all 23. Inverting the first start's link count left the stored row correct but inverted the start outcome. The added port regression checks both the unmatched start and the successful link. Two head-result mutations the initial run could not compile while the worktree was changing were both caught in this isolated run.
- The independent reviewer continued on `0fb0baac2`, verified the earlier attribution fix and found two own-code cases. During restart, a second answer on the same model was suppressed by the per-subagent cache. During a slow account write, the reviewer could report and continue before the supervisor installed the old limit’s recovery state.
- The two regressions in `2272fbf1d` failed on the reviewed implementation. Two Opus answers for consecutive requested Fable rounds left round 2 without an answering model. The held account write left `(Some("rev"), true)` as the reviewer and scheduled move after both a resume and a report followed by continuation.
- Nextest run `22b06f39-3bdd-448f-9f81-aa446c7ed708` passed all 40 targeted tests on the fix. Both rounds recorded Opus. The held write still recorded Fable usage at 100 percent while both accepted hook sequences cancelled the obsolete recovery.
- Independent review approved `f060958806c641c6dd9115a52e17b3e729e107f2` against dev `4403c07de4eb1d3f7959966b853714112bbc0727` with no actionable findings. It checked duplicate stop delivery, continued reports, restart replay, account-write recovery and merged analytics. The reviewer reran 40 cases using the pre-integration binaries and ran `git diff --check` on the reviewed head.
- Clippy passed for the changed application, store, product and test targets with all features and warnings denied.
- The author rebuilt the reviewed integration head `f06095880` and reran the same 40 cases. All 40 passed, including actual Git head capture, authenticated hook replay, merged analytics, delayed account writes and supervisor restart.
- The isolated mutation run for the cut-off ordering fix listed 16, caught 10, missed 0, timed out 0, found 6 unviable and reached all 16. It supersedes an earlier run from a worktree that changed under the runner.
- Rerunning all 23 first follow-up mutants with the link-outcome regression caught 15, missed 0, timed out 0, found 8 unviable and reached all 23. This includes the previously missed inverted link count.
- The isolated transcript-attribution mutation run listed 46, caught 23, missed 0, timed out 0, found 23 unviable and reached all 46.
- The recovery mutation run listed 37, caught 22, missed 1, timed out 0, found 14 unviable and reached all 37. Its missed mutation exposed a redundant synthetic-model guard. The Claude parser already removes the synthetic model before the watcher sees the observation. A fixture now puts a synthetic notice between two real answers. Removing the parser filter in a negative control produced a spurious third answer and failed the fixture. With the parser restored and the duplicate watcher guard removed, the fixture passed. The integration merge also brought ADR-0399 into the index. The quick check caught its ordering with ADR-0400 and passed after they were sorted.
- The post-approval mutation run listed 3, caught 2, missed 0, timed out 0, found 1 unviable and reached all 3. All 40 focused tests passed again on `ac7e6304f`.
- Previously overwritten verdicts cannot be reconstructed from the affected rows. Undated replay with several candidate rounds leaves them untouched. Local behavioral evidence uses synthetic hooks and transcripts with the real Git, SQLite, HTTP and supervisor paths.
