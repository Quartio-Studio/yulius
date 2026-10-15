---
yulius-task: d519c05d-44fd-45a0-b26e-b35c52bc8a37
status: in-progress
---

# SPEC-177: The session view says when the server cut a diff or a context, and every scroller is reachable

Status: in-progress
Date: 2026-10-15
Owner: Yulius task #137, issues #350, #380, #363 (part of task #118, SPEC-138)
Scope authorization: task #137: "Fix GitHub issues #350, #380 and #363 of Quartio-Studio/yulius
[…] All are in the session view (SPEC-075, SPEC-090)".

## Goal

Three defects of the read-only session view (SPEC-075, SPEC-090) are gone:

- An edit whose diff the server cut no longer claims a short count; it says it was cut (#350).
- A meta text (skill body, hook context, command output) the server cut says so, and its folded
  size reads as a lower bound (#380).
- Every block that scrolls at the current width is a named, keyboard-reachable region, also on a
  phone, and the region names tell one result from another (#363).

## Context

- `crates/yulius-app/src/transcripts.rs` bounds every text it serves. `text`, `thinking` and
  `tool_result` blocks carry `truncated`; `diff_of` cuts each side of a hunk at `MAX_HUNK_CHARS`
  (16 Ki characters) and keeps `MAX_HUNKS` (20) hunks of a `MultiEdit` silently, and `meta` blocks
  are cut with `bounded()` (16 Ki for user meta, 32 Ki for `system` lines) silently too.
- `sessions.js` `callPreview` counts the diff's lines for "Wrote N lines to path" and "Updated path
  with N additions and M removals"; `foldedNote` prints `⎿ Context added (size)` from the text it got.
- `scroller(text, label)` decided at build time from an estimate (line length / 80, more than 16
  rows) whether a block is a region (`tabindex="0"`, `role="region"`, `aria-label`). At 375 px
  a block of 16 lines × 60 characters wraps to about 32 rows, scrolls, and gets none (axe
  `scrollable-region-focusable`). At 1280 px a block of 17 short lines is a tab stop that does not
  scroll. Entries use `content-visibility: auto`, so off-screen entries are not laid out.
- On `dev` (983d3a34) all three are still open: no `truncated` on `TranscriptDiff` or `Meta`, and
  `scroller` still estimates.

## Requirements

- **SCV-001 (#350).** `TranscriptDiff` has `truncated: bool` (additive; absent reads as `false`),
  `true` when a side of a hunk was cut at `MAX_HUNK_CHARS` or hunks beyond `MAX_HUNKS` were
  dropped. For a cut diff the session view claims no count ("Wrote path", "Updated path") and shows
  "Cut here: the change is longer." under the diff. `session_read` ends a cut diff with
  "…(cut: the change is longer)".
- **SCV-002 (#380).** `TranscriptBlock::Meta` has `truncated: bool` (additive; absent reads as
  `false`), `true` when its bound cut the text. The view shows "Cut here: the context is longer."
  under a cut meta block's last line: inside the opened fold when that line is a folded context,
  whose summary then reads `Context added (at least <size>)`. `session_read` does not print meta
  blocks, so its output is unchanged.
- **SCV-003 (#363).** A code, input, result, diff or context block is a named region exactly when it
  overflows its box as laid out (`scrollHeight > clientHeight`), checked when it is laid out and
  whenever its size changes (a `ResizeObserver` per view, disconnected when the view goes). Before
  it is laid out (an off-screen entry under `content-visibility: auto`, a closed fold) the block
  keeps an estimate for the narrowest layout (38 columns), so a block that may scroll is never
  skipped by the keyboard. A block that does not scroll is no tab stop; one that stops scrolling
  while it has the keyboard stays a region until the keyboard leaves it.
- **SCV-004 (#363).** Region names say whose block it is, naming the call as its line does:
  `Result of Bash(npm test)`, `Input of Grep(…)`, `Changed lines of path` with `(n of m)` for
  several hunks, and `Result (line N)` for a result whose call is not on the page. A name already
  used in the view (a command run twice, a second added context) adds ` (line N)`.
- **SCV-005 (review round 1).** A cut meta block whose last line is command output says "Cut here:
  the output is longer."; an opened result whose call is not on the page keeps a name for its
  summary ("Tool result", for assistive technology).

## Plan

- [x] Spec and issue checks on dev
- [x] A cut diff says so and claims no count (#350)
- [x] A cut context says so and shows its size as at least (#380)
- [x] Scrollers become regions by measured overflow, with distinct names (#363)
- [x] Docs, ADR and changes updated
- [x] Gates green and independent review passed
- [x] Review round 1 findings fixed: focus kept, unique names, cut output, tests

## Acceptance criteria

- `transcripts.rs` unit tests: a `Write` over `MAX_HUNK_CHARS` and a `MultiEdit` of 21 edits are
  `truncated`, a small `Edit` is not; a meta text over its bound is `truncated` and one under it is
  not (user meta and `system`); `render_text` marks a cut diff. Fail without SCV-001/002.
- `sessions.e2e.ts`: a cut `Write` reads "Wrote path" with no count and "Cut here: the change is
  longer."; a cut context reads "Context added (at least 16 KiB)" and, opened, "Cut here: the
  context is longer.", with no axe violations. Fail without SCV-001/002.
- `sessions.e2e.ts` at 390 × 844: a result of 16 lines × 60 characters is the region
  `Result of Bash(…)` and axe passes; at 1280 px the same result is no tab stop. Fails without
  SCV-003/004.
- `sessions.e2e.ts` (review round 1): two hunks are `(1 of 2)` and `(2 of 2)`, a result without
  its call is `Result (line 19)`, a second run of a command is `… (line 21)`, no region says
  `Edit(`, a cut command output says "output", with no axe violations; in the page, a focused
  region that stops scrolling keeps its tab stop until blurred.
- Existing session e2e tests stay green (the long diff and the skill body stay regions).

## Test plan

- `scripts/dev test` (unit tests above), `scripts/dev check`, `scripts/dev lint`.
- `cd e2e && npx e2e run tests/sessions.e2e.ts`, then the whole e2e suite.
- `scripts/dev ci` and `scripts/dev security` before delivery.

## Decisions

- **Measure, keep a narrow estimate as the fallback (ADR-0143).** Only measuring would leave an
  off-screen block (skipped by `content-visibility: auto`) out of the tab order until it is near the
  view; only estimating for 38 columns would make many non-scrolling desktop blocks tab stops.
  Estimating for the narrowest layout until the block is measured gives both.
- **One observer per session view**, not per page module: observed elements are released with the
  view (`destroy` disconnects it).
- **"at least <size>"** instead of `<size>+`: read aloud the same way it reads.
- Code blocks inside messages keep the name "Code": a message rarely holds several long ones, and
  naming them by line would add a parameter through `richText` for little gain.
- The new fields are `#[serde(default)]` so stored or older payloads still deserialize.
- Review round 1 (approve, 290f72ff; minors fixed here per ADR-0141): a focused region is not
  demoted under the keyboard; repeated names add their line; names use the TUI's labels.
  `render_text` capping hunks at 40/80 lines without a marker is older than this change and is
  left as it is (noted in the task summary; this task creates no issues).
