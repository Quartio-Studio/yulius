---
yulius-task: 6a8eb15e-ac03-4205-a095-a91e8fd76242
status: in-progress
---

# SPEC-127: the Editor gets the notepad's smooth selection and teammates' presence

Status: in progress
Date: 2026-10-14
Owner: Yulius task #114
Scope authorization: user request (task #114): "Task #97 (SPEC-112) gave the Notes panel a smooth,
animated selection with rounded corners, teammates' carets, names and selections in their own
colours (like Google Docs, but smooth and wholesome), a rate-limited server relay of notepad
presence, and drag/keyboard/menu reordering of its tabs. Do the same in the built-in Editor
(crates/yulius-server/web/assets/editorview.js, Monaco over the files API, EDT-001..005)", with
three asks (smooth rounded selection from Monaco's rendering; teammates' caret, name and selection
in the colour they have in Notes, reusing #97's relay and rules, same access rules; tab reorder by
drag, keyboard and menu), "Presence is awareness only", and the done criteria quoted in
Acceptance criteria.

## Goal

The Editor feels like the notepads:

1. A selection glides from one shape to the next with rounded corners, drawn from what Monaco
   itself renders, so it never drifts from the text when scrolled or wrapped.
2. People who have the same file open see each other: a caret, a name and a selection in the
   person's own colour (the one they have in Notes), moving smoothly, gone promptly when they
   close the file, switch to another or leave.
3. The open files' tabs can be put in any order by dragging, by keyboard and from a menu.

No live co-editing: unsaved text stays each person's own, and saving works as today (EDT-004).

## Context

- The Editor (`assets/editorview.js`, EDT-001..006) is Monaco over the files API
  (`/api/sandboxes/{sid}/files…`, `files.rs`): only people with write access (`Action::Work`)
  list, read or change files; `root` is `sandbox` or `task:<id>` (reading another person's task
  worktree is allowed, writing needs its owner, SPEC-070). Open files and unsaved text live in the
  browser tab only. A file is opened with its `sha256`; a save sends it as `base_sha256` (409 on a
  stale one). Tabs are a `role=tablist` strip with Delete/Backspace closing (`closableTabKeys`).
- SPEC-112 (ADR-0091) built notepad presence: `PUT …/notes/presence` relays `note.presence` on the
  bounded event bus to the notepad's readers, at most 20 a second per person and notepad (4096
  counters), stored nowhere. In the browser, `notes.js` holds the liveness loop (100 ms throttle,
  15 s heartbeat, `hello` on show and after `resync`, answers at most once a second, `null` on
  hide and with `keepalive` when the page goes, retry with backoff, one update in flight, 45 s
  expiry, a per-tab `client`) and `textlayer.js` draws the notepad's own and teammates' marks
  (rounded per-line shapes that glide, caret with a name tag, colour `peerColour(user id)` from
  `--peer-1…8`, name = GitHub login else display name).
- The event stream (`routes/events.rs`) filters an envelope by `admin_only`, one `user`, and a
  sandbox's **viewers** (`Action::View`). Files need `Action::Work`, so file presence needs a new
  audience: the sandbox's writers. A reader must never learn which files are open or where.
- Monaco renders a selection as `.cslr.selected-text` boxes (one per range per visual line,
  absolutely placed in `.view-overlays` inside `.lines-content`, which Monaco moves on scroll) and
  re-creates them on every render (`innerHTML`), so CSS cannot animate them. With
  `roundedSelection` it adds corner pieces. Decorations with a `className` render the same way.
- Yulius never runs inside a frame (`boot.js`), and the e2e harness drives one tab, so a
  two-person e2e opens the second person's window from the page (`window.open`, same origin) and
  scripts it from there; two mock backends relay presence over a `BroadcastChannel`.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| EDT-007 | Smooth selection: in the Editor, Monaco's selection highlight is not painted; each of its boxes (one per range and visual line, several with several cursors) is drawn instead as a rounded shape (radius 4 px, the accent selection colour, softer when the editor does not have the keyboard) in a layer inside Monaco's scrolled content, behind the text. A shape glides (about 120 ms, eased) as the selection grows, shrinks or moves; scrolling moves shapes with the text in the same frame and new lines do not glide in; word wrap on or off draws the wrapped rows. Monaco's caret glides (`cursorSmoothCaretAnimation`). Reduced motion collapses the transitions (global rule); forced colours bring the native highlight back and hide the layer. | e2e `editor.e2e.ts` (shapes match Monaco's boxes, rounded, transition on `transform`, native hidden; wrap on/off; scrolled; blurred) |
