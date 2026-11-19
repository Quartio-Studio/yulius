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
- [x] Preserve the asserted behavior across delayed updates
- [ ] Verify the full browser suite under load and deliver the reviewed fix

## Acceptance criteria

- AC-1. For each named scenario record its cause and evidence under a controlled delay, including the historical fixes.
- AC-2. The admin’s switch works across an intervening redraw and a delayed PATCH. The other sandbox keeps its value. The error path remains usable.
- AC-3. Run the full e2e suite repeatedly under load with these scenarios enabled and retries disabled.

## Test plan

Use held mock requests and browser events to force the relevant ordering. Show the regression on the old code, repeat on the fix, and run the full suite with its configured budgets. Record load, skips and unrelated failures.

## Decisions

Compare current behavior with the historical revisions before changing tests. Preserve the scope of the five reported scenarios. A task-page hold, file join hold and write hold exercise different boundaries and can be released independently of host speed.


## Evidence

Chromium ran against static copies of the mock UI at the original base (`911e3127`) and the task base (`8d05c25b`). Each probe released a request at a specified browser event. No test retry or elapsed delay controlled the ordering.

| Original scenario | Probe on November 19 and classification | Current behavior |
| --- | --- | --- |
| Unsaved README across sandboxes | Hold the first live join, focus the available textbox and type. Releasing the join replaced Monaco’s placeholder model, losing the text and leaving the tab clean. Product race. | With #1159 the held file has no textbox. Typing after the join marks the real tab modified. The full scenario checks sandbox isolation and restoration. |
| Notes focus | Hold studio’s task list, choose Notes, then release the list. Once task #12 appeared the Team notes box lacked focus. Early tool selection raced sandbox initialization. | #1150 waits for task #12 in this test. #1151 also honors early selection in the product. The held-read probe now gives Team notes focus. |
| Removed sandbox’s views | Choose Superagent during the same hold, release it and switch to product. The visible terminal reported the global cwd (`users/you/superagent/work`). Product context race exposed by early test setup. | With the same ordering the visible cwd belongs to product. The full scenario also removes studio’s view. |
| Shell focus | Choose Shell during the held studio task read. It reported “Open a sandbox first.” After release its terminal lacked focus. Same tool context race. | The held-read probe gives the chosen Shell focus. The full scenario also checks that the first visit to product leaves focus on the page. |
| Stored protection value | Hold the PATCH that switches studio back on. While the checkbox was checked, the mock still held false. Test race between optimistic UI and persistence. | Releasing the write stores true. #1150 polls this value and checks the other sandbox. |

The original CI heads preceded #1150 (`e2dd441d`, Nov 11 12:30 UTC). #1156’s green head `0387eb01` included that wait. #1159 (`882a2864`, 13:23 UTC) replaced the temporary editor text wait with the product fix. The pre-review run in [SPEC-461](SPEC-461-claude-code-in-sandboxes-always-knows-yu.md) belongs to a branch whose first reviewed head (`beb8fca9`) did not contain #1159. #1151 (`4ed1234f`, 15:05 UTC) fixed tool activation before sandbox initialization.

The later missing-toast report was on `3308cd33`, which contained #1150 and #1159. On current dev, holding the Git overview read, pressing Space down, releasing the read, waiting for Activity and then releasing Space left focus on the replacement switch and the stored value on. This reproduces the symptom independently of the unavailable CI trace. With the fix the same probe stored off.

The committed regression at `34dadb66a` forced the same overview render during a native Space press and failed at the original missing-toast assertion. The focused e2e selection passed all five named tests and the refused PATCH during a real card update (6/6). The unchanged input handles keyup, and a changed card still redraws.
