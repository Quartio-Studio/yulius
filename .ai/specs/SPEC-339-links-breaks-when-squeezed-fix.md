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
  - the row's last non-blank cell is in the last column, or in one of the last 2 columns in Claude
    Code's prompt, every row of which keeps one column of padding. A row belongs to the prompt
    when walking up through blank rows and indented rows without a marker reaches a `❯` row
    that is not a menu's cursor (`❯ 1.`). A row with another marker (`●`, `⏺`, `-`, `⎿`, `▎`,
    …), a soft-wrapped row or text at the first column stops the walk with no padding;
  - the next row is not soft-wrapped;
  - the next row's text starts, with a non-blank character, in the column where the row's text
    starts. That column comes after leading spaces, one marker followed by a space (such as `❯`,
    `●`, `⏺`, `-`, `⎿` or `▎`) and a list number (`1.`) followed by a space. The cells before
    it are blank, or repeat the quote bar `▎`, as Claude Code repeats it on each row of a quote;
  - the word that spans the break is longer than the row's text (from that column to its last
    cell). Claude Code splits only words that do not fit; a shorter word moves whole to the next
    row;
  - the next row does not start a new URL (`scheme://`), unless the cut fell inside the scheme
    (`htt|ps://`).

  A URL (the web-links addon's pattern) that crosses at least one such break is a single link over
  all of its rows, and activating it opens the joined URL.
- **WRL-002 Unchanged elsewhere.** URLs that fit on one row, soft-wrapped URLs and file links work
  as before. Text after a URL is never joined onto it when the URL ends short of the right edge,
  when the text starts at a different indent, or when the URL and the word together fit a row.
- **WRL-003 Bounded.** A link spans at most 32 rows. A longer run of continued rows is no link at
  all, rather than a link to a cut address. The provider reads only the rows around the pointer's
  row (32 below; 32 above plus 64 more for finding the prompt), only when xterm asks (the pointer
  rests), never per output chunk.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Spec and issue for URLs Claude Code wraps onto the next row
- [x] Failing regression tests: a wrapped URL is one link, opened whole
- [x] Terminal links a URL continued on the next rows as one link
- [ ] Independent review answered
- [ ] Gates green, docs and changes entry, merged into dev

## Acceptance criteria

- The rows Claude Code printed (prompt, a later prompt row, answer text, list item, numbered item,
  quote, code, a word cut mid-row) give one link each, to the whole URL (`findContinuedUrls`, e2e).
- In the mock task terminal, a press on the first row of a wrapped URL, on the next row, or on the
  last row opens the whole URL (e2e).
- No join for a row that ends short of the edge, a different indent, a word that fits the row
  (also exactly at the boundary), an answer row (first or later) ending one column short of the
  edge, a next row
  that starts a new URL, or a run longer than 32 rows (e2e, pure function).
- The existing terminal link e2e tests still pass.

## Test plan

- `e2e/tests/terminal.e2e.ts`: a pure-function test of `findContinuedUrls`. It replays rows
  recorded from Claude Code 2.1.292 at 60 columns, including the prompt, answer text, a list item,
  a numbered item, a quote and code, and checks the negative and limit cases. A browser test in
  the mock (`?wrapurl=1`) cuts a 370-character URL for the terminal's width when drawn, as Claude
  Code does, and presses the first row, the next one and the last, with `window.open` recorded.
  Without the provider, the press on the continuation row finds no link (mutation checked).
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
  from Claude Code's measured layout. A shell prompt that also draws `❯` (starship, pure) makes
  the indented rows under it count as prompt rows, so a URL there may end one column short of the
  edge too; shell output rarely starts indented, and the word-length rule still applies. One case
  stays ambiguous: a URL that ends exactly at the
  edge, followed by a word long enough that the two together exceed the row, looks the same as a
  longer URL cut there, and is joined. See
  [ADR-0230](../../docs/decisions/0230-a-url-a-program-wraps-itself-is-one-terminal-link.md).
- **One range over all rows.** The link covers every cell from its first character to its last, so
  hovering any row underlines the whole URL, as xterm does for soft-wrapped links. The indent cells
  of a continuation row fall inside the range, which is a cosmetic cost.
