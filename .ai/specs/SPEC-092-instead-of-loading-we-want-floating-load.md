---
yulius-task: c6939128-3adf-4c21-8aa2-59c06780af15
status: done
---

# SPEC-092: loading shows shimmering fields everywhere, not "Loading…"

Status: implemented
Date: 2026-10-14
Owner: Yulius task #77
Scope authorization: task request: "Instead of loading we want floating loading fields in all
places", with a screenshot of the Git page showing a spinner and "Loading delivery…".

## Goal

Content that is still loading shows shimmering bars ("floating loading fields") where it will
appear, never a spinner with "Loading…" text. The same calm skeleton the composer, the split card
and the task view's steps already use applies to every page, panel, dialog and list. Screen readers
still hear what is loading.

## Context

- The skeleton exists: `skeleton(label, lines)` in `crates/yulius-server/web/assets/dom.js`
  (F 2026-10-10, "better loading (some animation)"). It draws shimmering bars and an `sr-only`
  label in a `role="status"` element. The web README's state patterns (SPEC-001 UI-050) already
  say "`skeleton` while loading".
- Few places use it. In this sandbox on 2026-10-14, `placeholder(text, 'loading')` (a spinner
  ring and the text) appeared 24 times. Among them were `loader()` in `pages/common.js`, the Git
  page in the screenshot, the delivery and prompt dialogs, Sessions, the skill, workflow, MCP,
  automation and task-info pages, CI runs and the boot screen. Seven more spots wrote the same
  markup by hand: the task lists, the status popover, Instance, the Jev key card and the
  Superagent's start. Four drew their own "Loading…" text: the editor tree, the notes status, the
  home cards' activity and the browser panel's page load; the status popover and Accounts said "Reading usage…" for usage not read yet, and the superadmin
  recovery card and the "Forgot password?" form said "Checking…" while they read the recovery state.
- Spinners also mark things that are not content loading: an action under way (an update, "Asking
  Jev one small question…"), a terminal connecting, a working task's icon. Each says what is
  happening and has no content shape to stand in for. They stay as they are (ADR-0073).
- `e2e/tests/consistency.e2e.ts` waits for the task list with `.placeholder:not(.placeholder-loading)`.
  It must keep waiting for real content once the loading state is a skeleton.

## Requirements

| ID | Requirement | Verification |
| --- | --- | --- |
| FLD-001 | `placeholder(text, 'loading')` renders the skeleton: shimmering bars announced as `text` (`role="status"`, the text only for screen readers), padded like the placeholder it replaces. Every caller gets it, `loader()` included. | e2e `loading.e2e.ts` (Git page held) |
| FLD-002 | Every hand-written loading placeholder (task lists in the sidebar and on Tasks, the status popover, Instance, the Jev key card, the Superagent's start, the recovery card and form) uses the skeleton, and each label names what loads (`loader()` says "Loading the workflows…" from its `what`). | e2e `loading.e2e.ts`; source search finds no `placeholder-loading` |
| FLD-003 | Inline loading text becomes a skeleton of the content's size: a folder loading in the editor tree is one bar at its depth; a home card's unknown activity is one short bar; a notepad not loaded yet shows bars over its empty text box and no "Loading…" status; the browser panel loading a page shows bars on its cover; a subscription whose usage is not read yet shows one bar where its reset time goes, not "Reading usage…". Where many such bars load at once (home cards) or the container allows only its own items (the editor tree), the bars are decoration and the container is `aria-busy`, so screen readers hear no burst of status messages. | e2e `loading.e2e.ts`, `notes.e2e.ts` |
| FLD-004 | No visible "Loading…" text remains while content loads. Spinners for actions under way, connections and working tasks are unchanged. A button busy with its own action ("Load more") still says so on the button. | source search (Evidence); e2e |
| FLD-005 | Under `prefers-reduced-motion` the bars do not shimmer (the global rule); the loading pages have no WCAG violation. | e2e with axe |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Loading placeholders shimmer as skeleton fields on every page, with the Git page test
- [x] Hand-written loading placeholders become skeleton fields
- [x] Inline loading text in the editor, notes, home cards and browser becomes skeleton fields
- [x] ADR, design page and UI docs say skeleton for loading, spinner for actions
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- With the Git read held, `/s/studio/git` shows shimmering bars below the header, a status that
  says "Loading delivery…" to assistive technology, and no visible "Loading" text and no spinner.
- The source has no `placeholder-loading` class and no visible "Loading…" string, apart from the
  Load more button and the skeletons' screen-reader labels.
- The task list consistency test still waits for real rows.
- `/design` shows the one loading state.

## Test plan

- New `e2e/tests/loading.e2e.ts`, written first and failing on today's UI:
  - Git page with `__yuliusHoldGit`: a `.skeleton` with `role=status` named "Loading delivery…",
    no visible text matching /Loading/, no `.placeholder-loading`, no axe violation; after release
    the delivery headings show and the skeleton is gone.
  - Editor tree, home cards, status popover, notes and browser: each held or slowed in the mock
    where it has a hook (one is added where a read cannot be held), checking for a skeleton and
    no visible "Loading".
- `consistency.e2e.ts` waits for `.task-list .task-group, .task-list .placeholder` (the skeleton
  is no longer a `.placeholder`).
- Gates: `scripts/dev ci` and the full e2e suite.

## Decisions

- One loading pattern: `placeholder(text, 'loading')` becomes the skeleton instead of every call
  site changing. The call sites keep their labels, now heard rather than seen. This is the
  smallest change that reaches every page, and a future `placeholder(…, 'loading')` gets the
  skeleton too.
- A spinner stays where nothing is drawn in place: an action under way, a connection, a working
  task. The bars stand in for content; an action has no content to stand in for (ADR-0073).
- Three bars by default, as the existing skeleton. Mimicking each page's exact layout would mean
  a skeleton per page, with drift risk and no gain for a load that takes well under a second.

## Evidence

- Red first: `npx e2e run tests/loading.e2e.ts` failed on the old UI (the Git page had no
  `.skeleton`; the browser test failed with the old overlay line restored). The reviewer ran
  the suite against the `origin/dev` UI with only the new `mock.js`: all 4 tests of the first
  round failed.
- `cd e2e && npx e2e run tests/loading.e2e.ts`: 5 passed. It covers the Git page held (the
  reported screen), the sidebar and Tasks lists, the status popover and its usage bar width, a
  home card under a busy grid, the editor tree, a notepad, the browser cover, the `loader()`
  label and the recovery card and form, each with axe where the page is complete.
- Full e2e suite at `934756ef`: 284 passed, 1 failed, 1 skipped. The failure,
  `terminal.e2e.ts`'s file-path test, was an uncaught Monaco error ("depends on UNKNOWN service
  ICodeLensCache") at load average 27 on 12 CPUs; alone it passed 4 runs in a row (12/12 each).
  Filed as #394, unrelated to this change.
- `scripts/dev ci` (after `python3 scripts/bootstrap.py`): exit 0, 1520 tests run, 1520 passed,
  16 skipped; cargo-deny and the repository check clean.
- Source search: `grep -rn "placeholder-loading"` in `crates/yulius-server/web/assets` finds
  nothing. `grep -rn "Loading"` outside `placeholder(`/`skeleton(` labels finds only comments and
  the Load more button.
- Independent review in a fresh session: round 1 approved `4d371055` with 6 minors and 4 nits,
  all fixed in `934756ef` (labels name what loads, recovery views, a busy home grid, the usage
  bar width, tree busy state, tests); round 2 approved `934756ef` with no findings.
