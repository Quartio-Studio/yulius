---
yulius-task: 6f00a08c-5c27-43a1-a38e-567ae9caaad2
status: in-progress
---

# SPEC-120: loading fields take the shape and layout of what loads

Status: in progress
Date: 2026-10-14
Owner: Yulius task #106
Scope authorization: task request: "Floating loaders should have the shape and layout of the final
layout", with a screenshot of the Git page loading as three grey bars under its header.

## Goal

While content loads, the shimmering fields (SPEC-092) are drawn in the shape of the content that
replaces them. A page of cards loads as cards, a table as a table with its columns, a task list as
task rows and the Git page as its two status cards, its sections and its board. When the content
arrives it lands where the fields were, without the page jumping.

## Context

- SPEC-092 (task #77, ADR-0073) made every loading state the skeleton: `skeleton(label, lines)` in
  `crates/yulius-server/web/assets/dom.js`, three bars 72 %, 54 % and 36 % wide in a box at most
  640 px wide, announced as `label` through `role="status"`. `placeholder(text, 'loading')` and
  `loader()` in `pages/common.js` return it. ADR-0073 rejected per-page shapes ("drifts with the
  page"); this request overrules that choice, so a new ADR supersedes that part of it.
- The screenshot is the Git page (`pages/git.js`): it loads as three bars, then shows a strip of two
  `.dl-card`s (Live in production, Waiting to go live), a Blocked section (`.dl-needs` rows), the
  board (`.dl-board` columns of `.dl-card-task`s) and Activity (`.dl-activity` rows).
- Most `loader()` pages render a list of `itemCard`s (`.item-list` > `.item-card`): Workflows,
  Skills, MCP, Automations, the team and agent secrets, CI runs. Users and archived tasks are a
  `dataTable` (`.table-wrap` > `table.table`); Subscriptions is a `.grid-2` of cards. The
  editor pages (skill, workflow, MCP server, automation) show a form; the runs dialogs a table.
- The sidebar task list renders `.task-group`s of 36 px `.task-row`s under a `.task-group-head`
  band (`layout.js`); the Tasks page renders `.tl-row`s 52 px high (`pages/tasks.js`).
- Tests select a few real layout classes (`.task-list .task-row`, `.task-list .task-group`), so a
  skeleton must not add elements that a test or a keyboard handler would take for real content.

## Requirements

| ID | Requirement | Verification |
| --- | --- | --- |
| SHP-001 | `dom.js` offers loading shapes built from the page's own layout classes, so their geometry follows the final layout's CSS: `lines` (today's bars), `items` and `item-section` (item cards: icon tile, name, description, meta; with a section heading), `table` (header and rows of a given column count), `cards` (a grid of cards with a title and lines), `form` (the editors' titled section cards around a field) and `editor` (those beside the summary card). A page with a layout of its own passes a function that draws it with `bar()`. A bar is a percentage of its line or a length from a CSS scale, never a fixed inline length. Every shape keeps SPEC-092's contract: `role="status"` with the label for assistive technology only, the shape itself `aria-hidden`, nothing focusable; an unknown shape name draws lines. | e2e `loading-shapes.e2e.ts` |
| SHP-002 | `placeholder(text, 'loading', shape)` and `loader(container, what, fetcher, render, { shape })` take the shape; every `loader()` page and editor passes the shape of what it renders: item cards (Workflows, Skills, MCP, Automations, secrets), a table (Users, archived tasks, runs), a card grid (Subscriptions), editor sections (workflow, automation and MCP server editors), and their own layouts for Sessions, CI and the Dashboard. A card's height follows its content, so it matches a loaded one's within a fifth; left edge and width match within 2 px. | e2e: held Workflows, Users, Subscriptions, workflow editor, Sessions, CI, Dashboard |
| SHP-003 | The Git page loads as its final layout: the two-card strip, the Blocked and board sections with their headings, board columns (the profile's) with task cards, and Activity rows. The strip's cards sit where the loaded ones do (same top, left and width, ±2 px). A note or banner that comes with the read (the App permission note) is drawn above them and pushes them down; that is the one move left. | e2e: held Git page with the note dismissed |
| SHP-004 | The sidebar task list loads as a group band and task rows of the loaded rows' height; the Tasks page keeps its toolbar (tab names, chip outlines, summary) and column names in place and loads rows of the loaded `.tl-row` height, two lines in a narrow list as the real rows. Skeleton rows use skeleton-only classes, so `.task-row` and `.tl-row` match real tasks only. | e2e: held task lists, also at 390 px |
| SHP-005 | Under reduced motion the shapes do not shimmer (the global rule covers the bars' pseudo-element and every shimmering box); the loading pages have no WCAG violation, in both themes. | e2e: the reduced-motion rule's selectors, axe in the light theme |

## Plan

- [ ] Read the product context and related work, then write and validate the spec
- [ ] Loading shapes for item cards, tables, card grids and forms, used by every loader page and editor
- [ ] The Git page loads as its strip, sections and board
- [ ] Task lists in the sidebar and on Tasks load as task rows
- [ ] ADR, design page and UI docs describe shaped loading
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- With the Git read held, `/s/studio/git` shows two card outlines side by side, then section
  headings, board columns with card outlines and activity rows. After release, the two status
  cards appear where the outlines were.
- A held Workflows page shows item-card outlines that the loaded cards replace in place.
- The held sidebar and Tasks lists show rows of the real rows' height, and `.task-row` counts only
  real tasks.
- Every held page has `role="status"` naming what loads, no visible "Loading" text and no axe
  violation.

## Test plan

- `e2e/tests/loading-shapes.e2e.ts`, written first and failing on today's UI:
  - Git page held: the skeleton contains `.dl-strip` with two `.dl-card`s, `.dl-section` headings,
    a `.dl-board` and activity rows. The rectangles of the two strip cards are recorded and
    compared with the loaded cards' rectangles after release.
  - Workflows held through `loader()`: an `.item-list` of `.item-card`s inside the skeleton; the
    first card's rectangle matches the loaded first card's.
  - Users held: a table with the loaded table's column count.
  - Sidebar and Tasks held: skeleton rows of the loaded row height; `.task-list .task-row` count 0
    while held.
  - Review round 1 added: the icon tile shimmers; the workflow editor's sections and summary,
    Sessions rows, CI's configuration card and runs and the Dashboard's key figures land where
    their outlines were; narrow Tasks rows (390 px) are as tall as real ones; the reduced-motion
    rule covers the bars; axe passes in the light theme. Round 2 added the Dashboard's
    Subscriptions card and CI's configuration card top edges.
  - Each keeps the SPEC-092 checks: `role="status"` label, no visible "Loading", axe clean.
- `loading.e2e.ts` (SPEC-092) now counts `.skeleton-bar`s on the Git page.
- Gates: `scripts/dev ci` and the full e2e suite.

## Decisions

- Shapes reuse the final layout's classes (`.item-card`, `.dl-card`, `.table`, `.card`) instead of
  copying their sizes into skeleton CSS. A change to the page's layout reshapes its loader with it,
  which answers ADR-0073's drift concern. Where a real class is a hook for tests or keyboard
  handling (`.task-row`, `.tl-row`), the skeleton uses its own class with the same geometry rules.
- A small shape vocabulary in `dom.js`, not one skeleton per page: most pages share item cards,
  tables or forms. Only the Git page, whose layout is its own, composes its skeleton in `git.js`
  from its own classes.
- Detail views whose final content is text (a prompt, a transcript, a CI run's log, a task's
  overview, the delivery dialog) keep the `lines` shape. Text lines are their shape. Shaping each of
  them separately is follow-up work if it is wanted. The non-text ones left as lines (the new
  sandbox dialog's lists, the delivery and CI run dialogs, a task's changes and commits, the task
  overview, the hygiene and GitHub cards) are #449.
- The Tasks toolbar always draws two chip outlines (Author, Origin): whether the real chips show
  depends on the tasks (two or more authors or origins). A team sandbox usually has both; a
  one-person sandbox sees the summary move up a line when they do not.
- The Dashboard draws what the page usually shows: the Subscriptions card under the key figures
  (it shows whenever the capacity is read) and a note line under Tokens (what came from the
  cache). Without them the cards below move by that card or that line, the one move left.
- Parts of a page that do not depend on the read are drawn at once: the Tasks column names and
  toolbar (tab names, chip outlines, a summary bar), and the Subscriptions capacity panel, now
  read beside the subscriptions instead of after them, so it does not push them down when it
  lands.
