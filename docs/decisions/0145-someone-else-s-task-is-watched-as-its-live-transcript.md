# 0145. Someone else's task is watched as its live transcript, read on from where it stopped

## Status

Accepted. Specified in [SPEC-179](../../.ai/specs/SPEC-179-ui-problem-when-someone-else-has-a-contr.md).
It changes what [0060](0060-a-task-is-owned-by-one-person.md) shows people who do not own a task
(the terminal opened read-only) and reuses [0059](0059-a-session-is-printed-as-the-tui-in-html.md)'s
TUI-in-HTML printing for a live session.

## Context

A task's terminal is drawn by Claude Code for the size its writer's browser sets. Everyone else who
opened the task got the same PTY output in an xterm of another size, so the full-screen TUI wrapped
and overlapped: the view was broken for every watcher (task #166). A task's session transcript is
already served as a session document from its live file, redacted (SPEC-075, SES-004), but only
page by page from the start: `next` is null at the end, so a reader of a growing file cannot go on
from where it stopped, and a long session has no tail.

## Decision

- A person who does not own the task sees its session as the Sessions view prints it, in the live
  terminal's colours, instead of a terminal; no terminal socket is opened for them. When control
  passes to them the terminal comes back, and goes again when they hand it over.
- `GET /api/tasks/{tid}/transcript` (read) serves the latest lineage entry's transcript: without
  `from` its tail (the last 200 entries), with `from` the lines after it. `end` is the line after
  the last complete line read and `bytes` the file's size. The watcher asks again every 1.5 s with
  `from=end&seen=bytes&session=…` while the task is live and the page is shown; an unchanged size of
  the same session is answered from one `stat`, without reading the file.
- A tool result that arrives after its call is printed inside that call (the call's entry is drawn
  again), on the Sessions page too.
- An electro-blue pixel octopus lives in a lane stuck to the top of the watched transcript, as the
  spider web marks a past session: arm frames in CSS `steps()`, corner-to-corner travel by script.

## Alternatives

- Draw the terminal at the writer's size for watchers (scaled or scrolled). The TUI stays unreadable
  at a different width and the size changes whenever the writer resizes. Rejected.
- Push new lines from Claude Code's hooks. They fire per tool call and prompt, not per assistant
  message, and would tie the hook path to the web. Rejected for polling.
- A file watcher per watched task. More moving parts with their own bound, for a saving that the
  size short-circuit already gives. Rejected.
- Read from a byte offset. It would lose the dotenv tracking that redacts the results of a `.env`
  read earlier in the file. Rejected: every changed read scans from the start, as session pages do.

## Consequences

- A watcher costs one request every 1.5 s and one `stat` while nothing changes; a change reads the
  transcript from its start (bounded: 2 MiB of text a page, 500 entries).
- What the owner types before Enter is not shown: only the prompts sent appear, with their time,
  not labelled with a name (Yulius types into sessions too). The locked line says who is at the
  keyboard.
- The read-only terminal remains for a person's own task in a project they can only read.
