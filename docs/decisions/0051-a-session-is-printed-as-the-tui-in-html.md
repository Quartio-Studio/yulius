# 0051. A session is printed as Claude Code's TUI in HTML, faded by its own palette

## Status

Accepted. Specified in [SPEC-075](../../.ai/specs/SPEC-075-sessions-should-be-printed-as-their-defa.md).
It changes how UI-049 (SPEC-001) shows one session; the transcript API is unchanged.

## Context

The Sessions page showed a session as rounded cards, with Claude Code's wrappers
(`<command-name>/goal</command-name>`, `<local-command-stdout>`) printed raw. The person asked for
a session to look like the agent's own TUI, with lower contrast and saturation, so that it reads as
an old session and not a live task. It must stay read-only, and a pixel-art spider web in the corner
was welcome. The page's transcript is the normalized one the server returns. Agents read the same
API through `session_read`. A live task's terminal is xterm in the `--term-*` and `--ansi-*`
palette, and every page is checked with axe (WCAG 2.x AA).

## Decision

- The transcript is HTML styled as the TUI, in the terminal's typeface: `>` prompts, `⏺`
  messages and tool calls, `⎿` results folded to their first lines, `✻ Thinking…`, edits as
  `Update(path)` with their lines. It stays a list with folded `details`, so links to a line,
  search targets and screen readers keep working.
- Claude Code's wrappers are parsed in the browser (`metaLines` in `pages/sessions.js`): slash
  commands and `!` commands as typed, their output under `⎿`, and an interruption as "Interrupted
  by user". Reminders, hook context and the caveat line are hidden, as the TUI hides them.
- The fade is a palette of its own, `--session-*` in `tokens.css`, for both themes. Its text is
  fainter than the live terminal's (about 8.8:1 in dark, 7.3:1 in light, against about 15:1), still
  at least 4.5:1, and its colours are less saturated. `check_repository.py` checks its pairs.
- The well ends with a locked prompt box that is not a control. A spider web drawn by code as
  square SVG pixels floats in its top-right corner, hidden from assistive technology, and the
  first lines wrap beside it.

## Alternatives

- Replaying the session in xterm (ANSI output, or `claude --resume` in a terminal). The first gives
  up the list's semantics, axe checks and links to lines. The second starts a process that can
  write and spends a subscription. Rejected.
- A CSS `filter: saturate() contrast()` over the well. People would see the fade, but axe would
  measure the unfiltered colours, and the filter softens text rendering. Rejected for explicit tokens.
- Parsing the wrappers on the server. That would change the API agents read and the search
  passages. The server already redacts the raw text, and only the page decides how to print it.
  Rejected.

## Consequences

- The transcript API, its redaction and agents' reads are unchanged. The page alone decides the look.
- A wrapper that Claude Code adds later shows faint, as written, until `metaLines` learns it.
- Hidden reminders stay searchable for agents, but people do not see them on the page.
