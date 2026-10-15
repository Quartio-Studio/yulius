---
yulius-task: 7e3b8966-8570-4246-b8f4-8cfdad5d602e
status: in-progress
---

# SPEC-179: Someone else's task is watched as its live transcript, with an octopus

Status: in-progress
Date: 2026-10-15
Owner: Yulius task #166
Scope authorization: task #166: "when someone is an owner of the task, other users should see a
terminal. They should see similiar transcripts like for archived sessions, but: (1) it should keep
streaming output and the owner's input (2) colors should stay normal (3) message about ownership
should be more clear and even somewhere here maybe [the sidebar's task list] (4) […] In archive, we
have a spider. In other's owner, we need and electroblue octopus which is moving between corners,
being sticked to the top, and moving tentacles in pixel animation. This should be a real octopus
with functional tentacles, but the face should be exactly like here [two dot eyes, an ε mouth]
[…] This animation should be dynami, wholsome and alluring".

## Goal

A person who opens a task someone else owns (SPEC-070) no longer sees the owner's terminal drawn at
the owner's size. They see the task's Claude Code session printed as the read-only Sessions view
prints it (SPEC-075), live: new messages, tool calls and the owner's prompts appear as the session
writes them, in the live terminal's colours. The view says plainly whose task it is and that they
are watching, the sidebar marks other people's tasks, and an electro-blue pixel octopus with the
reference face crawls along the top between the corners.

## Context

- **The defect.** `taskview.js` gives everyone the task's terminal (`/api/tasks/{tid}/terminal`).
  Only the owner types and only a writer's browser sets the PTY size, so a watcher's xterm shows a
  screen drawn for another window size: Claude Code's full-screen TUI wraps and overlaps (the first
  attachment). The read-only label (`.term-corner`, "Read-only: @superadmin owns this task") sits
  under the attention bar's ownership card, cut off.
- **What exists.** Task sessions are already session documents read from the live transcript file
  (`session_docs.rs`, `transcripts::read_page`, SPEC-075/SES-001), redacted on serve (SES-004, the
  vault's matcher). `pages/sessions.js` prints a transcript as Claude Code's TUI in HTML
  (ADR-0059), with a faded palette (`--session-*`, "less contrast and saturation than a live
  terminal"), a pixel spider web in the corner (ADR-0082, ADR-0088) and a cockroach. Paging reads
  forward from a line; `next` is null at the end, so a reader cannot continue a growing file
  without reading it whole again, and nothing gives the tail of a long session.
- **Who is a watcher.** `otherOwner(task, me)` (status.js) names the owner when it is not the
  viewer. Watchers include people with only read access to the project. The attention bar already
  carries the control card (Request control, Withdraw, Ask again) and floats over the terminal.
- **Parallel work.** Task #137 changes `pages/sessions.js` (cut diffs, scrollers). This change adds
  to that file (exports) instead of moving its renderer, so both merge.

## Requirements

- **WAT-001 Live transcript endpoint.** `GET /api/tasks/{tid}/transcript` (read access to the task,
  as `GET /api/tasks/{tid}`) answers `TaskTranscript {session_id, doc_id, entries, next, end,
  bytes, earlier, skipped_lines}` for the task's latest provider session (the last lineage entry),
  read from where its transcript lives (`task_transcript_path`), redacted exactly as session pages.
  - Without `from`: the **tail**, the last `limit` entries (default 200, at most 500) within the page
    text bound (2 MiB); `earlier` says entries before them were left out. With `from`: entries from
    that line forward, as the Sessions page reads (`next` when the page is full).
  - `end` is the line after the last complete line read (a line still being written is not read);
    `bytes` the transcript's size when it was read. A reader continues with `from=end`.
  - With `seen=<bytes>&session=<session_id>` equal to the current session and its file size, the
    answer comes without reading the file: no entries, `end = from`, the same `bytes`.
  - A task without a session yet, or whose transcript is gone, answers no entries, `end` 0, `bytes`
    0, `session_id` null. `doc_id` (the session's document, for "the whole session in Sessions") is
    filled on a tail read once the session has one. Unknown task or no access: 404.
- **WAT-002 Watchers see the transcript.** When the task has another owner, the task view shows the
  live transcript in the terminal's place and opens no terminal socket; when control passes to the
  viewer the terminal comes back (and goes again when they hand it over), without a reload. The
  transcript prints entries as the Sessions view does (`>` prompts, `⏺` messages and calls, `⎿`
  results, diffs, folds). A prompt sent into the session shows as the TUI's `>` line with its
  time; a tool result that arrives after its call is printed inside that call.
- **WAT-003 It streams.** While the task is live (`isLive`) and the page is shown, the view asks for
  new lines about every 1.5 s with `from=end`, `seen` and `session`, one request at a time, and
  appends what came. A new session (a continue that started another) or a transcript rewritten
  shorter (`end < from`) reads the tail again. A hidden page asks nothing until it is shown again;
  leaving the task cancels the read in flight. An ended task is read once more and then not again.
  The view follows the bottom while the reader is at the bottom; scrolled up, it stays where they
  read and a "New messages" button jumps down. While the agent works, a `✻ Working…` line shimmers
  at the bottom (none under reduced motion's shimmer rules). Before the session has lines, the view
  shows the terminal's empty message for the task's state (Queued, Starting…, Prepared…).
- **WAT-004 Normal colours.** The live transcript uses the live terminal's palette (`--term-bg`,
  `--term-fg`, the ANSI green and red for results), not the faded `--session-*` palette of a past
  session; every text pair stays at least 4.5:1 in both themes.
- **WAT-005 Ownership said clearly.** The control card says "You are watching @owner's task. Only
  @owner can type in it; it acts on GitHub as them." with Request control (or the existing asked /
  Withdraw / Ask again states). In watch mode the card sits above the transcript in the flow, so it
  covers neither text nor the octopus. Where the TUI's prompt box was, a locked line says
  "@owner is at the keyboard · you are watching" (not a control). The terminal's read-only label is
  not shown, as no terminal is. In the sidebar, a task someone else owns shows a small round mark
  with the owner's initials before its time, titled "@owner owns this task"; its tooltip and its
  screen-reader sentence say "owned by @owner". Your own tasks show no mark.
- **WAT-006 The octopus.** A pixel-art octopus in electro blue (`--octo-*` tokens, both themes) lives
  in a lane stuck to the top of the transcript (`position: sticky; top: 0`) above the text, so it
  stays at the top while the transcript scrolls and never covers a line. Its face is the reference's:
  a round head, two single dark dot eyes set wide and an `ε` mouth between and below them, and no
  other features. It has eight tentacles (six seen from the front and two at the sides) that move
  in pixel frames (`steps()`): crawling, they ripple in a wave that travels across them in the
  direction it goes and its head bobs a pixel; resting, they sway slowly. It crawls from one top
  corner to the other at a steady speed, its eyes looking where it goes, rests a few seconds in the
  corner (blinks, waves one tentacle), then crawls back, for as long as the view is open. It is
  `aria-hidden`, catches no pointer, and is removed with the view. Under reduced motion it rests
  still in the top-right corner. The spider and the cockroach stay on past sessions only.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] The server serves a task's live transcript to its watchers, cheaply when nothing changed
- [x] Someone else's task shows its live transcript instead of a broken terminal, following new output and the owner's prompts
- [x] The live transcript keeps the terminal's colours, and ownership is said clearly in the view and the sidebar
- [ ] An electro-blue pixel octopus with the reference face crawls between the top corners of a watched task
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- **AC-WAT-1.** An integration test reads a task's transcript through `GET /api/tasks/{tid}/transcript`:
  the tail with `earlier`, a page from `from`, `end` past a half-written last line that is read once
  completed, the unchanged answer for `seen`/`session` without new entries, the latest lineage
  entry's session, redaction of a token and an agent secret, read access (a reader of the project
  reads, an outsider gets 404) and the empty answer for a task with no session.
