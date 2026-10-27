---
yulius-task: c3f3ebdf-5ce6-4e7e-9db7-b34e4a893503
status: in-progress
---

# SPEC-342: A deploy is never cut by an update of Yulius

Status: in-progress
Date: 2026-10-27
Owner: Yulius task #326
Scope authorization: task #326 (`/goal`): "make sure deployment doesnt break while yulius itself is
upgraded to the newer version", with a screenshot of a CD dev deploy of a project running for
10 minutes. Amends the update flow of [SPEC-160](SPEC-160-tasks-should-be-paused-imidietly-not-wai.md)
(OPS-010..OPS-012) and SPEC-001 OPS-008; CD runs are [SPEC-134](SPEC-134-implement-cd-it-should-write-cd-bash-scr.md),
environment runs [SPEC-219](SPEC-219-fix-656-delivery-environment-runs-must-r.md). Decision:
[ADR-0232](../../docs/decisions/0232-an-update-waits-for-running-deploys-and-holds-new-ones.md).

## Goal

An update of Yulius never stops a deploy halfway: a CD run or a delivery environment run that is
running when the new version is built finishes (passed, failed or timed out as it would have)
before the server restarts, and no deploy starts between then and the restart.

## Context

- **Symptom.** A CD run deploying a project (the screenshot: `scripts/cd/deploy.sh dev`, 10 minutes
  in) is running when a superadmin updates Yulius. The update builds, freezes the tasks and
  restarts the service. The deploy's process host outlives the restart (`KillMode=process`), but
  the server that starts has no executor for it: `reconcile_cd_runs` (`crates/yulius-app/src/ci/cd.rs`)
  terminates its process host and records it as `error` ("Yulius restarted while it ran: it was
  stopped, its outcome is unknown, and it is not run again by itself"); `reconcile_environment_runs`
  (`delivery.rs`) does the same to environment runs. The target is left half-deployed, an agent
  state the run held (SPEC-278) is not saved back, and nobody restarts the deploy.
- **Why.** The update's drain (`updates.rs`, `drive_update`) waits only for tasks (freeze or turn
  boundary, ADR-0131). CD runs are not tasks: the drain neither sees nor waits for them, and nothing
  stops a new deploy from starting while the update pauses tasks or installs.
- **Why not re-adopt the run after the restart.** The process host does outlive the server, but
  what the run needs after its command ends lives in the old server's memory: the attach token, the
  agent-state hold and the restored state to save back (ADR-0207), the containment lease, the
  output stages and the disk bound. Persisting all of that is a larger change (ADR-0232,
  Alternatives); it would also cover crashes, which this task does not ask for.
