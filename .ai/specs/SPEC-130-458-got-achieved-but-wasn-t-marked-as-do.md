---
yulius-task: 2df292c3-521c-4323-b6cb-bc08cf82281f
status: done
---

# SPEC-130: Done means done: orphaned subshells and `yulius wait`

Status: implemented
Date: 2026-10-14
Owner: Yulius task #117
Scope authorization: task #117 ("#458 got achieved but wasn't marked as done. I closed it manually.
Fix.", with a screenshot of task #108 reading "working 3h 46m" under "✓ Goal achieved"). The same
session's follow-up, a screenshot of task #109 under Done: "this is done but pr isnt purple".
Fixes #473 and #474. Refines SPEC-001 TSK-014 and SPEC-085 STT-002.

## Goal

The sidebar's Done is true in both directions:
- A task whose goal is achieved reads Done, even when its agent left a server running behind a
  backgrounded subshell.
- A task whose agent still waits in `yulius wait` for its armed merge does not read Done before
  that merge.

## Context

**Symptom 1 (#473, task #108).** The goal was achieved at 20:09:48Z: `goal_status met:true`, a
clean Stop, then the turn record with no pending agents. Replaying the transcript through
`observe_line` gives `TurnComplete`, then `Goal{Met}`, then `TurnEnded{None}`. The finish rule
(`finish.rs`) makes that Done. The sidebar still read Working. A Done task with background shells
renders exactly like Working (`stillWorking` in `status.js`).

**Root cause 1.** At 16:38Z, task #108 ran this as a foreground Bash command:
`(cd ../crates/yulius-server/web && python3 -m http.server 20100 >/dev/null 2>&1 &)`.
- Bash forks an async subshell for it. That subshell carries Claude Code's wrapper command line
  (`… && pwd -P >| /tmp/claude-…-cwd`, `SHELL_SIGNATURE`) and remains the server's parent.
- When the wrapper exits, the orphan is re-parented to the init of the sandbox's PID namespace:
  the inner `bwrap` (`containment.rs`: `--unshare-pid`).
- The idle-shell probe (`yulius_ptyhost::shells`) walks from the run's `child_pid`, which is that
  sandbox's `bwrap`. So the orphan is a top-level wrapper shell under the root, and it counts as a
  background shell for as long as the server lives. Nothing ever ends it.

This was reproduced in a task sandbox: python → `/bin/bash -c … pwd -P >| …` → `bwrap` (pid 1).
Claude Code itself does not track the orphan (no "· 1 shell"), and it never wakes the agent.

**Why the tests missed it.** The shell tests run the agent unsandboxed: the probe's root is the
agent itself, and orphans go to the host's init, outside the tree. The sandbox (bwrap with a PID
namespace) came later and made the root an ancestor of the orphans' new parent.

**Symptom 2 (#474, task #109, PR #471).** The agent armed its merge and ended its turn in
`yulius wait`; its goal is not achieved. Its row reads `waiting`, `yulius_wait: true`, steps 6/6,
PR open with green checks. The sidebar showed "done 10m ago". The chip was rightly not purple: the
PR is open.

**Root cause 2.** `taskInfo` (`status.js`) and its server twin `AttentionFacts::group`
(`yulius-domain/src/task.rs`) handle a finished turn with a complete plan and an open, tested PR
like this: background shells keep it Working, and everything else is Done. `yulius_wait` is
consulted only for failing checks and for the no-PR path. Both doc comments already say a
`yulius wait` keeps the task Working (SPEC-085 STT-002).

## Requirements

- **DMD-001.** The probe counts a wrapper shell only when it is the agent's. A wrapper shell whose
  parent is the init of a PID namespace was adopted as an orphan. It does not count, and nothing
  under it counts either.
- **DMD-002.** Shells under the agent still count in the sandboxed layout (bwrap → bwrap (ns init)
  → claude → shell) and in the unsandboxed one (agent → shell). `yulius wait` shells are still
  counted apart.
- **DMD-003.** An idle task whose goal is not achieved and whose agent waits in `yulius wait` reads
  "Waiting for checks or merge" (group Working), whatever its plan or pull request says. An
  achieved goal stays Done (`activity: done`). Server and client give the same group.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] An orphaned subshell the sandbox adopted no longer counts as a background shell
- [x] A task waiting in yulius wait with a tested pull request reads Working, not Done
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- The probe's count is 1 for a sandboxed table with one shell under claude and one orphaned
  wrapper subshell (with its server and a nested subshell) under the namespace init. Today it is 2.
- On Linux, `/proc` reads mark this process's PID-namespace init (pid 1 as seen from inside) as an
  init and this test process as not one.
