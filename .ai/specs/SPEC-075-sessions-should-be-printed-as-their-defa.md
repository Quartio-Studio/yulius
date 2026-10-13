---
yulius-task: ce7486ce-5755-437e-b0e7-43d88ccd9c29
status: in-progress
---

# SPEC-075: a session reads as Claude Code's TUI, faded and read-only

Status: in progress
Date: 2026-10-13
Owner: Yulius task #71
Scope authorization: user request (task #71): "Sessions should be printed as their default agents
sessions (the default TUI), not the CLI but: the contrast and the saturation should be lower to
communicate that this session is old and this is a session not a task, and it should be
read-only. You can even add some spider web pixelart at the corner. No way to write to them."
The attached screenshot shows today's session page (task #59's session): raw
`<command-name>`, `<command-args>` and `<local-command-stdout>` tags printed as text, every
message a rounded card, every tool call a folded `▸ Bash cat …` row.

## Goal

A session opened from Sessions looks like the Claude Code session it was, as the agent's own
terminal interface (TUI) printed it: `>` prompts, `⏺` messages and tool calls, `⎿` results,
slash commands as typed. It is faded (lower contrast and saturation than a live task's
terminal), so it reads as an old, finished session rather than a task, and it is visibly
read-only: a locked prompt box, nothing to type into.

## Context

- The session page is `mountSessionView` in `crates/yulius-server/web/assets/pages/sessions.js`
  (SPEC-001 SES-002, UI-049). It reads `GET /api/sessions/{id}/transcript` pages of normalized
  entries (`role`: user, assistant, summary, system; blocks: text, thinking, tool_call with
  summary, input and an optional diff, tool_result, image, meta) and renders each entry as a
  card. Tool results are shown inside the call they answer.
- `meta` blocks carry Claude Code's wrappers as recorded (`crates/yulius-app/src/transcripts.rs`
  `META_PREFIXES`): `<command-name>/goal</command-name><command-message>…<command-args>…`,
  `<local-command-stdout>`, `<local-command-stderr>`, `<bash-input>`, `<bash-stdout>`,
  `<bash-stderr>`, `<system-reminder>`, `<user-prompt-submit-hook>`, "Caveat: The messages below
  were generated…", "[Request interrupted…". The page prints them raw (the screenshot).
- A live task's terminal is xterm with `--term-bg`, `--term-fg` and the `--ansi-*` palette
  (`tokens.css`, `theme.js` `terminalColors`), in `--font-mono` (IBM Plex Mono).
- Claude Code's TUI prints a prompt as `> text`, an assistant message as `⏺ text`, a tool call
  as `⏺ Name(argument)` (green when it succeeded, red when it failed) followed by `⎿` and the
  first lines of its result, an edit as `⏺ Update(path)` with the changed lines, a slash command
  as `> /name args` with its output under `⎿`, thinking as `✻ Thinking…`, an interruption as
  `⎿ Interrupted by user`. It does not print system reminders, hook context or the caveat line.
- The page already has no text box (e2e `sessions.e2e.ts` checks it); "Continue in a new task"
  and "Delete" are the only actions and stay.
- The e2e suite runs every page through axe (WCAG 2.x AA), so lower contrast stays at or above
  4.5:1 for text.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| STU-001 | The transcript is one terminal well in the code typeface, printed as the TUI prints a session: a prompt as `>` and its text; Claude's text as `⏺` and its text; a tool call as `⏺ Name(summary)` with its result under `⎿` (the first lines and "… +N lines" when folded, all of it and the call's input when unfolded); an edit's diff shown unfolded under its call with `+`/`-` lines; thinking folded as `✻ Thinking…`; a failed call's marker in the error colour and the word "error" in its line; a subagent's entries indented under a rule. The list keeps its accessible name "Transcript", search targets (`?line=`) and paging. | e2e `sessions.e2e.ts` |
