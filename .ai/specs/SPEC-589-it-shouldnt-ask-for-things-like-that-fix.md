---
yulius-task: b2d041b9-f438-4630-8b8c-a686f9c0ef0d
status: in-progress
---

# SPEC-589 Continue authorized work at the end of a goal turn

## Goal

A goal task whose next action is already authorized continues that work itself. Its owner can leave the task running through verification on dev.

Fixes #1600.

## Context

The screenshot shows a live goal task with one open step, “Verify the merged revision on dev”. The system asks the owner to type “continue”. It does not identify the harness or sandbox. The shared Stop hook validates the spec and occasionally nudges for stale or untracked progress. An ordinary in-progress step meets neither condition. Settling the turn then derives a Needs You notice from the open plan. The Antigravity idle Stop regression even expects this result.

Earlier user requests establish the ownership rule. Task #534 concerned automated CI waits ([SPEC-552](SPEC-552-on-codex-agent-keeps-calling-me-even-if.md)), #540 asked for bounded continuation of unfinished Codex goals ([SPEC-558](SPEC-558-parked-codex-tasks-no-codex-exit-screen.md)), and #572 removed requests to authorize another independent review ([SPEC-588](SPEC-588-it-shouldn-t-ask-me-for-this-agents-must.md)). Post-merge machine evidence is already available through [SPEC-562](SPEC-562-complete-post-merge-verification-from-du.md).

## Requirements

- GCT-001. At a clean Stop of a live goal-mode task, continue actionable plan work or an unfinished spec under the existing authorization. Apply the rule across harnesses, including when the remaining step is post-merge verification. Preserve prompt-mode turn boundaries and the native goal evaluator.
- GCT-002. An outstanding human decision, a pause, an active background operation, or delivery news that already owns the next turn takes precedence over a continuation for declared work. The existing spec-format gate still checks required artifacts. A failed Antigravity turn uses its existing error path. Completion requires the normal evidence and step updates.
- GCT-003. Use the existing persisted Stop retry limit. When continuations are exhausted, expose the unsuccessful attempts and remaining work. A clean stop with finished work releases the turn.

## Plan

- [x] Trace the reported handoff against earlier continuation requests
- [x] Reproduce the incorrect handoff in regression tests
- [ ] Route unfinished work to its automatic continuation owner
- [ ] Verify the fix and obtain independent approval

## Acceptance criteria

- AC-1. The Stop response for a goal with the screenshot's one remaining step instructs the agent to continue. Task detail shows Working, preserves the step and records no Needs You episode.
- AC-2. A clean Stop can end a finished goal turn or a prompt-mode turn. A real owner question or a machine wait retains its owner.
- AC-3. An unfinished spec can continue even after the plan is complete. Repeated stops without completing the work reach the existing three-block limit.

## Test plan

Exercise the real HTTP hook and task query using isolated SQLite and Git fixtures. Check each harness, a verification-only step, spec-only work, completed work, prompt mode, existing human and machine handoffs, and repeated stops. Run the affected Rust tests and repository checks. CI verifies the full change.

## Decisions

[A goal turn continues its declared work](../../docs/decisions/0428-a-goal-turn-continues-its-declared-work.md).

Make the continuation decision where the agent can act on it, at the shared Stop hook. Sidebar and notification consumers can continue to report the task's actual state. Use the existing hook retry limit, delivery wake and human-request records. The earlier Codex prototype in #540 informed the ownership checks. Only a confirmed background operation or pending delivery wake takes ownership. A missing process observation permits the bounded continuation in the existing session. Rotation retains its separate evidence requirement before moving that session. PR-only recovery and parked-session repair have their own delivery paths.


## Evidence

- On dev revision `86eb47202`, `cargo test -p yulius --test main agent_hooks::goal_stop_` returned three failures. The post-merge case reproduced the screenshot's notice and its wait-history entry. The unfinished-spec case and the case where a machine wait had ended both returned an empty Stop answer. The explicit permission request and held scope decision retained their owner.
- On the first implementation the verification case passed for all three harnesses. The unavailable-host regression returned an empty Stop answer. It exercises a live attempt with a recorded child PID and a host that cannot read the process tree.
