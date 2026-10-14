# 0071. Long injected context folds to one line in the session view

## Status

Accepted. Specified in [SPEC-090](../../.ai/specs/SPEC-090-fix-347-the-session-view-hides-ismeta-co.md).
It extends [ADR-0059](0059-a-session-is-printed-as-the-tui-in-html.md); the transcript API is unchanged.

## Context

The session view prints a session as Claude Code's TUI (ADR-0059). Text that Claude Code records
as `isMeta` and that is not one of its known wrappers was shown faint and in full, up to 16 KB:
a skill's whole body, context injected by hooks. The TUI does not print `isMeta` messages, but
some of them carry what the TUI shows another way and people act on: the Stop hook's goal line
and its feedback. Measured in this sandbox's transcripts (SPEC-090), those notes are at most
4 lines and 1,068 B; a skill body is 92 lines and 6.3 KB.

## Decision

- A meta note longer than 8 lines or 2,000 characters folds to one faint line,
  `⎿ Context added (<size>)`, that opens on demand to the text as recorded, in a bounded,
  keyboard-reachable region. Shorter notes are shown as before.
- The page decides (`foldsNote` in `pages/sessions.js`), as it does for wrappers. The server,
  `session_read` and the search index are unchanged.

## Alternatives

- Hide every non-wrapper meta note, as the TUI does. A person could no longer see the Stop hook's
  goal or its verdict, nor that a skill shaped the next turn. Rejected.
- Fold by kind, matching known texts ("Base directory for this skill:"). Hook context from other
  tools or later CLI versions would still print in full until the list learns it. Rejected for a
  size bound, which needs no list.
- Fold on the server. It would change what agents read through `session_read`. Rejected.

## Consequences

- A skill body or long hook context takes one line until opened.
- A long note of a kind people act on (a Stop hook feedback over 8 lines) folds too, one click away.
- The bound is a page constant; a new kind of note needs no change.
