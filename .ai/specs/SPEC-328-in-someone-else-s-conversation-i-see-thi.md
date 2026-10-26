---
yulius-task: f570e659-52d8-4b7a-b6af-58203338638b
status: done
---

# SPEC-328: A background task's wake-up reads as the TUI prints it, not as a raw prompt

Status: done
Date: 2026-10-26
Owner: Yulius task #312
Scope authorization: task #312 (`/goal`), fixes #913. Amends the read-only transcript printing
of [SPEC-075](SPEC-075-sessions-should-be-printed-as-their-defa.md), used by the Sessions page
and by the watcher view of [SPEC-179](SPEC-179-ui-problem-when-someone-else-has-a-contr.md); no
server change.

## Goal

When a person watches a task someone else owns (or opens its session on the Sessions page), the
moment its background work ends reads as one line saying what ended, as Claude Code's TUI prints
it, instead of a `>` prompt box full of `<task-notification>` markup (`<task-id>`,
`<tool-use-id>`, `<output-file>`, `<usage>`…) labelled as the person's prompt with its time.

## Context

- **Symptom.** The attached screenshot (watching @BartlomiejNowak04's task): a prompt box
  `> <task-notification> <task-id>a8f9…</task-id> <tool-use-id>… <output-file>/tmp/claude-0/…
  <status>completed</status> <summary>Agent "Independent review of fix diff" finished</summary>
  <note>…</note> <result>The gate is still running…</result> <usage>…</usage>
  </task-notification>` with "17 min ago".
- **Root cause.** Claude Code wakes an idle session with a user record whose text starts with
  `<task-notification>` (SPEC-079, SPEC-085: the harness counts it as a wake, not a prompt). The
  server's transcript reader (`crates/yulius-app/src/transcripts.rs` `text_block`) keeps it as a
  user `text` block, since `<task-notification>` is not one of its `META_PREFIXES`; the page's
  renderer (`transcriptEntries` in `crates/yulius-server/web/assets/pages/sessions.js`) prints
  every user `text` block as a typed `>` prompt and labels the entry "Prompt" with its time.
  The watcher view (`watchview.js`) reuses that renderer, so watchers see it; the owner sees the
  live terminal, which prints it properly.
- **Why tests missed it.** The SPEC-075/SPEC-090 tests cover the wrappers of slash commands,
  `!` commands, reminders and hook context; no mock transcript has a wake-up.
- **What else the cause could break.** Any other text Claude Code writes as a user record with
  its own markup; those known are already meta. Search hits keep the role "Prompt" for a
  wake-up (the server indexes it as user text); left as is, since the hit's text is still the
  right passage.

## Requirements

- WUP-001: A user text block that starts with `<task-notification>` prints as one `⏺` line,
  faint, naming what ended: its `<summary>`; without one, "Background task" and its `<status>`.
  Several notifications in one record print one line each.
- WUP-002: The notification's `<result>`, when it has one, opens from that line (a fold) as
  output under `⎿`; its other tags (`<task-id>`, `<tool-use-id>`, `<output-file>`, `<note>`,
  `<usage>`) are never printed.
- WUP-003: An entry made only of notifications is not a prompt: not labelled "Prompt", no time,
  no prompt box; its screen-reader label is "Background task".
- WUP-004: A notification the server cut at its bound (no closing tags) still parses to its
  summary and the result as far as it was kept.
- WUP-005: Text after the notifications in the same record stays readable as a faint note.

## Plan

- [x] Spec and issue for the raw wake-up prompt
- [x] Regression test: a wake-up reads as the TUI prints it
- [x] Transcript prints a wake-up as one notification line
- [x] Independent review answered
- [x] Gates green, docs and changes entry, delivered

## Acceptance criteria

- AC-1 (WUP-001, WUP-002, WUP-003): on the mock session `ses_task_4` the wake-up entry shows
  `⏺ Agent "Review" finished`, no `<task-notification>`/`<task-id>`/`<usage>` text, no "Prompt"
  label and no time; opening the line shows its result; axe finds no violations. Fails on dev.
- AC-2 (WUP-001, WUP-004, WUP-005): `notificationsOf` (unit, in the page) parses a full, a
  summary-less, a cut and a two-notification text, and keeps trailing text; plain prompts and a
  tag mentioned in prose are not notifications.

## Test plan

- `e2e/tests/sessions.e2e.ts`: the unit test for `notificationsOf` and the rendering test on
  `ses_task_4` (a wake-up added to the mock webhook transcript).
- Full gate: `scripts/dev check`, `scripts/dev lint`, `scripts/dev test`, the e2e suite.

## Decisions

- **Client-side only.** Marking `<task-notification>` as meta on the server would also drop the
  agent's result from session search (meta is not indexed) and change `session_read` for agents;
  the defect is how the page prints it, so the page parses it. Reversible.
- **What the line says.** Claude Code's own `<summary>` (`Agent "…" finished`, `Background
  command "…" completed`), as the TUI's line names it; the result folds, as tool results do.
- **Review round 1** (request changes, all own code, fixed test-first): `ss-notice` was already
  the page header's class (inline-flex, centred) and threw the line off the gutter and the opened
  result beside it, so the line's class and the entry's role are `ss-wake`; a result ends at the
  `</result>` followed by `<usage>`, the closing tag or the cut (Claude Code's order), skipping
  quoted pairs and a quoted close; a cut result says "Cut here" inside its fold; the Plan was
  kept in step. New assertions: the `⏺` mark's x equals a neighbouring Claude line's, the opened
  result sits under its line, and two quoting results parse whole.
- **Review round 2** (approve with one minor, own code, fixed test-first): a result naming a
  lone `<result>` left the depth count unbalanced and swallowed the next notification; without a
  balanced close, the first `</result>` that Claude Code's order follows ends it (unit case
  `proseOpen`). Round 3 approved `2da70bff` with no findings.

## Evidence

- Regression tests fail on dev (`84b45934` web assets): `notificationsOf is not a function`, and
  the wake-up entry read `Prompt > <task-notification> <task-id>a8f93b704fcd18284</task-id> …
  9 h ago`. Round-1 assertions failed before their fix (`expected 524 to be 340`, a quoted
  `</result>` read `Fix: close`), round 2's too (the result ran into notification B).
- `npx e2e run tests/sessions.e2e.ts tests/watching.e2e.ts`: 52/52 passed at `6e7d0360`; at
  `2da70bff` 51/52 under load average ~118, the failure being a 5 s `expect.poll` in the watching
  test "someone else's task shows its live transcript…", which passed alone right after and in
  both earlier full runs; it touches no wake-up record (the reviewer agreed).
- `python3 scripts/bootstrap.py && scripts/dev ci`: exit 0, nextest 2626 tests passed, 16 skipped.
- `scripts/dev security`: exit 2, not run: Vidoc is out of quota (HTTP 429), the 3 changed code
  files were not scanned. The change adds no network, storage or HTML injection path: text goes
  into text nodes through `h()`.
- Not tested: a real Claude Code session's wake-up on the live instance (the mock carries the
  record of the screenshot verbatim apart from ids and paths).
