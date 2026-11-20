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
- [ ] Preserve each distinct report and document round accounting
- [ ] Verify the fix and obtain independent approval for delivery

## Acceptance criteria

- **AC-1** A duplicate stop leaves the first report intact. A start after that report and a second stop produce two numbered verdicts. A duplicate start during a round creates no extra row.
- **AC-2** Request changes, approve and approve again with one reviewer. The merged analytics endpoint returns three rounds, one re-review after changes and one round after approval. A late hook for the merged reviewer leaves the next pull request alone.
- **AC-3** Replays with an earlier client send time cannot settle the next round. A continued round takes its own head at start and subsequent transcript observations affect the latest round.

## Test plan

Drive the authenticated reviewer hooks with synthetic starts/stops, inspect SQLite through the store, merge via the normal delivery transition and read `GET /api/analytics/reviews`. Run the review-round suite, repository quick checks, change security scan and mutation command. Independently review and deliver on Yulius CI.

## Decisions

Use the start after a report as the round boundary. Comparing verdicts or report text loses a second approval with identical wording. The existing schema holds several rows for one subagent. Past overwritten verdicts have no recoverable value in those rows.

## Evidence

- On dev `645eff207`, `cargo test --locked -p yulius --test main review_rounds::continued_review` failed both new regressions. Repeating the stop moved the end time by 1 ms. The analytics endpoint returned median rounds 1, first rounds requesting changes 0, re-reviews after changes 0 and rounds after approval 0 for a three-report sequence.
- The existing `a_resumed_reviewer_and_other_subagents_change_no_round` test passed on the same base. It only resumes an unfinished reviewer.
