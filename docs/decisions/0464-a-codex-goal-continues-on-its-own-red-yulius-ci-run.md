# 0464. A Codex goal continues on its own red Yulius CI run

## Status

Accepted for [SPEC-621](../../.ai/specs/SPEC-621-finish-540-on-claude-code-parked-codex-t.md), completing [SPEC-558](../../.ai/specs/SPEC-558-parked-codex-tasks-no-codex-exit-screen.md) PKR-003. Extends [0428](0428-a-goal-turn-continues-its-declared-work.md).

## Context

Quartio-v2 #325, a Codex goal, ended its turn with its plan finished and its pull request red, and waited for a person. Claude Code has a native goal evaluator that keeps such a goal going; Codex has none. Decision 0428 continues a goal's open steps and unfinished spec, not a failing pull request. The PR watcher pastes an untold failed Yulius CI run into the session once (SPEC-578); a Stop after that was the end of the goal.

## Decision

At a clean Stop of a Codex goal whose plan and spec are finished, the gate continues the goal when the newest Yulius CI run of its recorded open pull request at the commit its worktree is on failed, and nothing superseded it: no newer run of that pull request exists on another commit and no pushed head has moved past that commit, as the PR watcher decides for the same failure. The continuation names the pull request and points to `ci_status`. It uses decision 0428's retry budget: after three continuations the next unrepaired Stop flags the task. Only a failure the PR watcher has already pasted into the session counts (SPEC-578); an untold one is the watcher's, and every handoff of 0428 keeps its precedence. The evidence is local: the delivery record, the newest run of that pull request at the commit `git rev-parse HEAD` names in the worktree, and the watcher's record of what it told, read within the gate's 750 ms probe stage. No GitHub request is made at Stop.

## Alternatives

Reading the pull request through the task list's display verifier (the first version of this change) let the display quota reserve and its 30-second cache decide an action, against [0447](0447-task-display-shares-github-reads-and-yields-to-delivery.md) and [0409](0409-report-the-pr-that-blocks-a-parked-task.md). Continuing when a read could not finish, as SPEC-558 first decided, would charge the budget for a slow or paused GitHub. An operational GitHub read needs several uncached round trips that may not fit one Stop. Applying the rule to every harness would override Claude Code's native goal evaluator.

## Consequences

A Codex goal repairs its own red Yulius CI before it rests, within the existing bound. A failure of a commit the agent has moved past, pushed or not, is not charged. Checks run only on GitHub, with instance CI off, still reach the agent through the PR watcher's news, not through this continuation.