- **Existing pieces.** `App::ci.executing` maps a running run to its cancel channel;
  `store.running_cd_runs()` and `store.running_environment_runs()` list runs recorded as running
  (all sandboxes); a run gets the CI job timeout (`config.ci.job_timeout`, default 30 minutes,
  `YULIUS_CI_JOB_TIMEOUT_MIN` up to 360) and is killed past it, so a running deploy always ends.
  `cd_start` is the only start of a CD run (people, agents' `cd_deploy`); `dlv_start_environment_run`
  the only start of an environment command run.

## Requirements

| ID | Requirement |
| --- | --- |
| DPU-001 | **Deploy gate.** From the moment an update's build is done until its install ended (the run moved to `resuming` or ended), a new CD run or environment command run is refused before anything ran (no promotion, no audit entry of a deploy, no run record) with: "nothing ran: Yulius is being updated; deploy again once the update finished (Settings → Instance shows it)". People and agents get the same refusal; an agent's `cd_deploy` returns it as its error. A manual environment confirmation (no command) is not a deploy and is not refused. |
| DPU-002 | **No race.** A deploy that passed the gate before it closed is counted as starting until it is recorded as running or refused; the update treats a starting deploy as running. A deploy is therefore either refused or seen by the update, never both missed. |
| DPU-003 | **Wait after the build.** When the helper reports the build done and a CD run or environment run is running or starting, the update stays in `building` (tasks keep working, nothing is frozen or held, starts do not queue) and moves to `draining` on the first pass that finds none. "Continue without waiting" is not offered while it waits for deploys (it exists only while pausing tasks). |
| DPU-004 | **Bounded.** The wait for deploys is bounded by the longest a deploy can last plus 5 minutes: a CD run may wait the CI job timeout for its agent states and then deploy for the job timeout; an environment run may prepare for 35 minutes and then run for the job timeout; so the bound is max(2 × job timeout, job timeout + 35 min) + 5 min (70 minutes by default). It is measured per update run from when this server first saw its build done with a deploy running (a wait recorded for another run never counts); past it the update is cancelled with "N deploy(s) were still running T minutes after the build; nothing was installed." (T the bound in minutes) and the gate opens. The 3-hour build bound applies to the build only, not to this wait. A person can cancel the update while it waits, as while it builds. |
| DPU-005 | **Defensive drain.** While `draining`, a running or starting deploy (one the gate could not have let through, e.g. after a server restart mid-drain) keeps the install from being requested, even after "Continue without waiting". |
| DPU-006 | **Shown.** While the update waits for deploys, `GET /api/update`'s run has `deploys`: each running deploy with its project slug, environment, kind (`cd` or `environment`) and run id (at most 20), and its message reads "Built. Waiting for N deploy(s) to finish before pausing tasks; new deploys are refused until the update installed." (a store error: "Built. The running deploys could not be read; waiting before pausing tasks."). While `draining` with a deploy (DPU-005) the message is "Waiting for N deploy(s) to finish before restarting." and "Continue without waiting" is not offered. The instance page lists them under the current step as "<project> · <environment> (CD)" linking to the project's CI/CD page, or "(environment run)" linking to its Git page, where environment runs show. Without such deploys `deploys` is empty and the run view is as before. |
| DPU-007 | **Gate opens.** The gate opens when the run is cancelled (by a person, a bound or a missing helper), fails, resumes, or no update is open; a server that starts has it open, and closes it again on its first pass of a run that is `draining` or `installing`. |
| DPU-008 | The architecture document, the product summary, the operations guide (§8), ADR-0232, SPEC-160's back-link and `docs/changes.md` describe it, including what it does not cover (a crash or a manual restart still ends a running deploy as `error`). |

## Plan

- [ ] Read the product context and related work, then write and validate the spec
- [ ] New deploys are refused while an update pauses tasks or installs
- [ ] The update waits after its build for running deploys before pausing tasks
- [ ] The instance page names the deploys the update waits for
- [ ] ADR-0232, architecture, product and operations docs say deploys survive updates
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- With a CD run recorded as running when the build is reported done, update passes keep the run in
  `building` with the deploy listed and the message above; no task is frozen and no install request
  is written. Once the run ends, the next pass moves to `draining` and the install follows as before.
- While the update waits, pauses tasks or installs, `POST /api/sandboxes/{sid}/cd/deploy` (person)
  and the agent's `cd_deploy` and `POST …/delivery/environments/{env}/run` are refused with the
  DPU-001 text and create no run; after the update is cancelled the gate is open again.
- A deploy still running past the bound cancels the update with the DPU-004 message.
- A starting deploy (past the gate, not yet recorded) keeps the update waiting.

## Test plan

- Integration (`crates/yulius/tests/self_update.rs`, fake helper, fake clock, seeded runs):
  `a_running_deploy_is_waited_for_before_tasks_pause` (DPU-003, DPU-006),
  `new_deploys_are_refused_while_the_update_installs` (DPU-001, DPU-007; CD by a person and the
  environment run), `a_deploy_past_the_bound_cancels_the_update` (DPU-004),
  `a_deploy_running_while_draining_holds_the_install` (DPU-005). Each fails without the change
  (the update moves to `draining`/installs, the deploy starts).
- Unit (`deploy_gate.rs`): the gate counts a starting permit until dropped and refuses once
  closed (DPU-002); a wait counts from its own run's first pass, never another run's (DPU-004).
- Integration: `a_starting_deploy_is_waited_for` holds a real permit across the build's end
  (DPU-002, the update side).
- e2e (`e2e/tests/update.e2e.ts`, mock server): the waiting deploys are listed under the build step
  (DPU-006).
- Gates: `scripts/dev ci`, `scripts/dev security`, the e2e suite.

## Decisions

- **Wait for running deploys after the build, before pausing tasks** (ADR-0232), not during the
  pause: tasks keep working while a deploy finishes, so the freeze stays as short as ADR-0131 made
  it. Freezing first and then waiting would hold every task frozen for up to the job timeout.
- **Refuse new deploys rather than queue them**: a queued deploy would run with secrets and
  agent-states chosen before an update the person did not see; a refusal says what to do and
  nothing has run. Agents retry after `yulius wait` or later.
- **An in-memory gate with a starting count** under one mutex: closing it and reading the store
  after the close sees every deploy that passed the gate (it holds its permit until its run is
  recorded). A restart reopens it; a run that is draining or installing closes it on the first pass.
- **Bound = the longest legitimate deploy + 5 minutes** (review round 1, M1): a CD run's agent-state
  wait and its deploy each get the job timeout, an environment run's preparation 35 minutes; only
  something broken reaches it. A fixed 30 minutes, or the job timeout alone, would cancel updates
  behind legitimate long deploys.
- **The wait is keyed to its update run** (review round 1, H1): a pass a cancel overtook could
  record a start for a cancelled run; keyed, it never counts against a later run, and the gate
  opens whenever no update is open or the run is still building.

## Evidence

(filled at the end)
