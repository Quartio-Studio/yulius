---
yulius-task: 8aaaccbb-3262-4079-8467-1cf266019307
status: in-progress
---

# SPEC-339: A URL Claude Code wraps onto the next row is one terminal link

Status: in-progress
Date: 2026-10-27
Owner: Yulius task #323
Scope authorization: task #323 (`/goal`, with a screenshot of a squeezed task terminal: Claude
Code's prompt row `❯ where is login and pass for https://quartio-v2-dev` continued on the next row
as `.vm.quartio.ai/admin/connectors`), "links breaks when squeezed, fix". Fixes #940. Extends
SPEC-001 UI-018 (web links in the terminal open as links).

## Goal

In a narrow terminal, a URL that Claude Code breaks across rows becomes a single link. Hovering any
of its rows shows the link, and a click opens the full URL instead of the first fragment.

## Context

- **Symptom.** In a squeezed task terminal, only the first row of a long URL is a link, and a
  click opens a truncated address (`https://quartio-v2-dev`). The continuation row is plain text.
- **Root cause.** Claude Code lays out every row itself: it breaks a word longer than the row (a
  URL) at the right edge, continues it on the next row at the text's indent, and moves the cursor
  to that row (`\r`, `CSI B`). It sends no OSC 8 hyperlink. Reproduced with real Claude Code
  2.1.292 (`claude --resume` of a synthetic session, rendered with pyte) at 60 columns:

  ```
  ❯ where is login and pass for https://quartio-v2-dev.vm.qua
    rtio.ai/admin/connectors?tab=all&page=2
  ● Open https://quartio-v2-dev.vm.quartio.ai/admin/connectors
    ?tab=all&page=2 and sign in.
    - Docs at https://quartio-v2-dev.vm.quartio.ai/admin/conne
      ctors?tab=all&page=2/help
    https://quartio-v2-dev.vm.quartio.ai/admin/connectors?tab=
    all&page=2
  ```

  xterm marks a row as a continuation (`isWrapped`) only when it soft-wraps text itself. The
  web-links addon (`terminal-core.js`) joins only those rows, so it sees `https://…vm.qua` and
  nothing on the row below.
- **Why tests missed it.** The mock transcript (`mock.js`) and the terminal e2e only had URLs that
  fit on one row (`https://example.com/docs/guide.md`).
- xterm asks every link provider for a row. Where two links overlap, the provider registered first
  wins (`Linkifier2._checkLinkProviderResult`, `_removeIntersectingLinks`).

## Requirements

- **WRL-001 Continued URL.** A row continues into the next one when all of these hold:
  - the row's last non-blank cell is in the last 2 columns, since Claude Code's prompt row keeps one
    column of padding;
  - the next row is not soft-wrapped;
  - the next row starts, after its indent, with a non-blank character, and its indent equals the
    column where the row's text starts (after leading spaces and one marker such as `❯`, `●`,
    `⏺`, `-` or `⎿` followed by a space);
  - the word that spans the break is longer than a row can hold from that indent. Claude Code
    splits only words that do not fit; a shorter word moves whole to the next row;
  - the next row does not start a new URL (`scheme://`).

  A URL (the web-links addon's pattern) that crosses at least one such break is a single link over
  all of its rows, and activating it opens the joined URL.
- **WRL-002 Unchanged elsewhere.** URLs that fit on one row, soft-wrapped URLs and file links work
  as before. Text that follows a URL ending short of the right edge, or that starts with a
  different indent, is never joined onto it.
- **WRL-003 Bounded.** A link spans at most 8 rows. The provider reads only the rows around the
  pointer's row (a window of 8 above and 8 below), only when xterm asks (the pointer rests), never
  per output chunk.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Spec and issue for URLs Claude Code wraps onto the next row
- [ ] Failing regression tests: a wrapped URL is one link, opened whole
- [ ] Terminal links a URL continued on the next rows as one link
- [ ] Independent review answered
- [ ] Gates green, docs and changes entry, merged into dev

## Acceptance criteria

- The rows Claude Code printed above (prompt, answer text, list item, code) give one link each,
  to the whole URL (`findContinuedUrls`, e2e).
- In the mock task terminal, hovering the continuation row of a wrapped URL shows a link, and a
  press there opens the whole URL (e2e).
- No join for a row that ends short of the edge, a different indent, a word that fits a row, or a
  next row that starts a new URL (e2e, pure function).
- The existing terminal link e2e tests still pass.

## Test plan

- `e2e/tests/terminal.e2e.ts`: a pure-function test of `findContinuedUrls` on the recorded Claude
  Code rows and on the negative cases. Then a browser test in the mock: the task terminal at 70
  columns (`?othercols=70`, so the grid does not follow the window) shows a URL wrapped as Claude
  Code wraps it. The test hovers its continuation row and presses, with `window.open` recorded.
- Full e2e suite and `scripts/dev ci` (the web assets are embedded in the server).

## Decisions

- **A link provider in front of the addon, not a replacement.** The new provider returns only URLs
  that cross a hard break, and it is registered before the web-links addon, so xterm prefers its
  link where both match the first row. Every other URL stays the addon's, unchanged.
  Rejected: replacing the addon with our own URL provider, a larger change to behaviour that works
  today. Also rejected: rewriting Claude Code's output on the server, which would mean parsing a
  TUI's cursor moves.
- **Strict joining rules over generous ones.** A wrong join opens a wrong address, while a missed
  join leaves today's behaviour. So every condition in WRL-001 must hold, and each one is taken
  from Claude Code's measured layout. See
  [ADR-0230](../../docs/decisions/0230-a-url-a-program-wraps-itself-is-one-terminal-link.md).
- **One range over all rows.** The link covers every cell from its first character to its last, so
  hovering any row underlines the whole URL, as xterm does for soft-wrapped links. The indent cells
  of a continuation row fall inside the range, which is a cosmetic cost.
