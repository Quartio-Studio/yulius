---
yulius-task: c0e1fbe5-0e04-4474-973a-5b52218d1031
status: in-progress
---

# SPEC-160: an update freezes every task at once

Status: in progress
Date: 2026-10-15
Owner: Yulius task #151
Scope authorization: the request of task #151, with a screenshot of the instance page stuck at
"Pause tasks — Pausing tasks at their next step: 7 of 8 paused. Still in a turn: #148 …": "tasks
should be paused imidietly, not waiting for something to finish, but the agent should never notice
that. Like tokens should just stopped streaming and they should get resumed after the update".
It changes how OPS-008 (SPEC-001) pauses tasks; the rest of OPS-006..OPS-009 stays.

## Goal

When an update's build is done, every running task stops where it is, mid-turn and mid-token,
without being told: its output stops, and it goes on from the same instruction after the restart.
The update no longer waits for long turns (an autonomous `/goal` turn lasts hours) and no longer
gets cancelled after 30 minutes because one task was still working.

## Context

- SPEC-002 ADR-18 (§2.5, `crates/yulius-app/src/updates.rs`): an update run goes `building` →
  `draining` → `installing` → `resuming`. While draining, the `Stop` and `UserPromptSubmit` hooks
  are answered `202 Retry-After: 3` (the hold) and the install waits until every live task is at a
  **pause point** (`yulius_domain::update::at_pause_point`: held in the last 15 s, paused for
  capacity, or not in a turn) on two consecutive 2 s passes; 30 minutes → cancelled; "Continue
  without waiting" installs anyway. Resuming releases the held hooks task by task, 2 s apart.
- A turn in progress was not a pause point because a restart under it could fail a Yulius tool
  call, a hook or a Git credential request the agent would see.
- Each agent run is a detached `yulius ptyhost` (the restart keeps it: `KillMode=process`) whose
  PTY child is the agent, inside `bwrap` for contained runs. With per-sandbox limits (ADR-22 §5,
  `crates/yulius-sandbox/src/cgroup.rs`), the server owns the delegated cgroup v2 subtree
  `<unit>/sandboxes/<sandbox>/{system,agents,ci}`; the ptyhost joins `agents/` before it forks, so
  the agent and every process it starts live there, next to people's shells and browsers.
- cgroup v2's freezer (`cgroup.freeze`, confirmed by `frozen 1` in `cgroup.events`) stops every
  process of a group with no signal: nothing in the group can observe it (unlike `SIGSTOP`, which
  parents see through `SIGCHLD`/`waitpid` and programs through their `SIGCONT` handlers). A
  frozen process's sockets stay open; data sent to it waits in the kernel.
- The server's listeners shut down gracefully (`serve.rs`, `with_graceful_shutdown`): a request
  already received is answered before the server exits, its answer waiting in the frozen client's
  socket.
- Agent HTTPS goes through the server's egress proxy (ADR-20, in the server process): a provider
  stream open at the restart is cut whatever pauses the agent.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| OPS-010 | When the build is done (`draining`), every live task's agent run is **frozen at once** with the cgroup v2 freezer: the agent's whole process tree (the agent, its tools, its background shells, found as the descendants of the run's recorded child in the child's cgroup) moves into the run's own group `run-<attempt>` under it and that group is frozen. The ptyhost, people's shells and browsers, CI jobs and the sandbox holder are never frozen. Nothing is typed into, signalled to or told to the agent, and task states are untouched. A task counts as paused once its group reports `frozen 1`. | unit `freeze.rs`; int `self_update.rs` |
