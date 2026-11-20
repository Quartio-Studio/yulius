---
yulius-task: 92636ff0-3e0c-43ab-a520-97e4666a2df5
status: in-progress
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
- [ ] Verify the fix and obtain independent approval for delivery

## Acceptance criteria

- **AC-1** A duplicate stop leaves the first report intact. A start after that report and a second stop produce two numbered verdicts. A duplicate start during a round creates no extra row.
- **AC-2** Request changes, approve and approve again with one reviewer. The merged analytics endpoint returns three rounds, one re-review after changes and one round after approval. A late hook for the merged reviewer leaves the next pull request alone.
- **AC-3** Replays with an earlier client send time cannot settle the next round. A continued round takes its own head at start and subsequent transcript observations affect the latest round.

- **AC-4** An old hook preserves the continued reviewer’s pending recovery after a usage limit. A report arriving during its start-time Git lookup still closes the continued round.

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