| EDT-008 | File presence API: `PUT /api/sandboxes/{sid}/files/presence` takes `{root, path, sha256, selection: {anchor, head} \| null, hello?, client?}` (unknown fields refused): the caller's selection in UTF-16 offsets of the file's text as saved at `sha256` (64 hex digits, or empty for a file not yet read), `null` when they left it. It needs write access to the sandbox (as reading files does: else 404/403), normalizes the path like the files API (`.git` and escapes refused) and relays `file.presence {sandbox_id, root, path, user_id, client, name, sha256, selection, hello}` only to the people with write access to that sandbox; stored nowhere; 204. Offsets above 2 Mi (the largest file) are refused (422), as is a bad `client` or `sha256`. | Rust `files::presence_is_relayed_to_writers_only`, `files::presence_is_refused_when_malformed` |
| EDT-009 | Bounded like notes: file presence shares the notepad's limiter: at most 20 updates per second per person and file, 429 past it (not relayed), at most 4096 counters. | Rust `files::presence_is_rate_limited` |
| EDT-010 | Teammates in a file: while the Editor is shown with a file in front, the browser sends that file's presence with the notepad's rules (NOT-020: throttle, heartbeat, `hello` and answers, `resync`, retries, one in flight, per-tab `client`, 45 s expiry) through the same browser module; switching to another file, closing it, switching folder, hiding the Editor, another sandbox coming to the front or the page going sends `null` for it (`keepalive` when the page goes). Others' updates for the same sandbox, root and path draw a caret with a name tag and their selection in their Notes colour (`peerColour(user id)`), gliding like EDT-007, positioned by Monaco (wrap and scroll correct); offsets are mapped through the unsaved typing on both sides, someone at another `sha256` is hidden until both sides have the same saved text, the person's own updates (any tab) are not drawn, and a person who left fades out. | e2e `editor.e2e.ts` (two signed-in people on one file see each other's caret, name and selection in their colour, live, and lose it when one switches file, closes it or hides the Editor; mapped over typing; another sha hidden; own echo ignored; hello answered) |
| EDT-011 | Tab order: the Editor's file tabs are reordered by dragging one along the strip (the others make room, it settles into place; a press without movement is a click and Escape cancels), by Alt+← / Alt+→ on a focused tab (focus follows it), and by "Move left" / "Move right" in its context menu (which also offers Close). The order lives with the open files (this browser tab only, like them) and survives a sandbox switch (SPEC-113). | e2e `editor.e2e.ts` (drag, keyboard, menu) |
| EDT-012 | Accessible: the drawn shapes and marks are `aria-hidden`; the tab's moves are announced by its position and `aria-keyshortcuts`; the name tag keeps AA contrast (the notepad's tokens); the Editor with a teammate shown keeps no WCAG A/AA violations. | e2e `editor.e2e.ts` (axe with a teammate shown) |

## Acceptance criteria

- AC-127-1 (from the request): an e2e test with two signed-in users on the same file shows each
  one's caret, name and selection to the other in their colour, and removes it when one leaves.
- AC-127-2: tab reorder works by drag and keyboard (and the menu).
- AC-127-3: the selection looks right (Monaco's boxes, rounded, gliding) with word wrap on and
  off and when scrolled.
- AC-127-4: a person with only read access to the sandbox never receives `file.presence`.
- AC-127-5: spec, `web/README.md`, `docs/api.md` and the changes log are updated, and the change
  is delivered to `dev` on green CI.

## Plan

- [ ] Read the product context and related work, then write and validate the spec
- [ ] The notepad's presence loop is a shared browser module, notes unchanged
- [ ] The server relays file presence to the sandbox's writers, rate-limited
- [ ] The Editor draws its own selection rounded and gliding, from Monaco's rendering
- [ ] Teammates' carets, names and selections show in the Editor (two-user e2e)
- [ ] The Editor's file tabs reorder by drag, keyboard and menu
- [ ] Docs, ADR and the changes log updated
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, merged into dev on green CI

## Design

**Shared browser presence (`assets/presence.js`, from `notes.js`).** `createPresence({ put,
state, draw, myId })` holds what `notes.js` did inline: `state()` says what to send (`null` when
the source cannot speak yet, else `{revision, selection}`), `put(body, {keepalive})` sends it
(the source adds its own fields: `root`, `path` for files), `draw(peers)` redraws. It exposes
`soon()`, `setShown(next, {going})`, `peer(event)`, `lost()`, `tick()`, `destroy()` with the same
timings. `notes.js` keeps its behaviour (its e2e suite is the guard). `peerColour` stays in
`textlayer.js`, so a person has one colour in Notes and in the Editor.

**Server.** `App::file_presence(ctx, sid, input)` resolves the target with `file_target` (write
access, normalized relative path; the root must exist), checks `sha256`, `client` and offsets
(`≤ MAX_FILE_BYTES`, since a UTF-16 offset never exceeds the UTF-8 length), counts the update with
the notepad limiter under `user|file:sid:root:path`, and publishes `UiEvent::FilePresence` through
a new `EventBus::publish_to_writers(sandbox, event)`: the envelope gets `writers_only: true`, and
the stream checks `Action::Work` on the sandbox (cached like the view check, re-checked with it).
The validation of a selection and a client and the name rule move to helpers shared with
`note_presence`.

**Drawn selection (`assets/editorlayer.js`).** Monaco keeps rendering (with
`roundedSelection: false`, so one box per range and visual line); CSS makes `.selected-text`
transparent. A layer is inserted in `.lines-content` right after `.view-overlays` (behind
`.view-lines`), so it scrolls with the text. A `MutationObserver` on `.view-overlays` (Monaco
re-renders it on every selection, scroll, layout and wrap change) reads the boxes of
`.selected-text` (own) and of `.ed-peer-range-<n>` decorations (teammates, transparent too)
relative to the layer and hands them to the notepad's shape painter (the pool that reuses shapes in
order so they glide, extracted from `textlayer.js` as `createMarks`). A teammate's caret is placed
from `editor.getScrolledVisiblePosition(head)` (hidden when the line is not rendered). A scroll
marks the next draw as a jump (no glide), as the notepad does.

