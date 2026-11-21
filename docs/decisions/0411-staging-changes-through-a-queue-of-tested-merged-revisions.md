# 0411. Staging changes through a queue of tested merged revisions

## Status

Accepted. Specified in [SPEC-575](../../.ai/specs/SPEC-575-make-task-local-testing-and-one-staging.md)
(MFS-003 to MFS-009); task #561. Builds on
[0410](0410-the-full-suite-runs-on-the-merged-revision-beside-a-merge-first-staging-target.md)
(the full suite on the merged revision). Keeps
[0109](0109-cd-runs-verified-commits-out-of-the-agents-reach.md) (CD runs verified commits) and
[0110](0110-only-a-person-deploys-production.md) (only a person deploys production).

## Context

With merge-first delivery a pull request lands on the quick checks, and the full suite runs on
the merged revision. A shared staging target must not move to a revision before that run passed.
CD as it was could not express that:

- it deploys whatever the branch head is when someone asks;
- it refuses a second request while a run is going;
- it records a run lost to a restart as an error and never runs it again;
- nothing ties a deploy to the task whose work it carries.

Several tasks landing close together, retries, a duplicate request and a server restart would
deploy older revisions after newer ones, deploy a task twice, or leave staging on a revision
that failed.

## Decision

- **A request, not a deploy.** A task's agent asks for the merge-first environment with
  `cd_deploy`, and Yulius records a staging request (table `staging_requests`, migration 0078)
  for the task's newest landing on the integration branch (`task_merges`).
  - It needs the task's owner to be active and to have write access, as every deploy for a task
    does; the access is checked again before its deploy starts.
  - It is refused while the task has merged nothing, has pending or blocked steps, or the
    project has no CI gate in force (or CI is off).
  - A plain deploy of that environment, a person's, an automation's or the head's, is refused in
    `cd_prepare`: staging changes only through the queue. Rollbacks stay possible (a person, or an
    agent's `cd_rollback`), and so does each deploy's rollback drill.
- **The full gate on exactly that commit.**
  - The integration branch's run of the revision, or a candidate run (`candidate/<sha>`,
    `CiRef::Candidate`), judges it, unless it skipped the full suite.
  - A candidate run counts as testing its commit, so the branch reaching that commit queues no
    second full run of it. The branch moving on never supersedes it.
  - A revision with no verdict and no active run gets one candidate run.
  - A cancelled candidate run is a person's decision to stop that gate: the request fails its
    gate.
- **Staging moves forward along first-parent history.**
  - Each revision's place is its first-parent position on the verified integration head
    (`Git::first_parent_position`, in the CD mirror); a Git failure ends the round, never reading
    as "off the history".
  - What staging serves is the current release in its kept history, so a rollback moves it
    back.
  - Yulius deploys the newest ready revision above what staging serves, and never deploys while
    an older request is still testing.
  - A request whose revision staging already serves (also one deployed before merge-first) is
    `included`, with that run's id, and deploys nothing.
  - A revision that left the branch's own history, or an environment that stopped being the
    target, supersedes its waiting requests.
- **One deploy, pinned.**
  - The queue starts a `CdRequest::DeployRevision` of exactly that commit (its own tables and
    scripts) as the task's owner, checked to be on the integration head's first-parent history
    in the mirror.
  - It waits, costing no attempt, while another run of the environment goes on (a rollback, its
    drill) or an update holds new deploys.
- **One success per task, and every move once.**
  - Unique indexes allow one live request per task and revision, one `verified` or `included`
    request per task and environment, and one `deploying` request per environment.
  - Every move is a compare-and-set from the state it was decided on, and the decisions are pure
    (`yulius_domain::staging`).
  - A repeated request answers with the recorded one, and a failed deploy of the same revision
    may be asked again (a retried deployment).
- **Failure.**
  - A failed deploy or verify fails its request and restores staging: each part's current kept
    release (its last verified one, since a failed deploy records none) is redeployed and
    verified once, as a rollback run.
  - A part for which the failed deploy announced a rollback barrier is not restored (RBK-022).
    Its request says why, and a person decides. The run's end keeps each announced barrier as a
    fixed last line of its stored output (`rollback::failed_barrier_line`), where the restore
    reads it.
  - A person's cancel of the deploy stands: the request ends `deploy_failed`, and nothing deploys
    it again by itself.
- **Restarts.** A deploying request whose run ended in error (a restart reconciles a running run
  to an error) or was never recorded is ready again, until 3 attempts ended. A merge-first
  target's deploy scripts are idempotent by contract. A run that passed before the crash
  verifies the request at the next round. Each new attempt forgets the previous run.
- **The task hears of it.**
  - Its outcome is typed into its session once, or resumes it when parked: verified, included,
    gate failed with the run, deploy failed with the run and what happened to staging, or
    superseded by Yulius.
  - Until it asked, `delivery_status` tells a task whose work merged that its request is due,
    and afterwards where its newest request stands (MFS-013). The receipt of a merge on
    green sends the agent there, so a task whose briefing predates the switch, such as the
    switch's own task, learns it too.
  - Ticks never overlap.
  - `cd_status` and the CD page list the queue with its revisions and run ids, and the revision
    staging serves.
- **Production from verified staging.** [ADR-0425](0425-pin-a-production-deploy-to-verified-dev.md) supersedes the head-equality rule for a person’s production CD deployment.
  - With a merge-first target at the verified integration head, every promotion is refused
    unless that head is the revision staging serves and a verified request deployed it: a
    person's production deploy (`cd_prepare`), the Delivery page's Promote and the profile's
    automatic promotion (`staging_promotion_head`, then `dlv_promote_exactly` of exactly that
    head).
  - The tables are read through the CD mirror. When they cannot be read, the promotion is
    refused whatever the project, so an agent breaking its clone can only cause a refusal. A host
    whose Git keeps no CD mirror runs no CD, so it has nothing to gate.
  - An automatic promotion that waits for staging is audited as `delivery.promote.waiting`, not
    as failed, and is run by the queue once staging verified a revision.
- **Deleted tasks.** A deleted task's requests stay, without the task, so what staging verified
  stays known.
- **Who moves it.** The background tick advances every queue with a request on its way (and for
  an hour after a failed deploy), each sandbox under one lock; a request kicks its queue at once.

## Alternatives

- **Deploy automatically after every merge.** That deploys revisions no task asked for, and a
  task with several pull requests would deploy once per pull request. The request comes from the
  task when its delivery is complete.
- **A queue of CD runs keyed by commit time.** Commit time is not revision order (a
  long-running pull request can land later with an older commit date). The first-parent position
  on the verified head is.
- **Redeploy older revisions after newer ones.** That would show each task "its" revision, but
  move staging backwards under concurrency. Inclusion records which verified deploy carries a
  task's work, with its run id.
- **Promote an older verified revision while the integration branch moved on.** Promotion
  merges the integration branch through a pull request. Promoting another commit would need a
  branch made for it. Waiting until staging verified the head keeps one promotion path.
- **Extend `[[delivery.environments]]` and `delivery_deploy`.** That mechanism has no secrets,
  releases, rollback or drill; CD has them all.

## Consequences

- A failing candidate leaves staging on its last verified release. The request reads
  `gate_failed` with the run id and the failed jobs; the agent fixes forward, merges and asks
  again.
- A `gate_failed` request stays the answer for its revision, even once a later verified deploy
  contains it: its task reaches its one successful deployment by landing its fix.
- A production deploy whose rollback must be proven on the merge-first environment cannot prove
  it by deploying that environment's head. It is proven by the queue's deploys and their drills.
- A person can no longer deploy the merge-first environment's head from the CD page.