| OPS-011 | A run that cannot be frozen (no delegated cgroup, its child not under `<unit>/sandboxes/`, the run gone, a write refused, its child not moved) **pauses at its turn boundary as before** (the hold, the 30-minute bound, "Continue without waiting"). **Nothing is frozen while such a run is still in a turn** (checked without freezing, `SessionHost::can_freeze`): the others freeze once it is held, so no task sits frozen while another works. A run the kernel has not confirmed frozen after 5 passes (10 s) is thawed and pauses at its turn boundary instead. Freezing never fails an update by itself. | unit `freeze.rs`; int `self_update.rs` (existing tests unchanged) |
| OPS-012 | The install is requested once every live task is frozen or at a pause point on two consecutive passes (one pass, 2 s, lets Yulius requests the agents had already sent be answered; the graceful shutdown answers the rest). | int `self_update.rs` |
| OPS-013 | **Thawing.** Freeze passes and thaws of all runs are serialized, and each freeze re-checks that the update still drains, so a cancel never leaves a run frozen; with no open run, anything still recorded frozen is thawed. While the hold is on, every task not yet released counts as paused (`held_for_update`: a frozen agent makes no hook call, so the launch goal's watch stops its clock). Resuming thaws the runs one by one (2 s apart), each task's hook release before its thaw; a cancel (by a person, the 30-minute bound or a missing helper) thaws every frozen run at once; a failed or rolled-back install resumes and thaws the same way. A server that starts with no update holding tasks thaws every run group left frozen under its delegated unit (a crash between freeze and resume); one that starts while the run installs or resumes keeps them frozen until it resumes them. | unit `freeze.rs`; int `self_update.rs` |
| OPS-014 | The instance page and the banner say tasks pause **at once**: the step "Pause tasks" reads "Tasks pause at once, mid-step. The agents don't notice."; the message "Pausing tasks: N of M paused."; "Still in a turn" lists only tasks that could not be frozen; the confirmation says tasks pause at once. | e2e `update.e2e.ts` (mock) |
| OPS-015 | SPEC-001 OPS-008, SPEC-002 ADR-18, the architecture document, the product summary, an ADR and `docs/changes.md` describe it, including its limits. | review |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] A run's agent tree can be frozen and thawed in its own cgroup, never its host
- [x] An update freezes every task as soon as the build is done and installs within seconds
- [x] Tasks thaw one by one after the restart, at once on a cancel, and none stays frozen after a crash
- [x] The instance page, banner and confirmation say tasks pause at once
- [x] Docs, ADR and the changes log describe the freeze and its limits
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, evidence recorded, delivered on green CI

## Acceptance criteria

- AC-OPS-10: with a fake cgroup tree and process table, freezing a run whose child (pid 100) has
  descendants 101 and 102 (102 reparented under 101) in `…/sandboxes/s1/agents` writes `1` to
  `agents/run-a1/cgroup.freeze` and moves 100, 101 and 102 (never the ptyhost 99 or a shell 200)
  into `agents/run-a1/cgroup.procs`; it reports frozen only when `cgroup.events` says `frozen 1`;
  freezing again (the child now in `run-a1`) reuses the group.
- AC-OPS-11: a child outside `<unit>/sandboxes/`, an unknown child, or no delegated unit is
  "cannot freeze" and creates nothing. Through the API, with a host that cannot freeze, the
  existing turn-boundary tests pass unchanged.
- AC-OPS-12: through the API with a host that freezes, two tasks — one mid-turn, one idle — are
  both frozen on the first round after the build and the install is requested on the next round,
  with no hook having been held; the view says "Every task is paused. Restarting next." and lists
  no task still in a turn. A run the host cannot freeze is still waited for.
- AC-OPS-13: after the restart, resuming thaws each task's run, one per stagger; cancelling while
  draining thaws both at once; a rolled-back install thaws them; a server that starts with no open
  run thaws every left-over frozen group (`thaw_all` called), one that starts while installing
  does not.
- AC-OPS-14 (mock UI): the instance page shows "Tasks pause at once, mid-step." and "Pausing
  tasks: 2 of 3 paused."; no WCAG violations.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- Unit, `crates/yulius-sandbox/src/freeze.rs` on a temporary cgroup tree with a fake process
  table: AC-OPS-10, AC-OPS-11 (refusals), thawing one run and `thaw_all` (only frozen `run-*`
  groups under `sandboxes/`), leaf names that are not attempt ids refused.
