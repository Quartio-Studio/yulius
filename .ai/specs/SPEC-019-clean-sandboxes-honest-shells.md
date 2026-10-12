---
yulius-task: none
status: done
---

# SPEC-019: sandboxes stay clean and background shells stay honest

Status: done
Date: 2026-10-12
Owner: finished by hand after the analysis of the `yulius` sandbox's task transcripts
Scope authorization: user request (F 2026-10-12): "(1) dead monitors and shells sometimes lags
Claude Code and tasks are still working then formally. (2) Atm 222 GiB of the disk is used. It
seems like agents leave old compiles and worktrees etc. It should keep things clean".

## Goal

Nothing an agent or a finished task leaves behind fills the disk, and a background shell that can
never end neither keeps a task "working" unnoticed nor starts at all when it is a self-matching
wait.

## Context

On 2026-10-12 the VM disk held 207 GB:
- removed sandboxes kept 51 GB of worktrees and CI checkouts;
- one idle task's worktree had a 19 GB `target`;
- an agent's own extra worktree (`wt-206`) held 2.7 GB.

SPEC-008's upkeep pruned idle tasks' caches only above 90 % of the quota. It never removed an
ended task's worktree that held ignored tool directories (`.tools`, `node_modules`). It once
deleted the `target` of a task whose agent idled while its background gate still ran (task #10
lost 70 minutes).

Every task transcript had wait loops like `until ! pgrep -f "scripts/dev ci"`. They match their own
`bash -c` command line, so they never end, and the task showed "Working" for hours (up to
2 h 45 min). `pkill -f` killed the agent's own shell.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| DSK-001 | An idle task whose background shells still run counts as working: its caches are never pruned. | `disk::tests::background_shells_keep_a_task_working_and_long_idleness_ends_it` |
| DSK-002 | An idle task that has not moved for 4 hours has its caches pruned whatever the quota. | same |
| DSK-003 | An ended task's worktree is removed when its ignored files are only rebuildable directories (`target`, `node_modules`, `.tools`, `.venv`, caches) or declared caches; local configuration or data keeps it. | `disk::tests::a_worktree_whose_ignored_files_are_rebuildable_loses_nothing` |
| DSK-004 | A directory under `worktrees/` that no task owns, untouched for a day, has its caches pruned. | `disk.rs` scan |
| DSK-005 | A sandbox removed more than a day ago loses its worktrees and CI checkouts; its home (sessions) and clone stay. | `disk.rs` purge |
| DSK-006 | Recognised caches include `node_modules` and `.venv`; this repository declares `e2e/node_modules`. | `cache_paths` test |
| SHL-001 | A Bash command whose `pgrep -f`/`pkill -f` pattern matches its own command line is refused before it runs, with the way to wait instead. | `guard_client` tests |
| SHL-002 | ~~An idle agent whose background shells run for 30 minutes is told once~~ (withdrawn by SPEC-028: no messages), and the UI shows how long they have run. | `finish.rs` probe; `status.js` |

## Design

- **The rules** live in `yulius_domain::disk`: `owner_of(status, activity, shells, quiet_ms)` and
  `only_rebuildable`. The app passes the probed shell count and the time since the task last
  changed.
- **Ignored files:** the Git port gains `ignored_entries` (the `--directory` listing).
- **The guard** is the in-sandbox `PreToolUse` hook for Bash (`yulius hook claude PreToolUse`), next
  to the `PostToolUse` redaction. It needs no server round trip, and it allows anchored (`^…`) and
  bracket-trick patterns.

## Plan

- [x] Domain rules and tests (owner, long idleness, rebuildable worktrees, caches)
- [x] Upkeep: orphan worktrees, removed sandboxes, worktree removal by rebuildable entries
- [x] Self-matching wait guard and long-running shell notice with its age in the UI

## Evidence

- `cargo test -p yulius-domain disk` and `-p yulius-harness guard_client`: see the pull request.
- Manual cleanup of the VM on 2026-10-12 (207 GB → 86 GB) followed these rules.
