---
yulius-task: ecd4476a-f231-43c0-b9d6-de0cf18baa0f
status: in-progress
---

# SPEC-575 Merge-first staging: task-local testing and one staging deploy per task

## Goal

Make merge-first staging the Yulius default for projects that have a shared staging target:

- each task builds, deploys and tests a local deployment inside its own sandbox;
- the PR merges into the integration branch after independent review and the quick checks;
- the full gate runs once, on the exact merged revision;
- shared staging changes only to a revision whose full gate passed, once per completed task, in
  revision order;
- production is promoted only from a verified staging revision.

Projects without a shared target keep the full suite on the PR, and Yulius reports that they have
no target.

## Context

What exists today (map of 2026-11-20, `dev` at f80f42cad):

- **CD** (`crates/yulius-app/src/ci/cd.rs`, ADR-0109, ADR-0195):
  - CD always deploys the head of a protected branch as GitHub reports it (`cd_verified_head`).
  - One run per environment at a time; a second request is refused, not queued.
  - A run left by a restart is recorded as `error` and never retried (`reconcile_cd_runs`).
  - No run carries a task, revision or idempotency key. `cd_runs.task_id` is set only for the
    agent that asked.
  - A merge deploys nothing (`briefing.rs`: "Merges deploy nothing").
- **Production promotion** (`cd_prepare`, `cd_promote`) promotes the integration head at request
  time. Its only proof is the dev rollback drill at the same tree (ADR-0263, ADR-0282). Nothing
  ties it to a revision verified on staging.
- **CI** (`crates/yulius-app/src/ci/mod.rs`) tests PR heads, merge trains, playground tasks and
  the integration and production heads.
  - Every job of the gate runs on a PR head, except jobs skipped by `paths` (SPEC-493).
  - A branch commit with the tree of a passed run reuses that verdict. An up-to-date squash merge
    therefore never runs the full suite again.
  - The full suite always blocks the PR. There is no quick/full split.
- **Delivery** (`delivery.rs`, `agent_delivery.rs`): `task_merges` records each landing
  (task → merged SHA, migration 0077). The profile's `[[delivery.environments]]` with
  `delivery_deploy` is a separate, secret-less mechanism. This spec does not extend it.
- **Bootstrap**: `catalog/skills/yulius/infra-bootstrap/SKILL.md` step 3 and
  `catalog/workflows/infra-bootstrap.md` teach "a merge deploys nothing; deploy dev when asked".
  The onboarding prompt (`onboarding.rs` `CD_SETUP`) points there. Bundled skills reach every
  sandbox at the next server start (`skills/refresh.rs`). A project's `yulius.toml` changes only
  through a task.
- **Instances**: this task runs on the Quartio instance (`yulius.quartio.ai`, sandbox `yulius`).
  - Agents here cannot reach other sandboxes: the handoff switch is off and `sandbox_handoff` is
    absent from the tools.
  - Releases reach an instance only after a person promotes `dev` to `main` and a superadmin
    chooses Update now.

## Requirements

- **MFS-001 Merge-first environment.**
  - A dev-stage `[cd.<name>]` may set `merge_first = true`; at most one per project.
  - That table needs `deploy`, `verify` and `rollback`.
  - This environment is the project's shared staging target.
- **MFS-002 Quick checks on the PR.**
  - A `[[ci.jobs]]` entry with `after_merge = true` belongs to the full suite.
  - While the integration head defines a merge-first environment that deploys, those jobs are
    skipped on PR heads, merge trains and playground tasks, with a reason.
  - They run on integration-branch runs and candidate runs.
  - Without such an environment they run everywhere, so a project without a target keeps its
    full suite on the PR.
  - A branch run never reuses a tree whose run skipped a job that the branch run would run.
- **MFS-003 Candidate gate.**
  - A staging request names a revision: the task's newest landing on the integration branch.
  - Before staging can change, the full gate must pass on exactly that commit, in the sandbox's
    CI.
  - A run that already judged that commit counts. Otherwise Yulius queues one candidate run
    (`CiRef::Candidate`).
  - The candidate run is not superseded while its request is waiting.
- **MFS-004 Agent requests deployment through Yulius.**
  - A task agent's `cd_deploy` of the merge-first environment records a staging request; it
    never deploys the branch head directly.
  - It is refused while the task has an unfinished step or no landing.
  - A person's deploy of that environment records a request for the integration head.
- **MFS-005 One successful staging deployment per completed task.**
  - A task has at most one `verified` request per environment.
  - A repeated request for the same task and revision returns the existing request.
  - A newer revision of the same task supersedes its waiting request.
  - A request whose revision a passed deploy of that environment already contains is answered
    `included`, with that run, without deploying. This covers deploys before the rollout.
- **MFS-006 Revision order.**
  - Staging only moves forward along the integration branch's first-parent history.
  - Yulius deploys the newest ready request that descends from the last verified revision. It
    never passes over an older request that is still testing.
  - When the deploy verifies, the ready requests it contains are marked `included`, with its run
    id.
- **MFS-007 Failure stays with the task.**
  - A failed or errored candidate gate marks the request `gate_failed` with its run id and failed
    jobs. Yulius tells the owning task's session, and staging does not change.
  - A failed deploy or verify marks it `deploy_failed`. Staging is rolled back to its last
    verified release (when one exists) through the existing rollback, and the task is told.
  - The agent fixes forward, merges and requests again.
- **MFS-008 Restarts, retries and duplicates are idempotent.**
  - Requests are durable rows.
  - After a restart, a request whose deploy run was lost is retried, at most 3 attempts. A
    merge-first deploy script is idempotent by contract.
  - A request whose run passed before the crash is marked `verified` from the run.
  - A second request in the same tick creates no second row or run.
