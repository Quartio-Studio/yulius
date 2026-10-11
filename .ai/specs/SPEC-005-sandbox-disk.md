---
yulius-task: 7fd8d053-b40c-4bc4-ab19-11bac3505999
status: draft
---

# SPEC-005: a full sandbox disk never kills a running session; Yulius frees build caches itself

Status: draft
Date: 2026-10-11
Owner: Yulius task #8
Scope authorization: user request (production incident of 2026-10-11 19:40 UTC)

## Goal

A sandbox that nears or passes its disk quota neither ends healthy sessions nor needs a person
to delete `target` directories: a switch checks that the new session can start before it ends
the running one, a refusal over the quota is never based on a stale reading, and Yulius removes
the build caches and leftover worktrees of tasks that are not working.

## Context

Incident: the sandbox "yulius" measured 112.2 GB of its 100 GB quota, mostly Cargo `target`
directories of closed and cancelled tasks (38, 46, 11 and 7 GB).

1. `App::switch_task_now` (`supervisor.rs`) moves the task to `Switching`, terminates the
   running session and only then calls `start_run`, whose `isolation_join` refuses over the
   quota (`Manager::prepare`). Four healthy sessions ended `stopped_switch_failed` with nothing
   in their terminal or log. Automatic switches (98 % threshold, capacity resume) take the
   same path.
2. `Manager` measures a sandbox at most every `USAGE_EVERY` (10 minutes) and `prepare` refuses
   from the last reading, so freeing space helped only up to 10 minutes later.
3. Hygiene (`ci/hygiene.rs`, SPEC-001 DLV-007) removes worktrees only of archived, deleted or
   merged tasks; a closed or cancelled task keeps its worktree and `target`. Nothing removes
   caches.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| ROT-014 | A switch, manual or automatic, first checks that the new session can start (the task exists in a cloned repository, a subscription's credential opens, the sandbox is within its disk quota and its holder starts). If it cannot, the running session stays untouched, the task keeps its status, and the reason is written in the task's terminal and recorded as a refused switch decision; a manual switch answers with the reason. A launch that fails after the old session ended also writes its reason in the terminal and in the switch decision. | `supervisor::switch_preflight_*`, `supervision.rs` |
| NFR-008 | A refusal over the disk quota measures the sandbox again first when its reading is older than a minute (`QUOTA_RECHECK`), so freeing space helps at once. | `manager::over_quota_*` |
| NFR-009 | Yulius frees disk without a person: (a) build caches (a Cargo `target` next to a `Cargo.toml`, `node_modules/.cache`, `.next/cache`, and the paths of `[disk] caches` in the worktree's `yulius.toml`) of the worktrees of tasks that are not live, queued or waiting are removed every few minutes; (b) hygiene lists and removes worktrees of closed and cancelled tasks (clean ones only; the branch stays, so Continue recreates the worktree); (c) when a sandbox reaches 90 % of its quota, or a start would be refused, the caches of idle tasks (a running task at a turn boundary, paused or chained) are pruned too, ended tasks first and the largest first, until it is below 80 %, before anything is refused; (d) a task whose cache was removed gets a message saying what was freed and that it comes back with the next build. | `disk::*`, `hygiene` tests, `supervision.rs` |

## Design

- `yulius-domain::disk`: pure rules: `cache_paths(entries, declared)` names the cache
  directories of a worktree from its root entries; `plan_prune` orders candidates (ended tasks
  first, then idle, largest first) and stops once enough is freed. Working tasks are never
  candidates.
- `SandboxIsolation` gains `usage(sandbox)` (last reading and quota) and `measure(sandbox)`
  (a fresh one); `Manager::prepare` re-measures over quota when the reading is older than
  `QUOTA_RECHECK`.
- `App::free_disk(sandbox, level)` removes caches (bounded walk, never follows links, only
  paths inside the worktree) and sends the owning task a message; run every `DISK_EVERY`
  from the background loop (ended tasks always, idle tasks at 90 %) and by `isolation_join`
  before a refusal (measure again, retry once).
- `switch_task_now` calls `preflight_start` before the `SwitchStarted` transition.

## Acceptance criteria

1. Over quota, a switch of a running task fails with the reason; the session is still running
   and the task is `running`.
2. A reading 2 minutes old that says "over quota" is replaced by a fresh measurement before the
   refusal; a fresh one is not measured again.
3. After a task is closed, its `target` is gone at the next pass and its worktree is listed by
   hygiene; a running task's `target` stays below 90 %.
4. At 90 % an idle task's `target` is pruned and the task receives a message with the size.

## Test plan

- `yulius-sandbox` manager: over-quota refusal re-measures an old reading, keeps a fresh one,
  and passes once space is freed.
- `yulius-domain` disk: recognised and declared caches, traversal and absolute paths rejected,
  pruning order and stop condition; `[disk] caches` parsing and errors.
- `yulius-app`: preflight leaves the session and status untouched and notes the reason; hygiene
  classifies closed and cancelled tasks' worktrees; pruning removes caches of ended tasks only,
  of idle ones only under pressure, never of a working one, and messages the task.
- Not automated: a real 100 GB sandbox (checked with the unit's figures).

## Plan

- [ ] Quota refusals measure again when the reading is old
- [ ] A switch checks the new start before it ends the running session
- [ ] Cache and worktree rules: recognised and declared caches, closed and cancelled worktrees in hygiene
- [ ] Yulius prunes caches of ended and idle tasks and tells their agents
- [ ] Docs, ADR, spec rows and changes log updated; every gate green

## Decisions

- Caches are removed, never moved: a build recreates them. Only recognised or declared paths,
  never `.git` or sources.
- A working task's caches are never pruned, even over quota: the quota stays soft and refuses
  new starts only, as before.
- The preflight runs the real start preconditions (credential, quota, holder) instead of a copy
  of them, so a new precondition cannot be forgotten.
