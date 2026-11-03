---
yulius-task: da9ce3c6-bc5c-4e71-98b2-c5fd7ae27386
status: done
---

# SPEC-411: the Editor's text is shared live and saves itself

Status: done
Date: 2026-11-03
Owner: task #393
Scope authorization: the person's decision of 2026-11-03 on issue #566: "#566 - let's do live
editing and autosave". It reverses the founder requirement EDT-003 ("open files and unsaved text
… neither stored nor shared", F 2026-10-07) and the rejections that followed it (SPEC-181
Decisions, ADR-0146 "Co-editing of unsaved text", ADR-0091 "Operational transforms or a CRDT").

## Goal

1. Teammates who have the same file open in the Editor see each other's keystrokes as they type,
   before anyone saves, with each other's carets and selections on the right text.
2. The Editor saves by itself: shortly after typing stops, and when the person leaves the file.
   What agents and shells write into the same file is never overwritten silently.

## Context

- **Today (EDT-003, SPEC-127, SPEC-181).** Each browser tab holds its own Monaco model; unsaved
  text never leaves it. `PUT files/content` saves with `base_sha256` (409 when the disk moved on).
  The file in front is looked at every 2 s (`GET files/content?known=`), and unedited text takes
  the disk's version as one undoable edit. Presence (`PUT files/presence`, `file.presence` to the
  sandbox's writers, ADR-0103) is relayed, not stored; offsets are in the text saved at `sha256`,
  and a teammate at another `sha256` is hidden.
- **The realtime channel** is the event WebSocket (`/api/events`): the server publishes
  `UiEvent`s on a bounded broadcast bus (1024 per subscriber; a lagging subscriber gets `resync`),
  each stream re-checks visibility and, for `writers_only` events, write access to the sandbox.
  Browsers send through HTTP requests; the socket carries nothing upstream.
- **Who may read a sandbox's files:** people with write access to the sandbox (`Action::Work`,
  EDT-002). A task worktree is changed only by its task's owner (SPEC-070, SPEC-340 EWT-003);
  the others read it.
- **The issue's analysis** (#566, task #366): co-editing needs a person's decision on EDT-003,
  then an OT/CRDT design that fits SPEC-181's disk reloads and bounds what the server holds, and
  a two-user end-to-end test.
- **The e2e suite** runs the UI against the in-page mock (`?mock=1`); a second window opened by
  the first (`window.open`, the SPEC-127 test) is how two people are simulated.

## Requirements

