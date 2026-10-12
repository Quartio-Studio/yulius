---
yulius-task: 27142782-09d6-4403-b3b5-ff3968b7978c
status: in-progress
---

# SPEC-024: Claude Code sessions start without the diff side panel

Status: in-progress
Date: 2026-10-12
Owner: Yulius task #22
Scope authorization: user request (screenshot of task #20's terminal): "in claude this keeps
showing. Hide it. I mean the right bar"

## Goal

A Claude Code session in a Yulius task terminal shows the conversation across the whole terminal
width. Claude Code's diff side panel ("source: Current", "No changes this session") no longer opens
by itself to the right of the transcript.

## Context

- The screenshot shows task #20's terminal split in two, with Claude Code's own diff side panel on
  the right. The panel is drawn by Claude Code inside the terminal; the Yulius web UI does not draw
  it (no Yulius source contains its text).
- Claude Code 2.1.292 (the pinned CLI, checked in its bundle) auto-opens the panel once the session
  has tracked file edits, if the terminal is wide enough. The gate reads the global config key
  `diffSidebarOpen` in `<CLAUDE_CONFIG_DIR>/.claude.json`: `false` never auto-opens; `true`
  auto-opens from 110 columns; unset auto-opens from 144 columns. Yulius task terminals are wider
  than 144 columns on a laptop screen, and a fresh sandbox home has no key, so the panel opens in
  every task.
- Toggling the panel by hand writes the key (`true` when opened, `false` when closed). One opening
  is then remembered for every later session in that sandbox.
- Yulius already merges first-run keys into that file before each Claude Code launch
  (`ensure_claude_config`, `crates/yulius-harness/src/claude_config.rs`; SPEC-002 section on launch
  preparation). This change adds one key to that existing merge. No second mechanism is needed.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| DSP-001 | Before every Claude Code launch (new or resumed), `<CLAUDE_CONFIG_DIR>/.claude.json` has `diffSidebarOpen: false`. | `claude_config` unit test |
| DSP-002 | A `true` left by an earlier session (the panel opened by hand) is reset to `false` at the next launch. | same test |
| DSP-003 | Claude Code's other keys and the user's choices (theme, projects) are kept, as before. | existing merge test |
| DSP-004 | The panel can still be opened by hand within a session (Claude Code's own toggle); Yulius does not remove it. | design: only the auto-open gate key is set |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Every Claude Code launch starts with the diff side panel closed
- [ ] Architecture spec and changes log say the panel stays closed
- [ ] Independent review passes and the change merges into dev on green CI

## Acceptance criteria

- `ensure_claude_config` writes `diffSidebarOpen: false` into a missing config, an existing config
  without the key, and an existing config that has `true`.
- Existing tests of the merge keep passing; `scripts/dev ci` passes.
- SPEC-002's launch-preparation paragraph lists the key.

## Test plan

- Unit (`claude_config.rs`): a test that fails without the change covers a fresh config, a config
  with `diffSidebarOpen: true` (reset to `false`) and preservation of other keys.
- Not tested here: the rendered Claude Code TUI. Driving the real Claude Code in a wide terminal
  needs a paid, logged-in session. The gate in the bundle was read directly instead (Context).

## Decisions

- **Force `false` on every launch rather than only seed it when missing.** Seeding keeps a `true`
  written by a single manual opening, and the panel would then auto-open in every later session in
  that sandbox, which is what "keeps showing" describes. Forcing it costs nothing: within a session
  the panel still opens by hand.
- **Use the Claude Code key, not a terminal-width trick or a UI overlay.** Narrowing the PTY would
  waste screen space. Hiding part of the terminal in the web UI would hide real output. The key is
  Claude Code's own switch for exactly this behavior.
- **No ADR.** This is one more default in an existing launch-preparation merge (like
  `autoCompactEnabled` in the run settings), not a new architectural choice.
