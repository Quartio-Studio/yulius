# 0091. Notepad presence is relayed, not stored, and drawn by the page

## Status

Accepted. Specified in
[SPEC-112](../../.ai/specs/SPEC-112-i-want-to-be-able-to-move-those-3-bars-t.md).

## Context

People asked to see each other in the shared notepads "like in Google Docs but smooth": a
teammate's caret, name and selection in their own colour, and their own selection drawn rounded
and gliding. A `<textarea>` paints its selection itself, in square boxes that jump, and nothing in
the server knew who had a notepad open. Notes are saved whole with a revision 600 ms after typing
(ADR-0032), so two people's texts differ only briefly.

## Decision

- **Relay.** `PUT /api/notes/presence` and `PUT /api/sandboxes/{sid}/notes/presence` take
  `{revision, selection: {anchor, head} | null, hello}` and publish `note.presence` on the existing
  event bus to the notepad's readers (everyone signed in for Team, the sandbox's viewers for a
  sandbox; nothing for "Me"). The server stores nothing; it counts at most 20 updates a second per
  person and notepad (429 past it) so one browser cannot flood the bounded bus.
- **Liveness in the browser.** A browser tab (a random `client` id) sends its selection at most
  every 100 ms, again every 15 s while the notepad is shown, `hello` when it is shown and after
  lost events (the others answer at once, at most once a second), `null` when it is hidden or the
  page goes, and a refused update again after 1 s, doubling up to 15 s. A tab not heard of for
  45 s fades out.
- **Positions.** Offsets are UTF-16 offsets in the saved text at `revision`; each side maps them
  through its own unsaved typing (the common prefix and suffix of the two texts). A teammate at
  another revision is hidden until both meet again.
- **Drawing.** The notepad hides the native selection highlight (not the caret) and draws every
  selection itself in a layer under the transparent text box: one rounded shape per line,
  measured with a DOM `Range` over a hidden mirror of the text box, moved with CSS transitions.

## Alternatives

- A server-held presence map with a `GET` on load: an exact list at once, but state to bound,
  expire and re-check per sandbox permission, for a gain of one round trip over `hello`.
- Operational transforms or a CRDT for live text: the right tool for live co-editing, far beyond
  this request; whole-text saves within 600 ms keep the mismatch window short, and hiding is
  honest where a mapped guess could point at the wrong word.
- Drawing the caret too: it would lag typing and fight IMEs and magnifiers; the native caret stays.

## Consequences

- Presence costs no storage and survives restarts trivially: browsers say hello again after the
  reconnect's `resync`.
- A person who keeps a notepad open sends one tiny request every 15 s; a dragged selection at most
  ten a second.
- Until both sides save and reload, a teammate who typed elsewhere is hidden rather than misplaced.
- The layer's mirror must copy every style that affects wrapping; a new one on `.notes-text` must
  be added to `MIRRORED` in `textlayer.js`.