| ID | Requirement |
| --- | --- |
| EDT-003 (amended) | Every user still opens their own files and tabs (which files are open, the folder, the tab order and the cursor stay in that browser tab). The **text** of an open file is shared live (EDT-016) and saved automatically (EDT-019); it lives on the server only while someone edits it, in memory (EDT-018). Git in the user's shells authenticates as the user's own GitHub account. |
| EDT-016 | Live text: everyone who has a file open in the Editor (same sandbox, folder and path) edits one shared text. A keystroke shows in the others' Editors within about the network round trip plus 100 ms, before anyone saves. Concurrent edits converge: every Editor ends with the same text, and nobody's typing is lost or reordered within itself (server-ordered operational transformation of UTF-16 text). Remote edits move the person's cursor with its text. |
| EDT-017 | Live text API (write access to the sandbox, as reading files): `POST /api/sandboxes/{sid}/files/live` joins `{root?, path, client, sha256?, text?, saved?, eol?}` and answers the document `{session, revision, text, sha256, saved, disk_sha256}`; a caller who may change the folder and sends `text` starts the document from it when none exists (else from the disk); `GET …/files/live?root&path&session&since` answers the steps after `since` or, when they are no longer kept or the session differs, the whole text; `POST …/files/live/edit {root?, path, session, client, revision, op}` applies an operation made at `revision` (transformed past the steps since) and answers `{revision}`; the same edit sent again (a kept step of that person's tab after `revision`) is answered with its step and not applied again; each step is published as `file.live` to the sandbox's writers only, with its person and tab, and with its operation when it inserts at most 64 KiB (else without it, for the tabs to catch up). Refused: a folder the caller may not change (403), an unknown or ended session or a revision older than the kept steps (409), an operation with a count above 2 Mi or more than 10 000 parts, whose length does not fit, that splits a character outside the BMP, inserts another line ending than the text's or makes the text larger than 2 MiB (422), more than 30 edits a second per person and file (429). |
| EDT-018 | Where unsaved text lives and who sees it: the shared text is held in the server's memory (never in the database, never in a file other than the edited file itself), per sandbox, folder and path. Only people with write access to the sandbox (the readers of its files) can join it, read it or receive its steps; a task worktree's text is changed only by the task's owner, the others see it read-only. A person who loses write access stops receiving steps at their stream's next re-check and is refused on every request. Bounded: at most 256 documents, each at most 2 MiB, keeping its last 256 steps as long as they cost at most 1 Mi (inserted units plus 16 per part); one lock per document, so one document's edits never wait for another's; a document saved and untouched for 10 minutes is dropped (the disk has its text); one with unsaved text is kept until it is saved or reloaded, up to 24 hours untouched; a full server drops its oldest saved document, else refuses a new one (429). A restart loses unsaved text the browsers no longer hold; the first Editor to come back starts the document again from its own text. |
| EDT-019 | Autosave: the shared text is written to the file by the server (`POST …/files/live/save {root?, path, session, force?}`, with the caller's access, the existing atomic write and the document's `sha256` as the base). The Editor asks for it 1 s after the person's last keystroke, at once when the editor loses focus, the file leaves the front, the Editor hides, the sandbox leaves the front or the page is hidden (a request that outlives the page), and on Ctrl+S / ⌘S. A teammate's Editor asks 3 s after the last step it saw, so text typed by a closed browser is saved. Saves of one document run one at a time; a save of text already saved writes nothing. Every Editor shows "Saving…", "Saved" or "Unsaved" for the shared text, and a tab's ● only while it is unsaved. A file closed (or a folder switched, the view dropped) keeps its document in the tab until its typing is confirmed and written (at most 10 s); leaving the page while typing is on its way asks first (`beforeunload`); small edits go with `keepalive`, and as the page goes its typing goes at once in an edit that asks the server to save it too (`save: true`; an edit already on its way is sent again for that and applied once). Typing behind an edit on its way when the page goes is not sent; a teammate's Editor or the next one to open the file saves what reached the server. |
| EDT-020 | Disk changes while editing (agents, shells, other tools): the file in front is still looked at every 2 s (EDT-015). A newer version on disk is taken (`POST …/files/live/disk {root?, path, session, force?}`): when the shared text is saved, it becomes the disk's text as one step for everyone (one undoable edit, cursors kept, line endings normalized to the document's); when it is unsaved, it is kept, every Editor shows "Changed on disk" ("Deleted on disk" for a file gone) with **Keep mine** (save over the disk, `force`) and **Take disk version** (replace the text, for everyone, `force`; not for a deleted file), also offered by Ctrl+S, and nothing is saved automatically until one is chosen; a disk back at the text's version ends the conflict. A save that finds the disk moved on does the same. The file is never overwritten without that choice. A disk whose line endings changed gives the text its line ending (every Editor takes the whole text again). |
| EDT-015 (amended) | The look at the file in front (SPEC-181) stays as it was, but it now asks the server to take a newer version (EDT-020) instead of replacing the tab's text, and it also runs while a save is on its way: the server takes saves and the disk one at a time per document. SPEC-298's "Overwrite" / "Reload from disk" choice is now Keep mine / Take disk version, for everyone. |
| EDT-021 | Presence in live text: file presence may carry `session` and `revision`; offsets are then in the shared text at that revision, and receivers map them through the steps since and their own unsent typing, so a teammate's caret and selection stay on their text while both type. A teammate at a revision not yet received is hidden until it arrives. |
| EDT-022 | Recovery: lost events (`resync`, a reconnect, a gap in revisions, an edit not confirmed within 5 s) are caught up from `GET …/files/live` with `since`, and an edit is sent again only once a catch-up showed it did not land; a session the server no longer has (restart, idle drop) is joined again with the Editor's own text, and typing not confirmed yet is put on top of the text the document then has (only the typing since the join when the document started from the text the join sent); a catch-up answered with the whole text (the tab fell behind the kept steps) or a resync gives up typing whose edit is on its way, and the document then takes a new tab id so that edit, landing late, is nobody's; a version on disk the server refuses (no longer text) is asked for once and a toast says so. A remote step that would leave Monaco's text different from the shared text (line endings) makes the Editor take the whole text again. |

## Plan

- [x] Read the product context, the issue and related decisions, then write and validate the spec
- [x] Text operations in Rust: apply and transform UTF-16 operations, with unit tests
- [x] Live documents on the server: join, catch-up, edits and file.live to writers only
- [x] The server saves the shared text and follows the disk, with conflicts kept for a choice
- [x] Text operations in the browser and live documents in the mock, shared by two windows
- [x] The Editor edits the shared text: teammates' keystrokes and carets show live
- [x] The Editor saves by itself and offers Keep mine or Take disk version on a conflict
- [x] Earlier Editor tests follow live saving; docs, ADR and EDT-003 updated
- [x] Review round 1 findings fixed test-first: bounded edits, per-document locks, closing saves
- [x] Review round 2 findings fixed test-first: rejoin merge, per-document tab ids, leaving saves
- [x] Review round 3 findings fixed test-first: unsent typing kept, refused edits caught up
- [x] Every gate green, evidence recorded and the changes log updated
- [x] The final review approves the exact head

