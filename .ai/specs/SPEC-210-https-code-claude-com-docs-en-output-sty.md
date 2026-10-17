---
yulius-task: b8380045-3c62-46bd-add3-7fcf66a39101
status: done
---

# SPEC-210: Claude Code sessions start in the Concise output style

Status: done
Date: 2026-10-17
Owner: task #197
Scope authorization: task #197 (requested by MrFishPL): "https://code.claude.com/docs/en/output-styles#concise
broo implement this concise everywhere".

## Goal

Every Claude Code session Yulius starts, task runs and the Superagent (Cockpit) alike, answers in
Claude Code's built-in **Concise** output style: the first sentence states the result, without
lead-in, narration or closing recap. A person who picks another style with `/output-style` keeps it.

## Context

Claude Code (2.1.237+, the instance runs 2.1.292) ships the built-in style `Concise`, selected by
the case-sensitive `outputStyle` setting. Yulius starts Claude Code in two places, both in
`crates/yulius-harness/src/launch.rs`: `plan_claude` (task runs, `--settings <run dir>/claude-settings.json`)
and `plan_cockpit` (the Superagent, inline `--settings` only when there is something to set).
Before either launch, `claude_config::ensure_claude_config` merges Yulius's keys into the config
dir's `.claude.json` (onboarding, trust, theme, diff pane). Two other `claude` processes are no
conversation and stay as they are: the split proposer (`claude -p --json-schema` in a throwaway
config dir, `crates/yulius/src/wiring/split.rs`) and sign-in (`claude setup-token`).

Measured on 2.1.292 in this sandbox (`claude -p "/output-style"`):

- `--settings '{"outputStyle":"Concise"}'` makes Concise current, and also outranks a
  `.claude/settings.local.json` holding `Explanatory`.
- With that `--settings`, `/output-style Explanatory` answers "Output style set to Explanatory"
  and writes the local settings file, yet the next `/output-style` still reports Concise: a style
  forced through `--settings` silently ignores the person's choice.
- Without `--settings`, the local file's style is current.

Settings precedence is user (`<CLAUDE_CONFIG_DIR>/settings.json`) < project < local < `--settings` <
managed. The user layer is the lowest one that every Yulius session reads.

Subagents other than forks run their own system prompt and ignore output styles (Claude Code docs):
that stays so; "everywhere" means every session Yulius launches.

## Requirements

- CON-001: Before every Claude Code launch (task run or Superagent) the config dir's `settings.json`
  holds `"outputStyle": "Concise"` when it had no `outputStyle` key; the file is created (0600, in
  the 0700 config dir) when missing.
- CON-002: An existing `outputStyle` value (any style, including `default`) is kept, and every other
  key of the file is kept.
- CON-003: A `settings.json` that is not a regular file (a link, dangling or not, a FIFO, a
  directory), not UTF-8, not a JSON object, or larger than 8 MiB, is left untouched and the launch
  goes on at once (Claude Code reports its own settings errors). The file is opened without
  following links or blocking: the agent owns its config dir, which the host prepares.
- CON-004: Run settings (`--settings`) do not set `outputStyle`, so `/output-style` and a project's
  committed `outputStyle` keep working.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Sessions default to the Concise style through the sandbox's user settings
- [x] Docs, ADR and the changes log describe the Concise default
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Final review round approves the exact head
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- A fresh config dir gets `settings.json` with `outputStyle: "Concise"` after `ensure_claude_config`.
- A config dir whose `settings.json` has other keys gets `outputStyle` added and keeps the others.
- A `settings.json` with `outputStyle: "Explanatory"` or `"default"` is unchanged.
- A corrupt `settings.json`, a link, a FIFO or a directory in its place is unchanged and
  `ensure_claude_config` still succeeds at once.
- Neither launch's `--settings` holds `outputStyle`.
- `claude -p "/output-style"` with that config dir reports Concise as current (measured live).

## Test plan

Unit tests in `crates/yulius-harness/src/claude_config.rs` beside the existing `.claude.json`
tests (temporary dirs, no Claude Code): fresh dir, merge with other keys, a kept style, a kept
`default`, corrupt, non-object, oversized, non-UTF-8 files, a symlink (its target untouched), a
dangling link, a FIFO and a directory untouched. `crates/yulius/src/wiring/harness.rs` plans a task
run and a Superagent through the real adapter and reads `settings.json` of each config dir
(CON-001 on both paths). `crates/yulius-harness/tests/launch.rs` asserts that neither the task
run's settings file nor the Superagent's inline settings carry `outputStyle` (CON-004).
Live check: `claude -p "/output-style"` with `CLAUDE_CONFIG_DIR` pointing at a prepared dir.

## Decisions

- Default at the user settings layer, not forced through `--settings`
  ([ADR-0166](../../docs/decisions/0166-sessions-default-to-the-concise-output-style.md)): forcing
  makes `/output-style` lie (measured above).
- Only an absent key is filled in: a person or an earlier `/config` choice in the user file wins.
- No per-project switch in `yulius.toml`: a repository that wants another style commits
  `outputStyle` in its `.claude/settings.json`, which outranks the user layer.

## Evidence

- `scripts/dev ci` at `cc23034a` (`origin/dev` merged): exit 0, 2134 tests passed, 18 skipped
  (lint, deps, checks included). Earlier at `d75b97bb`: exit 0, 2132 passed.
- New tests:
  - `claude_config::tests::sessions_default_to_the_concise_output_style` failed before the
    change (no `settings.json`).
  - `claude_config::tests::the_output_style_default_touches_only_a_regular_settings_file`
    failed on the first implementation, which replaced the link with a copy of its target; it
    passes with `read_regular`.
  - `wiring::harness::tests::every_claude_code_launch_defaults_to_the_concise_output_style` and
    the `outputStyle` assertions in `crates/yulius-harness/tests/launch.rs` pass.
- Live, Claude Code 2.1.292, `CLAUDE_CONFIG_DIR` holding `settings.json` = `{"outputStyle": "Concise"}`:
  - `claude -p "/output-style"` → `Output style: Concise`.
  - In one session, `/output-style Explanatory` → "Output style set to Explanatory", then
    `/output-style` → `Output style: Explanatory`; the next process also reports Explanatory.
  - Run settings as a contrast: with `--settings '{"outputStyle":"Concise"}'` the same switch
    reported success but stayed Concise.
- `scripts/dev security`: the runs at `61c71bca` and `d75b97bb` reported no open critical or high
  finding in this change's code.
  - One run at `d75b97bb` reported a high in the pre-existing `main_repository` folder trust
    (SPEC-106). It is ignored in `.config/vidoc.toml` with its reason, subject to the reviewer's
    agreement.
  - The pre-existing host-side reads of the agent-owned config dir (links, FIFOs in
    `.claude.json` and the plugin stores) are tracked in #634.
- `scripts/dev security` at `cc23034a`: no open critical or high finding. One low finding, on
  the pre-existing `read_small`, is #634.
- Review round 1 (fresh reviewer) requested changes; all own-code findings are fixed in
  `d75b97bb`. Round 2 requested changes, all fixed in `cc23034a`:
  - the dangling-link assertion;
  - the ignore title narrowed to "as trusted";
  - `CockpitLaunch.contained` from dev.
  Round 2 also agreed that the folder-trust finding is a false positive, on condition the title
  was narrowed.
- Review round 3 approved `79fd93d6`: all round-2 fixes verified, and CON-004 holds after dev's
  SPEC-208 merge. Its one nit, the name of the review step in this Plan, is applied here.
- Not tested here: a full task run in the web UI after deployment (no deploy in this task).
