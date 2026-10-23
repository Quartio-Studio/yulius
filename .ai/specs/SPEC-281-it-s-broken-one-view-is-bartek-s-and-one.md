---
yulius-task: edbcf5af-22bc-45bb-b605-899b14ef442c
status: in-progress
---

# SPEC-281: Only the task's own Claude Code changes the task's session

Status: in-progress
Date: 2026-10-23
Owner: task #267
Scope authorization: task #267 (requested by MrFishPL): "it's broken, one view is Bartek's and
one is mine", two screenshots of task #266: the owner's terminal shows the live session, a
watcher's view (SPEC-179) shows lines `266`…`300` and `Not logged in · Please run /login`.
Issue #862. Decision: ADR-0209.

## Goal

A task's session is the one its own Claude Code TUI runs. A `claude` the agent starts in its
worktree (a repro, a probe) never becomes the task's session: watchers keep reading the real
session, and a Continue, a limit rotation or a queued start resumes the real session.

## Context

- Symptom: task #266's agent ran a nested `claude` in its worktree at 17:06:56 (a scroll repro
  printing `seq 300`; not signed in). Task #266 now has two sessions in Sessions: `28c4996b…` (the
  TUI's) and `cc2a9cb8…` (the nested one, one line: "Not logged in · Please run /login").
- Root cause 1, adoption: the supervisor's drift check (`supervisor.rs`, every 5 s, HRN-004)
  adopts the most recently modified `<id>.jsonl` in the worktree's Claude project directory as
  the attempt's session and into the lineage. A nested `claude` with the same `HOME` and cwd writes
  its transcript there too, so it was adopted (and the transcript watch flipped to it, and back
  when the TUI wrote again, each flip counting as "a person took the session in hand").
- Root cause 2, order: `provider_sessions` lists the lineage by `first_seen`; re-adopting a known
  session only moves its `last_seen`. So `.pop()` / `.last()` stayed on the nested session:
  `GET /api/tasks/{tid}/transcript` served it to watchers, and `start_run` resumes
  `lineage.last()`: the next Continue of #266 would have resumed the nested session.
- Why the tests missed it: the drift test (`supervision.rs`) simulated `/clear` by writing a new
  `.jsonl` into the project directory, exactly what a nested `claude` does; nothing distinguished
  the TUI's session change from another process's file.
- What tells them apart: Claude Code runs the hooks of the run's `--settings` file
  (`claude-settings.json` in the run directory) only in the TUI Yulius launched; every hook input
  carries the TUI's `session_id`, and `SessionStart` fires whenever the TUI's session begins or
  changes (`source`: `startup`, `resume`, `clear`, `compact`). A nested `claude` does not load that
  file (the sandbox's `settings.json` and the repository's carry no Yulius hooks).

## Requirements

- SSN-001: the task's session changes only when the run's own `SessionStart` hook names another
  session id: it becomes the attempt's session at once; the supervisor then records it in the
  lineage, re-opens the transcript watch on it and treats it as a session a person took in hand
  (as `/clear` and `/resume` before). The drift check no longer reads the project directory.
- SSN-001a: the hook records the new session in the lineage at once (a restart before the next
  drift check keeps it), and refuses an invalid id or one already in another task's lineage (the
  sandbox's tasks share one Claude directory); the store's upsert never moves another task's entry.
- SSN-002: a session file no hook of the run named (a nested `claude`) is never adopted: the
  attempt's session and the lineage stay as they were.
- SSN-003: the lineage is listed in the order sessions were adopted (`last_seen`, then
  `first_seen`): watchers (`GET /api/tasks/{tid}/transcript`) and resumes use the session adopted
  last, including an earlier session adopted again (`/resume` back to it). Task #266's lineage,
  where the TUI's session was adopted again after the nested one, reads right without a migration.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Spec and regression tests that fail: a nested session is not adopted, a hook-named /clear is
- [x] Only the run's hooks change the task's session (drift poll follows the attempt)
- [x] Watchers and resumes use the session adopted last
- [x] ADR, architecture and changes updated
- [x] Review round 1 fixed test-first: lineage at the hook, another task's session refused
- [ ] Review round 2 approves the exact head
- [ ] Gates green and merged into dev

## Acceptance criteria

- A session file written into the task's Claude project directory without a hook (the nested
  `claude`) is not adopted across several drift checks: the attempt's session, the lineage and the
  watcher's transcript stay on the TUI's session.
- `/clear` in the TUI (`SessionEnd` `clear`, a new transcript, `SessionStart` `clear` with the new
  id) is adopted: the attempt's session is the new id, the lineage has both, the watcher reads the
  new one.
- Re-adopting an earlier session puts it last in the lineage.

## Test plan

- `session_sync.rs` (fake agent with hooks and the HTTP server): a nested session file, then
  seven watcher rounds (more than one 5 s drift period); then `/clear` in the fake agent. Fails on
  `dev` at the nested-file assertion.
- The fake agent gains `/clear`: Claude Code's order (`SessionEnd` `clear`, a new session id and
  transcript file next to the old one, `SessionStart` `clear`).
- Store test: `provider_sessions` lists an earlier session adopted again last.
- The old drift test (`supervision.rs`), which wrote a file to simulate `/clear`, is replaced by
  the hook-driven one.

## Decisions

- Hooks, not the file system, name the task's session (ADR-0209). Alternatives: tell the files
  apart by content (a nested interactive `claude` writes the same records), by the TUI's open file
  descriptors (Claude Code appends and closes), by Claude Code's `~/.claude/sessions/<pid>.json`
  registry (undocumented, version-dependent); or make the agent's nested runs use another `HOME`
  (relies on every agent remembering it).
- Only `SessionStart` changes the session, not every hook event: it is the event Claude Code fires
  when the TUI's session begins or changes, and other events (a background agent's, a teammate's)
  are not guaranteed to carry the TUI's id.
- Order by `last_seen` instead of a migration of the stored lineage: the data was right, only the
  order read was wrong.

## What else the cause could break

The attempt's session id feeds the transcript watch (activity, usage, turn ends, goal state),
the Sessions page's task documents, rotation and resume. All of them followed the nested session
while it was adopted; with SSN-001 none can.
