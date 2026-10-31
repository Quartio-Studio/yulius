# 0247. A sandbox running CI jobs weighs more on the host

## Status

Accepted. Specified in [SPEC-362](../../.ai/specs/SPEC-362-ci-slots-per-job-configurable-per-projec.md);
task #345, follow-up 4 of #342. Amends [ADR-0039](0039-ci-jobs-get-the-cpu-first.md) (its open
consequence: one sandbox's agents could still slow another sandbox's CI). Keeps
[ADR-0065](0065-processes-plan-for-a-cpu-and-memory-budget.md) (CI jobs' budgets, `cpus` per job)
and the cgroup layout of [ADR-0201](0201-all-sandboxes-share-a-task-limit-below-the-services.md).

## Context

On 2026-10-31 the 12-CPU host ran at load 30–52 for hours. Each CI job planned for 2 CPUs, the
equal share of its run, and every task's local cargo builds ran on top. A job's CPUs ("slots")
are already set per job with `cpus` in `[[ci.jobs]]`, and quartio-v2 is cutting its jobs to 1.
That lowers what CI asks for, but not who wins when the host is full.

ADR-0039 gives each sandbox's `ci/` leaf four times the weight of its `agents/` leaf. Weights
only compare siblings, though. Every `sandboxes/<sandbox>` group has the default weight 100, so
a sandbox whose agents build gets as much of the host as a sandbox running CI. Take one sandbox
running CI while its own agents build, and the agents of two other sandboxes build too. CI then
got about 27 % of what the sandboxes get (1/3 × 4/5) and the local builds about 73 %.

## Decision

- **A sandbox weighs more while its CI runs.** With the `cpu` controller delegated, Yulius
  writes `<unit>/sandboxes/<sandbox>/cpu.weight` = 500 while at least one of the sandbox's CI
  jobs runs, and 100 when none does. That is 100 plus what the `ci/` leaf weighs inside the
  sandbox, so the sandbox's CI weighs on the host what four sandboxes that only run agents weigh.
  The weight does not grow with the number of jobs: the jobs share the `ci/` leaf, and their
  budgets already bound what they ask for.
- **Counted by the app, written by the adapter.** A CI job counts from when it holds its slot and
  budget until its process is reaped, on every end (a guard). The isolation adapter keeps the
  counts and writes the weight. Preparing a sandbox's groups writes the weight for the jobs
  running now, so a run starting in a sandbox never resets it. A starting server writes 100 to
  every sandbox, since it knows of no job, and so does a stopping one.
- **No new knob for slots.** `cpus` in `[[ci.jobs]]` stays the per-project setting of a job's CPUs.
  Without it, the equal share stays the default.

## Alternatives

- **A lower `cpu.weight` for the agents' leaf while CI runs** (the task's first suggestion). It
  only compares the agents with their own sandbox's CI, where `ci/` already gets four fifths.
  Another sandbox's agents compete one level up and keep their full share.
- **A `cpu.max` cap on every sandbox's `agents/` leaf while any CI job runs.** It reaches across
  sandboxes, but it is a hard cap. Agents would be throttled while CI waits on the network or the
  disk, and a capped leaf stalls in periods (ADR-0065 rejected quotas for the same reasons).
- **A host-wide CI group beside `sandboxes/`.** It would rank CI above all agents at once. But a
  sandbox's memory and task limits sit on its own group, which must hold its CI jobs too, so CI
  cannot leave it.
- **A fixed higher weight for every sandbox that has CI configured.** It favours that sandbox's
  agents while no job runs, which is most of the time.
- **400 more per running job** (the first version, review round 1). With `YULIUS_CI_MAX_JOBS` at
  its default of 4, a sandbox running 4 jobs would weigh 1 700, and two sandboxes that only build
  would get about 5 % of the host each (about 4 % with 6 jobs, as on 2026-10-31). Their sessions
  would slow down, not just their builds.
- **A larger step (1 000 or more).** Same outcome: CI would win almost completely, and a sandbox
  with only agents would get a few percent of the host while two others run CI.
- **A project-wide `[ci] cpus` default.** It would repeat what `cpus` per job already says. A
  project with many jobs sets it on each, and the jobs differ anyway (an e2e job and a lint job
  need different budgets).

## Consequences

- Take one sandbox running CI while its own agents build, and the agents of two other sandboxes
  build too. CI gets 4/7 (57 %) of what the sandboxes get, and all agents together 3/7. Each other
  sandbox keeps at least 1/7 however many jobs run. Against three other sandboxes whose agents
  all build, CI gets exactly half (500/800 × 4/5). Against four or more it gets less than half,
  still about twice what it got before.
- Shares are of what `<unit>/sandboxes` gets. The server's `server/` group and the host's other
  slices compete above it, as before.
- Idle CPU still goes to whoever wants it: weights act only under contention, so the agents slow
  down only while the host is full and CI runs.
- The sandbox's own agents gain too while its CI runs: they share its raised weight at their 1:4
  ratio to CI. While its jobs wait on the network or the disk, they get the whole raised share.
- Not measured on a live kernel here: the task's sandbox has no cgroup mount. A test applies
  cgroup v2's weight sharing to the files Yulius writes. The VM measurement follows the deployment,
  as with ADR-0039.
- An older binary (a rollback) never writes the sandbox weight. Every start and every graceful
  stop of this one leaves 100 everywhere.
