# 0135. The task graph is drawn in the browser from the task summaries

## Status

Accepted. Specified in [SPEC-164](../../.ai/specs/SPEC-164-structure-of-the-tasks-became-very-compl.md)
(task #154). Adds a view to the Tasks page (UI-024, UI-051).

## Context

A project's tasks wait for each other in several ways: a chain (`after`), a start rule naming
several tasks (`after.rule`), a supertask holding the queue (`held_by`), a prepared start, and the
queue itself, which offers free sessions supertasks first, then in the order tasks were queued.
The sidebar and the Tasks list say each task's state in words, but with 16 open tasks nobody could
see what waits for what or which queued task starts next. Every task summary the web store holds
already carries those links, live through the event stream.

## Decision

- The Tasks page has a List / Graph switch (`?view=graph`); the sidebar's Working group head links
  to it ("Show as a graph").
- `taskgraph.js` builds the structure in the browser from the summaries: **Starts next** (queued
  tasks, supertasks first, then by `state_since`, numbered), **Chains and rules** (every open task
  with a link, and the tasks its links name), **Prepared** and **On their own**. A task is drawn in
  one place; the queue lists queued tasks even when the graph draws them, since it shows order.
- The graph is laid out by its own small layered layout: a column per longest path from the tasks
  nothing waits for (links closing a cycle ignored for the columns, still drawn), rows ordered by
  two barycenter sweeps. Cards are HTML links (the task's overview) over one SVG of arrows: solid
  for a chain, dashed for a start rule (one arrow per task the rule names), dotted for a supertask
  hold. A linked task that is not open is faded; one not in the list is a dashed ghost with its
  number.
- No server change: no endpoint, no event, no field.

## Alternatives

- A server endpoint returning the graph (`/api/sandboxes/{sid}/task-graph`): the same facts twice,
  with its own event to keep it live, and the store's words (`taskInfo`) would have to be ported.
- A vendored layout library (dagre, ELK): tens of kilobytes, or a build step SPEC-002 ADR-01 rules
  out, for graphs this shallow (a sandbox holds at most 32 chained and 32 prepared tasks).
- Top-to-bottom layout: fits narrow screens, but every other timeline in Yulius (the Pipeline
  waterfall) reads left to right; the graph's frame scrolls on its own instead.
- Filters in the Graph view: a filtered graph cuts links and shows a task waiting for nothing.

## Consequences

- The queue position is this project's order. Sessions are shared by every project of the instance
  (`max_live_sessions`), which the page says; a task queued twice may sit a little off, since the
  summary carries `state_since`, not the server's `queued_at`.
- A rule's text is read for `#N` references; a rule naming a task that was archived or deleted
  draws it as a ghost rather than dropping the link.
- A long arrow passes behind the cards of the columns it crosses; with the bounds above the graph
  stays small enough to read, and a node's words still say what it waits for.
