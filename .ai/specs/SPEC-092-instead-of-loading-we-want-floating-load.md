---
yulius-task: c6939128-3adf-4c21-8aa2-59c06780af15
status: in-progress
---

# SPEC-092: loading shows shimmering fields everywhere, not "Loading…"

Status: in progress
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
  home cards' activity and the browser panel's page load.
- Spinners also mark things that are not content loading: an action under way (an update, "Asking
  Jev one small question…"), a terminal connecting, a working task's icon. Each says what is
  happening and has no content shape to stand in for. They stay as they are (ADR-0073).
- `e2e/tests/consistency.e2e.ts` waits for the task list with `.placeholder:not(.placeholder-loading)`.
  It must keep waiting for real content once the loading state is a skeleton.

## Requirements

| ID | Requirement | Verification |
| --- | --- | --- |
| FLD-001 | `placeholder(text, 'loading')` renders the skeleton: shimmering bars announced as `text` (`role="status"`, the text only for screen readers), padded like the placeholder it replaces. Every caller gets it, `loader()` included. | e2e `loading.e2e.ts` (Git page held) |
| FLD-002 | Every hand-written loading placeholder (task lists in the sidebar and on Tasks, the status popover, Instance, the Jev key card, the Superagent's start) uses the skeleton. | e2e `loading.e2e.ts`; source search finds no `placeholder-loading` |
| FLD-003 | Inline loading text becomes a skeleton of the content's size: a folder loading in the editor tree is one bar at its depth; a home card's unknown activity is one short bar; a notepad not loaded yet shows bars over its empty text box and no "Loading…" status; the browser panel loading a page shows bars on its cover. | e2e `loading.e2e.ts`, `notes.e2e.ts` |
| FLD-004 | No visible "Loading…" text remains while content loads. Spinners for actions under way, connections and working tasks are unchanged. A button busy with its own action ("Load more") still says so on the button. | source search (Evidence); e2e |
| FLD-005 | Under `prefers-reduced-motion` the bars do not shimmer (the global rule); the loading pages have no WCAG violation. | e2e with axe |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] Loading placeholders shimmer as skeleton fields on every page, with the Git page test
- [ ] Hand-written loading placeholders become skeleton fields
- [ ] Inline loading text in the editor, notes, home cards and browser becomes skeleton fields
- [ ] ADR, design page and UI docs say skeleton for loading, spinner for actions
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

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
