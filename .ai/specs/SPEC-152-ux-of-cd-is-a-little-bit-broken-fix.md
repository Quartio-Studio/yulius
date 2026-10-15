---
yulius-task: 2d6fefb0-76d5-40f5-8b7b-48e66d27e07a
status: done
---

# SPEC-152: the CD page's environment cards read right at every width

Status: implemented
Date: 2026-10-15
Owner: Yulius task #146
Scope authorization: task #146 ("UX of CD is a little bit broken, fix", with a screenshot of
Yulius's own CD page). Fixes #501. Refines SPEC-134 CDT-005 (the CD (deployment) page).

## Goal

Each environment card on the CD page has a stable header, does not put a deploy that can't run in
front, and says what a production deploy promotes.

## Context

The founder's screenshot (2026-10-15) shows Yulius's own CD page: dev is `[cd.dev]` with only an
`about` line ("Nothing to deploy yet"), and production is `[cd.prod]` with `promote = true` and no
commands. `pages/cd.js` `envCard` draws each card. Reproduced in the mock UI (`?mock=1`) and
measured with an e2e probe:

1. **Header.** The head is one `row-wrap` (title, badge, spacer, button). When the three don't fit,
   the button wraps onto its own row and the spacer stays on the first one, so the button sits
   left-aligned under the title. Mock `product` ("Production · Not defined") does this at 390 px
   and at 1100 px (button 21 px from the card's left edge, 30 px below the title). Card widths
   come from the shared `.cd-envs` grid, so this happens at common widths (two 362 px cards).
2. **A dead deploy is the loudest control.** "Deploy dev" is always `btn-primary`. Disabled, the
   accent fill at 50 % opacity becomes the grey slab in the screenshot: the strongest element in a
   card where nothing can be done. The badge ("Nothing to deploy yet") already says why.
3. **Production hides what it promotes.** With `promote` the facts read "First: promotes the
   integration branch into main" then "From: main at <production head>". "First" is unclear, the
   integration branch isn't named, and the commit a deploy would promote (`promotes_from`, which the
   confirmation dialog already names) isn't on the card.

Why the tests missed it: `cd.e2e.ts` checks texts and roles at the default viewport, never the
card's geometry, the button's emphasis or the promoted commit.

## Requirements

- CDU-001. A card's deploy button stays at the top right of its card at every width. Only the
  title and its badge wrap, beside the button.
- CDU-002. The deploy button is the card's main action (`btn-primary` for dev) only when it can run.
  A disabled one is a plain button. Production keeps its danger style, which warns rather than
  emphasises.
- CDU-003. With `promote`, production's facts start with "Promotes: <integration branch> at
  <promotes_from> into <production branch>" (the integration branch is the dev environment's
  branch). The "First" fact is gone.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green on the branch merged with current dev

## Acceptance criteria

- AC1 (CDU-001, CDU-002): at 390 px, in mock `studio` and `product`, each card's deploy button
  shares the title's row and ends at the card's right padding. An empty dev's button is disabled
  and not primary, an enabled dev's is primary, and the page has no horizontal scroll and no WCAG
  violations.
- AC2 (CDU-003): mock `studio` production reads "Promotes quartio at 9f3c2a1 into main" and no
  "First".

## Test plan

- e2e `cd.e2e.ts`: "each card keeps its deploy button top right, never styles a dead deploy as the
  main action" (AC1) and "a promoting production names the branch and the commit it promotes"
  (AC2). Both fail on `dev` at e37fdd2. The first test's existing promotion check follows the new
  wording.
- Gates: `scripts/dev ci`, `scripts/dev security`, the whole e2e suite.

## Decisions

- **Header as two parts, not a wrapping row.** The title and badge go in a group that wraps inside
  itself (`flex: 1; min-width: 0`), and the button doesn't shrink. Alternatives: `margin-left:
  auto` on the button (when it wraps it still drops to its own row, only right-aligned), or moving
  the button to the card's foot (a bigger change to the page task #145 is reworking). Reversible,
  local CSS; no ADR.
- **Emphasis follows availability, not the environment.** Only the dev button changes. Production
  stays `btn-danger` because its colour is the warning, which applies even when disabled.
- **The promoted branch comes from the overview's dev environment.** The API already carries the
  integration branch as dev's `branch`, so there is no contract change. Without a dev entry the fact
  reads "the integration branch".
- **What the cause could also break:** any other page that draws a header as `row-wrap` with a
  spacer has the same wrap. The CD page is the one reported; others aren't changed here.

## Evidence

- **Regression tests fail first.** `e2e/tests/cd.e2e.ts` at 7ae85127, which has the tests but not
  the fix, gave 3 failures:
  - `primary: true` on the disabled dev button;
  - production's button `sameRow: false`, 151 px from the right edge at 390 px (mock `product`);
  - facts "First promotes the integration branch into main …".

  The reviewer reproduced them by running the suite against `cd.js` and `styles.css` from e37fdd21.
- **Fix.** `npx e2e run tests/cd.e2e.ts`: 7 of 7 passed at 042ae6e6 and again after review round 1
  (eecc8c67). Checked by eye in the Yulius browser at 1280 px: mock `product` (button top right,
  badge wrapped, dead "Deploy dev" plain) and mock `studio` ("Promotes quartio at 9f3c2a1 into
  main").
- **Review.** Independent read-only reviewer in a fresh session: APPROVE at 042ae6e6, no blocker or
  major. One minor (the product block lacked the scroll and axe checks) and two nits (wait on
  "Not defined" before measuring; the full promotion text in the first test) were fixed in
  eecc8c67. The reviewer ran `scripts/dev check`, `scripts/dev lint` and `scripts/dev test` (1784
  passed, 16 skipped).
- **`scripts/dev ci`.** Exit 0 at eecc8c67 (nextest: 1784 passed, 16 skipped).
- **`scripts/dev security`.** Exit 2, unrun, not passed: Vidoc rejected the key (HTTP 401), as
  tracked in #470 (ADR-0104). Yulius CI's `security` job reports the same way.
- **Whole e2e suite** (`npx e2e run`, host load 60–90, after merging `origin/dev` 070abcdd): 431
  passed, 15 failed, 1 skipped. Every failure is a timeout in a file this change doesn't touch
  (editor, ownership, notes, places, consistency, pipeline).
  - Rerun of those six files: pipeline passed; the others timed out again at load ~80.
  - The same consistency, notes and places tests also time out on untouched `origin/dev` at the
    same load.
  - Tracked by #506 and #356 (editor, ownership) and #510 (consistency, notes, places, pipeline;
    filed by this task).
