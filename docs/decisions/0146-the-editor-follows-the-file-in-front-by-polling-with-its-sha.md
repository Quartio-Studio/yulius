# 0146. The Editor follows the file in front by looking at it with its sha256

## Status

Accepted. Specified in
[SPEC-181](../../.ai/specs/SPEC-181-here-ui-got-bugged-a-little-bit-we-want.md); refines EDT-003
(SPEC-001) and keeps [0103](0103-editor-presence-mirrors-monaco-and-reaches-writers-only.md).

## Context

The Editor reloaded a file only when its tab came to the front. A file left in front never showed
what an agent, a shell or a teammate wrote into it; the person learned of it as a 409 dialog when
saving. The request (task #168) asks for the editor to reload by itself, to see other people's
input. Teammates already see each other's caret, name and selection (0103), and unsaved text is
each person's own (EDT-003). Most writers (agents, shells) write the disk directly, not through the
files API.

## Decision

- While the Editor is shown, the page is visible and its sandbox is in front, the browser looks at
  the **file in front** every 2 s (`LOOK_MS`), and at once when the Editor or the page comes back.
  The timer waits for its last look; none starts while the file saves; the newest look wins (a save
  or a later look makes an earlier answer moot). Other tabs look when they come to the front, as
  before.
- A look sends `known=<sha256>` (`GET …/files/content`); the server still reads and hashes the
  file and answers **204** with no body when it is unchanged, so a look costs a few bytes.
- A newer version replaces unedited text as **one edit of the part that changed**
  (`pushEditOperations`, its own undo step; a changed line ending is applied with `pushEOL`, a BOM
  is not compared): the cursor follows its text, the undo history survives, and no toast repeats
  while an agent works. A teammate still on the older version is hidden until their Editor has
  the same one (within about 2 s while they have not edited it), as for any other `sha256`.
- Edited text is never replaced: the status bar says "Changed on disk", and saving goes through
  the existing choice (Overwrite or Reload from disk).

## Alternatives

- **A server event on API saves** (`file.changed` to the sandbox's writers): instant for
  teammates, but agents and shells write the disk directly, so a look at the disk is needed anyway.
  One mechanism covers every writer.
- **A filesystem watcher per open file** (inotify): needs a watch budget, a lifecycle bound to
  each viewer, and a fallback for filesystems that do not report changes. More moving parts for a
  latency gain of under 2 s.
- **Co-editing of unsaved text** (relaying keystrokes, merged by OT or CRDT): it would make
  unsaved text shared, against EDT-003, and is a feature of its own (#566).

## Consequences

- A person with a file in front costs the server one read and hash of that file every 2 s (at
  most 2 MiB); nothing crosses the wire while it is unchanged.
- Writes show within about 2 s; teammates' saves too, through the same look.
- A reload is undoable: undoing it makes the text modified again, and its save asks as any
  conflicting save does.
