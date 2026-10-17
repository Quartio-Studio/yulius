---
yulius-task: de8d2402-4860-4b4f-8147-77e7f1136a6f
status: in-progress
---

# SPEC-206: Removing a sandbox stops everything Yulius runs for it

Status: in-progress
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
  request does not wait for them); a shell or browser whose start was under way at the removal
  ends once it reads the removal.
- **RMV-004** Removing a sandbox retires its holder: `prepare` refuses the sandbox, and the
  manager stops the holder (its Docker engine, containers and egress) `RETIRE_GRACE` (60 s) after
  the retirement once no lease of a run is left, whatever else still runs there; it is not
  adopted after a restart (a marker in its state directory, whose time is the retirement's). A
  run's own processes end with the run: each run has its own PID namespace (`--die-with-parent`).
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
- A retired sandbox's holder is kept within the grace and stopped by a tick after it once its
  leases are gone, even with other processes in its namespaces; `prepare` refuses it;
  `adopt_one` stops it instead of adopting it.
- A Superagent whose start was held while the sandbox was removed ends once it registers; a
  removed sandbox's stopping task gets no browser from the browser tools.

## Test plan

- `cockpit::removing_the_sandbox_ends_its_superagent_and_terminals`: the real detached ptyhost;
  a watched Superagent and a terminal end after `DELETE` (failed on dev).
- `browser::removing_the_sandbox_stops_its_browser`: fake Chromium; the removed sandbox's process
  dies, the other sandbox's lives (failed on dev).
- `local_ci::removing_the_sandbox_cancels_its_ci_runs`: a slow running PR run is `cancelled`
  ("the sandbox was removed") within 5 s, before its 8 s job timeout, with no dispatch (failed on
  dev).
- `cockpit::a_superagent_starting_while_the_sandbox_is_removed_ends`: the start held by the
  gated host across the `DELETE` (failed before the fix).
- `browser::a_removed_sandbox_gets_no_browser_from_its_stopping_task`: `browser_open` with the
  stopping task's token is refused and starts no Chromium (failed with the checks disabled).
- `projects::remove_retires_the_sandbox_holder`: a fake isolation records `retire` of exactly the
  removed sandbox.
- `yulius-sandbox` manager: `holder_stops` table with the grace; `prepare` refuses a retired
  sandbox; a child `sleep` as an adopted holder is kept within the grace and stopped by a tick
  after it (the marker dated back) while this test's own process runs in its namespaces, and is
  stopped instead of adopted by a restarted manager (both failed with the marker check disabled).
- `delivery::a_removed_sandbox_merges_nothing_it_had_armed`: guard for RMV-001 (passes on dev).
- Not run here: the opt-in host isolation suite (`YULIUS_ISOLATION_TEST=1`, real `bwrap` holder
  and Docker engine as the service user); the manager tests use a stand-in holder.

## Decisions

- **Retire, then stop after a grace.** The tasks are only asked to stop at removal (they are
  Stopping for up to the escalation, about 20 s) and shells get their exit grace. Leases do not
  cover a run's life (a task or shell drops its lease once launched; only CI and CD jobs hold one
  to their end), so the holder waits `RETIRE_GRACE` (60 s) from the retirement, then stops once
  no lease is left, whatever else runs in it. The marker's mtime is the retirement time, so a
  restart after the grace stops it at once. Alternatives rejected: stop the holder in
  `delete_sandbox` directly (cuts egress and the engine under runs still ending); wait until
  nothing runs in it (the engine and its containers never end by themselves).
- **Starts under way at the removal.** Access is checked when a start begins, so a start can
  pass it just before the removal. Each start reads the removal again after it registers (a
  shell) or after the browser runs, and ends what it started; the removal records `removed_at`
  before it snapshots the shells, so one of the two always ends it. `prepare` refusing a
  retired sandbox keeps such a start from making a new holder (mapped to `NotFound`, never to
  the previous containment).
- **A CD deploy already running finishes.** New deploys are refused (they need access to the
  sandbox); cancelling one half-way can leave an environment half-deployed, and a deploy is
  bounded by its timeout.
- **A marker file, not a store query.** The manager knows nothing of the store; a `retired`
  file in the holder's state directory survives a restart and is read by `adopt_one`, without a
  new port method that lists removed sandboxes.
- **No change for armed merges.** Investigated first (the screenshot spoke of merging); the
  removed sandbox's armed merges are never merged, so only a guard test is added.
- **Shells and the browser end in the background**, after the sandbox is marked removed (which
  refuses new ones): the request does not wait for each shell's exit grace.
- No ADR: this completes PRJ-009 ("stops live and queued tasks") rather than taking a new
  lasting decision.

## Evidence

- Root cause reproduced on `dev` (a19efbac): `cockpit::removing_the_sandbox_ends_its_superagent_and_terminals`
  timed out with both shells live 20 s after `DELETE`; `browser::removing_the_sandbox_stops_its_browser`
  found the browser alive; `local_ci::removing_the_sandbox_cancels_its_ci_runs` found the PR run
  still `Running` 5 s after `DELETE`. The armed-merge guard passed on `dev` (no change needed).
- After the fix, each of those passes; the review's two in-flight tests failed before their fix
  and pass after; the manager tests fail with the marker check disabled and pass with it.
- `scripts/dev ci` at 11b8aac5 (before the review fixes): exit 0, 2103 tests passed, 18 skipped
  (opt-in suites), lint, deps and docs clean. Re-run on the final head: see the pull request.
- `scripts/dev security` (Vidoc vidoc-1) on the change: exit 0, no open critical or high finding.
  Two pre-existing high findings in files this change touched were handled: `write_scaffold`
  (`projects.rs`) now refuses non-plain paths (`a_scaffold_path_never_leaves_the_clone`), and the
  browser port's unauthenticated loopback DevTools endpoint (documented trust model, Chromium
  flags in yulius-browser) is an `[[ignore]]` entry in `.config/vidoc.toml` (path
  `crates/yulius-app/src/ports/browser.rs`, title "Unauthenticated"), agreed by the independent
  reviewer and tracked in #626.
- Not run here: the opt-in host isolation suite (`YULIUS_ISOLATION_TEST=1`: a real `bwrap`
  holder and Docker engine as the service user), so the retired holder's stop was tested with a
  stand-in holder, not a real engine.
- Issues: #624 (this defect), #625 (pre-existing: removed sandboxes' worktrees are never purged),
  #626 (pre-existing: unauthenticated DevTools endpoint).
