# 0232. An update waits for running deploys and refuses new ones until it installed

## Status

Accepted. Specified in [SPEC-342](../../.ai/specs/SPEC-342-make-sure-deployment-doesnt-break-while.md);
task #326. Extends [0131](0131-an-update-freezes-every-task-at-once.md) (the drain) and
[0013](0013-updates-from-main.md) ("updates must not lose running work") to CD runs
([0109](0109-cd-runs-verified-commits-out-of-the-agents-reach.md)) and delivery environment runs
([0175](0175-environment-runs-run-contained-from-a-mirror-of-origin.md)).

## Context

An update froze the tasks and restarted the service, but did not know about deploys. A CD run or
an environment run running at the restart kept its process host (`KillMode=process`), yet the
server that started had no executor for it: start-up recovery terminated its process host and
recorded it as `error` ("its outcome is unknown"). The target was left half-deployed, an agent
state the run held was not saved back, and nothing ran the deploy again. Nothing stopped a new
deploy from starting while the update paused tasks or installed, either.

## Decision

- **Wait after the build, before pausing.** When the helper reports the build done, the update
  closes a deploy gate and reads the CD and environment runs recorded as running. While any runs
  (or is starting), the run stays `building`: tasks keep working, nothing is frozen or held, and
  the instance page lists the deploys ("Built. Waiting for N deploy(s) to finish before pausing
  tasks; new deploys are refused until the update installed."; a CD run links to its project's
  CI/CD page, an environment run to its Git page). The first pass that finds none moves to
  `draining`.
- **Refuse new deploys until the install ended.** `cd_start` and `dlv_start_environment_run` take a
  permit from the gate before they do anything (no promotion, no audit entry, no run record) and
  hold it until their run is recorded as running. Closed, the gate refuses with "nothing ran: Yulius
  is being updated; deploy again once the update finished (Settings → Instance shows it)". It
  stays closed through `draining` and `installing` and opens when the run resumes, is cancelled or
  fails, or no update is open.
- **No race.** The gate and its count of starting deploys are one mutex: the update closes it and
  then reads the store, so a deploy that passed the gate before the close is either counted as
  starting or already recorded.
- **Bounded by the longest deploy.** A CD run waits up to the CI job timeout for its agent states
  and deploys for up to the job timeout; an environment run prepares for up to 35 minutes and runs
  for up to the job timeout. Past max(2 × job timeout, job timeout + 35 min) + 5 min (70 minutes
  by default), counted per update run from when this server first saw its build done with a
  deploy running, the update is cancelled ("N deploy(s) were still running T minutes
  after the build; nothing was installed."). A person can cancel the update while it waits.
- **Defensive drain.** While `draining`, a running or starting deploy keeps the install from being
  requested, also after "Continue without waiting" (which skips waiting for tasks, never deploys);
  the page then says "Waiting for N deploy(s) to finish before restarting." and offers no Continue.

## Alternatives

- **Re-adopt the deploy after the restart.** Its process host survives, but the attach token, the
  agent-state hold and restored state to save back (ADR-0207), the containment lease, the output
  stages and the disk bound live in the old server's memory. Persisting and re-acquiring all of
  them is a larger change; it would also cover a crash or a manual restart, which this does not.
- **Wait for deploys while tasks are frozen** (in `draining`): every task would sit frozen for up to
  the job timeout, undoing ADR-0131's short pause.
- **Queue new deploys and run them after the update.** The deploy would run with what was chosen
  before an update its requester did not see; a refusal runs nothing and says when to retry.
- **A fixed 30-minute bound** like the task drain, or the job timeout alone: a legitimate deploy
  (one that first waited for an agent state, or an environment run's preparation) would cancel
  the update.

## Consequences

- A deploy running at an update finishes as it would have (passed, failed, timed out) and saves its
  agent states; the update installs after it. The update can take up to the bound longer.
- A CD run or environment run recorded as running with no executor (a request dropped between its
  record and its start) is waited for until the bound, then the update is cancelled; a person ends
  a CD run with Cancel on the CI/CD page, an environment run only expires once its lost-run limit
  passed and it is read (pre-existing, #948 and #952; closed by
  [0233](0233-a-cd-run-starts-detached-from-its-request.md) and
  [0235](0235-an-environment-run-starts-detached-and-a-lost-one-is-not-waited-for.md)).
- What a deploy does outside the job timeout (a production promotion and the checks before its
  run is recorded, writing its files, restoring and saving agent states, joining its
  containment) is covered only by the 5 minutes of slack: with a long job timeout and a slow
  promotion the update can be cancelled behind a legitimate deploy. That is safe (nothing is
  cut, the update is started again); the bound is a backstop, not a promise.
- Deploys asked for from the build's end until the install ended are refused, by people and by
  agents (`cd_deploy` returns the refusal); agents retry later.
- The gate is in memory: a server that restarts mid-update reopens it and closes it again on its
  first pass of a `draining` or `installing` run.
- A crash, a manual `systemctl restart` or an instance export still end a running deploy as
  `error`, as before (start-up recovery is unchanged).
