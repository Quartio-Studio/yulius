---
yulius-task: ecd4476a-f231-43c0-b9d6-de0cf18baa0f
status: done
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
  - At least one job is a quick check without `after_merge` (the file is refused otherwise).
  - A change's own new or changed jobs ("(this change)") never carry the flag: they run on its
    PR (ADR-0360).
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
  - It is refused while the task has a pending or blocked step, no landing, or the project no
    CI gate.
  - Any plain deploy of that environment (a person's on the CD page, an agent's or an
    automation's head deploy) is refused, naming the queue. Rollbacks stay a person's
    (ADR-0411).
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
- **MFS-009 Production from verified staging.** The production CD head-equality rule below is superseded by [SPEC-586](SPEC-586-we-should-be-able-to-deploy-on-prod-last.md).
  - With a merge-first environment, every promotion (a person's production deploy, the Delivery
    page's Promote, the profile's automatic one) is refused until the integration head is the
    revision that environment serves, verified, with the tables read through the CD mirror.
    Promotion merges the integration branch, so the promoted revision is exactly the verified one
    (ADR-0411).
  - Slice 2 gates the production deploy; slice 3 gates the other two, and the queue runs a
    waiting automatic promotion once staging verified the head.
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
- **MFS-013 A merged task hears that staging waits for it.**
  - Once a task's work merged into a project with a merge-first target, `delivery_status` tells
    its agent that its staging request is due: the merged revision and the `cd_deploy` call.
  - Once it asked, the same line says where its newest request stands, with its run ids and
    the next step. A failed or superseded request of an older revision leaves the newer landing
    due again.
  - The receipt of a merge on green sends the agent to `delivery_status`, so a task whose
    session began before its project switched learns it there. The rollout of 2026-11-21 showed the need:
    three upgrade tasks switched their project and ended without a staging request (their
    briefing still said merges deploy nothing), and only a task can ask.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Parse `merge_first` on dev-stage CD tables and `after_merge` on CI jobs
- [x] Skip after-merge jobs on PRs, trains and playground tasks while a merge-first environment deploys
- [x] Branch runs never reuse a tree that skipped an after-merge job; ci_status says why a job was skipped
- [x] Store staging requests (migration and store API)
- [x] Pure staging state machine: request, verdict, ordered pick, deploy outcome, restart
- [x] Full gate on the exact merged candidate as a candidate CI run
- [x] CD deploys a pinned integration revision
- [x] Agent cd_deploy of the merge-first environment queues an idempotent staging request; plain deploys are refused
- [x] Staging coordinator: test, deploy in revision order, include ancestors, retry lost runs
- [x] Failures reach the owning task; a failed deploy rolls staging back to its last verified release
- [x] cd_status and the CD page show the staging queue with run and revision ids
- [x] Production promotion takes the last verified staging revision
- [x] `[local]` task-local deployment in config and briefing
- [x] Bootstrap skill, workflow, delivery-pipeline, onboarding prompt and briefing teach merge-first
- [x] Upgrade task per onboarded sandbox for the new bootstrap revision
- [x] Every promotion path waits for the verified staging revision, read through the CD mirror
- [x] Scenario test: fresh bootstrap, existing sandbox, failed candidate then fix, concurrent tasks with a retried deploy
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated
- [x] delivery_status asks a merged task of a merge-first project for its staging request
- [x] Independent review of the staging request note, findings fixed test-first
- [x] Gate green, docs and the changes log updated for the staging request note

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
- A production promotion with a merge-first environment promotes only the revision staging
  verified: while the integration head is a newer revision, it is refused.
- Upgrade task: a ready sandbox at an older bootstrap revision gets one upgrade task. A restart
  creates no second one.
- A merged task's `delivery_status` names its due staging request; after a failed gate and a
  merged fix, the fix's revision is due; then it names the newer request's state and run ids.

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
  - upgrade task creation;
  - `delivery_status`'s staging line (MFS-013).
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

Delivered in three pull requests:
- #1530 (slice 1, MFS-001 and MFS-002), merged at 5c8d321d8;
- #1535 (slices 2 and 3, MFS-003 to MFS-009 for production deploys), merged at 81709b75a;
- this pull request (MFS-009 for every promotion, MFS-010 to MFS-012).

Every head went through Yulius CI and an independent review in a fresh session (Fable 5.2).

### Scenarios (integration tests)

All in `crates/yulius/tests/staging.rs` unless named otherwise. Each scenario reads run ids and
revision ids from the store and the fake origin.

- **Failed merged candidate, then a fix**
  (`a_failing_candidate_leaves_staging_on_its_release_and_the_fix_deploys`):
  - r1 is verified with CI run `ci-<r1>` and CD run X.
  - r2's full gate fails: the request is `gate_failed` and its note names run `ci-<r2>` and the
    failed job `full`.
  - The deploy script's mark still holds r1, and the only deployed revision is r1.
  - The fix r3 passes and is the one verified there. The deployed revisions are exactly
    `[r1, r3]`.
  - A duplicate request returns the same id.
  - The API and `cd_status` list the queue with the revisions and the CI and CD run ids.
- **Concurrent tasks, with a retried deploy**
  (`a_lost_deploy_is_retried_and_concurrent_tasks_deploy_in_revision_order`):
  - A deploy reconciled as lost after a restart is retried and verified (attempts 2).
  - Two tasks asking together deploy in revision order, never back to an older revision.
  - A delivery staging already serves is answered `included` with that run's id.
  - Each task has exactly one success.
- **More queue behaviour**, each with its own test:
  - a failed deploy restores the last verified release, once
    (`a_failed_deploy_puts_staging_back_on_its_last_verified_release`), but never past an
    announced rollback barrier;
  - a person's cancel stands;
  - waiting behind another run or a held deploy gate costs no attempt;
  - the owner's write access is required;
  - a candidate run survives the branch moving on and decides the request through the real
    executor (`ci_dispatch`);
  - refusals and supersedes: open steps, no CI gate, a newer landing, a revision force-pushed
    away, a target removed;
  - pinned deploys only of a commit on the branch's own first-parent history.
- **Fresh bootstrap:**
  - The onboarding prompt teaches the merge-first default (`onboarding::tests`).
  - The workflow, skill and template are shipped in the catalog (`workflows::catalog` tests).
  - The adopting pull request still runs its full suite
    (`local_ci::adopting_merge_first_runs_the_full_suite_on_its_own_pull_request`).
- **Upgrade restarts and eligibility** (`bootstrap_upgrade::`, review round 1 of slice 4 and a
  person's rollout finding):
  - a claim left without its task is finished at the next start;
  - a task created but not linked is adopted once;
  - a deleted upgrade task is not made again;
  - playgrounds, removed sandboxes and inactive creators get none;
  - a sandbox created after the release (the real import path,
    `sessions::a_sandbox_created_after_the_release_gets_no_upgrade_task`) or onboarded after it
    gets none.
- **The promotion gate fails closed** (`a_promotion_whose_verified_head_cannot_be_read_is_refused`):
  a merge-first project with no staging request yet and a broken clone is refused, with `main`
  unchanged. Automatic promotion never takes an unverified head
  (`automatic_promotion_never_takes_an_unverified_head`).
- **Existing sandbox:**
  - The upgrade task (`bootstrap_upgrade::an_onboarded_sandbox_gets_one_upgrade_task_per_revision`):
    one Infra bootstrap task per onboarded sandbox and revision, with an automation's origin;
    none for a sandbox still onboarding, even once it is onboarded; none on a second start. A
    creation refused for want of a Claude account is made at the next start.
  - Adoption (`a_project_that_adopts_merge_first_deploys_its_next_delivery_through_the_queue`):
    staging deployed by hand before the switch, then the earlier delivery is `included`, and the
    next one is verified through the queue.
- **Duplicate pre-merge full-suite runs are gone**
  (`local_ci::merge_first_runs_the_full_suite_once_on_the_merged_revision`):
  - The pull request head's run skips `full` with the merge-first reason.
  - The integration branch's run at the squash commit is the only run of it.
  - A train skips it too, and the merged head runs it
    (`merge_train::a_train_skips_the_full_suite_and_the_merged_head_runs_it`,
    `merge_train::a_reused_candidate_still_leaves_the_full_suite_to_the_merged_head`).
  - A candidate run counts as testing its commit
    (`ci::tests::a_candidate_run_is_kept_and_counts_as_testing_its_commit`).
- **Production from verified staging:**
  - The production deploy is gated (`production_is_promoted_only_from_the_verified_staging_revision`).
  - So are the Delivery page and the automatic promotion
    (`every_promotion_waits_for_the_revision_staging_verified`): refused with the clone's
    object rewritten, refused naming why when the CD mirror cannot be read, and promoted once
    verified.
  - Automatic promotion follows the verified deploy
    (`automatic_promotion_follows_the_verified_staging_deploy`).

### Mutations checked by hand

Each of these was reverted, its test failed, and the code was restored:
- the tree-reuse copy keeping the full suite's reason;
- the restore call;
- the barrier skip;
- the cancel mapping;
- the owner's write-access check;
- the deploy-gate half of `staging_must_wait`;
- the promotion gate failing closed when the verified head cannot be read;
- the automatic promotion's gate;
- the adoption of a task created but not linked.

### Commands

- `cargo nextest run` over the app, domain, store, git and contracts packages, plus the
  `local_ci`, `merge_train`, `cd`, `cd_rollback`, `staging`, `delivery`, `production_branch`,
  `parking`, `self_update`, `ui_contract`, `playground`, `bootstrap_upgrade` and onboarding
  suites: 2081 passed.
- `npx e2e run tests/cd.e2e.ts`: 28 passed, including the staging queue on the CD page.
- `cargo clippy --workspace --all-targets -D warnings` and `scripts/dev quick`: clean.
- The security scan is turned off for this project in Yulius: `yulius security-scan` prints
  "Security scan disabled", and CI's `security` job ends skipped. Disabled, not passed.

### MFS-013 (PR #1579 and its review round)

- `delivery_status_asks_a_merged_task_for_its_staging_request`: no line before a landing, the
  due line, then the verified request with both run ids; no line without a merge-first target.
- `delivery_status_after_a_failed_gate_names_the_fix`: a `gate_failed` request with its next
  step, then the fix's revision due, then the newer request verified.
- Review round 1 (Fable 5.2) approved 404f27586 with five minors; the second PR fixes them:
  - the stale failure line;
  - a fetch only when the verified head is missing;
  - no read on evidence pages;
  - a logged read failure;
  - the two-request test.

### Rollout (2026-11-21)

- Promoted to `main` at 3fc0e5976 (#1567, main run 14a26ad8); Quartio and PerfectData run it.
- **Quartio-v2 #338, the first real project: switched.**
  - PR #1498 set `[cd.dev] merge_first`, `tests` `after_merge`, and `[local]`, merging at e8bdfcf6d.
  - PR run 97701a18 and merge-train run 86a41cbc passed. The full gate ran once on the merge,
    as `dev` run 5d943395.
  - Staging request 4d4c40d6 is verified: CD run 469c3db1 deployed e8bdfcf6d, and the rollback
    drill aeb9857e followed. Production was untouched.
- **Switched:**
  - autoresearch #24, staging verified with run bd590594;
  - VistulaValley #11, jobs after the merge (run 12da928d);
  - Branding #6;
  - N3.1-yulius-owned #67;
  - FW-N2-Device #25, with its build moved off pull requests.
  VistulaValley, Branding and N3.1 ended without a staging request, which is what MFS-013 fixes.
- **No shared target:**
  - yulius #563 (PR #1568);
  - product #81;
  - studio #3;
  - OS-CMS #5, blocked on a person creating its dev environment.
- centrum (PerfectData) got no task: an admin skipped its onboarding (ADR-0415).

### Not tested here

- The update gate's real flow (an install closing the gate) is covered through a test seam
  (`hold_deploys_for_tests`), not the self-update fixture.
- Yulius's own repository has no shared staging target (`[cd.dev]` is empty), so it keeps its
  full suite on pull requests. Its own upgrade task will report "no shared target".
