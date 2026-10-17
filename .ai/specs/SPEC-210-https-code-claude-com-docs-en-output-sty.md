---
yulius-task: b8380045-3c62-46bd-add3-7fcf66a39101
status: in-progress
---

# SPEC-210: Claude Code sessions start in the Concise output style

Status: in-progress
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
dir's `.claude.json` (onboarding, trust, theme, diff pane).

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
- CON-003: A `settings.json` that is not a JSON object, or is larger than 8 MiB, is left untouched and
  the launch goes on (Claude Code reports its own settings errors).
- CON-004: Run settings (`--settings`) do not set `outputStyle`, so `/output-style` and a project's
  committed `outputStyle` keep working.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Sessions default to the Concise style through the sandbox's user settings
- [x] Docs, ADR and the changes log describe the Concise default
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- A fresh config dir gets `settings.json` with `outputStyle: "Concise"` after `ensure_claude_config`.
- A config dir whose `settings.json` has other keys gets `outputStyle` added and keeps the others.
- A `settings.json` with `outputStyle: "Explanatory"` or `"default"` is unchanged.
- A corrupt `settings.json` is unchanged and `ensure_claude_config` still succeeds.
- `claude -p "/output-style"` with that config dir reports Concise as current (measured live).

## Test plan

Unit tests in `crates/yulius-harness/src/claude_config.rs` beside the existing `.claude.json`
tests (temporary dirs, no Claude Code): fresh dir, merge with other keys, a kept style, a kept
`default`, corrupt and non-object files untouched. Task-run and Superagent launches both already
call `ensure_claude_config` (`crates/yulius/src/wiring/harness.rs`), so one function covers both.
Live check: `claude -p "/output-style"` with `CLAUDE_CONFIG_DIR` pointing at a prepared dir.

## Decisions

- Default at the user settings layer, not forced through `--settings`
  ([ADR-0166](../../docs/decisions/0166-sessions-default-to-the-concise-output-style.md)): forcing
  makes `/output-style` lie (measured above).
- Only an absent key is filled in: a person or an earlier `/config` choice in the user file wins.
- No per-project switch in `yulius.toml`: a repository that wants another style commits
  `outputStyle` in its `.claude/settings.json`, which outranks the user layer.
