# 0429. Continue stopped Codex work in its saved session

## Status

Accepted for task #574. [SPEC-590](../../.ai/specs/SPEC-590-look-bro-the-taks-says-it-working-but-th.md)

## Context

A native Codex resume can reopen history and sit at an empty prompt. After a person stopped a task and restarted it, the task list showed Working while the Superagent reported that Codex was waiting for another message.

## Decision

Continue on a stopped or failed Codex task with a saved conversation submits a continuation in that conversation. The latest attempt’s Codex session ID can restore a missing history index entry. When older history cannot identify that attempt, Continue reports the conflict. The supervisor’s queue, capacity pause and subscription move carry the prompt. A fresh launch uses the original request.

The supervisor initializes a bare Codex resume with idle activity. Reopening a closed conversation leaves the next prompt to the person. Submitting a prompt and interrupting a turn use the existing hook and rollout observations.

## Alternatives

Correcting only the activity label would still require a second message to resume stopped work. Inferring activity from the terminal's visible text would mistake an old interruption in scrollback for the state of the current turn.

## Consequences

The page and Superagent share the same restart behavior. A person can stop work indefinitely and use Continue when they want it to run again.