## Acceptance criteria

- AC-411-1 (Rust, unit): applying and transforming operations: for random pairs of concurrent
  operations on random texts (with characters outside the BMP), `apply(apply(t, a), b') ==
  apply(apply(t, b), a')`; inserts at the same place keep the server's order; lengths that do not
  fit and splits of a surrogate pair are refused.
- AC-411-2 (Rust, integration): two writers join one file; edits made at the same revision both
  land, transformed, and both get the steps in order as `file.live`; a reader of the sandbox
  without write access gets no `file.live` and is refused join, catch-up and edit; a non-owner of
  a task worktree joins it read-only (edit 403); catch-up returns steps, or the whole text once
  the steps are gone; an edit at an unknown session or a dropped revision is 409; oversize,
  misfit and surrogate-splitting edits are 422; the edit limit answers 429.
- AC-411-3 (Rust, integration): save writes the shared text with the document's base and marks
  it saved for everyone; an unchanged save writes nothing; a disk change under saved text becomes
  one step with the new `sha256`; under unsaved text it sets `disk_sha256`, a save then writes
  nothing and reports the conflict, `force` on a save (Keep mine) writes over it and `force` on a
  take of the disk (Take disk version) takes the disk's text.
- AC-411-4 (e2e, two windows): two people open one file; each types and the other sees the text
  within the poll's time, with concurrent typing at the same line converging to the same text in
  both windows, and each one's caret drawn on the right character in the other's.
- AC-411-5 (e2e): typing saves by itself about 1 s after it stops (no Ctrl+S), the tab's ●
  shows only meanwhile; leaving the file (blur, another tab, hiding the Editor) saves at once.