- **MFS-009 Production from verified staging.**
  - With a merge-first environment, `[cd.prod] promote` promotes that environment's last verified
    revision instead of the integration head.
  - The deploy is refused when there is none.
  - The authorization rules are unchanged: an administrator types the slug, and the rollback
    gate applies.
- **MFS-010 Task-local deployment.**
  - A `[local]` table (`deploy`, `verify`, `about`) names the reusable local deployment.
  - The briefing tells the agent to run it inside its own sandbox: its ports, its isolated test
    data, and only the dev service credentials agent sessions are allowed.
  - Its scripts refuse any target other than the sandbox. Staging SSH, release signing and
    production secrets stay in their CD scopes.
- **MFS-011 Bootstrap default and guidance.**
  - The built-in `infra-bootstrap` skill and workflow make merge-first the default when a shared
    target exists, and teach `[local]`.
  - The `delivery-pipeline` skill, the onboarding CD prompt and the briefing describe the flow:
    review → quick checks → merge → candidate gate → `cd_deploy` → verified staging.
  - Without a target, they keep the full suite and record "no shared target" in the table's
    `about`.
- **MFS-012 Upgrade path.**
  - Yulius creates one upgrade task in each ready sandbox whose recorded bootstrap revision is
    older than the release's.
  - The task uses the Infra bootstrap workflow and its normal delivery.
  - The upgrade task must keep the project's commands, targets and vault grants. It reconciles
    conflicting repository guidance and reports whether the project switched or has no target.
  - The task is created once per sandbox and revision, including across restarts.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] Parse `merge_first` on dev-stage CD tables and `after_merge` on CI jobs
- [ ] Skip after-merge jobs on PRs, trains and playground tasks while a merge-first environment deploys
- [ ] Branch runs never reuse a tree that skipped an after-merge job; ci_status says why a job was skipped
- [ ] Store staging requests (migration and store API)
- [ ] Pure staging state machine: request, verdict, ordered pick, deploy outcome, restart
- [ ] Full gate on the exact merged candidate as a candidate CI run
- [ ] CD deploys a pinned integration revision
- [ ] Agent and person cd_deploy of the merge-first environment queue an idempotent staging request
- [ ] Staging coordinator: test, deploy in revision order, include ancestors, retry lost runs
- [ ] Failures reach the owning task; a failed deploy rolls staging back to its last verified release
- [ ] cd_status and the CD page show the staging queue with run and revision ids
- [ ] Production promotion takes the last verified staging revision
- [ ] `[local]` task-local deployment in config and briefing
- [ ] Bootstrap skill, workflow, delivery-pipeline, onboarding prompt and briefing teach merge-first
- [ ] Upgrade task per onboarded sandbox for the new bootstrap revision
- [ ] Scenario test: fresh bootstrap, existing sandbox, failed candidate then fix, concurrent tasks with a retried deploy
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- Quick checks on the PR, full suite once on dev:
  - A project with `[cd.staging] merge_first = true` and `[[ci.jobs]] after_merge = true`: a PR
    head's run records the after-merge job as skipped with the merge-first reason.
  - The integration run at the squash commit runs it. No second full run of the PR head exists.
- A project without a merge-first environment runs every job on the PR head.
- Failed candidate, then a fix:
  - The candidate run of revision R1 fails. The request reads `gate_failed` with that run id,
    staging's current release is still the earlier release, and the task's session got the
    failed job.
  - After a fix lands at R2 and passes, staging serves R2: one `verified` request with the
    deploy's run id and revision R2.
- Concurrent tasks with a retried deploy:
  - Two tasks request revisions A < B. The deploy runs are in revision order.
  - A duplicate request returns the same request id. A deploy lost to a restart is retried once
    and ends `verified`.
  - Each task has exactly one successful staging deployment: `verified`, or `included` in a
    verified one.
- A production promotion with a merge-first environment pushes the last verified staging
  revision, not a newer integration head.
- Upgrade task: a ready sandbox at an older bootstrap revision gets one upgrade task. A restart
  creates no second one.

## Test plan

- **Domain unit tests:**
  - config parsing: `merge_first` only on dev stage, at most one, needs rollback; `after_merge`
    is a boolean;
  - the skip rule per `CiRef`;
  - the staging state machine: transitions, ordered pick, include, supersede, lost-run retry
    bound, already-deployed answer.
- **Store tests:** the migration, unique keys (task + revision; one verified per task and
  environment) and compare-and-set transitions.
- **Integration tests** (`crates/yulius/tests`, fake GitHub, fake process host):
  - CI skip and reuse;
  - candidate run;
  - pinned-revision deploy;
  - request through `cd_deploy` (agent and person);
  - coordinator scenarios from the acceptance criteria with run and revision ids;
  - restart reconcile;
  - production promotion pin;
  - upgrade task creation.
- **Real rollout:** Yulius's own repository has no shared target (`[cd.dev]` empty), so it keeps
  its full suite on PRs. Quartio-v2 and the PerfectData projects adopt through their upgrade
  tasks once the release is installed.

## Decisions

- Staging is a CD environment with a flag (`merge_first`), not a third stage. ADR-0195's two
  stages already carry the trust, and a staging machine is dev-stage.
- The full suite is marked per job (`after_merge`), not a second command. It reuses the
  `paths`-skip machinery (skipped with a reason, passes a run), so mandatory checks keep their
  definition.
- The agent requests deployment with the existing `cd_deploy`. No second deploy tool exists for
  one environment.
- Tasks that land close together share a deploy rather than redeploying older revisions. Their
  requests are included in the newer verified revision, which contains them.
- ADRs: recorded with the slices that make them (numbers from `record_number`).

## Evidence

Not yet.
