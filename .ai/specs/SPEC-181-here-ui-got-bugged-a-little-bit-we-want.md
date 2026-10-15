---
yulius-task: 85993b4d-d6f9-4150-93a6-4e674015ec20
status: in-progress
---

# SPEC-181: the Explorer's buttons stay in the Explorer, and the open file follows the disk live

Status: in progress
Date: 2026-10-15
Owner: task #168
Scope authorization: the person's request (task #168), with a screenshot of the Editor: "here UI
got bugged a little bit. + we want the editor to autoreload if it doesnt atm (and we want to see
other people input). Fix and implement if needed".

## Goal

1. The Explorer's header never paints over the file tabs, however narrow the Explorer is.
2. The file in front in the Editor shows what others write into it (teammates saving, agents and
   shells editing the sandbox) within about two seconds, without a click, as long as the person
   has not edited it. Their own unsaved text is never overwritten.

## Context

- **The screenshot.** The Editor's Explorer at its narrowest (140 px wide, `EXPLORER_MIN`; the
  person's own width, kept per browser by the splitter): the header row "EXPLORER + ⤒" with the
  third button, Refresh (↻), drawn over the start of the first tab ("○pen-issue…").
- **Root cause.** `.ed-tree-head` is a flex row of the label (`flex: 1`) and three 28 px icon
  buttons. A flex item's minimum width defaults to its content, so the label "EXPLORER" never
  shrinks: label (about 64 px) + 3 buttons + gaps + 16 px padding is about 170 px. Below that the
  row overflows the Explorer, and `.ed-side` sits above the editor (`z-index: var(--z-raised)`,
  so the splitter's grip is never covered), so the overflowing button paints over the tab strip.
  Reproduced in the mock UI with the Explorer at 140 px.
- **Reload today (EDT-003).** "A file changed on disk since it was opened (for example by an
  agent) is reloaded when unchanged in the editor and never overwritten silently." The editor
  checks only when a tab is **activated** (`refreshFromDisk` in `activate`). A file left in front
  never changes while an agent or a teammate writes it; the person sees it only after switching
  tabs, or as a 409 "changed on disk" dialog on save. A reload replaces the whole model
  (`setValue`), which drops the undo history, and shows a toast each time.
- **Other people.** SPEC-127 (EDT-007..012) already shows teammates' carets, names and selections
  in the same file, live, and keeps their unsaved text their own (EDT-003: "open files and unsaved
  text live only in that user's browser tab and are neither stored nor shared"). What a teammate
  or an agent **writes** reaches others only through the disk. So "see other people's input" is
  met by reloading the file in front live; sharing unsaved keystrokes would be co-editing, which
  EDT-003 rules out (see Decisions).
- **Agents write the disk directly**, not through the files API, so a server event on API saves
  alone would miss most changes. Only a look at the disk catches them all.
- The files API reads a file whole (up to 2 MiB, `MAX_FILE_BYTES`), with its `sha256`.

## Requirements

| ID | Requirement |
| --- | --- |
| EDT-013 | The Explorer's header keeps its buttons (New, Upload, Refresh) inside the Explorer at every width it can take (its 120 px floor included): the label "Explorer" shortens with an ellipsis and the buttons keep their size. Nothing of the header paints over the tab strip. |
| EDT-014 | `GET /api/sandboxes/{sid}/files/content` takes an optional `known` (64 lowercase hex digits, the `sha256` the caller has): when the file's bytes on disk still have that `sha256`, it answers `204 No Content` with no body; otherwise as before. A malformed `known` is refused (422). Access and path rules are those of a read. |
| EDT-015 | While the Editor is shown, its page is visible and the sandbox is in front, the file in front is looked at every 2 s with `known` (one look at a time; none while it is saving). A newer version on disk replaces the text when the person has not edited it: as one edit of the part that changed (the cursor and the scroll stay where they were relative to the text around them), undoable, with no toast; the teammates' marks follow it. When the person has unsaved edits, their text is kept and the status bar says "Changed on disk", so a save goes through the existing choice (Overwrite or Reload). A file that cannot be read (deleted, binary now) keeps the editor's text. Other tabs are checked when they come to the front, as before. |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] The Explorer's header stays inside the Explorer at any width
- [ ] A file read with the sha256 the caller knows answers 204 when unchanged
- [ ] The file in front reloads by itself when it changes on disk, keeping the cursor and edits
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-181-1: in the mock Editor with the Explorer at 140 px (and at its 120 px floor), every
  button of the Explorer's header lies inside the Explorer, and the first tab's left edge is not
  covered by any of them.
- AC-181-2: Rust: a read with the current `sha256` as `known` answers 204 with no body; with an
  older one it answers the file; a malformed `known` is 422; a reader without write access is
  refused as for a read.
- AC-181-3: e2e: with a file open and unedited, a change on disk (another person's save, an
  agent's write) shows in the editor without any click, the cursor stays on the same text, no
  toast appears, and Ctrl+Z brings the previous text back.
- AC-181-4: e2e: with unsaved edits, a change on disk keeps the edits and the status bar says
  "Changed on disk"; saving offers Overwrite or Reload.
- AC-181-5: e2e: while nothing changes, the looks carry `known` and get no body; with the Editor
  hidden no look is made.

## Test plan

- `e2e/tests/editor.e2e.ts`: the Explorer header at the minimum widths (AC-181-1); autoreload of an
  unedited file, cursor kept, undo, no toast (AC-181-3); unsaved edits kept with "Changed on disk"
  (AC-181-4); looks carry `known`, none while hidden (AC-181-5). Waits on conditions
  (`expect.poll`), never on sleeps.
- `crates/yulius/tests/files.rs`: `known` matching, stale, malformed, and access (AC-181-2).
- Full gate: `scripts/dev ci`, `scripts/dev security`, the e2e suite.

## Decisions

- **Poll the file in front instead of pushing events** ([ADR-0146](../../docs/decisions/0146-the-editor-follows-the-file-in-front-by-polling-with-its-sha.md)).
  Agents and shells write the disk directly, so an event published by the files API would miss
  most changes, and a filesystem watcher per open file needs an inotify budget, a lifecycle bound
  to each viewer, and still a fallback for network or overlay filesystems. A look every 2 s at the
  one file in front, answered 204 when unchanged, catches every writer with one mechanism and
  costs a read and a hash on the server, a few bytes on the wire.
- **2 s.** Fast enough to watch an agent work, slow enough that ten people in front of 2 MiB files
  cost the server about 10 MiB/s of hashing at worst. Background tabs keep the check on activation.
- **One minimal, undoable edit** instead of `setValue`: the cursor and teammates' marks stay with
  the text, the undo history survives, and a reload that arrives while the person reads is quiet
  (no toast every 2 s while an agent works).
- **No co-editing of unsaved text.** "See other people's input" is met by showing what they save
  and where they are (SPEC-127). Sharing keystrokes before a save would make unsaved text shared
  (against EDT-003) and need merging of concurrent edits (OT or CRDT). A follow-up issue records
  it as a possible feature.

## Evidence

(Filled in when the work is done.)