- AC-411-6 (e2e): an agent's write under saved text shows for both; under unsaved text both see
  "Changed on disk", nothing is saved until **Keep mine** (the person's text on disk) or **Take
  disk version** (the disk's text for both).
- AC-411-7 (e2e): a lost event stream (`resync`) and a session the server no longer has recover
  without losing the person's text.

## Test plan

- `crates/yulius-app/src/live_text.rs` unit tests (AC-411-1), a seeded randomized convergence
  test with no new dependency.
- `crates/yulius/tests/live_files.rs` (AC-411-2, AC-411-3), through the router with the event
  socket for `file.live`, like `files.rs`'s presence tests.
- `e2e/tests/live-editor.e2e.ts` (AC-411-4..7): the first window and a second one opened by it,
  both on the mock, which keeps one live document per file in the first window (the mock's
  "server") and relays its steps to both; `editor.e2e.ts` updated where saving changed.
- Waits on conditions (`expect.poll`), never on sleeps; the autosave delay is the app's constant.
- Gates: `scripts/dev ci`, the e2e suite, `scripts/dev security` (reported as disabled while
  Vidoc is off for this project).

## Decisions

- **Supersedes EDT-003's "neither stored nor shared"** and the rejections in SPEC-181's
  Decisions, ADR-0146 ("Co-editing of unsaved text") and ADR-0091 ("Operational transforms or a
  CRDT … far beyond this request"), by the person's decision of 2026-11-03 (#566). Recorded in
  [ADR-0261](../../docs/decisions/0261-the-editors-text-is-shared-live-and-saved-by-the-server.md).
- **Server-ordered OT over Yjs.** The server already orders everything (one process, one bus) and
  must hold the text anyway to save it (EDT-019) and to follow the disk (EDT-020). A Jupiter-style
  OT (one in-flight operation per client, the server transforms it past the steps since its
  revision) needs only `transform` and `apply` on the server and `compose` in the browser, about
  200 lines each side, no dependency, and the text stays a plain string the server writes. Yjs
  would need a vendored bundle and `y-monaco` in a UI without a build step (SPEC-002 ADR-01), the
  `yrs` crate to read the text on the server, tombstone growth and an encoding to bound; its
  peer-to-peer strengths do not apply behind one server.
- **HTTP up, the event socket down.** Edits go as requests like presence (access, CSRF and limits
  already apply to requests); steps come back on the event WebSocket as `file.live` with
  `writers_only`, so the stream's re-check governs who sees text. Each tab orders steps by
  revision and treats its own as the acknowledgement; a gap or `resync` is caught up by `GET`.
- **The server writes, the browsers ask.** Saving the server's text (not a browser's copy) means
  two people never save different versions, and one write path keeps EDT-020's rule. The browser
  decides when (it knows typing and focus), so the server runs no timer and holds no one's
  credentials: a save runs with the asking person's access, checked at that moment.
- **1 s after typing stops**, as VS Code's `afterDelay` default: short enough that an agent rarely
  meets unsaved text, long enough not to write a file on every key. Teammates' 3 s fallback saves
  what a closed browser typed; the server writes nothing for text already saved.
- **Conflicts are a choice, not a merge.** A merge of an agent's write into unsaved text would need
  a line diff and could interleave edits silently; the conflict window is now about a second, so
  the existing choice (now Keep mine / Take disk version, for everyone) stays.
- **Shared undo order.** Remote steps go through Monaco's undo stack (as SPEC-181's reloads do), so
  Ctrl+Z undoes the latest edit in the person's Editor, which may be a teammate's. A per-person undo
  needs an undo manager of its own; Monaco's stack cannot skip edits without corrupting text.
- **Earlier tests that described the replaced flow are rewritten or removed.** SPEC-298's
  "Reload from disk" cases (a deleted file, one too large, one back at the tab's version, a look
  during the reload) become the deleted, too-large and back-at-version cases of the conflict; the
  two SPEC-181 tests that no look starts while the browser saves are removed: the browser no longer
  writes the file, and the server serializes saves and takes of the disk (`writes` lock per
  document, Rust tests).
- **One pull request.** Autosave and sync share the save path (the server writes the shared text);
  shipping sync alone would ship a manual-save model the next change replaces, and Yulius delivers
  one pull request per task branch. Steps are commit-sized slices.

## Evidence

- **Red, then green.** Each regression test was run against the code before its fix and failed for
  the reason it names:
  - Rust `live_text::concurrent_operations_converge_in_either_order` failed with the delete branch
    of `transform` removed (a mutation check), then passed (2000 seeded random pairs, with
    characters outside the BMP).
  - Rust `live_files::edits_that_do_not_fit_are_refused` found a no-op step marking saved text
    unsaved and a dropped document still answering a join (both fixed).
  - Review round 1: `an_edit_with_counts_beyond_any_text_is_refused_and_the_document_keeps_working`
    panicked in `live_text.rs` (overflow), `an_edit_sent_twice_is_applied_once` answered revision 2,
    `large_steps_are_announced_without_their_text_and_steps_name_their_person` got the whole insert
    in the event; e2e "a file closed right after typing still saves all of it", "a document started
    again by someone else keeps the person's unconfirmed typing on top" and "a disk version that is
    no longer text is asked for once" failed on the previous client code.
  - Review round 2: e2e "typing while a lost document is joined again appears once" and "leaving
    the page sends the typing with a request to save it" failed on the previous `livedoc.js`; Rust
    `an_edit_may_ask_to_be_saved_with_it` was refused (unknown field) before `save` existed.
  - Review round 3: e2e "a closed file still saving when the page goes sends its typing…" and the
    in-page unit test "a whole-text catch-up keeps unsent typing, and a refused old edit is not
    applied twice" failed (`{"kept":"Xabc!","once":"abcd","edits":48}`) before the fix.
  - e2e "two people type into one file…" first failed on a teammate's caret drawn one character
    early (redrawn before the keystroke joined the unconfirmed typing); "a disk back at the text's
    version ends the conflict by itself" failed with the look's old comparison.
- **Review.** Independent reviewer in a fresh session (`om-code-review`), read-only: round 1
  requested changes (blocker: a crafted edit poisoned the shared lock; majors: an edit applied twice
  after a lost answer, typing lost on close, loose memory bounds), round 2 requested changes (a
  rejoin merge duplicating typing), round 3 approved `e8d97bf9` with three minors, all
  fixed test-first above; it ran the real `livetext.js`/`livedoc.js` against a fake server with 3
  clients, 300 random concurrent edits, over several seeds: every client converged.
- **Gates.**
  - `scripts/dev ci` on `371353fd` (the branch merged with `origin/dev`): exit 0, nextest 3049
    passed, 16 skipped; fmt, clippy, docs, deny, shear, macOS targets. After it only JavaScript,
    e2e tests and documents changed.
  - `cd e2e && npx e2e run` on `e915e7bc`: 809 passed, 1 skipped (72 files), including
    `live-editor.e2e.ts` (12 tests) and `editor.e2e.ts` (32).
  - `python3 scripts/check_repository.py`: passed.
  - `yulius security-scan` / Vidoc: **disabled** for this project in Yulius ("Security scan
    disabled … nothing was scanned"); not passed, not run.
- **Not testable here.** Two real browsers on two machines against a deployed server (network
  latency, a real restart of the server with Editors open), Monaco's IME composition while a
  teammate's step arrives, and `keepalive` requests outliving a real page close: the mock windows
  (one browser, one hub in the first window) and the Rust router tests stand in. Undo is shared in
  time order (a teammate's step is an undo stop of its own), as decided above.
