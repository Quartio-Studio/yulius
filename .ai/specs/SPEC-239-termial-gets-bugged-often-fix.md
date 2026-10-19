---
yulius-task: 2cf95875-6d3c-49dd-a8f0-30897ff23619
status: in-progress
---

# SPEC-239: A secret redacted in the terminal keeps the screen's layout

Status: in-progress
Date: 2026-10-19
Owner: task #225
Scope authorization: task #225: "termial gets bugged often, fix", with a screenshot of a task
terminal full of overlapping fragments such as `[secret:PIPEYuliusPtracks_threeOseparate`. Part of
#705, filed by task #223. Extends VLT-007 (SPEC-001) and SPEC-002 ADR-20 (exact-value redaction of
the terminal people watch).

## Goal

A task's terminal, and a Superagent's or Cockpit shell's terminal, stays readable when the agent
prints an agent secret's value. Each value is replaced by a mask exactly as wide as the value, so a
full-screen program's cursor-addressed redraws still land in the cells it meant.

## Context

Confirmed against `dev` at c7b2df6b.

- **Symptom.** In the screenshot the project has an agent secret `PIPEDREAM_PROJECT_ENVIRONMENT`
  whose value is `production`. Every place Claude Code's answer said "production" shows overlapping
  text: "[secret:PIPEYuliusPtracks_threeOseparate allowances", "[secret:PIPone.AM_PROJECT_ENVIRONMENT]".
  The prompt line shows `[secret:PIPEDREAM_PROJECT_ENVIRONMENT]` where the user typed the word.
- **Root cause.** `scrub_output` (`crates/yulius-app/src/vault/terminal.rs`) passes PTY output
  through `StreamRedactor` (`crates/yulius-domain/src/vault/redact.rs`), whose `apply` replaces the
  value's bytes with `[secret:NAME]`. Here that is 10 columns replaced by 38. Claude Code draws its
  screen with cursor motions computed from the text it wrote: it moves up N rows, or to column C,
  and rewrites part of a line. The viewer's screen (xterm.js, and the hub's `vt100` model behind
  snapshots) now has longer lines that wrap. So those motions land in the wrong cells and overwrite
  parts of the label and of the following text. The garbling repeats on every redraw while the value
  is on screen, which in this project is every time "production" is said. That is why it happens
  "often".
- **Why the tests missed it.** `secrets_redaction.rs` and the redactor's unit tests check only
  that the value is gone and the label is present in plain line-by-line output. Nothing replays a
  cursor-addressed stream.
- **Same cause elsewhere.** Every agent secret has this problem, not only ordinary words: an API
  key echoed inside any full-screen program (Claude Code, an editor, `less`) shifts the screen the
  same way. The Superagent's and Cockpit shells' terminals share `scrub_output` and are fixed with
  it. Transcripts, MCP results, tool output and proxy responses are text, not screens, and keep the
  full label.
- **Out of scope (#705).** A value that is an ordinary word (`production`) is still treated as a
  secret and masked wherever it appears. Whether to warn about, refuse or mark such a value as not
  sensitive is a policy decision on the vault. It stays on #705.

## Requirements

- **TRM-001** In the terminals people watch (task, Superagent, Cockpit shell: the live stream and
  the snapshot), a redacted form of a secret is replaced by text of the same display width, counted
  as the hub's `vt100` screen counts it: the `unicode-width` of each character added up.
- **TRM-002** That text reads `[secret:NAME]`, followed by `*` up to the value's width when the
  value is wider, so the full label stays searchable. When the value is narrower, the label is cut
  and closed with `]` (for example `[secret:P]` for a 10-column value). Below two columns it is
  `*` alone. When the redactor has already shown a short beginning of the value (under the 4-byte
  hold minimum, as before), the mask covers the value's columns minus those of the whole characters
  already shown.
- **TRM-003** A cursor-addressed redraw over a redacted value lands in the same cells as it would
  over the value: the screen equals the unredacted screen with the value's cells masked.
- **TRM-004** The value never shows in the terminal, as before (split chunks, held tails,
  snapshots). Every other channel keeps the full `[secret:NAME]` label.

## Plan

- [ ] Spec records the root cause of the garbled terminal
- [ ] Regression test replays a cursor-addressed stream and fails on dev
- [ ] Terminal masks a secret with a label of the value's width
- [ ] ADR, architecture and changes record the terminal mask
- [ ] Independent review approved
- [ ] Repository gate and security scan pass
- [ ] Merged into dev on green CI

## Acceptance criteria

- A unit test in `yulius-app` streams Claude-Code-like output through the terminal redactor into a
  `vt100` parser: a line holding the value, a cursor move back into that line and an overwrite.
  The screen matches the same stream unredacted, with the value's cells masked. It fails on `dev`.
- Unit tests of `fitted_label` cover a wider value, an exact fit, a narrower value, a one-column
  remainder and a wide-character value.
- `secrets_redaction.rs::the_terminal_and_the_run_environment_hold_no_value` and the Cockpit and
  Superagent redaction tests stay green: the value never shows, and the label is still there.

## Test plan

Unit tests in `crates/yulius-domain/src/vault/redact.rs` (the fitted label, and a stream redactor
with a replacement) and in `crates/yulius-app/src/vault/terminal.rs` (the regression: the real
terminal replacement through `StreamRedactor` into `vt100`, split at every chunk boundary). The
existing integration tests (real ptyhost and fake agent) check the live terminal end to end. Then
the repository gate (`scripts/dev ci`) and the security scan.

## Decisions

- **Same-width mask in the terminal only** (ADR-0181). Alternatives: a fixed-width `****` mask
  (layout kept, but the name is lost); stripping redaction from the terminal (leaks the value);
  re-rendering the screen server-side (a large change for the same result). Text channels keep the
  full label, because nothing there addresses cells.
- Width is measured with `unicode-width` 0.2, the version `vt100` already uses, and added up
  character by character, as `vt100` does. That makes the hub's screen and the replacement agree
  (a review finding: the grapheme-aware string width differed for emoji sequences). A value with
  ambiguous-width characters or a tab can still shift xterm.js by a column. Values are ASCII in
  practice.
- The mask shows a value's width, which is roughly its length. The agent can already read the
  length, so this is accepted (ADR-0181).
- The ordinary-word policy of #705 is not part of this change: the PR says "Part of #705".
