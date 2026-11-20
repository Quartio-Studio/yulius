# 0400. Continuing a reviewer after its report opens the next round

## Status

Accepted for [SPEC-565](../../.ai/specs/SPEC-565-preserve-verdicts-when-a-reviewer-report.md). Extends [ADR-0341](0341-yulius-picks-the-model-of-each-review-round.md).

## Context

Claude Code emits `SubagentStart` with the same subagent ID when an author uses `SendMessage` to continue a reviewer. An interrupted reviewer may still owe its first report. A reviewer that already reported can review fixes and give the same verdict again.

## Decision

The first stop completes the unfinished round. Starting that reviewer again after its report opens the round after the last reported round of the same pull request. It records the head at the new start, keeps the reviewer's model and tracks its own cut-off and report. An unfinished reviewer continues its existing round. The hook client send time orders delayed deliveries. Older clients use arrival time.

## Alternatives

Comparing report text would merge two identical approvals across a genuine continuation. A separate reports table would require every round reader to reconstruct the sequence the existing schema can already represent.

## Consequences

The delivery view, merge record and analytics consume the same numbered rounds as before. Merged rounds are immutable. Old databases need no migration. Verdicts overwritten before this change cannot be reconstructed from the stored rows.
