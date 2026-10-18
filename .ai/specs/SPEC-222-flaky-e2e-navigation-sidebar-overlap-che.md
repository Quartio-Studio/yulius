---
yulius-task: 21ae408c-cc79-4cec-887f-1b5b2442664c
status: in-progress
---

# SPEC-222: The sidebar's hover overlap checks run on a still list (#515)

Status: in-progress
Date: 2026-10-18
Owner: Yulius task #208
Scope authorization: the person's request (task #208). Fix GitHub issue #515 in
`e2e/tests/navigation.e2e.ts` ("sidebar rows show the task number apart from the pull request
number", SPEC-122 review rounds 1 and 2). Create no new GitHub issue.

## Goal

The two hover checks of SPEC-122 assert the layout of the row they hovered, whatever the machine's
speed. The checks are: the title ends before Archive and the menu at 296 px, and before the menu
alone at 264 px. No row moves under the pointer while they run, and they read the row only once its
hover state shows.

## Context

- **Symptom.** CI run f56d3efa (PR #512 at 0c06427, a composer-only change) failed at the
  `covered` check with `expected 18 to be less than or equal to 0`. The "Checkout API" row's
  content ended 18 px past the left edge of its `.task-kebab` at a 264 px sidebar.
- **What 18 px is.** At rest, a row's `.task-link` has `padding-right: 12px`. The kebab sits at
  `right: 6px` and is `--control-sm` (24 px) wide, so its left edge is 30 px from the row's right
  edge. Content ending 12 px from the right overlaps it by exactly 18 px. Hovered, the link's
  padding grows by 24 px (`styles.css`, the `min-width: 768px` block), and the content ends before
  the kebab. So the failing read saw "Checkout API" **not hovered**.
- **Cause: the mock's ambient activity moves rows under the pointer.** `mock.js` `startAmbient`
  ticks every 4 s once the event stream is open:
  - Tick 3 (about 12 s after the stream opens) moves the queued task #8 "Nightly feedback digest"
    to starting.
  - Tick 4 moves #9 out of switching.
  - Tick 5 moves #8 to running.

  Each change regroups the sidebar (`layout.js` `renderTasks`), so rows shift by a row's height.
  `hover()` takes the target's centre, checks the hit target, then moves the mouse. If the list
  regroups between the hit check and the move, the pointer lands on the row that moved in. That
  row keeps `:hover` while the pointer stays still: Chromium does not move `:hover` until the
  mouse moves again.
  - At rest, the test takes about 7.7 s, so it never reaches the hovers near a tick.
  - Under load, it takes 24-43 s and reaches them about when the ticks fire.
- **Reproduced** under a held 0.2 CPU share (a controller adds or removes busy loops so that a 2 s
  CPU-bound probe gets 0.2 of a CPU). A throwaway probe hovered "Improve the code" and "Checkout
  API" in turn until 24 s after load, and read `:hover` right after each `hover()`:
  - With ambient activity, 4 of 8 page loads missed once each, all between 13.7 and 14.5 s. In
    every miss, "Checkout API" was not hovered and `:hover` was on "Nightly feedback digest".
  - With `ambient=0`, 8 of 8 page loads (114 hovers) had no miss.
  - On a miss, `:hover` stayed on the wrong row for a 5 s poll. So polling alone (the issue's
    suggestion) would not have fixed it.
- **Why the tests missed it.** The unchanged test passed 31 of 31 runs under the same share. Its
  hovers reach the tick window only on some runs, and only one hover in a few hundred lands
  between the hit check and the move. A timed DOM swap during `hover()` (0-90 ms) did not hit that
  window either, so the reproduction is statistical, not deterministic.
- **Not a product defect.** A person's pointer moves, so `:hover` follows the row under it. A
  regrouping under a pointer that stays still is what the static check cannot tolerate.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| HOV-001 | The mock takes `ambient=0`, which leaves out all of its ambient activity (task changes and the browser's sign-in request). It is documented with the other mock parameters. | `mock.js` `startAmbient`; `web/README.md`; A/B probe in Evidence |
| HOV-002 | "sidebar rows show the task number apart from the pull request number" opens its page with `ambient=0`, so no row moves under a hover. | `navigation.e2e.ts` |
| HOV-003 | Each hover check first waits until the hovered row's control shows (`.task-quick-archive` at 296 px, `.task-kebab` at 264 px). `toBeVisible` is false while the control is `visibility: hidden`. The check then polls the overlap (`expect.poll(...).toBeLessThanOrEqual(0)`) instead of reading it once. A row whose title really overlaps still fails. | same case; mutation run in Evidence |

## Plan

- [x] Spec with the reproduced cause of the 18 px overlap
- [ ] Overlap checks wait for the hover state and poll the layout
- [ ] Twenty runs in a row under a held 0.2 CPU share
- [ ] Independent review answered, docs and changes entry
- [ ] Gates green, PR merged into dev on green CI

## Acceptance criteria

- The case opens the mock with `ambient=0`. Both checks wait for the control to be visible and
  poll the overlap, with no single read left.
- `npx e2e run tests/navigation.e2e.ts --grep "task number apart"` passes 20 runs in a row under a
  held 0.2 CPU share, with no retries and no sleeps.
- With the hover padding removed (`styles.css`), the case still fails with `expected 18`: the
  poll does not hide a real overlap.

## Test plan

- Reproduction (kept out of the repository): the A/B hover probe above, with and without
  `ambient=0`, under a held 0.2 CPU share.
- After the fix, under the same share: 20 runs in a row, then the mutation run, then the whole
  `navigation.e2e.ts`.
- The full e2e suite, `scripts/dev ci` and `scripts/dev security`.

## Decisions

- **Stop the cause (a still list), and also wait for the hover.**
  - Polling alone, as the issue suggested, fails: the hover stays on the wrong row.
  - Hovering again until the control shows would work, but would hide a list that moves for
    other reasons.
  - With `ambient=0`, the list does not move, and the visibility wait says what the check waits
    for. The poll covers a layout pass that is still pending (`content-visibility: auto` rows,
    `@container tasks` rules).
- **`ambient=0` turns off all ambient activity, not only the task changes.** A layout check wants
  a still page. The browser's sign-in toast is ambient noise too, and `browserask=0` stays for the
  tests that want the task changes without the toast (`notes.e2e.ts`).
- **Fix the test and the mock, not the page.** The page is right for a moving pointer.
- No ADR. This applies the existing rule (wait on conditions on a page that only the test
  changes), as `browserask=0` did (SPEC-148 E2L-006).
- **Out of scope.** `queued-messages.e2e.ts` says its tests use the chained product #7 to stay
  clear of the same ambient tick. Other files that hover sidebar rows may race it the same way.
  None is reported failing. `ambient=0` is now available to them.

## Evidence
