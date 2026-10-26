---
yulius-task: f12d4e48-a870-4c48-9b93-5bf2962da845
status: in-progress
---

# SPEC-334: The Sessions tile says why it is over the limit, not "9 / 8"

Status: in-progress
Date: 2026-10-26
Owner: Yulius task #318
Scope authorization: task #318 (`/goal`, "Bruh 9/8? Fix" with a screenshot of the status panel),
fixes #928. Keeps SPEC-050 SES-101 and ADR-0040 (a Done task's idle session holds no slot).

## Goal

The status panel's Sessions tile never shows a nonsensical fraction such as "9 / 8". When resumed
Done tasks take the instance over its session limit, the tile shows the limit full, marks how many
sessions run beyond it, and says why and when the queue moves again. The same holds for every way
the count passes the limit.

## Context

- **Symptom.** The screenshot's Machine section shows Sessions **9 / 8**, a full red bar and
  "6 queued", at load 117 on 12 cores. The mock (7 running tasks with `ambient=0`) shows **7 / 4** with `?sessionlimit=4`.
- **Code path.** `App::status` (`crates/yulius-app/src/status.rs`) sends `sessions.live` =
  `working_attempts().len()`: the live attempts minus those of Done tasks, which is what the start
  door counts (`sessions_in_use`, `crates/yulius-app/src/supervisor.rs`). `renderStatus`
  (`crates/yulius-server/web/assets/layout.js`) prints `${live} / ${limit}`.
- **Root cause.** A Done task keeps its idle session for follow-ups but holds no slot (SES-101), so
  the queue starts another task in its place. When that Done task works again, after a person types
  a follow-up into its terminal or its own background work wakes the agent, it counts again without
  passing the start door. Typed input in a live terminal cannot be queued. The overshoot is the
  documented trade-off (`sessions_in_use`: "A follow-up prompt can take the instance over the limit
  for a while"). The start door and `drain_queue_now` still refuse every new start while
  `live >= limit`, so the count falls back as those turns end. The display is the defect: it shows
  the raw ratio with no explanation, so the limit looks broken.
- **A second way over the limit** (review round 1). Sessions survive a server restart, and
  `reconcile` reattaches every live run without checking the limit. A restart with a lower
  `YULIUS_MAX_SESSIONS` therefore keeps the runs it finds. A Superagent's or remote `task_message`
  typed into a Done task wakes it like a person's follow-up. The tile's explanation names no
  single cause.
- **Not a counting bug.** `one_live_attempt` (unique index, migration 0001) allows one live attempt
  per task, so no task counts twice.
- **Why tests missed it.** No e2e test read the Sessions tile's text.

## Requirements

- OVL-001: When `live > limit` (limit > 0), the tile's value reads `limit / limit` followed by a
  `+N` mark (N = live − limit), and its bar is full. Its detail reads `N over the limit` on one
  line and `Q queued` on the next when tasks are queued, both shown whole at phone width.
- OVL-002: The over-the-limit tile explains itself on hover and to assistive technology. A Done task
  that works again counts again, and runs started before a restart with a lower limit keep running.
  Queued tasks start once fewer than `limit` run.
- OVL-003: Within the limit, without a limit or with an unknown count, the tile is unchanged:
  `live / limit` (or `—`), then the queued count or "agent sessions running".

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Spec and issue for the over-the-limit Sessions tile
- [x] Regression e2e test showing the tile over its limit
- [x] Sessions tile shows the limit full plus the overshoot, explained
- [ ] Docs, independent review and delivery

## Acceptance criteria

- AC-1 (OVL-001, OVL-002): in the mock (7 running tasks, `?sessionlimit=4&sessionqueue=5`, plus
  the 1 queued task the mock seeds) the status panel's Sessions tile reads `4 / 4` and `+3`. Its
  detail lines are `3 over the limit` and `6 queued`, neither cut off at 360 px. Its title names
  both causes and the limit. The test fails on dev e3487a8c's `layout.js`.
- AC-2 (OVL-003): with the mock's limit at 20 the tile reads `7 / 20` and `1 queued`, with no `+`
  mark and no title.

## Test plan

- e2e (`e2e/tests/subscriptions.e2e.ts`, next to the status panel's other tests): `the Sessions
  tile shows the limit full and the overshoot, not 7 / 4` (AC-1, AC-2), written first and run
  against dev's `layout.js`. It waits on the panel's elements, never on sleeps. Also checks axe.
- The whole e2e suite and `scripts/dev ci` before delivery.

## Decisions

- **Show `limit / limit +N` (chosen)** over capping the number silently (hides sessions that
  really run), the raw `live / limit` with a note (still reads as a broken limit at a glance), or
  enforcing the limit for resumed Done tasks (would mean refusing or holding a person's keystrokes
  in a live terminal, or killing an agent mid-turn; SES-101 chose to let them run). Recorded as
  [ADR-0226](../../docs/decisions/0226-the-sessions-tile-shows-the-overshoot-of-resumed-done-tasks.md).
- The mock takes `?sessionlimit=N` (default 8), so a test sets the limit below or above its 7
  running tasks. It takes `?sessionqueue=N` to add queued tasks to the count, because the mock's
  queued tasks start by themselves.
- The explanation is both the tile's `title` (hover) and hidden text (screen readers), as the tile
  is not focusable. A screen reader may read a generic element's title as its description too
  (review round 1 nit); axe reports nothing, and the duplicate is accepted over a visible
  paragraph in a tile that fits a phone.
- No server change: the status view already counts as the start door does.
