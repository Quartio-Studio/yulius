---
yulius-task: 495d1c13-9b41-40f8-a15b-f96aefe8cd73
status: done
---

# SPEC-164: the Tasks page shows how a project's tasks hang together, as a graph

Status: done
Date: 2026-10-15
Owner: task #154
Scope authorization: the person's request (task #154), "Structure of the tasks became very
complex. I want to be able to see graph preview with logical structures of things, queue etc",
with a screenshot of the sidebar's Working group: 16 tasks, some queued, four "queued 10h 53m"
behind a link icon (a chain #135 → #136 → #137 → #138 after #134), PR chips, running ones. The
list says each task's state, but not what waits for what, nor which queued task starts next.

## Goal

From the Tasks page (and one click from the sidebar's Working group) a person sees the structure
of the project's open work at a glance: which task starts next from the queue, which tasks wait
for which (chains, start rules, supertask holds) as a graph, what is prepared for later, and what
runs on its own; each task one click from its overview.

## Context

- **What a task already says.** `TaskSummary` (docs/api.md) carries everything the structure
  needs: `after` (the task it runs after, or for a start rule the rule's anchor with `after.rule`,
  e.g. `#12 done and (#13 done or #14 ended)`), `next` (tasks chained after it), `supertask` and
  `held_by` (task numbers a supertask makes a queued task wait for, SPEC-071), `status`
  (`queued`, `chained`, `prepared`, …), `start_at`, and `state_since` (since when it is in its
  state). The web store holds every task of a sandbox and updates them live. No server change is
  needed (ADR-0135).
- **The queue.** The server offers free sessions to queued tasks in `start_order`
  (`crates/yulius-app/src/supertask.rs`): supertasks first, then by `COALESCE(queued_at,
  created_at)`; a task a supertask holds is skipped. The summary has no `queued_at`, but a queued
  task's `state_since` is when it became queued, so the client orders by it. Sessions are shared by
  every project of the instance (`max_live_sessions`), so a position is the order within this
  project, said so on the page.
- **Words already exist.** `status.js` `taskInfo` / `activityLine` give each task's state label and
  its "what is happening" sentence (e.g. "Starts by itself when #12 is done", "Queued after
  supertask #20", "Scheduled · Fri 09:00"), with the sidebar's grouping (UI-050). The graph reuses
  them, so a task reads the same everywhere.
- **The Tasks page** (`pages/tasks.js`, UI-024/UI-032) is a grouped list with filters in the URL.
  Related work: chains (TSK-012, SPEC-017 start rules), supertasks (SPEC-071), prepared tasks
  (SPEC-105), changing a start (SPEC-126). Task #155 (queued) is about the Pipeline waterfall's
  readability, not this.
- **No framework, no build step** (SPEC-002 ADR-01): plain ES modules, nothing from other origins.

## Requirements

| ID | Requirement |
| --- | --- |
| TGR-001 | The Tasks page has a List / Graph switch in its header; Graph is kept in the URL (`?view=graph`), so a link opens it. The sidebar's Working group head has a "Show as a graph" button that opens it. |
| TGR-002 | **Starts next**: the project's queued tasks in the order the queue offers them a session (supertasks first, then the longest queued first), numbered 1, 2, 3, each with what it waits for (a free session, or a supertask), and a note that sessions are shared with other projects. |
| TGR-003 | **Chains and rules**: every open task (the sidebar's Needs you and Working groups) that still waits on another task — a `chained` task by its chain (`after`) or start rule (every `#N` the rule names), a queued one by a supertask hold (`held_by`) — is drawn with the tasks it waits for as a node in a left-to-right graph: a task right of everything it waits for, one arrow per link, solid for "runs after", dashed for a start rule, dotted for a supertask hold, with a legend. A linked task that is not open (done, ended) shows as a faded node; one not in the list (archived or deleted) as a ghost node with its number. |
| TGR-004 | A node shows the task's state mark, number, title and its state line in words (`activityLine`), with the rule's text on a rule-started task; it is a link to the task's overview whose accessible name says the task, its state and what it waits for. Cards come column by column, top to bottom, in the Tab order, and j / k move between the view's tasks as in the List. |
| TGR-005 | **Prepared**: prepared tasks with when they start (a time, or "When you decide"). **On their own**: the other open tasks (running, needs you, paused…) with their state. A section with nothing is left out; with no open task at all, the page shows one empty state. |
| TGR-006 | The graph follows live task events (coalesced as the list is), keeping the focused card and the frame's scroll, loads in its own shape (SPEC-120), and stays within the page: it scrolls inside its own frame when wider than the window (390 px phones included), never the page sideways. A cycle or a dangling link never hangs or breaks the drawing. |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] A mock project whose tasks form chains, a rule, a supertask hold, a queue and prepared tasks
- [x] The task graph model: nodes, links, queue order and layered layout from the task summaries
- [x] The Graph view on the Tasks page: Starts next, the drawn graph with its legend, Prepared, On their own
- [x] The List / Graph switch in the URL and the sidebar's "Show as a graph" button
- [x] Docs, ADR and requirements updated for the graph view
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- `?mock=1&tasks=graph`, studio's Tasks page → Graph: Starts next lists the supertask #20 first,
  then #8 and #21 in the order they were queued, numbered 1-3, #8 saying it waits for supertask
  #20.
- The graph has #15 right of #12, #16 right of #15, and #17 (rule `#12 done and #16 done`) right
  of #16, with a solid arrow #12 → #15 and dashed arrows #12 → #17 and #16 → #17; #20 has dotted
  arrows from the working tasks it waits for. A node opens `/s/studio/tasks/<n>`.
- Prepared lists #18 (scheduled, with its time) and #19 ("When you decide"); On their own lists an
  open task with no link (e.g. #7, paused) and none of the graph's nodes.
- A rule naming an archived task draws a ghost node `#N`; links forming a cycle still draw every
  node once.
- The switch's Graph choice sets `?view=graph`; List removes it; the sidebar's "Show as a graph"
  opens the Graph view; a task chained meanwhile appears without a reload, and a live update keeps
  the focused card and the frame's scroll. The search's "Show every task matching" opens the List.
- #14, which once ran after #13 and has started, is on its own, not in the graph; #11 (done, held
  by the supertask) is faded; cards are in the DOM column by column; j / k move between tasks.
- While the tasks load, the Graph shows section outlines and no toolbar; with no open task, one
  empty state.
- At 390 px the page has no sideways scroll; the graph frame scrolls instead. No axe violations.

## Test plan

- `e2e/tests/task-graph.e2e.ts` (mock, Chromium), new: the acceptance criteria above, reading node
  positions (`getBoundingClientRect`) and edge classes; each test fails before its slice.
- `mock.js`: `tasks=graph` seeds studio with the chain, the rule, the supertask and its holds, the
  queued tasks and the prepared ones; and the cycle / archived-reference cases for the robustness
  test.
- Gates: `scripts/dev ci` (repository check, Rust gate untouched), the full e2e suite, and
  `scripts/dev security`.

## Decisions

- **Client-side, from the summaries** (ADR-0135): the store already has every task with its links
  live; a server graph endpoint would duplicate it and need its own event. Alternative rejected:
  `GET /api/sandboxes/{sid}/task-graph`.
- **Own small layered layout, no library**: columns by longest path from the tasks nothing waits
  for, rows ordered by two barycenter sweeps; cards are HTML links over an SVG of arrows, so text,
  focus and screen readers work. A vendored graph library (dagre, elk) is far larger than the
  ~150 lines needed for chains this shallow (bounded: 32 chained, 32 prepared per sandbox).
- **Left to right**, as the Pipeline waterfall reads time; the frame scrolls on narrow screens.
- **No filters in Graph**: the state tabs and Author/Origin filters stay with List; a filtered graph
  would cut links and mislead. Reversible.
- **Queue order on the client** from `supertask` and `state_since`: equal to the server's
  `queued_at` for every task queued once; a supertask hold is shown, not hidden.
- **Faded context nodes**: only tasks a link reaches, so done work does not crowd the picture.
- **Only waits are drawn** (review round 1): a started task keeps its `after` in the summary, but
  it no longer waits, so its link is history and it sits On its own.
- **A live repaint rebuilds the graph** and puts focus and the frame's scroll back (review round
  1); the layout is cheap at these bounds, so no diffing.

## Evidence

Run on 2026-10-15 in the task's worktree (Linux, Node 24.21, Chromium 153), branch head merged with
`dev` at 33f8c8f5.

- `cd e2e && npx e2e run tests/task-graph.e2e.ts`: 7 tests failed before the feature (no Graph
  radio, no `.tg-*` nodes), then 7/7 passed; the 4 review regression tests (focus and scroll on a
  live update, the loading shape and empty state, a started task's old chain, card order, j / k
  and the search) failed before their fixes, then 10/10 passed.
- Full e2e suite, `npx e2e run`: before review, 478 passed, 1 skipped (17m 35s). After the review
  fixes and the `dev` merge, 487 passed, 1 skipped, 2 failed at host load ~33 on 12 cores
  (`ownership.e2e.ts` "the owner is asked for control…", `terminal.e2e.ts` "the program has the
  mouse…", both timeouts on the task view, outside this change); both files then passed alone,
  19/19. Tracked in #506 (comment with this run), not retried into green.
- `scripts/dev ci` (after `python3 scripts/bootstrap.py`): exit 0; 1823 Rust tests passed, 18
  skipped; repository check, lint, doctests, cargo-deny (advisories, bans, licenses, sources) ok.
- `scripts/dev check`: exit 0 (the repository check needed `--sync-brand` once: `index.html`
  preloads the new `taskgraph.js` module).
- `scripts/dev security`: exit 2, **not run**: Vidoc rejects the agent secret's key (HTTP 401,
  #470); unrun is not passed (ADR-0104).
- Independent review (fresh session, `om-code-review`), round 1: REQUEST CHANGES, one major (a
  live repaint dropped focus and the frame's scroll) and minors (search's Show all matching in the
  Graph, the loading shape, Tab order, j / k, untested empty state, faded node and 390 px axe, a
  started task's old chain still drawn); all fixed test-first in 10ab9152. Long arrows passing
  behind cards in skipped columns are accepted (ADR-0135).
- Not testable here: the real server's live data (the mock answers like it); a real queue across
  several projects of one instance.
