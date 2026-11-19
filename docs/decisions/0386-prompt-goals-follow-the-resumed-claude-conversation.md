# 0386. Prompt goals follow the resumed Claude conversation

## Status

Accepted for SPEC-540.

## Context

Claude Code can restore the evaluator for a person's goal on native resume. A new run's transcript window starts after the goal was set. A clean Stop can arrive before the evaluator writes another record.

## Decision

On a prompt-mode Claude resume, initialize the supervisor's goal reading from the last goal status in the conversation. A new verdict supersedes it. The goal evaluator belongs to Claude Code. Yulius uses its existing bounded Stop wait even if workspace trust or hook policy prevented a restore.

## Alternatives

Typing a person's goal again would submit a new command and could disturb their session. Replaying earlier turns would also apply old activity, completion and usage events to the new attempt.

## Consequences

An open prompt goal survives a move in Yulius's reading. If the evaluator stays silent, the turn settles on its closing record or at the verdict deadline. A silent evaluator provides no evidence that the person cleared the goal.