- **AC-WAT-2.** e2e: a watcher of a running task sees its transcript (no `.xterm`, no read-only
  label), new entries appended by the mock appear without a reload, a late tool result lands inside
  its call, and when the owner hands it over the terminal returns; handing it away brings the transcript back.
- **AC-WAT-3.** e2e: scrolled up, new entries do not move the view and "New messages" appears and
  jumps down; at the bottom the view follows.
- **AC-WAT-4.** e2e: the live transcript's foreground and background equal the terminal's and are
  not the faded session palette, AA in both themes; axe finds nothing.
- **AC-WAT-5.** e2e: the control card's words and the locked line; the sidebar's owner mark on
  another owner's task (title, initials, screen-reader words) and none on one's own.
- **AC-WAT-6.** e2e: the octopus is in the sticky lane at the top of the watched transcript, stays
  there while the transcript scrolls, has the face (two eye pixels, an ε mouth) and eight tentacles,
  moves between the corners and changes tentacle frames, rests still in the top-right corner under
  reduced motion, and is gone when the view is left. Past sessions keep the spider and no octopus.

## Test plan

- Rust: `crates/yulius/tests/task_transcript.rs` (one binary, declared in `tests/main.rs`) for
  AC-WAT-1; `transcripts.rs` unit tests for the tail and `end` (half-written line, `earlier`).
- e2e (mock server): `e2e/tests/watching.e2e.ts` for AC-WAT-2..6, the mock serving
  `/api/tasks/{tid}/transcript` from the task's session document with `appendTranscript` to grow it;
  `ownership.e2e.ts` updated where it read the terminal's read-only label. Movement and frames are
  read through the octopus's own state (classes and the running animation's progress), waited on
  as conditions, never on sleeps.
- Gates: `scripts/dev check`, `lint`, `test`, `scripts/dev security`, the e2e suite.

## Decisions

- **Poll the transcript, short-circuited by size (ADR-0145).** A watcher asks every 1.5 s with the
  size it saw; an unchanged file costs one `stat`. Alternatives: push from Claude Code's hooks (they
  fire per tool call and prompt, not per assistant message, and would couple the hook path to the
  web), a file watcher per task (more moving parts and its own bound), or a byte-offset reader (it
  would lose the dotenv tracking that redacts results of a `.env` read earlier in the file).
- **Tail first.** A live session runs to thousands of entries; the view starts at the last 200 and
  links the whole session in Sessions, rather than paging forward from the start.
- **Reuse the Sessions renderer.** `pages/sessions.js` exports a transcript list (entries and the
  results shown in their calls) that both views use; the live view adds its palette, lane and
  polling in `watchview.js`. Moving the renderer to its own module would collide with task #137.
- **"The owner's input" is the prompts sent into the session.** What is typed before Enter lives
  only in the PTY's screen; reading it would mean parsing Claude Code's drawing. Prompts appear when
  sent. They are not labelled with the owner's name: Yulius also types into sessions (goals, wakes,
  queued messages), and the transcript does not say who sent a prompt. The locked line names who is
  at the keyboard instead.
- **The octopus moves by script, its tentacles by CSS.** Corner-to-corner travel depends on the
  lane's width and rests at random lengths: Web Animations from a script (as the cockroach). The
  tentacle and blink frames are CSS `steps()` on SVG groups (as the spider), so reduced motion stops
  them with one rule.
- **The face.** Dot eyes and an `ε` mouth, pixel for pixel: one dark pixel per eye, the mouth a
  three-row zigzag between them, no cheeks, brows or highlights on the face.
