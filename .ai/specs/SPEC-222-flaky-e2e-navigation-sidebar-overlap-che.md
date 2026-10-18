---
yulius-task: 21ae408c-cc79-4cec-887f-1b5b2442664c
status: done
---

# SPEC-222: The sidebar's hover overlap checks run on a still list (#515)

Status: done
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
  - Tick 4 also bumps #12, "Checkout API" itself.
  - The browser's sign-in request, about 5 s in, moves #10 into "Needs you" (`status.js`
    `taskInfo`; `layout.js` repaints on `browser-action`).

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
- [x] Overlap checks wait for the hover state and poll the layout
- [x] Twenty runs in a row under a held 0.2 CPU share
- [x] Independent review answered, docs and changes entry
- [x] Gates green, PR merged into dev on green CI (merge armed on the reviewed head)

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
  a still page. The browser's sign-in request regroups the list too (#10 into "Needs you"), so
  `browserask=0` alone does not keep a hovered list still. `browserask=0` stays for the tests that
  only need the toast gone (`notes.e2e.ts`).
- **Fix the test and the mock, not the page.** The page is right for a moving pointer.
- No ADR. This applies the existing rule (wait on conditions on a page that only the test
  changes), as `browserask=0` did (SPEC-148 E2L-006).
- **Out of scope.** `queued-messages.e2e.ts` says its tests use the chained product #7 to stay
  clear of the same ambient tick. Other files that hover sidebar rows may race it the same way.
  None is reported failing. `ambient=0` is now available to them.

## Evidence

All runs were on `feat/208` at dev a3b9b04a, in a sandbox with 12 cores. A load controller held a
CPU-bound probe at 0.2 of a CPU: it added or removed busy loops (`sh -c 'while :; do :; done'`)
every ~20 s from the median of three 2 s probes.

- **Unchanged test under load, before the fix.** `npx e2e run tests/navigation.e2e.ts --grep
  "task number apart"` passed 31 of 31 runs, at 23.8-42.9 s per run against 7.7 s at rest. The
  CI failure is rare, so a plain loop does not show it.
- **Mechanism probe (throwaway file, removed).** It hovered "Improve the code" and "Checkout API"
  in turn until 24 s after load and read `row.matches(':hover')` right after each `hover()`:
  - Ambient on: 4 of 8 page loads missed once each, at 14132, 13692, 13809 and 14490 ms. Each miss
    was `{"name":"Checkout API","hover":false,"on":"Nightly feedback digest"}`, with the link's
    padding at 12 px.
  - `ambient=0`: 0 misses in 8 page loads (114 hovers).
  - An earlier probe polled `:hover` on a miss: it was still `false` after 5 s
    (`expect.poll ... timed out after 5000 ms`).
- **Probes that did not reproduce.** These were not committed. Holding the unchanged test after its
  264 px hover until the tick had started #8 passed 3 of 3: `:hover` stays on the element that
  moved. A DOM swap timed 0, 10, 20, 30, 45, 60 or 90 ms into `hover()` passed 7 of 7. A patched
  `elementsFromPoint` passed 3 of 3: Playwright's hit check runs in its own world.
- **After the fix, 20 runs in a row under load.** Each ran
  `npx e2e run tests/navigation.e2e.ts --grep "task number apart"`: 20 of 20 passed, at
  24.0-39.4 s per run. Between 10:23:38 and 10:39:04 the controller logged 51 samples: mean share
  0.203 (min 0.100, max 0.370), with 39 loops on average. No retries (`retries: 0`) and no sleeps.
- **Mutation.** With the hover padding rule removed from `styles.css`, the case fails at
  `navigation.e2e.ts:309` with `expect.poll(...).toBeLessThanOrEqual(...) timed out after 5000 ms`,
  `last: expected 18 to be less than or equal to 0`. That is the CI signature, and the poll does
  not hide it.
- **Full e2e suite, without load.** `E2E_TELEMETRY_DISABLED=1 npx e2e run`: 60 files passed and 1
  skipped. 617 tests passed and 1 was skipped (the agent step without a model), in 18 min 49 s.
- **Gate.** `python3 scripts/bootstrap.py && scripts/dev ci` exited 0.
- **Security.** `scripts/dev security` ran Vidoc on vidoc-1 over 5 changed files (1 sent):
  "no open critical or high finding, and no open medium one in the change".
- **Not tested.** Yulius CI's own load, before the PR's CI run. The hover miss under the real
  ambient tick is statistical (about 1 in 2 page loads that hover through the tick window), not a
  deterministic regression test.
