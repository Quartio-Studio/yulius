# 0261. The Editor's text is shared live and saved by the server

## Status

Accepted. Specified in
[SPEC-411](../../.ai/specs/SPEC-411-editor-live-co-editing-of-unsaved-text-a.md) (task #393,
issue #566). Supersedes EDT-003's "open files and unsaved text … neither stored nor shared"
(SPEC-001), the "Co-editing of unsaved text" alternative of
[0146](0146-the-editor-follows-the-file-in-front-by-polling-with-its-sha.md) and the "Operational
transforms or a CRDT" alternative of [0091](0091-notepad-presence-is-relayed-not-stored.md) for
the Editor. Keeps [0103](0103-editor-presence-mirrors-monaco-and-reaches-writers-only.md) and
0146's look at the disk.

## Context

The founder's EDT-003 kept each person's unsaved text in their browser tab, and every later
decision rejected co-editing on that ground. On 2026-11-03 the person decided on #566: "let's do
live editing and autosave". Teammates should see each other's keystrokes before anyone saves, and
the Editor should save by itself, while agents and shells keep writing the same files on disk
(0146). The UI has no build step (SPEC-002 ADR-01), browsers talk to the server by HTTP and hear
from it on the event WebSocket, and the server is one process.

## Decision

- **One shared text per open file**, held in the server's memory (`live_text.rs`), keyed by
  sandbox, folder and path: its text (UTF-16), a revision, its last 256 steps, the disk's
  `sha256` it was last saved at or taken from, whether it is saved, and the disk's newer
  `sha256` while unsaved text meets a disk change. Never stored in the database or another file.
  At most 256 documents of 2 MiB; a saved one untouched for 10 minutes is dropped, an unsaved one
  after 24 hours untouched; a full server drops its oldest saved one, else refuses.
- **Server-ordered operational transformation.** An operation is a list of retains (`n`),
  inserts (`"text"`) and deletes (`-n`) over UTF-16 units, made at a revision. The server
  transforms it past the steps since, applies it, and publishes the step as `file.live` to the
  sandbox's writers (`publish_to_writers`). Each browser tab keeps one operation in flight and
  composes the rest (the Jupiter model of Google Wave and ot.js), recognises its own step as the
  acknowledgement, and catches up by `GET …/files/live?since=` after a gap or `resync`.
- **Access.** Joining, reading and catching up need what reading the folder needs (write access
  to the sandbox); editing and saving need what changing it needs (a task worktree is its owner's).
  Only a caller who may change the folder starts a document from their own text; otherwise it
  starts from the disk.
- **The server writes the file; the browser says when.** `POST …/files/live/save` writes the
  shared text with the existing atomic, handle-based write and the document's `sha256` as base,
  with the asking person's access; saves of a document run one at a time. The Editor asks 1 s after
  its person's last keystroke, at once when focus or the file leaves, on Ctrl+S, and as the page is
  hidden; teammates' Editors ask 3 s after the last step as a fallback.
- **The disk.** The 2 s look of 0146 stays. A newer disk version is taken by the server: as one
  step when the text is saved, else kept as a conflict that every Editor shows with Keep mine and
  Take disk version; nothing saves automatically meanwhile.
- **Presence** may carry `session` and `revision`, so carets are mapped through the steps instead
  of being hidden at another saved version.

## Alternatives

- **Yjs (CRDT) with `y-monaco`.** Mature, but a vendored bundle in a no-build UI, the `yrs` crate on
  the server to read and save the text, tombstones to bound, and peer-to-peer properties a single
  server does not need.
- **Relay keystrokes without transformation** (last writer wins per keystroke): diverges as soon as
  two people type at once.
- **Each browser saves its own copy** with `base_sha256`: two people would save different texts,
  and the second would meet a conflict made by the first.
- **A server-side autosave timer.** It would have to keep someone's credentials to write later and
  run timers per document; asking from the browser keeps every write under a live request's access.
- **Merging an agent's write into unsaved text.** Needs a line diff and can interleave edits
  silently; with saves a second after typing stops, the conflict window is small, and a choice is
  honest.
- **Edits over the WebSocket upstream.** The socket carries nothing upstream today; requests
  already have access checks, CSRF, limits and errors per edit.

## Consequences

- Unsaved text is now on the server, in memory, visible to the sandbox's writers; a restart loses
  what no browser still holds (the first Editor back starts the document from its text).
- Every keystroke costs a small request (at most one in flight and one every 50 ms per tab) and a
  `file.live` event to each writer's stream; a large paste is one event of at most 2 MiB.
- Ctrl+Z undoes the latest edit in the person's Editor, which may be a teammate's.
- A file is written about a second after typing stops; agents see the person's edits on disk
  almost at once, and git status shows them without an explicit save.
- The mock UI holds the live documents in the first window, so a second window it opens shares
  them, as two browsers share the server's.
