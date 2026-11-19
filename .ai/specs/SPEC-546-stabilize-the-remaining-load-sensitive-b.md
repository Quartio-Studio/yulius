---
yulius-task: c875055e-3b84-4599-a00a-d727e4719d4f
status: in-progress
---

# SPEC-546 Stabilize the five browser scenarios reported in #1157

## Goal

An admin can change production protection with the keyboard while the Git page updates. The five browser scenarios reported in #1157 exercise their intended behavior even when reads, writes or rendering finish late.

## Context

On November 11, PR #1150 waited for the task page in the editor, Notes, Shell and removed-sandbox scenarios and polled the stored protection value. PR #1159 then fixed the editor accepting text in a placeholder Monaco model while the first file joined ([SPEC-452](SPEC-452-editor-e2e-a-folder-chosen-while-the-wor.md)). PR #1151 fixed early tool selection in the product ([SPEC-450](SPEC-450-browser-screenshot-never-shows-a-secret.md)). SPEC-461 records a full-suite editor failure on its pre-review branch. The keyboard-triggered toast also failed on a head carrying the stored-value wait. The old trace does not establish whether the switch activated. A live Git render replaces the input, including during a Space gesture, and the success toast has a finite lifetime.

## Requirements

- BR-1. After the first file joins, typing marks its tab modified and the unsaved text survives a sandbox switch. Notes, Shell and Superagent open in the intended sandbox with the expected focus.
- BR-2. Keyboard activation changes only the selected sandbox’s stored requirement once and leaves focus on its switch. An unrelated overview redraw can arrive during activation. A refused write restores the stored value.

## Plan

- [x] Trace the five reported failures across the merged fixes
- [x] Reproduce the remaining failure with controlled browser events
- [ ] Preserve the asserted behavior across delayed updates
- [ ] Verify the full browser suite under load and deliver the reviewed fix

## Acceptance criteria

- AC-1. For each named scenario record its cause and evidence under a controlled delay, including the historical fixes.
- AC-2. The admin’s switch works across an intervening redraw and a delayed PATCH. The other sandbox keeps its value. The error path remains usable.
- AC-3. Run the full e2e suite repeatedly under load with these scenarios enabled and retries disabled.

## Test plan

Use held mock requests and browser events to force the relevant ordering. Show the regression on the old code, repeat on the fix, and run the full suite with its configured budgets. Record load, skips and unrelated failures.

## Decisions

Compare current behavior with the historical revisions before changing tests. Preserve the scope of the five reported scenarios. A task-page hold, file join hold and write hold exercise different boundaries and can be released independently of host speed.
