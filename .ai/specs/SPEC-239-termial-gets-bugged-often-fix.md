---
yulius-task: 2cf95875-6d3c-49dd-a8f0-30897ff23619
status: done
---

# SPEC-239: A secret redacted in the terminal keeps the screen's layout

Status: done
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

- [x] Spec records the root cause of the garbled terminal
- [x] Regression test replays a cursor-addressed stream and fails on dev
- [x] Terminal masks a secret with a label of the value's width
- [x] ADR, architecture and changes record the terminal mask
- [x] Independent review approved
- [x] Repository gate and security scan pass
- [x] Merged into dev on green CI

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

## Evidence

- Red: at 2b916d2b (the new test against dev c7b2df6b's redaction),
  `cargo test -p yulius-app --lib vault::terminal` failed:
  `redraws_over_a_redacted_value_keep_their_cells` at split 0 left the screen
  `["allowed for [secret:PIPEDREAM_PROJECT_EN", "dev is not [secret:PIPEDREAM_PROJECT_ENV",
  "IRONMENT]owances             !", "fine", …]`. That is the screenshot's garbling, where the
  expected screen was `["dev is not [secret:P]        !", "fine", …]`.
- Red (review round 1): at 47688503, `values_take_the_columns_the_screen_counts` failed for
  `ok 👍🏽 deploy-key` (cursor column 21, expected 23), and `fitted_label("ÉTÉ", 10)` panicked.
- Green: `cargo test -p yulius-app --lib vault::terminal`: 3 passed.
  `cargo test -p yulius-domain --lib vault::redact`: 10 passed.
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & (test(/^secrets_redaction::/) |
  test(/^cockpit::/) | test(/^secrets::/) | test(/^sessions::/) | test(/^task_transcript::/) |
  test(/^browser::/))'` at e25b00f8: 92 passed. These run the real ptyhost with a fake agent,
  Superagent screens and the text channels. In `cockpit.rs`, the two screen checks now expect
  the padded label.
- `python3 scripts/bootstrap.py && scripts/dev ci` at 6a386771: exit 0. Nextest: 2265 passed,
  17 skipped.
- `scripts/dev security` at 6a386771: exit 0. The vidoc-1 change scan found no open critical or
  high finding, and no medium finding in the change. It listed no low finding in the change. It
  reported three findings in code this branch did not touch (`vault/runtime.rs:612`, `:745`,
  `:835`), which Yulius CI files as issues.
- Independent review in a fresh session (`om-code-review`):
  - Round 1, at a47596e3: APPROVE, with four minors and one nit in this change's own code. They
    were the width counted per grapheme rather than per character, a cut character counted as a
    stray byte, the width disclosure missing from the ADR, the spec unfinished, and a non-ASCII
    name panicking. All were fixed test-first in 47688503 and 6a386771.
  - Round 2, at 6a386771: APPROVE. Nits were left as they are:
    - The lookup by `end` in `process()` has a fallback that never runs (harmless, few matches
      per chunk).
    - A literal U+FFFD in a value would count one column too many (not a realistic value).
    - xterm.js's handling of an interrupted UTF-8 sequence was not verified (only the rare
      mid-character split).
- Not tested: a live Claude Code session in the browser terminal (xterm.js) showing a secret.
  The `vt100` screen stands in for it, as the hub's snapshots do.
