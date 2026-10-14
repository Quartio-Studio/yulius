---
yulius-task: 32bdac19-661e-4de5-b0e1-d986fd927eac
status: done
---

# SPEC-122: Sidebar rows show the task number, and pull request chips say "PR"

Status: implemented
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
overview (`taskinfo.js`) and the Git review page. The task view's delivery strip
(`actions.js` `deliveryStrip`) and the Git board (`git.js`) draw their own copies. The delivery
details (`prLine`) link the number.

## Requirements

- TID-1: Every sidebar task row shows the task's number as `#N`, in the muted colour with
  tabular figures, between the state icon and the pull request chip (or the title, when there is
  no pull request). It shows whatever the task's state and in every group.
- TID-2: The number says what it is. Its tooltip is `Task #N`, and screen readers hear
  "Task #N", not a bare number.
- TID-3: Every pull request chip reads `PR #N` (the word "PR" replaces the branch icon), with
  its checks icon and its state colours as before: in the sidebar, the Tasks page, the task
  overview, the Git board's cards, the Git review page, the task view's delivery strip and its
  delivery details. Its tooltip keeps "Pull request #N · state · checks". Its accessible name
  starts with what it shows and then says what it is ("PR #N, pull request, open, checks
  running", WCAG 2.5.3), also inside a sidebar row's link.
- TID-4: The sidebar's rows stay one line at any sidebar width (200 to 480 px). Nothing spills
  out of a row, the number and any chip or time shown are whole, and every title keeps at least
  3 em. When space runs short, the time gives way first: below a 360 px task list on rows with a
  pull request, below 280 px on every row. Below 250 px the chip goes too. On a phone the
  row's right side keeps 48 px for the menu button, so each threshold is 36 px wider. On hover,
  where Archive and the menu show, the title may shrink below 3 em and stays clear of them. The
  row's tooltip still names the task, its pull request (`PR #N`, once) and the time.
- TID-5: The Tasks page's rows show the task's number the same way, before the title.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Sidebar rows show the task number before the title
- [x] Pull request chips read "PR #N" everywhere, so a bare #N always means a task
- [x] Review round 1 findings fixed test-first: Git board chip, phone drawer, hover, tooltip, screen reader name
- [x] Review round 2 findings fixed test-first: hover on non-archivable rows, shells tooltip, label in name
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- In the mock studio sandbox, the row of #13 "Improve the code" reads `#13`, then `PR #520`,
  then the title. A row without a pull request (#12 "Checkout API") reads `#12` before its
  title.
- The number's accessible text is "Task #13". The chip's tooltip starts with "Pull request #520",
  and its accessible name starts with "PR #520, pull request".
- The delivery strip of #13's task view and its chip on the Tasks page read `PR #520`.
- At sidebar widths of 456, 328 and 200 px the rows are one line high, nothing is cut or
  spills, and titles are at least 36 px wide. At 456 px every row shows its time. At 200 px no
  chip shows, and #13's tooltip names `PR #520`.
- The Tasks page's row of #13 shows "Task #13" before its title.
- No WCAG violations in the sidebar.

## Test plan

`e2e/tests/navigation.e2e.ts`:
- "sidebar rows show the task number apart from the pull request number" covers TID-1, TID-2 and
  TID-4. It checks three sidebar widths and axe. It also checks that a hovered title ends before
  Archive and the menu (an archivable row at 296 px, a non-archivable one at 264 px), and that
  the tooltip names the pull request once, also in the "still working" state with shells.
- "the navigation drawer rows fit a 320 px phone with task and pull request numbers" covers TID-4
  on a phone.
- "the Tasks page rows show the task number before the title" covers TID-5 and the row's
  "Task #N" tooltip.

`e2e/tests/git.e2e.ts`: "pull request chips read "PR #N" wherever they show" covers TID-3. It
checks the sidebar, the Tasks page, the task view's delivery strip, the Git board and the
delivery details, and that each chip's name starts with what it shows.

`e2e/tests/pulls.e2e.ts` finds chips by `#520` and keeps passing. In `e2e/tests/task-done.e2e.ts`,
the sidebar link's name now starts "Needs you in the browser Task #10 …".

## Decisions

- **`#N` is a task and `PR #N` is a pull request** (ADR-0099). Task numbers are the ones people
  use inside Yulius, and the app already writes them as a bare `#N`. The pull request gets the
  qualifier. Alternatives: `T-108` for tasks (a new notation found nowhere else in the app, the
  agents or the URLs), a legend in the group header (it takes a row and explains instead of
  labelling), or numbers in tooltips only (that is today's problem).
- **"PR" replaces the branch icon** instead of joining it. The chip stays about as wide as
  before (an icon of 12 px plus a gap against two letters), which matters in a squeezed sidebar.
  The violet border still marks a merged pull request.
- **The time gives way, not the title or the number.** At the default 300 px sidebar, a row with
  a number, a chip and a time leaves the title 0 to 5 px. Container queries on the task list hide
  the time where it cannot fit, which is more predictable than an ellipsis ("do…"), and keep it
  in the row's tooltip. The thresholds come from measured widths: a row with a chip and the
  longest times needs about 335 px of content, and one without needs about 260 px.
- **On hover the time is only hidden, not removed, on rows that cannot be archived.** The menu's
  button covers the row's right end, and the link's padding keeps the title clear of it. Review
  round 3 suggested `display: none` for the time on every hovered row, which would give the
  title the time's width while the pointer is on it. That gain is cosmetic and would reflow every
  hovered row, so it was left out after three rounds.
- **Muted plain text for the number, not a second pill.** Two pills side by side would compete,
  and the muted `#N` before a title matches the Git board and the step strip.

## Evidence

- **Tests failed first, for the right reasons.**
  - Sidebar test: it timed out on the missing `.task-num`. With the number in place, the widened
    squeeze test then failed at the default width with titles of 0 to 5 px.
  - Chip test: it failed with `expected "#520 pull request, open, checks running" to match
    /^PR #520/`.
  - Tasks page test: `.task-num` was missing.
  - Review round 1 tests: the Git board chip was missing, the tooltip read `["PR #520","PR #520"]`,
    and rows spilled in the 320 px drawer.
  - Review round 2 tests: the hovered title overlapped the menu by 18 px, and the chip name
    lookup (label in name) failed.
- **Full e2e suite on `ebd83bf1`** (`cd e2e && npx e2e run`): 370 passed, 1 skipped (47 files).
  After that, only the hover padding changed from 10 to 12 px (`2f83cbda`).
  `npx e2e run tests/navigation.e2e.ts` then gave 8 passed.
- **`scripts/dev ci` on `2f83cbda`:** exit 0. The repository contracts passed (UI design tokens
  included), and nextest ran 1626 tests: all passed, 16 skipped. The run on `ebd83bf1` had failed
  the 4 px spacing-scale contract (`10px`), which `2f83cbda` fixes.
- **Screenshots of the mock studio sandbox** (dark theme):
  - At a 300 px sidebar, rows read `#13 PR #520 ◷ Improve th…` with titles aligned.
  - At 480 px, every row shows its time.
  - The Tasks page shows `PR #520` chips.
- **Independent reviews** in fresh sessions (`om-code-review`):
  - Round 1 requested changes: the Git board chip, the phone drawer, hover, the tooltip and the
    spoken name.
  - Round 2 requested changes: hover on rows without Archive, the shells tooltip, and label in
    name.
  - Round 3 approved `ebd83bf1` with nits. The ADR wording and the plan ticks are fixed. The time
    on hover is a recorded decision.
- **Not testable here:** a real GitHub pull request and screen-reader speech. Both are covered
  by the mock and by accessible-name and axe checks.
