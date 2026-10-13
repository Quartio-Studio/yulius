# 0048. An armed merge, its decisions and its outcome are one value under one lock

## Status

Accepted. Specified in [SPEC-064](../../.ai/specs/SPEC-064-fix-248-249-255-251-yulius-wait-on-armed.md).
It refines SPEC-028 MSR-003 and SPEC-050 DLV-204/205.

## Context

`yulius wait` reports how a merge armed with `delivery_merge` ended. Per task, Yulius kept three
separate values: the armed merge, a counter of decisions in progress and the last outcome.
`merge_armed` raised the counter, took the merge out, wrote the outcome while deciding, put a kept
merge back and lowered the counter, and the wait read the three one at a time. Two defects
followed:

- a poll between "outcome written" and "counter lowered" read the merge as armed with its new
  outcome, so the wait missed the decision (#255);
- a merge armed while Yulius was still deciding the earlier one (GitHub slow) was overwritten by
  the earlier merge put back, or followed by the earlier outcome; the wait on the new head then
  ended with "dropped: you armed <old head>" (#248).

## Decision

- One `ArmedSlot` per task, behind one mutex, holds the armed merge, the decisions in progress, the
  last outcome and a generation raised by every arm and clear.
- A decision takes the merge out (it still reads as armed) and ends in one step: the merge kept, or
  its outcome recorded, and the counter lowered together.
- A decision of an older generation changes nothing when it ends: the newer arm (or an immediate
  merge) superseded it.
- The saved armed merges (DLV-205) are written while the slot is locked, so the file follows the
  slot's order of changes. The wait reads whether a merge is armed and the outcome in one read.

## Alternatives

- **Report `merge_armed && outcome.is_none()`** (#255's first suggestion). It closes the window,
  but a merge armed while an older one is decided would read as unarmed once the older outcome is
  written: it trades #255 for a #248 variant.
- **Keep the three values and order the writes carefully.** Any order leaves a window in which a
  poll reads a combination that never existed; only a single lock removes it.
- **Report a superseded decision to the agent.** The agent armed a newer head and was told the
  older one no longer counts by `delivery_merge`'s own answer; repeating it would end the wait on
  the new head with old news.

## Consequences

- `merge_armed_task` returns a verdict instead of writing the outcome; the slot applies it.
- The wait can no longer miss a decision or report another head's decision while a newer merge is
  armed. A decision that was superseded leaves no trace beyond its audit entry (merges only).
- In the rare case where the superseded decision did merge the pull request, the newer merge stays
  armed; the wait on it ends on the stage change ("Delivery moved: pr_open -> merged_integration"),
  and the merge stays armed until the task is deleted, or until a restart once the task no longer
  runs (as the earlier code also left it).
- A restore after a restart puts back a saved merge only while nothing was armed or cleared in
  the new process, so it never revives a merge armed and decided since the file was read.