**Positions.** Each open tab keeps `saved`, the model's text at `sha` (Monaco normalizes line
ends the same way on both sides). A sender maps its selection (model offsets) through
`textEdit(model, saved)` and sends it with `sha256`; a receiver at the same `sha` maps it through
`textEdit(saved, model)`; another `sha` hides that person.

**Presence per file.** Each open tab has its presence (`createPresence`) keyed by `root` and
`path`; the tab in front is shown while the Editor is shown, the page is visible and the sandbox
is in front. `store.js` emits `file-presence`; `mock.js` relays the route to the page and, with
`BroadcastChannel('yulius-mock-presence')`, to other mock pages of the same origin, signed in as
another person with `?mock=1&as=<login>`.

**Tab order.** The strip gets the notepad's drag (pointer events, 4 px threshold, FLIP settle,
Escape cancels), Alt+←/→ and a context menu, by extracting the notepad's drag code into
`assets/tabdrag.js` (`attachTabDrag(tablist, {keys, order, commit})`) used by both.

```mermaid
sequenceDiagram
    participant A as Alice's Editor (README.md)
    participant S as Yulius server
    participant B as Bob's Editor (README.md)
    A->>S: PUT /api/sandboxes/sb/files/presence {root sandbox, path README.md, sha256 9f…, selection 10..24, hello}
    S-->>B: file.presence (writers of sb only) {user alice, 10..24, hello}
    B->>S: PUT …/files/presence {…, selection 3..3}
    S-->>A: file.presence {user bob, 3..3}
    Note over A,B: Monaco renders each other's range as a decoration; the layer draws it rounded and gliding
    A->>S: PUT …/files/presence {…, selection null} (switched file)
    S-->>B: file.presence {user alice, null}: Alice fades out
```

## Test plan

- Rust `crates/yulius/tests/files.rs` (or the notes file's neighbour): relayed to a writer with
  the right envelope and normalized path; a read-only member's stream never gets it; a reader
  sending gets 403/404; unknown fields, a bad `sha256`, a bad `client`, an offset over 2 Mi and
  `.git` paths are refused; the 21st update in a second answers 429 and is not relayed.
- e2e `editor.e2e.ts`: own selection shapes equal Monaco's boxes, rounded, native transparent,
  with wrap on and off and after a scroll, blurred softer; two signed-in people (a second window
  as `bob`) see each other's caret, name and selection in their colours, which move live and go
  when one switches file, closes it and hides the Editor; a teammate's offsets map over unsaved
  typing and another sha hides them; the own echo is not drawn; a hello is answered; tabs reorder
  by drag, Alt+arrows and the menu; axe with a teammate shown.
- `notes.e2e.ts` passes unchanged (the shared presence module and painter).
- Not covered: real browsers against a real server (the relay is covered in Rust, the drawing
  against the mock), browsers other than Chromium, forced colours (CSS rule only).

## Decisions

- Mirror Monaco's own boxes rather than compute geometry or overlay the editor: Monaco already
  knows wrap, tabs, fonts, folding and bidi; re-creating its boxes as persistent shapes is the only
  way to animate them (it rebuilds its overlay HTML on every render), and putting the layer inside
  `.lines-content` makes scrolling exact. Cost: it depends on two class names of the vendored
  Monaco (`.view-overlays`, `.selected-text`), recorded in ADR-0103 and checked by the e2e.
- File presence goes to the sandbox's writers, not its viewers: the files API itself needs write
  access, so a reader learns nothing about which files are open (a new envelope audience rather
  than a weaker rule).
- One relay, one limiter, one browser loop and one painter for Notes and the Editor (the request:
  "reuse #97's presence relay … instead of building a second mechanism"); a separate route and
  event because the target (root, path, sha256) and the audience differ.
- The tab order is not stored: the open files themselves live in this browser tab only (EDT-001).
