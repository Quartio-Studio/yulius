# 0455. Count Codex subagents under their session's run, by model

## Status

Accepted for [SPEC-619](../../.ai/specs/SPEC-619-count-the-usage-of-codex-subagents-in-th.md).

## Context

Codex 0.170 writes each subagent's thread to a rollout of its own, in the sandbox's shared sessions folder, beside other tasks' and people's sessions. A run's metrics and the [list-price estimate](0440-estimate-codex-work-at-standard-and-ultrafast-api-prices.md) read the main rollout only, so a task that reviews with subagents is undercounted. A subagent may run another model than its session, and a session may be resumed on another subscription while its subagents answer.

## Decision

A run counts the responses of its session's descendant threads: rollouts whose first line names the session, or another descendant, as `parent_thread_id`, or names the session as `session_id` and has a parent. Each response counts once, by its response id, only in the thread that made it, and in the run whose window holds its time, as the main rollout's lines are windowed. The run keeps its own thread's model. The tokens that other models answered are stored as parts of the run, by model, and the estimates price each part at its own model. The watch and the recount share the discovery and the counting. Discovery lists only the date folders from the day before the session's own, examines only files changed since the window began, reads each first line once and is bounded in files examined and followed. The migration queues the last week's Codex runs for one recount.

## Alternatives

Matching children by working directory or start time would count a `codex` a person runs in the worktree and other sessions. Taking the children from Codex's `SubagentStart` hooks, as [ADR-0442](0442-identify-codex-reviewers-by-their-child-rollout.md) does for reviewers, would leave an ended run's recount and any child whose hook did not reach Yulius uncounted. Recording each subagent as a run of its own would change run counts, lifetimes and the task's run history. Pricing a subagent at its session's model would misprice the 15 retained Astra subagents of Sol sessions. Recounting every Codex run would reread every retained rollout at start.

## Consequences

Task metrics, per-subscription totals and the weekly estimate include subagents, and the estimate no longer discloses them as excluded. Codex runs that ended more than a week before the upgrade keep their main-rollout figures. A subagent whose rollout is gone, or beyond the bounds, is not counted. Claude Code sub-agents keep their session's model.
