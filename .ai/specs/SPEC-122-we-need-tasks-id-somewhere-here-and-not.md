---
yulius-task: 32bdac19-661e-4de5-b0e1-d986fd927eac
status: in-progress
---

# SPEC-122: Sidebar rows show the task number, and pull request chips say "PR"

Status: in progress
Date: 2026-10-14
Owner: Yulius task #108
Scope authorization: task #108 ("We need tasks id somewhere here and not only PRs id because we
use them quickly. But it should be communicated what id is for what etc", with a screenshot of
the sidebar's Working and Done groups). Extends SPEC-001 UI-005 (the sidebar's active tasks) and
the pull request chip of SPEC-104.

## Goal

People can read a task's number in the sidebar without opening the task. They can also tell at
a glance which number is the task's and which is its pull request's.

## Context

**What the screenshot shows.** Each sidebar row (`layout.js` `rowFor`/`paintRow`) has a state
icon, a pull request chip when the task has one (`dom.js` `prChip`: branch icon, `#450`, checks
icon), the title and the time. The task's own number (task #108 is `#108`) appears only in the
row's tooltip. A row without a pull request shows no number, and a row with one shows a number
that is not the task's.

**Why the number matters.** People type task numbers all the time. "Run after" lists `#102`, a
chained task reads "Queued after #102", toasts say "Task #108 archived", the URL is `/t/108` and
agents quote them. Outside the sidebar the app already shows a task as a muted `#N` before its
title: the Git board (`git.js`), the Git review page (`gittask.js`), the task view's step strip
(`.step-task-num`), the automations list. Pull requests, however, are a bare `#N` in their
chips, so the one number the sidebar shows looks like a task number.

**Where pull request chips are.** `prChip` draws them in the sidebar, the Tasks page, the task
overview (`taskinfo.js`) and the Git review page. The task view's delivery strip draws its own
copy in `actions.js` `deliveryStrip`.

## Requirements

- TID-1: Every sidebar task row shows the task's number as `#N`, in the muted colour with
  tabular figures, between the state icon and the pull request chip (or the title, when there is
  no pull request). It shows whatever the task's state and in every group.
- TID-2: The number says what it is. Its tooltip is `Task #N`, and screen readers hear
  "Task #N", not a bare number.
- TID-3: Every pull request chip reads `PR #N` (the word "PR" replaces the branch icon), with
  its checks icon and its state colours as before: in the sidebar, the Tasks page, the task
  overview, the Git review page and the task view's delivery strip. Its tooltip and accessible
  name keep saying "Pull request #N".
- TID-4: In a narrow sidebar, the number and the chip keep their width and the title ellipsis
  takes the squeeze. Nothing wraps, and the row stays one line.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] Sidebar rows show the task number before the title
- [ ] Pull request chips read "PR #N" everywhere, so a bare #N always means a task
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- In the mock studio sandbox, the row of #13 "Improve the code" reads `#13`, then `PR #520`,
  then the title. A row without a pull request (#12 "Checkout API") reads `#12` before its
  title.
- The number's accessible text is "Task #13". The chip's tooltip starts with "Pull request #520".
- The delivery strip of #13's task view and its chip on the Tasks page read `PR #520`.
- At the narrowest sidebar the rows are one line high, and the number and the chip are not cut.
- No WCAG violations in the sidebar.

## Test plan

`e2e/tests/navigation.e2e.ts`: "sidebar rows show the task number apart from the pull request
number" (TID-1, TID-2, TID-3 for the sidebar, TID-4 at the sidebar's narrowest width, axe).
`e2e/tests/pulls.e2e.ts` already finds chips by `#520`, so it keeps passing. `e2e/tests/git.e2e.ts`
covers the delivery strip and the details. A new assertion there checks `PR #520` in the strip
and on the Tasks page (TID-3).

## Decisions

- **`#N` is a task and `PR #N` is a pull request** (ADR-0099). Task numbers are the ones people
  use inside Yulius, and the app already writes them as a bare `#N`. The pull request gets the
  qualifier. Alternatives: `T-108` for tasks (a new notation found nowhere else in the app, the
  agents or the URLs), a legend in the group header (it takes a row and explains instead of
  labelling), or numbers in tooltips only (that is today's problem).
- **"PR" replaces the branch icon** instead of joining it. The chip stays about as wide as
  before (an icon of 12 px plus a gap against two letters), which matters in a squeezed sidebar.
  The violet border still marks a merged pull request.
- **Muted plain text for the number, not a second pill.** Two pills side by side would compete,
  and the muted `#N` before a title matches the Git board and the step strip.
