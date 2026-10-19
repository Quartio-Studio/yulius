---
yulius-task: e4bc884f-4d1b-41ad-be89-2bf0f8142478
status: in-progress
---

# SPEC-234: The supertask card closes when its "Run after" task leaves the list

Status: in-progress
Date: 2026-10-19
Owner: Yulius task #220
Scope authorization: task #220, fixes #587 ("Supertask proposal card keeps "It replaces “After
#N”" when task #N leaves the list while the card is open"), a nit left by the review of SPEC-188.

## Goal

The supertask proposal card (SPEC-188) never names a start choice the composer no longer has: when
the task chosen in "Run after" leaves the sandbox's task list while the card is open, the card
closes, as it does when the person changes the start choice.

## Context

- Symptom: pick "Run after: #12", type broad work, press Start. The card says "It replaces “After
  #12”". Task #12 is then deleted or archived (by anyone). The start chip reads "Start now", but the
  card still says "It replaces “After #12”", and "Start as chosen" starts the task now.
- Root cause: `showSupertask()` (`crates/yulius-server/web/assets/taskview.js`) fixes the card's
  `replaces` text when it opens. The `tasks` store listener calls `renderChips()` while
  `afterTaskId` is set; `renderChips()` clears `afterTaskId` when the task is no longer listed but
  leaves the card open. Only `startChanged()`, run on the person's own choice, calls `clearSplit()`.
- Why tests missed it: the SPEC-188 tests cover the person's own changes (`startChanged()`), not a
  change of the task list under an open card.
- The split card (TSK-011) is not affected: `start()` opens it only when `afterTaskId` and
  `startRule` are empty after the intake check, and `renderChips()` drops only `afterTaskId` (it
  never drops a rule). Draft changes from another tab (`draftChanged()`) do not touch the start
  choice, and every choice the person makes goes through `startChanged()`, which closes either
  card. So no list change can leave a split card naming a stale choice.
- Seen on the way (review, pre-existing since #586, commented on #587, outside this change):
  `start()` reads `supertask` and `prepare` before the intake check, so a choice picked during the
  check is not honoured by a card opened after it.

## Requirements

- STC-001: when `renderChips()` drops a chosen "Run after" task because it left the list, an open
  supertask card closes; the prompt stays, and the next Start asks again with the current choice.
- STC-002: when the keyboard was in the card, it lands in the prompt (as `startChanged()` does), not
  on the page's body.

## Plan

- [x] Spec SPEC-234 written and validated; issue #587 told the plan
- [x] Regression e2e test: the supertask card closes when #12 leaves the list (fails before the fix)
- [x] renderChips() closes the card when it drops the chosen task; e2e suite green
- [ ] Docs: spec evidence and changes.md entry; gates and security scan run
- [ ] Independent review in a fresh session, findings answered
- [ ] Delivered: PR merged into dev on green CI, #587 summarised

## Acceptance criteria

- An e2e test opens the supertask card with "After #12", makes #12 leave the mock's task list (a
  `task.updated` event archiving it, which the store turns into a `tasks` event), and the card
  closes; the start chip reads "Start now", the prompt is kept and holds the keyboard. The test
  fails before the fix (first the card, then the focus).
- The split card case is shown unreachable (Context) and needs no test.

## Test plan

- `e2e/tests/intake.e2e.ts`: the new test beside the SPEC-188 test "changing the start choice
  closes the supertask proposal", run red before the fix and green after.
- The whole e2e suite and `scripts/dev ci`, `scripts/dev security`.

## Decisions

- Close the card rather than rewrite its text: the card answered a choice that no longer exists,
  and closing is what `startChanged()` already does for the person's own change (SPEC-188). No ADR:
  this follows an existing decision.
- Close in `renderChips()` where `afterTaskId` is dropped, not in the `tasks` listener, so every
  caller that drops the choice closes the card.