- Integration, `crates/yulius/tests/self_update.rs` with a recording fake session host that freezes
  (or not, or never settles, or blocks a freeze while a cancel lands): AC-OPS-11..13; the existing
  tests keep the host that cannot freeze (AC-OPS-11).
- e2e, `e2e/tests/update.e2e.ts` against the mock: AC-OPS-14.
- Not testable here: the kernel freezer itself (this sandbox has no `/sys/fs/cgroup`); the paths
  and file contents are the kernel's documented interface, and the freeze is verified on a
  delegated host by the operator (docs/operations.md).

## Decisions

- **The cgroup v2 freezer, not `SIGSTOP`.** `SIGSTOP`/`SIGCONT` through the ptyhost would work
  without delegation, but programs see it: `SIGCONT` handlers run (Claude Code redraws), parents
  get `SIGCHLD`, and processes in their own session or group (Claude Code's background shells)
  escape it. The freezer stops the whole tree with nothing to observe, which is what "the agent
  should never notice" asks. Runs without delegation keep the turn-boundary pause.
- **A group per run, created at the freeze, holding the agent's tree only.** Freezing `agents/`
  would freeze people's shells, browsers and the ptyhosts; a frozen ptyhost would not answer the
  restarted server's attach, which then ends the run as lost. Moving the tree at the freeze (not
  at the spawn) needs no change to how runs start and works for runs started before this change.
  The group stays after the thaw (the processes stay in it, under the same sandbox limits) and is
  reused by the next freeze; an empty one is removed by the next sweep.
- **The hold stays.** It still pauses runs that cannot be frozen, and it is harmless for frozen
  ones (they make no hook calls).
- **The provider stream is cut by the restart.** The egress proxy lives in the server process, so
  a response streaming at the restart ends; after the thaw Claude Code sees a dropped connection
  and retries the request itself (the model sees nothing; the terminal may show a retry line).
  Keeping tunnels open across a restart needs the proxy outside the server: follow-up #529.
- **Nothing freezes while a run that cannot be frozen still works (review round 1).** Freezing
  the others at once and waiting for it would keep them frozen for up to the 30-minute bound, and a
  tool timeout (2 minutes for Claude Code's Bash tool) would then fire after the thaw: the agent
  would notice. They stay running and freeze the moment it is held, so the frozen time stays the
  install's. Without delegation every run is such a run and the update behaves as before.
- **The thaw needs a binary with this change.** The server that thaws is the one installed (or
  restored by a rollback, which is the binary that froze). Installing a binary without this change
  (a revert) would leave groups frozen until thawed by hand (`docs/operations.md` §8 gives the
  command); a revert must keep `thaw_all` at start.
- **Wall-clock timers count the pause.** A tool timeout (Claude Code's Bash tool: 2 minutes by
  default) keeps running while frozen, so a command within about a minute of its timeout may time
  out after the thaw. The pause is about a minute (install and health check), which keeps this
  rare.
- Recorded as ADR-0131.
- **Review round 1** (fresh session, REQUEST CHANGES of a2fcb80f: three majors, four minors, two
  nits), all fixed test-first: a cancel racing a freeze pass (serialized, re-checked, plus a
  backstop thaw with no open run; `a_cancel_during_a_freeze_pass_leaves_nothing_frozen`); runs kept
  frozen while an unfreezable one works (nothing freezes until it is held;
  `a_run_that_cannot_be_frozen_is_waited_for_before_any_freeze`); the launch goal's watch counting
  frozen time (`held_for_update` counts every unreleased task under the hold); a child that did
  not move counted as frozen (`FreezeError::NotMoved`); a run that never settles (thawed after 5
  passes; `a_run_that_never_settles_falls_back_to_its_turn_boundary`); the revert constraint
  recorded; the fake sweep no longer hides per-run thaws; both group paths and a manual thaw in
  `docs/operations.md`; a debug line when the last move pass still finds processes.
