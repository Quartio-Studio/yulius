# 0410. The full suite runs on the merged revision beside a merge-first staging target

## Status

Accepted. Specified in [SPEC-575](../../.ai/specs/SPEC-575-make-task-local-testing-and-one-staging.md)
(MFS-001, MFS-002); task #561. Extends
[0195](0195-cd-environments-are-named-and-take-their-rules-from-a-stage.md) (named CD
environments) and [0343](0343-a-ci-job-names-the-files-it-tests-and-is-never-skipped-on-doubt.md)
(jobs skipped with a reason).

## Context

Every job of a project's gate blocked each pull request head. A project with a long suite
(browser end-to-end tests, a full integration run) paid for it on every head of every pull
request, then again on a merge candidate when the integration branch had moved.

Merge-first delivery reverses the order:

1. A pull request merges after the independent review and the quick checks.
2. The full suite runs once, on the exact merged revision.
3. Shared staging changes only after it passes.

That trade is only safe where a shared target exists to protect, and where a failed full run
stops before staging rather than reaching people. Yulius itself and other projects without a
shared target have nothing after the merge that would catch a failure before it spreads.

## Decision

- **The staging target is a CD environment with a flag.** A dev-stage `[cd.<name>]` may set
  `merge_first = true`; at most one per project.
  - It needs `deploy`, `verify` and `rollback`: staging changes only to a verified revision and
    goes back to its last verified release after a failed deploy.
  - No third stage: the dev stage already carries the trust a staging machine has.
- **The full suite is marked per job.** A `[[ci.jobs]]` entry with `after_merge = true` belongs
  to the full suite.
  - While the integration head defines a merge-first environment that deploys, such jobs are
    skipped on pull request heads, merge trains and playground tasks. They end skipped with the
    reason "part of the full suite: it runs on the merged revision (merge-first)".
  - A skipped job passes a run and blocks no merge, like a `paths` skip.
  - They run on the integration and production branches' runs.
  - At least one job must not set it: a pull request merges on its quick checks alone.
- **Without a target, nothing moves.** Where the integration head has no such environment,
  `after_merge` is ignored and every job runs on every head.
- **No reuse across the move.** The integration branch's run never reuses a same-tree run that
  skipped a job as part of the full suite, whatever the gate now says of that job. A squash
  merge of an up-to-date pull request therefore runs the full suite on the merged commit instead
  of copying the pull request's verdict. That holds also for a pull request that turns the move
  off. A copied verdict (a reused tree, a merge train's) keeps the full suite's reason, so it is
  never a source either.
- **A change's own jobs stay on its pull request.** A job a change adds or changes runs before
  it merges ([0360](0360-dev-s-gate-judges-every-change-and-a-bad-yulius-toml-never-turns-ci-off.md)),
  so its "(this change)" twin is never part of the full suite. A pull request that only sets or
  clears `after_merge` adds no twin: the job runs the same way either way.

## Alternatives

- **A second command (`[commands] full`).** Projects already split their gate into
  `[[ci.jobs]]`. A command beside them would duplicate the job definitions (secrets, budgets,
  time limits) and let the two drift.
- **Skip the full suite whenever the project asks, target or not.** That leaves a project with
  no shared staging with nothing between a broken merge and the next person who pulls it.
- **Run the full suite only on demand before a deploy.** That is the candidate gate's job
  (SPEC-575 MFS-003). The integration branch's own run still judges every merged state, as
  `paths` skips do today ([0343](0343-a-ci-job-names-the-files-it-tests-and-is-never-skipped-on-doubt.md)).

## Consequences

- Until the staging queue of SPEC-575 lands, nothing gates a deploy of the merge-first
  environment on the full suite: `cd_deploy` deploys the integration head as before. The
  integration branch's own run still runs the full suite on each head it tests, but an
  intermediate merged revision that the branch moved past gets no run of its own.

- Pull requests of merge-first projects wait only for the quick checks. The full suite runs once
  per merged revision, and duplicate pre-merge full runs (pull request head plus merge
  candidate) disappear for those jobs.
- A defect only the full suite catches lands on the integration branch. The candidate gate and
  the staging queue (later slices of SPEC-575) keep it away from shared staging and report it to
  the owning task.
- A pull request that adds `merge_first` and `after_merge` is still judged by the integration
  head's gate, so its own run keeps the full suite. The move takes effect from the next pull
  request.
