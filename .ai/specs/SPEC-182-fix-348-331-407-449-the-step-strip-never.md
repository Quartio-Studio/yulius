---
yulius-task: b559732f-0ea5-4053-b36a-cb4e9d9ad43a
status: done
---

# SPEC-182: The step strip never overflows, focus survives a hidden Archive, and the last loading states take their final shape

Status: done
Date: 2026-10-15
Owner: Yulius task #138, issues #348, #331, #407, #449 (part of task #118, SPEC-138)
Scope authorization: task #138: "Fix GitHub issues #348, #331, #407 and #449 of
Quartio-Studio/yulius […] All are web UI layout and loading states".

## Goal

Four web UI defects are gone:

- The task step strip never passes its edge, at any width, and its Task actions menu stays
  visible and clickable. The lowest-priority parts give way first, as whole chips (#348).
- Keyboard focus on a row's quick Archive button that becomes hidden moves to that row's link,
  never to the page body (#331).
- The automation editor's start chip loads as a skeleton bar, not "…", and its Supertask tooltip
  uses New task's wording (#407).
- The remaining loading states that are not plain text load in the shape of their final layout
  (#449).

## Context

Checked on `dev` (5599635c) and on #137's branch, which this task starts from: all four are open.

- **#348.** `.stepstrip-main` (`taskview.js`) is one flex row: the toggle (step label; `flex: 1`,
  `min-width: 8rem` while the count shows, SPEC-073), the progress bar, the state pill (at most
  40%, shrinks to its icon), `.strip-extras` (Supertask, Auto or Goal, the chain chips "After #N" /
  "When …" and "Then #N"), `.strip-delivery` (the pull request chip) and the Task actions button.
  Chips do not shrink, so with enough of them the row's minimum width passes the strip's. Two
  container queries hide all chips at 560 px and the count and state words at 420 px, keyed on the
  width alone. Measured in the issue: beside the Shell panel at 1480 px the row overflows by about
  67 px and the menu slides under the panel; with the panel closed, Auto, After and two Then chips
  make the page scroll sideways at 980–990 px; beside Shell at 770–960 px windows the strip is
  110–300 px wide and its parts pass the edge by up to about 194 px (the bar, the state icon, the
  pull request chip and the menu do not fit).
- **#331.** `paintRow` (`layout.js`) hides `row.archive` when the task stops being archivable (it
  is resumed). A focused element that becomes `display: none` loses focus to the body; when the
  list is also rebuilt, `renderTasks` tries to `focus()` the hidden button, which does nothing.
  `archive.e2e.ts`'s last case only checks that focus is somewhere in Done and picks its button
  with `/Archive task #(13|14)/` and `.first()`.
- **#407.** `automationedit.js` `renderStart()` shows an `aria-hidden` "…" while the automation is
  read. ADR-0073/0097: loading shows only shimmering bars. Its Supertask tooltip says "…then runs
  alone: the project’s other tasks wait for it.", New task's (`SUPERTASK_HELP`, SPEC-094) "…then runs
  alone while the project’s other tasks wait."
- **#449.** SPEC-120 / ADR-0097 shaped the Git page, `loader()` pages, editors and task lists.
  Still drawn as three generic `lines`: the New sandbox dialog's repository and template lists
  (`actions.js`), the delivery dialog (`actions.js`) and the CI run dialog (`pages/ci.js`), a
  task's Git changes and commits (`pages/gittask.js`), the task overview (`pages/taskinfo.js`), the
  repository hygiene card (`pages/ci.js`) and the GitHub connection card (`pages/admin.js`).

## Requirements

- **STS-001 (#348).** No part of `.stepstrip-main` passes the strip's right edge at any width, and
  the page never scrolls sideways because of it. Parts give way whole (hidden, never cut
  mid-text), lowest priority first: the chain chips (the last "Then #N" first, "After #N" or
  "When …" last among them), then Auto (or Goal), then Supertask, then the state's words
  (SPEC-073's shrink, down to its icon), then the progress bar, then the pull request chip. The
  toggle (chevron and label), the state icon (the live status) and Task actions always stay.
- **STS-002 (#348).** A part that gave way comes back as soon as it fits again: on a wider strip,
  or when the strip's content changes. A hidden part leaves the tab order and the accessibility
  tree, as the container queries' hidden chips do.
- **STS-003 (#348).** The Task actions menu stays inside the strip and clickable beside a panel at
  1480–1560 px windows and at 770–960 px windows.
- **SAR-001 (#331).** When a row's quick Archive button has the keyboard and becomes hidden (its
  task is no longer archivable while its row stays listed), the keyboard moves to that row's link.
- **SAR-002 (#331).** The archive e2e focus check names `Archive task #14` and asserts that the
  focused element is a `.task-link` in Done.
- **AST-001 (#407).** While the automation is read, the start chip's visible label is an
  `aria-hidden` skeleton bar, no "…"; the chip is named "Start" and disabled.
- **AST-002 (#407).** The editor's Supertask tooltip reads "Each task waits for this project’s
  running tasks, then runs alone while the project’s other tasks wait.", New task's wording.
- **LSH-001 (#449).** Each view listed in Context loads in a shape built from its final layout's
  own classes, composed with `bar()` and `skeletonOf()` next to its renderer or shared from
  `dom.js` `SHAPES`: rows (`.list-item`) for the dialog lists, a stage track and section cards for
  the delivery dialog, a head, job tabs and an output box for the CI run, the toolbar, file tree
  and file cards for the changes, commit rows for the commits, fact tiles and cards for the task
  overview, and each card's own sections for the hygiene and GitHub connection cards.
- **LSH-002 (#449).** Each keeps SPEC-092's contract: one `role="status"` naming what loads, the
  drawing `aria-hidden`, nothing focusable inside, no axe violations.

## Plan

- [x] Spec SPEC-182 written and validated
- [x] Sidebar focus moves to the row's link when its quick Archive hides (#331)
- [x] Automation editor's loading start chip is a skeleton bar with New task's Supertask wording (#407)
- [x] Step strip gives way whole chips by priority and never passes its edge (#348)
- [x] New sandbox lists, delivery and CI run dialogs load in their final shape (#449)
- [x] Task changes, commits and overview load in their final shape (#449)
- [x] CI hygiene and GitHub connection cards load in their final shape (#449)
- [x] Docs, decision record, changes and every gate green
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Pull request merged into dev on green CI (armed with delivery_merge), issues closed

## Acceptance criteria

- `responsive.e2e.ts`: a task with Auto, an "After #12" and two "Then #N"
  chips beside the Shell panel at 1480–1560 px windows and at 770–960 px windows has no strip
  part past the strip's edge, no overlap, no sideways scroll; the chips shown are a prefix of the
  priority order (whole, never spilled), and Task actions opens its menu by a click. Fails on the
  branch before the fix. The rest of `responsive.e2e.ts` stays green.
- `archive.e2e.ts`: focusing a stopped row's quick Archive and resuming the task (mock) leaves focus
  on that row's link; fails before the fix. The rebuild case names `Archive task #14` and expects a
  `.task-link` in Done.
- `automations.e2e.ts`: with `&hold=/api/sandboxes/sb_studio/automations` the chip shows an
  `aria-hidden` skeleton bar and no "…", named "Start", disabled; the Supertask tooltip equals the
  sentence in AST-002.
- `loading-shapes.e2e.ts`: each view in LSH-001 is held with the mock's `hold=` and its shape
  compared with the loaded content (x and width within 2 px; y where nothing that comes with the
  read lands above it), with `role="status"`, nothing focusable and no axe violations.
- `scripts/dev ci` and the full e2e suite pass.

## Test plan

- e2e, test-first: each new case is run before the fix and fails for the reason named above.
- The strip test injects the chips through the mock (`touchTask` with `auto`, `afterLink`,
  `nextLinks`) on task #13, which has its pull request chip, and sweeps the window widths. It waits
  on conditions (`expect.poll`), never on sleeps.
- Loading shapes use the mock's `hold=<path>` and `release()`.
- Gates: `scripts/dev ci`, `scripts/dev security` (unrun if Vidoc cannot be reached, ADR-0104),
  `cd e2e && npx e2e run`.

## Decisions

- **The strip fits itself in the page, by priority, rather than by more container queries
  (#348).** After each paint of the strip and on every size change of the row (a `ResizeObserver`),
  `fitStrip()` shows every part, then hides parts in priority order while the row's content is
  wider than the row. While chips decide, the state pill keeps its words (`flex-shrink: 0` under a
  measuring class), so chips go before the state's words; then the pill may shrink as before.
  Hidden parts take a class (`strip-gone`, `display: none`), so `hidden` stays the parts' own
  (the bar's `!total`). Alternatives: more container query breakpoints guess at widths that
  depend on what the strip holds (the cause of #341 and of this issue); a wrapping chip row clipped
  by `overflow: hidden` drops whole chips in CSS alone, but the clipped links stay focusable and
  read, and the order between chips and the state's words cannot be set. Recorded as
  [ADR-0147](../../docs/decisions/0147-the-step-strip-fits-itself-by-priority.md). Measured with
  the fit: beside the Shell, a 1800 px window drops one "Then", 1700 px two, 1600 px keeps only
  Auto, 1480–1560 px keep no chip (Auto does not fit beside the label's 8 rem floor), and a 110 px
  strip keeps only the label, the state's icon and Task actions.
- **The strip also refits when a web font finishes loading (review round 1).** A font that swaps
  in changes the parts' widths but not the row's, so neither a paint nor the `ResizeObserver`
  would refit a quiet task. `document.fonts` `loadingdone` calls `fitStrip()`. It has no e2e test:
  the mock announces task updates continually and each repaints and refits the strip, so a test
  passes with or without the listener (tried in review round 1, with the chip grown just past the
  label's slack).
- **The 560 px query reads `.stepstrip .strip-extras` (review round 1).** Written as
  `.strip-extras` it lost to the later base rule, as the bar's width did, and a chip that fit
  showed in a 421–560 px strip, taking the label's room.
- **The narrow strip's bar is 60 px, as SPEC-073 meant.** The 560 px query's `.step-progress {
  width: 60px }` never applied: the base rule comes later in `styles.css` with the same
  specificity, so the bar stayed 120 px in narrow strips. The query now reads
  `.stepstrip .step-progress`, as the 420 px query already does for the toggle.
- **The 560 px query hides `.strip-extras` itself, not each chip,** so an empty extras box takes
  no gap in the row.
- **#407 uses `bar()`, ADR-0097's skeleton bar, inside the `aria-hidden` label.** The issue names
  `skeleton-line`, SPEC-092's bar; ADR-0097 made `bar()` (`skeleton-bar`, one line tall) the bar
  for shaped content. A chip label is a line of text, so `bar()` keeps the chip's height and
  matches the rest of the page's loading states.
- **The task overview's figures guess a two-line sub-line.** In the six-column grid (173 px tiles at
  a 1440 px window) the figures' words wrap by content (loaded tiles are 140–156 px tall), which
  a shape cannot know. Its tiles draw a two-line sub-line, the common case, and the test compares
  the figures' x, y and width and the cards' x and width, not the cards' y (ADR-0097: shapes guess
  counts; the content keeps its columns).
- **A dialog's shapes are measured from the dialog body's top.** A centred dialog moves as its
  content grows, so its top edge says nothing about the shape.
- **#449 shapes are composed next to their renderers,** except the `.list-item` rows, shared
  through `SHAPES.rows` because the repository and template lists both use them. Controls in a
  final layout (buttons, radios, tabs) are drawn as non-focusable spans with the same classes or as
  skeleton tiles of their size, so the shape keeps their room without a tab stop.

## Evidence

Checked on `dev` (5599635c) and #137's head first: all four issues were open (Context).

Test-first, each new case run before its fix:

- `archive.e2e.ts` "the keyboard on a quick Archive that hides moves to its row's link": failed
  (`expect.toBeFocused failed` on the Working row's link), passes with the `paintRow` fix. The
  tightened rebuild case (`Archive task #14`, a `.task-link` in Done) passed on both.
- `automations.e2e.ts`: the hold case failed with `{"text":"Start…","bar":false}` and the Supertask
  tooltip with the old wording; both pass.
- `responsive.e2e.ts` "the step strip gives way whole chips…": failed with
  `{"overlaps":[],"sideways":true}` at a 990 px window (the issue's case), passes with `fitStrip()`.
  "a narrow strip beside a panel shows no chip even when one fits" (review round 1) failed with
  `["Then #14"]` shown in a 539 px strip, passes with `.stepstrip .strip-extras`.
- `loading-shapes.e2e.ts`: the three new cases failed on the missing shapes (`expected 0 to be
  greater than or equal to 3/4`, no `.ci-hygiene` in the status), pass with them.

Gates:

- `scripts/dev ci` at 96b1aacc (this branch with `origin/dev` merged, bab6ca14): exit 0,
  `1874 tests run: 1874 passed, 18 skipped`. The commit after it (ac9402e6) changes only
  `e2e/tests/responsive.e2e.ts`.
- `cd e2e && npx e2e run` at ac9402e6: exit 0, `Test Files 55 passed | 1 skipped (56)`, `Tests 536
  passed | 1 skipped (537)`.
- `scripts/dev security`: not run, exit 2, "Vidoc rejected the key (HTTP 401: Invalid or expired
  token)", tracked in #470; recorded as unrun per ADR-0104.
- After review round 2 approved f2279f00, `origin/dev` (68cd7a3d, #563 and #567) was merged at
  42a7fb22; its only conflict was three rows of `e2e/README.md`, none changed on both sides. At
  42a7fb22: `scripts/dev ci` exit 0, `1895 tests run: 1895 passed, 18 skipped`; the e2e files this
  task and dev's merge touch (archive, loading-shapes, responsive, automations, playground,
  original-prompt, git, ci, loading) exit 0, `Tests 87 passed (87)`.
- `spec_check`: valid.

Review round 1 (fresh session, head 86a05410): request changes. Fixed: the 560 px query lost to
the base rule (Major, test-first above); gate evidence (this section); `fits()` polled as a whole;
Supertask, the bar before the pull request chip and chips coming back on a content change are now
tested; a font that finishes loading refits the strip (no e2e, see Decisions); the radio comment.
