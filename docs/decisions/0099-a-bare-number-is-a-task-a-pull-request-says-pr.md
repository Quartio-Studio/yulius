# 0099. A bare #N is a task; a pull request says "PR #N"

## Status

Accepted. Specified in [SPEC-122](../../.ai/specs/SPEC-122-we-need-tasks-id-somewhere-here-and-not.md);
task #108.

## Context

The sidebar's task rows showed a pull request chip (branch icon, `#450`) but not the task's own
number, so the only number in the row belonged to the pull request. People refer to tasks by
number all the time: "Run after", "Queued after #102", toasts, the `/t/108` address and agents'
messages. The request was to show task numbers too, "but it should be communicated what id is for
what". Outside the sidebar, the app already writes a task as a muted `#N` before its title (the
Git board, the Git review page, the task view's step strip), and pull requests as a bare `#N`
in their chips. In any one row the two notations looked the same.

## Decision

- A bare `#N` always means a Yulius task. The sidebar's rows and the Tasks page's rows show it,
  muted with tabular figures, before the title. Screen readers hear "Task #N", and its tooltip
  says the same.
- A pull request always carries its qualifier. Every chip reads `PR #N`: `prChip` in `dom.js`,
  the Git board's chip in `git.js`, the task view's delivery strip, and the link in the delivery
  details. The word replaces the branch icon. The tooltip keeps "Pull request #N · state", and the
  accessible name starts with what the chip shows: "PR #N, pull request, open" (WCAG 2.5.3).
- A narrow sidebar gives way in a fixed order. The task list is a size container, and below
  360 px rows with a pull request drop their time. Below 280 px every row drops it, and below
  250 px the chip goes too. The row's tooltip still names the time and the pull request.

## Alternatives

- **`T-108` or `Task 108` for tasks.** That notation would be new to the app, the agents, the
  briefings and the addresses, all of which use `#108`. The number people already type would
  change, not the one that was ambiguous.
- **A legend in the group header.** It costs a row in every group and explains the notation
  instead of labelling it where people read it.
- **A second pill for the task number.** Two pills side by side compete with each other, and the
  muted `#N` already matches the Git board and the step strip.
- **The branch icon and "PR" together.** That is clearer than either alone, but it adds about
  16 px to a chip in a sidebar that is already tight at its default width.

## Consequences

- **Rows are wider.** At the default 300 px sidebar, a row with a pull request no longer shows its
  time, which is still in the row's tooltip and in the Tasks page. Without the container rules,
  titles shrank to one letter there.
- **Accessible names start differently.** A sidebar row's link name now reads "<state> Task #N
  <title>", or "<state> Task #N PR #M, pull request, open … <title>" on a row with a pull request.
  Tests that match row names by their beginning include the number.
- **Future chips must follow the convention.** Any new place that shows a pull request number
  uses `prChip` or writes `PR #N`, and a bare `#N` stays reserved for tasks.
