# 0103. Editor presence mirrors Monaco's own boxes and reaches writers only

## Status

Accepted. Specified in
[SPEC-127](../../.ai/specs/SPEC-127-the-editor-gets-the-notepad-s-smooth-sel.md); extends
[0091](0091-notepad-presence-is-relayed-not-stored.md).

## Context

The Editor (Monaco over the files API) was asked for the notepads' look: a rounded selection that
glides, and teammates' carets, names and selections in their colour, reusing the notepads' relay
and rules, with the same access rules. Monaco draws a selection itself, as boxes it rebuilds with
`innerHTML` on every render, so CSS cannot animate them; and it knows things a page should not
recompute (wrapping, tabs, folding, bidi). Reading files needs write access to the sandbox, while
the event stream only knew "viewers of a sandbox", "administrators" and "one person".

## Decision

- **Mirror, don't measure.** Monaco keeps rendering (`roundedSelection: false`, one box per range
  and visual line). Its `.selected-text` boxes and teammates' range decorations
  (`.ed-peer-range-<n>`) are see-through, and after each render (a `MutationObserver` on
  `.view-overlays`) they are copied into the notepads' persistent, gliding shapes, in a layer
  inside `.lines-content`, so a scroll moves it in the same frame. A teammate's caret is placed by
  `getScrolledVisiblePosition`. The layer re-attaches when Monaco builds a view for another model.
- **One mechanism.** The browser loop (`presence.js`), the shape painter (`createMarks`) and the
  tab drag (`tabdrag.js`) are shared by Notes and the Editor; the server shares the validation,
  name rule and limiter (20 a second per person and file, 4096 counters). Files get their own
  route and event because their target (`root`, `path`, `sha256`) and audience differ.
- **Writers only.** `file.presence` is published with `Envelope.writers_only`; the stream delivers
  it only to people with write access to the sandbox, re-checked like visibility. A reader of the
  sandbox learns nothing about who has which file open.
- **Positions** are UTF-16 offsets in the file's text as saved at `sha256`, mapped through each
  side's unsaved typing; another `sha256` hides the person until both have the same saved text.

## Alternatives

- Compute each selection's geometry from Monaco's public position API: wrapped rows would need
  their break points, which the API does not give, and any rendering detail Monaco handles
  (folding, bidi, inline decorations) would drift.
- An overlay above the editor fed by scroll events: one frame late on every scroll and wrong
  whenever the layout changes without an event; the request ruled it out.
- Monaco decorations alone, with CSS: correct but cannot glide (rebuilt on every render).
- Relaying file presence to the sandbox's viewers like sandbox notes: leaks which files writers
  are working in to people who may not read them.

## Consequences

- The Editor depends on two class names of the vendored Monaco (`.view-overlays`,
  `.selected-text`) and on `.lines-content`; an upgrade of Monaco must keep `editor.e2e.ts`'s
  geometry test green (the drawn shapes must equal Monaco's boxes).
- One more envelope audience to remember: an event about files must use `publish_to_writers`.
- Presence costs one small request per 100 ms of movement at most, and one every 15 s while a
  file is in front.
