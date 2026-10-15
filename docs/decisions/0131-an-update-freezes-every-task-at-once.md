# 0131. An update freezes every task at once

## Status

Accepted. Specified in [SPEC-160](../../.ai/specs/SPEC-160-tasks-should-be-paused-imidietly-not-wai.md);
task #151. Amends ADR-18 of SPEC-002 (§2.5) and SPEC-001 OPS-008.

## Context

An update paused each task at its turn boundary: the install waited until every live task's
`Stop` or `UserPromptSubmit` hook was held, and gave up after 30 minutes. An autonomous `/goal`
turn lasts hours, so updates sat at "Pausing tasks at their next step: 7 of 8 paused" and were
cancelled. The request: "tasks should be paused immediately, not waiting for something to finish,
but the agent should never notice that. Like tokens should just stop streaming and they should get
resumed after the update."

The turn boundary was chosen because a restart under a turn could fail a Yulius tool call, a hook
or a Git credential request the agent would see. With per-sandbox limits the server already owns
a delegated cgroup v2 subtree in which every agent run lives (`<unit>/sandboxes/<sandbox>/agents`).

## Decision

- When the build is done, the server asks the process host to **freeze** every live task's run.
  The host moves the agent's process tree (the run's recorded child and its descendants in the
  child's cgroup) into the run's own group `run-<attempt>` under that cgroup and writes `1` to its
  `cgroup.freeze`. The kernel stops those processes with no signal; the ptyhost, people's shells
  and browsers, CI jobs and the sandbox holder keep running. A task counts as paused once its group
  reports `frozen 1`.
- The install is requested when every task is frozen or at a pause point on two consecutive
  passes (2 s apart); the server's graceful shutdown answers Yulius requests the agents had sent.
- Resuming releases and **thaws** the tasks one by one; a cancel thaws them at once; a server that
  starts with no update holding tasks thaws every run group left frozen under its unit.
- A run that cannot be frozen (no delegation, a run on the host, a write refused) keeps the turn
  boundary pause, its 30-minute bound and "Continue without waiting".

## Alternatives

- **`SIGSTOP`/`SIGCONT` through the ptyhost.** Works without delegation, but programs see it:
  `SIGCONT` handlers run, parents get `SIGCHLD`, and processes in their own session or process
  group (background shells) escape it.
- **Freezing the sandbox's whole `agents/` group.** Also freezes people's shells and browsers and
  the ptyhosts; a frozen ptyhost does not answer the restarted server's attach, which then ends the
  run as lost.
- **Putting each run in its own group when it starts.** Needs the ptyhost to move its child after
  entering the sandbox's user namespace, and leaves runs started before the change unfreezable;
  moving the tree at the freeze needs neither.
- **Keeping the turn boundary with a shorter bound.** Updates would still be cancelled by long
  turns, which is what the request is about.

## Consequences

- Updates no longer wait for turns; tasks pause for about the install and its health check.
- The provider stream of a frozen agent goes through the egress proxy in the server process, so a
  response streaming at the restart is cut; after the thaw Claude Code retries the request (the
  model sees nothing, the terminal may show a retry line). Keeping it open needs the proxy outside
  the server (follow-up #529).
- Wall-clock timers keep running while frozen: a tool command within about a minute of its timeout
  may time out after the thaw.
- Run groups stay after a thaw (their processes stay in them, under the same sandbox limits) and are
  reused by the next freeze; empty ones are removed by the next sweep.
- Without delegation nothing changes: tasks pause at their turn boundary as before.
