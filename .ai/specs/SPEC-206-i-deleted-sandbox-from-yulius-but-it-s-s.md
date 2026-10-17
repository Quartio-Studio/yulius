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

Symptom: a person removed a sandbox so that Yulius would stop working on its repository, and a
Claude Code session of it went on working (the screenshot: a session with "Seven read-only
inventory agents are running" asking how to handle Yulius). That is the sandbox's Superagent, a
per-person Claude Code session in a sandbox shell, still running behind the person's open panel.

Root cause: `App::delete_sandbox` (`crates/yulius-app/src/projects.rs`) stops the sandbox's tasks
and marks it removed, and nothing else. What survives:

| Activity | Why it survives |
| --- | --- |
| Superagents, terminals | `delete_sandbox` never touches shells; only the idle reaper ends them (no viewer for `SHELL_IDLE_MS`), and a person watching one keeps it alive. Reproduced: both still live 20 s after `DELETE`. |
| CI runs | Only a run that *starts* after removal is refused (`ci/exec.rs`); a queued run fails with an error and a running one runs to its end and publishes its status. |
| Browser | Ended only by its idle reaper (30 min). |
| Holder | `Manager::tick` stops a holder only when no process runs in its network namespace. A container, a dev server or a `nohup` an agent left keeps the holder (and its Docker engine) up forever. After a restart `adopt` takes it back. SPEC-171's purge of a removed playground waits for the same processes. |

Not a leak, checked: an armed merge of a removed sandbox's task is kept but never merged
(`merge_armed_task` cannot read the removed sandbox's delivery, and the merge train skips removed
sandboxes); a test now guards it.

Why tests missed it: `remove_stops_tasks_and_keeps_the_repository` checks task statuses only; no
test removes a sandbox with a shell, a browser, a CI run or a holder.

Related, kept out of this change: #582 (the removed sandbox's `sysroots/<id>` is never deleted)
needs the holder stopped first, which this change provides. `disk.rs` calls
`purge_removed_sandbox` over `store.sandboxes()`, which never lists removed sandboxes, so the
removed sandbox's worktrees are never purged (pre-existing, reported separately).

## Requirements

- **RMV-001** Nothing armed in a removed sandbox is merged, also when its checks pass after the
  removal (holds today; guarded by a test).
- **RMV-002** Removing a sandbox cancels its queued and running CI runs ("the sandbox was
  removed").
- **RMV-003** Removing a sandbox ends its shells and stops its browser, in the background (the
  request does not wait for them).
- **RMV-004** Removing a sandbox retires its holder: the manager stops it, and with it every
  process in its namespaces (Docker engine, containers, leftovers), as soon as no lease of a run
  is left, whatever else still runs there, and its holder is not adopted after a restart (a
  marker in its state directory).
- **RMV-005** PRJ-009 stays: repository, clone, worktrees, transcripts, task records and the
  export are kept.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] A removed sandbox's Superagents, terminals and browser end
- [x] A removed sandbox's CI runs are cancelled
- [x] A removed sandbox's holder and leftover processes stop
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- An armed merge whose checks turn green after its sandbox was removed is not merged; the
  pull request stays open.
- A running CI run of a removed sandbox is cancelled at removal (its jobs killed).
- A removed sandbox's Superagent and terminal end (the viewer's socket closes, no shell host is
  left) and its browser stops; another sandbox's browser goes on.
- A retired sandbox's holder is stopped by the next tick once its leases are gone, even with
  other processes in its namespaces; `adopt_one` stops it instead of adopting it.

## Test plan

- `cockpit::removing_the_sandbox_ends_its_superagent_and_terminals`: the real detached ptyhost;
  a watched Superagent and a terminal end after `DELETE` (failed on dev).
- `browser::removing_the_sandbox_stops_its_browser`: fake Chromium; the removed sandbox's process
  dies, the other sandbox's lives (failed on dev).
- `local_ci::removing_the_sandbox_cancels_its_ci_runs`: a slow running PR run is `cancelled`
  ("the sandbox was removed") within 5 s, before its 8 s job timeout, with no dispatch (failed on
  dev).
- `projects::remove_retires_the_sandbox_holder`: a fake isolation records `retire` of exactly the
  removed sandbox.
- `yulius-sandbox` manager: `holder_stops` table; a child `sleep` as an adopted holder is stopped
  by the tick once retired while this test's own process runs in its namespaces, and is stopped
  instead of adopted by a restarted manager (both fail with the marker check disabled).
- `delivery::a_removed_sandbox_merges_nothing_it_had_armed`: guard for RMV-001 (passes on dev).
- Not run here: the opt-in host isolation suite (`YULIUS_ISOLATION_TEST=1`, real `bwrap` holder
  and Docker engine as the service user); the manager tests use a stand-in holder.

## Decisions

- **Retire, not stop at once.** The tasks are only asked to stop at removal (they are Stopping
  for up to the escalation). Killing the holder under their runs would end them uncleanly and
  leave the supervisor to find out; waiting for the leases lets them end as any stop does, and
  stray processes do not hold the holder up. Alternative rejected: stop the holder in
  `delete_sandbox` directly.
- **A marker file, not a store query.** The manager knows nothing of the store; a `retired`
  file in the holder's state directory survives a restart and is read by `adopt_one`, without a
  new port method that lists removed sandboxes.
- **No change for armed merges.** Investigated first (the screenshot spoke of merging); the
  removed sandbox's armed merges are never merged, so only a guard test is added.
- **Shells and the browser end in the background**, after the sandbox is marked removed (which
  refuses new ones): the request does not wait for each shell's exit grace.
- No ADR: this completes PRJ-009 ("stops live and queued tasks") rather than taking a new
  lasting decision.
