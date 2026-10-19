---
yulius-task: e4bc884f-4d1b-41ad-be89-2bf0f8142478
status: done
---

# SPEC-234: The supertask card closes when its "Run after" task leaves the list

Status: done
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
- [x] Review round 1: focus finding fixed test-first, pre-existing finding commented on #587
- [x] Docs: spec evidence and changes.md entry; gates and security scan run
- [x] Review round 2 approves the head
- [x] Delivered: PR merged into dev on green CI, #587 summarised

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

## Evidence

All runs on 2026-10-19 under host load averages of 165-195.

- Red before the fix: `npx e2e run tests/intake.e2e.ts` at d04f5dcf failed the new test only
  (`getByRole("region", name: "Supertask proposal")` expected count 0, observed 1; the start chip
  already read "Start now"). After the focus assertion was added (7a3c39eb), it failed on
  `expect(prompt).toBeFocused()` (observed default states) until the focus fix.
- Green after the fix: `npx e2e run tests/intake.e2e.ts tests/supertask.e2e.ts tests/drafts.e2e.ts
  tests/new-task.e2e.ts` at 52c23dd2: 36/36 passed.
- Full e2e suite (`npx e2e run`) at d1a28400 (the first fix): 624 passed, 1 skipped, 1 failed:
  `pause.e2e.ts` "the Tasks list row menu pauses and resumes a task" (row action not visible
  within 29 s). This change does not touch that code. Run again on its own, it passed (4/4). It is
  a host-load flake (#354), reported on #587, and is not counted as green.
- `scripts/dev ci` at 52c23dd2: doctor, check, lint and macos passed. In test, nextest ran 2258
  tests: 2257 passed, 17 skipped, and 1 failed:
  `task_done::a_switch_waits_for_a_background_agent_launched_in_the_turn` timed out after 60 s.
  This is the flake of #464 (closed) coming back, commented there, and the change touches no Rust.
  Run alone right after, it passed in 22.4 s. The rest of the gate was then run step by step and
  passed: `scripts/doctests.py`, `scripts/dev deps` (deny and shear) and `cargo doc --workspace
  --all-features --no-deps`.
- Security: `python3 scripts/vidoc_scan.py --base origin/dev` (the scan `scripts/dev security`
  runs) at 52c23dd2 exited 0. There was no critical or high finding and no medium finding in the
  change. Two medium findings were in lines this branch did not touch (`taskview.js:340` and
  `:1011`, links without scheme checks); Yulius CI files those itself.
- Independent review in a fresh session: round 1 asked for changes. Its own-code finding was that
  focus was lost when the card closed, fixed test-first. Its pre-existing finding (a start choice
  picked during the intake check) is commented on #587. Round 2 approved 52c23dd2.
- Not tested: the deletion path itself (`task.removed`). It reaches `renderChips()` through the
  same `tasks` store event as the archive the test uses.
- The Delivered item is checked before the merge because the plan has to be complete for
  `delivery_merge`. The merge and Yulius CI decide whether it holds.
