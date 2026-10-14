# 0075. Menu descriptions wrap instead of being cut

## Status

Accepted. Specified in
[SPEC-094](../../.ai/specs/SPEC-094-menus-show-an-option-s-whole-description.md). Replaces the
description bullet of [0030](0030-menus-filter-when-long.md).

## Context

ADR-0030 cut every menu item's description to one line with an ellipsis, the full text in a
tooltip and on the keyboard-focused item, so that "Run after" (then two lines per task) stayed
inside the window. Since then "Run after" rows have no description, and the remaining descriptions
explain a choice before it is made: the composer's start menu showed Supertask as "Waits for this
project's running tasks, then ru…", which a person cannot read without hovering, and cannot read
at all on a touch screen.

## Decision

- An item's description (or a disabled item's reason) wraps onto as many lines as it needs and is
  never cut; a word longer than the line breaks. It carries no tooltip.
- Labels and hints stay on one line, cut with an ellipsis when too long.
- The menu keeps its width rule: as wide as its widest content, at most 320 px and the window less
  16 px. Wrapping does not change a line's max-content width, so menus are as wide as before and a
  description that fits still takes one line.

## Alternatives

- Widen the menu: every menu gets wider and a longer text is still cut.
- Keep the cut text and reveal it on hover or focus (ADR-0030): touch and pointer users who do not
  hover never see it.
- Clamp to two or three lines: still truncates the text the request wants whole, and descriptions
  are short sentences, so the clamp would rarely help.

## Consequences

- A row with a long description is taller than 44 px; menus with several such rows are taller,
  and still scroll inside the window (MNU-001).
- New descriptions should stay one or two short sentences: they are now read in full.
