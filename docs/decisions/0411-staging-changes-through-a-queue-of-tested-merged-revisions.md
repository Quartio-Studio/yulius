# 0411. Staging changes through a queue of tested merged revisions

## Status

Accepted. Specified in [SPEC-575](../../.ai/specs/SPEC-575-make-task-local-testing-and-one-staging.md)
(MFS-003 to MFS-008); task #561. Builds on
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
  - It is refused while the task has merged nothing, still has pending or blocked steps, or the
    project has no CI gate.
  - A plain deploy of that environment, a person's, an automation's or the head's, is refused:
    staging changes only through the queue.
- **The full gate on exactly that commit.**
  - A run of the revision on the integration branch (`branch/<integration>`) or a candidate run
    (`candidate/<sha>`, `CiRef::Candidate`) judges it, unless it skipped the full suite.
  - The integration branch's own run of its head is usually that verdict, so the full suite is
    not run twice.
  - A revision with no verdict and no active run gets one candidate run, which the branch moving
    on never supersedes.
- **Staging moves forward along first-parent history.**
  - Each revision's place is its first-parent position on the verified integration head
    (`Git::first_parent_position`, in the CD mirror).
  - Yulius deploys the newest ready revision above the last verified one, and never deploys while
    an older request is still testing.
  - A request whose revision staging already serves (also from a verified deploy before
    merge-first) is `included`, with that run's id, and deploys nothing.
  - A revision that left the branch's own history is superseded.
- **One deploy, pinned.**
  - The queue starts a `CdRequest::DeployRevision` of exactly that commit (its own tables and
    scripts), as the person behind the request: the task's owner.
  - The run is checked to be on the integration head's first-parent history in the mirror.
  - It waits while another run of the environment (a rollback, its drill) goes on.
- **One success per task, and every move once.**
  - Unique indexes allow one live request per requester and revision, one `verified` or
    `included` request per task and environment, and one `deploying` request per environment.
  - Every move is a compare-and-set from the state it was decided on.
  - The decisions are pure (`yulius_domain::staging`: `admit`, `after_verdict`, `pick`,
    `after_deploy`).
  - A repeated request answers with the recorded one.
  - A failed deploy of the same revision may be asked again (a retried deployment).
- **Restarts.** A deploying request whose run ended in error (a restart reconciles a running run
  to an error) or was never recorded is ready again, until 3 attempts ended (deploy scripts of a
  merge-first target are idempotent by contract). A run that passed before the crash verifies
  the request at the next round.
- **Who moves it.** The background tick advances every queue with a request on its way, each
  sandbox under one lock, and a request kicks its queue at once.

## Alternatives

- **Deploy automatically after every merge.** That deploys revisions no task asked for, and a
  task with several pull requests would deploy once per pull request. The request comes from the
  task when its delivery is complete.
- **A queue of CD runs keyed by commit.** Order by commit time is not revision order (a
  long-running pull request can land later with an older commit date). The first-parent position
  on the verified head is.
- **Redeploy older revisions after newer ones.** That would show each task "its" revision, but
  move staging backwards under concurrency. Inclusion records which verified deploy carries a
  task's work, with its run id.
- **Extend `[[delivery.environments]]` and `delivery_deploy`.** That mechanism has no secrets,
  releases, rollback or drill; CD has them all.

## Consequences

- A failing candidate leaves staging on its last verified release. The request reads
  `gate_failed` with the run id and the failed jobs; the agent fixes forward, merges and asks
  again.
- Telling the owning task, rolling staging back after a failed deploy, showing the queue in
  `cd_status` and on the CD page, and promoting production from the verified staging revision
  are later slices of SPEC-575.
- A person can no longer deploy the merge-first environment's head from the CD page; rollbacks
  stay theirs.
