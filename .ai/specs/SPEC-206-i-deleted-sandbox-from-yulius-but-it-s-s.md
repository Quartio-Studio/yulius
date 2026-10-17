---
yulius-task: de8d2402-4860-4b4f-8147-77e7f1136a6f
status: draft
---

# SPEC-206: Removing a sandbox stops everything Yulius runs for it

Status: draft
Date: 2026-10-17
Owner: task #193
Scope authorization: task #193 (requested by MrFishPL): "i deleted sandbox from yulius but it's
still running, fix", with a screenshot of a session saying "Yulius is merging into quartio right
now". Fixes #624. Amends PRJ-009 (SPEC-001) and SPEC-171's purge wait.

## Goal

`DELETE /api/sandboxes/{sid}` ends all of the sandbox's activity, not only its tasks: nothing is
merged into its repository afterwards, no CI run of it goes on, and its shells, browser, holder,
Docker engine and leftover processes stop. The repository, clone, worktrees, transcripts and
task records are kept, as PRJ-009 says.

## Context

Symptom: a person removed a sandbox so that Yulius would stop working on its repository, and
Yulius went on merging pull requests into it.

Root cause: `App::delete_sandbox` (`crates/yulius-app/src/projects.rs`) stops the sandbox's tasks
and marks it removed, and nothing else. What survives:

| Activity | Why it survives |
| --- | --- |
| Armed merges | The `ArmedSlot` of a task is never cleared when the task stops. `App::merge_armed` runs after **every** CI run's end (`ci/exec.rs`) and never checks `removed_at`; the delivery loop skips removed sandboxes, the CI end does not. A run already executing at removal ends, the armed pull request is green, and Yulius merges it. |
| CI runs | Only a run that *starts* after removal is refused (`ci/exec.rs`); a queued run fails with an error and a running one runs to its end and publishes its status. |
| Shells | Ended only by the idle reaper (no viewer for `SHELL_IDLE_MS`). |
| Browser | Ended only by its idle reaper (30 min). |
| Holder | `Manager::tick` stops a holder only when no process runs in its network namespace. A container, a dev server or a `nohup` an agent left keeps the holder (and its Docker engine) up forever. After a restart `adopt` takes it back. SPEC-171's purge of a removed playground waits for the same processes. |

Why tests missed it: `remove_stops_tasks_and_keeps_the_repository` checks task statuses only; no
test removes a sandbox with an armed merge, a CI run, a shell or a holder.

Related, kept out of this change: #582 (the removed sandbox's `sysroots/<id>` is never deleted)
needs the holder stopped first, which this change provides. `disk.rs` calls
`purge_removed_sandbox` over `store.sandboxes()`, which never lists removed sandboxes, so the
removed sandbox's worktrees are never purged (pre-existing, reported separately).

## Requirements

- **RMV-001** Removing a sandbox drops the armed merges of its tasks, and `App::merge_armed`
  merges nothing for a removed sandbox (also a decision already under way, a restart, a CI run
  that ends later).
- **RMV-002** Removing a sandbox cancels its queued and running CI runs ("the sandbox was
  removed").
- **RMV-003** Removing a sandbox ends its shells and stops its browser, in the background (the
  request does not wait for them).
- **RMV-004** Removing a sandbox retires its holder: the manager stops it, and with it every
  process in its namespaces (Docker engine, containers, leftovers), as soon as no lease of a run
  is left, whatever else still runs there. A retired sandbox is never prepared again and its
  holder is not adopted after a restart (a marker in its state directory).
- **RMV-005** PRJ-009 stays: repository, clone, worktrees, transcripts, task records and the
  export are kept.

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Write the fix spec and the regression test that fails
- [ ] A removed sandbox merges nothing and runs no CI
- [ ] A removed sandbox's shells, browser and holder stop
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- An armed merge whose checks turn green after its sandbox was removed is not merged; the
  pull request stays open.
- A queued CI run of a removed sandbox is cancelled at removal.
- A removed sandbox's live shell is ended and its browser stopped.
- A retired sandbox's holder is stopped once its leases are gone even with another process in
  its namespace; `prepare` refuses it; `adopt_one` stops instead of adopting it.

## Test plan

- `delivery.rs`: arm a merge (`ArmedTask`), remove the sandbox through `DELETE`, turn checks
  green, run `merge_armed`: the stage stays `pr_open`.
- `projects.rs` (or `local_ci.rs`): a queued run of the sandbox is `cancelled` after `DELETE`;
  a fake isolation records `retire`; a fake browser records `stop`; a live shell has ended.
- `yulius-sandbox` manager unit tests: `retire` writes the marker; `prepare` refuses a retired
  sandbox; `adopt_one` on a marked sandbox stops instead of adopting. The tick's stop decision is
  a pure function, tested for a retired sandbox with stray processes and no lease.

## Decisions

- **Retire, not stop at once.** The tasks are only asked to stop at removal (they are Stopping
  for up to the escalation). Killing the holder under their runs would end them uncleanly and
  leave the supervisor to find out; waiting for the leases lets them end as any stop does, and
  stray processes do not hold the holder up. Alternative rejected: stop the holder in
  `delete_sandbox` directly.
- **A marker file, not a store query.** The manager knows nothing of the store; a `retired`
  file in the holder's state directory survives a restart and is read by `adopt_one`, without a
  new port method that lists removed sandboxes.
- **Drop armed merges in two places.** Clearing the slots at removal is what a person expects;
  the guard in `merge_armed` covers a decision already under way and any other caller.
- No ADR: this completes PRJ-009 ("stops live and queued tasks") rather than taking a new
  lasting decision.
