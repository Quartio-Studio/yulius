# 0153. The watched task's Working line moves as Claude Code's spinner, in CSS, with a clock from `working_since`

## Status

Accepted. Specified in
[SPEC-190](../../.ai/specs/SPEC-190-this-animation-on-request-control-should.md) (task #175);
extends [0145](0145-someone-else-s-task-is-watched-as-its-live-transcript.md) and
[0149](0149-the-watched-transcript-follows-the-bottom-as-a-state.md).

## Context

A watcher of someone else's task (the view where they may Request control) sees `✻ Working…` under
the live transcript while the agent works. It looked still: the mark never moved, and the shimmer
ran over the line's whole box (`flex: 1`, about 900 px for 70 px of words) with the shared loading
keyframes, which cross an element for an eighth of their 1.6 s cycle: a flash every 1.6 s at most.
Nothing said how long the agent had worked. The transcript otherwise imitates Claude Code's TUI.

## Decision

- The line moves as Claude Code's spinner: the mark steps through `· ✢ ✳ ✶ ✻ ✽` and back (12
  frames of 120 ms), a light runs through the words, and the time worked follows, dim:
  `✻ Working… (1m 23s)`. A watcher cannot interrupt, so no "esc to interrupt".
- The mark is a one-character window over a column of the 12 frames, moved by
  `transform: translateY()` in `steps(12)`: CSS only, no timer.
- The words shimmer on their own inline box with their own `working-sweep` keyframes (a 250 %
  background from `100%` to `0`), so the light goes from just left of the words to just right of
  them, all cycle; the loading boxes keep the shared `shimmer`.
- The time counts from `working_since` (else `state_since`), the sidebar's "working 4m" measure,
  to the second, through a one-second tick scheduled only while the line shows and cleared when it
  hides or the view goes.
- Reduced motion: the mark stands on `✻`, the words are plain dim text; the time still counts.

## Alternatives

- A new indicator (a pulsing dot, a bar). A second visual language beside the TUI's. Rejected.
- Only fixing the shimmer. The mark still stands still and nothing says how long. Rejected.
- A JS interval swapping the mark's glyph. A second timer per view for decoration. Rejected.
- Animating `content`. Not animated in every browser. Rejected.
- The turn's own time, as Claude Code shows it. The server keeps no turn start; `working_since`
  is what the rest of the UI says. Rejected for now.

## Consequences

- The mark's glyphs come from the monospace font or its fallback; a font without `✳` or `✽` draws
  them from another font, one character wide.
- One more timer per watched working task (a tick a second, text only); its lifetime is the line's.