- `AttentionFacts::group` is Working for `waiting` + `yulius_wait` + plan complete + PR tested,
  in review or merged. It stays Done for `done` + `yulius_wait`.
- The sidebar shows "Waiting for checks or merge" for such a task, not Done.

## Test plan

- Unit (`yulius-ptyhost`): the counting rule on synthetic tables (sandboxed and unsandboxed), and
  the `NSpid` parser.
- Real process (`yulius-ptyhost`, Linux): the init marker read from `/proc`.
- Unit (`yulius-domain`): `AttentionFacts::group` for the waiting and done cases.
- E2E (`e2e/tests/task-done.e2e.ts`): a waiting task in `yulius wait` with a complete plan and a
  green open PR reads Waiting for checks or merge, in the Working group.
- Full gate: `scripts/dev ci` and the e2e suite.

## Decisions

- **Recognise orphans by their parent being a PID-namespace init (innermost `NSpid` is 1).** This
  is chosen over two alternatives:
  - Rooting the probe at the agent (claude) instead of bwrap needs the run to find its agent's pid
    inside the sandbox, across restarts.
  - Requiring each top shell to be a process-group leader relies on how Claude Code spawns
    commands, which the fake agent and other harnesses do not share.

  The namespace init is exactly the process that adopts orphans in the sandbox, and the kernel
  states it. Without `/proc` (macOS `ps`), nothing is marked; there is no sandbox there.
- **Orphans are not killed.** A leftover server is not the agent's background work, but it may be
  the person's preview. Ending it is the session's business (it ends with the sandbox).
- **`yulius wait` wins over a tested PR, not over an achieved goal.** The agent still merges,
  deploys and reports. An achieved goal is Claude Code's own verdict that the work is done, and the
  existing rule keeps it Done with a leftover wait.
- **The `/proc/<pid>/status` read stays per non-shell process.** It is bounded (64 KiB each,
  within `MAX_PROCESSES`), and only the probe of idle tasks pays it. Reading it lazily only for
  the parents of shells found outside counted shells would mix the `/proc` reads into the pure
  counting rule. That is reconsidered if the probe ever shows in a profile.
- **Both fixes ship in one change.** Both are the same concern, "the sidebar's Done is wrong", and
  the person raised the second in this task. Each is a few lines, with its own regression test.

## Evidence

- **Root cause 1, reproduced in a task sandbox.** `(cd /tmp && python3 -m http.server 20118 &)`
  run as a foreground command leaves `python3` (7285) under
  `/bin/bash -c … && pwd -P >| /tmp/claude-…-cwd` (7283), and that subshell is under `bwrap`
  (pid 1, `NSpid: 1`). Claude Code's own shells sit under `claude` (pid 2) →
  `yulius --scrub-exec …` → bash. Task #108's transcript, replayed through `observe_line`, ends
  `TurnComplete` → `Goal{Met}` → `TurnEnded{background_agents: None}`, so its activity was Done.
- **Root cause 2.** Task #109 read `running (waiting)`, steps 6/6, PR #471 open with green
  checks, armed merge, agent in `yulius wait` (`task_get`, transcript 20:18:37–20:18:41Z).
- **Regression tests failed first** (commit `349b82d3`):
  - `shells::tests::an_orphan_the_sandbox_adopted_is_no_background_shell`: left
    `ShellCount { shells: 3 }`, right 1.
  - `task::tests::a_wait_for_the_merge_is_not_done_before_it`: `Tested`, left `Done`, right
    `Working`.
  - e2e `a task waiting for its armed merge is not Done before it`: observed "Done · PR #221",
    expected /Waiting for checks or merge/.
- **After the fix:**
  - `cargo test -p yulius-ptyhost --lib shells::`: 8 passed, including the real `/proc` read of
    the namespace init.
  - `cargo test -p yulius-domain --lib task::`: 16 passed.
  - `npx e2e run tests/task-done.e2e.ts`: 14 passed.
- **Gates:**
  - `scripts/dev ci` exit 0 on `6226dc56`: 1643 tests passed, 16 skipped.
  - Full e2e suite (`npx e2e run`) exit 0 on `f61c9b7e`: 395 passed, 1 skipped. The later commits
    change only Rust doc comments and an ADR.
- **Independent review** (fresh session, om-code-review): APPROVE `f61c9b7e`. Its minor (complete
  the spec) and two nits (doc wrap, ADR-0068 amendment) are done; the others are recorded above
  or out of scope.
- **Not tested here:** the probe on a real host against a live sandboxed task. The sandbox cannot
  read its own init's child list from the host's side, so the rule is covered by the synthetic
  table of the observed tree and the real `NSpid` read.
