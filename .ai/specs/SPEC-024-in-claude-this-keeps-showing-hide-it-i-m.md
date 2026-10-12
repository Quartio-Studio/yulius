---
yulius-task: 27142782-09d6-4403-b3b5-ff3968b7978c
status: done
---

# SPEC-024: Claude Code sessions start without the diff side panel

Status: implemented
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
- With the feature flag `tengu_quiet_dolphin` (on by default, cached as on in Yulius sandboxes),
  the built-in plugin `cc-plugin-diff` draws the panel instead. It keeps its own key `"open"` in
  `<CLAUDE_CONFIG_DIR>/plugins/store/cc-plugin-diff_builtin-<12 hex of SHA-256 of the id>.json`
  and auto-opens unless that key is `false`. A store without the key is seeded once from
  `diffSidebarOpen` (`carryPanelPreference`); a missing store is first carried from the legacy id
  `diff@builtin`. `/diff` stores `true`, and a stored `true` also opens the pane at startup, so one
  manual opening would be remembered for every later session in that sandbox.
- Yulius already merges first-run keys into that file before each Claude Code launch
  (`ensure_claude_config`, `crates/yulius-harness/src/claude_config.rs`; SPEC-002 section on launch
  preparation). This change adds one key to that existing merge. No second mechanism is needed.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| DSP-001 | Before every Claude Code launch (new or resumed), `<CLAUDE_CONFIG_DIR>/.claude.json` has `diffSidebarOpen: false`. | `claude_config` unit test |
| DSP-002 | A `true` left by an earlier session (the panel opened by hand) is reset to `false` at the next launch, in `.claude.json` and in every existing diff plugin store (current and legacy id); other store keys are kept. | `claude_config` unit tests |
| DSP-005 | A missing plugin store stays missing, and an unreadable one is left untouched without failing the launch. | same tests |
| DSP-003 | Claude Code's other keys and the user's choices (theme, projects) are kept, as before. | existing merge test |
| DSP-004 | The panel can still be opened by hand within a session (Claude Code's own toggle); Yulius does not remove it. | design: only the auto-open gate key is set |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Every Claude Code launch starts with the diff side panel closed
- [x] Architecture spec and changes log say the panel stays closed
- [x] Independent review passes and the change merges into dev on green CI

## Acceptance criteria

- `ensure_claude_config` writes `diffSidebarOpen: false` into a missing config, an existing config
  without the key, and an existing config that has `true`.
- It sets `"open": false` in existing diff plugin stores (current and legacy file names, pinned in
  the test), keeps their other keys, and creates no store and fails no launch otherwise.
- Existing tests of the merge keep passing; `scripts/dev ci` passes.
- SPEC-002's launch-preparation paragraph lists the key.

## Test plan

- Unit (`claude_config.rs`): tests that fail without the change cover a fresh config, a config
  with `diffSidebarOpen: true` (reset to `false`), plugin stores holding `"open": true` (reset,
  other keys kept), a missing store (not created) and an unreadable one (left as is).
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
- **Reset the plugin store too, found by the independent review.** The global key alone only
  covers a fresh sandbox; the plugin pane reads its own store. Only existing stores are changed, so
  Claude Code still creates and carries them itself, and a store error never blocks a launch.
- **Disabling the plugin was rejected.** `enabledPlugins` would also remove `/diff`, and the panel
  is useful on request.
- **The store is written without Claude Code's lock file.** A Claude Code session already running
  in the same sandbox could lose one store update made at the same instant. Writes are atomic
  (temp file and rename), so the file cannot be corrupted. This is the risk SPEC-002 already
  accepts for `.claude.json` (R-11).
- **No ADR.** This is one more default in an existing launch-preparation merge (like
  `autoCompactEnabled` in the run settings), not a new architectural choice.

## Evidence

- The panel's gate was read in the pinned Claude Code 2.1.292 bundle (`strings` on the CLI): the
  built-in panel's `kTr` (`diffSidebarOpen`, 110 and 144 columns), and the `cc-plugin-diff` pane's
  auto-open (`storeGet("open")`), `carryPanelPreference`, store path (`plugins/store`) and file
  naming (`ue`). The pinned file names were recomputed with SHA-256.
- Test-first: `cargo test -p yulius-harness --lib claude_config` failed on the new assertions
  before each change (`diffSidebarOpen` was `Null`; the store's `"open"` stayed `true`). It then
  passed: 4 of 4.
- `scripts/dev ci` (2026-10-12, Linux, head `bad97c3`): exit 0; nextest 1151 tests passed and 16
  skipped. `scripts/dev lint` was clean.
- Independent review (fresh subagent, `om-code-review`, diff `58ad376..HEAD`). Round 1 asked for
  changes: the plugin pane's own store. Round 2 approved; its remaining nits are recorded above.
- Not verified live: a logged-in Claude Code TUI in a wide terminal (needs a paid interactive
  session). The end-to-end suite was not run, because no web UI code changed.