| STU-002 | Recorded wrappers read as the TUI shows them: a slash command as `> /name args`; command output (stdout, stderr) under `⎿` (stderr in the error colour); a `!` shell input as `! command` with its output under `⎿`; "[Request interrupted…]" as `⎿ Interrupted by user`. System reminders, hook context and the caveat line are not shown. Another meta text is shown faint, as written. No raw `<command-…>` or `<local-command-…>` tag is shown. A wrapper cut short by the server's bound still parses. | e2e `sessions.e2e.ts` (the parser unit, in the page, and the rendered page) |
| STU-003 | The well is faded in both themes: its own tokens (`--session-*` in `tokens.css`), lower in contrast and saturation than the live terminal (`--term-*`, `--ansi-*`) yet at least 4.5:1 for every text on its background, and its colours less saturated than their `--ansi-*` counterparts. | e2e (computed colours and contrast), axe |
| STU-004 | The page says it is a session, not a task, and read-only: the well ends with a locked prompt box that is not a control ("Read-only session · nothing can be sent") and the page has no text box, `contenteditable` or terminal. | e2e |
| STU-005 | A spider web in pixel art hangs in the well's top-right corner, drawn as an inline SVG of square pixels, hidden from assistive technology, behind the text and never catching the pointer. | e2e |
| STU-006 | SPEC-001 UI-049, an ADR, `docs/changes.md` and `e2e/README.md` describe it. | review |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] A session reads as Claude Code's TUI: prompts, Claude's messages, tool calls with results and diffs
- [x] Recorded slash commands and their output read as the TUI shows them, without raw tags
- [ ] The session looks old and read-only: a faded palette in both themes, a locked prompt box and a spider-web corner
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-STU-1: the mock's "Add Stripe checkout" session shows its prompt as a `>` line, Claude's
  text as a `⏺` line, `⏺ Read(/repo/.env)` folded with "Read 2 lines", which unfolds to the
  redacted value, `⏺ Bash(npm test -- checkout)` with `⎿ PASS src/api/checkout.test.ts`, and
  `⏺ Update(src/api/checkout.ts)` unfolded with its `+`/`-` lines; no WCAG violations.
- AC-STU-2: a session recorded with `/goal`, its stdout, a system reminder, a `!` command and an
  interruption shows `> /goal …`, `⎿ Goal set: …`, `! ls`, `⎿ Interrupted by user`, and no
  `<command-`, `<local-command-` or `<system-reminder>` text; the parser maps each wrapper,
  also one cut short.
- AC-STU-3: in the dark and the light theme, the well's text is fainter than the live terminal's
  (lower contrast against its background) but at least 4.5:1, and its prompt, ok and error
  colours are less saturated than `--ansi-*`.
- AC-STU-4: the well ends with "Read-only session · nothing can be sent"; the page has no
  `textarea`, `input[type=text]`, `[contenteditable]` or `.xterm`.
- AC-STU-5: the well has an `aria-hidden` SVG spider web in its top-right corner with
  `pointer-events: none`.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- e2e `e2e/tests/sessions.e2e.ts` (mock mode): AC-STU-1 to AC-STU-5, each red before its slice.
  The mock gains a session with recorded wrappers (a `/goal`, its stdout, a system reminder, a
  `!` command, an interruption) as the server normalizes them.
- The wrapper parser is a pure export of `pages/sessions.js`, tested in the page like
  `menuMatches` (unit, in the page).
- No server change: the transcript API and its redaction are unchanged and covered by
  `transcripts.rs` and the session tests.

## Decisions

- The TUI look is HTML in the terminal's typeface and palette, not xterm replaying ANSI: the
  transcript keeps its semantics (a list, folded details, links to lines, search targets), axe
  can check it, and nothing in the page can take keys. Replaying `claude --resume` in a terminal
  would start a process able to write and spend a subscription. See ADR-0051.
- The fade is a palette of its own (`--session-*`), not a CSS `filter`: a filter changes what
  people see but not what axe measures, and it would blur the text's rendering. Text stays at or
  above 4.5:1 (AA); "lower contrast" means lower than the live terminal's ~15:1, not below AA.
- Wrappers are parsed in the browser, not on the server: the transcript API stays the one the
  agents' `session_read` uses, the server keeps redacting the raw text, and only the page
  decides how a TUI prints them.
- System reminders, hook context and the caveat line are hidden as the TUI hides them; they stay
  searchable for agents (`sessions_search`), which read the transcript, not the page.
- The spider web is drawn by code from a small grid of square pixels in an inline SVG, so it
  needs no image file and takes the palette's colours in both themes.
