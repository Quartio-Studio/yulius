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
a sandbox whose agents build gets as much of the host as a sandbox running CI. With CI in one
sandbox and agents building in two others, CI got about 27 % of the host and the local builds
about 73 %.

## Decision

- **A sandbox's weight follows its running CI jobs.** With the `cpu` controller delegated, Yulius
  writes `<unit>/sandboxes/<sandbox>/cpu.weight` = `100 + 400 × n` while `n` of the sandbox's CI
  jobs run (at most 10 000), and 100 when none does. Each running job weighs on the host what the
  `ci/` leaf weighs in its sandbox: four times a sandbox that only runs agents.
- **Counted by the app, written by the adapter.** A CI job counts from when it holds its slot and
  budget until its process is reaped, on every end (a guard). The isolation adapter keeps the
  counts and writes the weight. Preparing a sandbox's groups writes the weight for the jobs
  running now, so a run starting in a sandbox never resets it, and the first preparation after a
  restart writes 100. A stopping server writes 100 with its split undo.
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
- **A larger step per job (1 000 or more).** CI would win almost completely. A sandbox with only
  agents would get a few percent of the host while two others run CI, and its sessions themselves
  (in `agents/`) would slow down, not just its builds.
- **A project-wide `[ci] cpus` default.** It would repeat what `cpus` per job already says. A
  project with many jobs sets it on each, and the jobs differ anyway (an e2e job and a lint job
  need different budgets).

## Consequences

- With one CI job in one sandbox and agents building there and in two other sandboxes, CI gets
  4/7 (57 %) of the host and all agents together 3/7. Two jobs give 9/11 × 4/5 (65 %). Against
  four or more sandboxes whose agents all build at once, one job gets less than half again. That
  is still twice what it got before.
- Idle CPU still goes to whoever wants it: weights act only under contention, so the agents slow
  down only while the host is full and CI runs.
- The sandbox's own agents gain too while its CI runs: they share its raised weight at their 1:4
  ratio to CI. While its jobs wait on the network or the disk, they get the whole raised share.
- Not measured on a live kernel here: the task's sandbox has no cgroup mount. A test applies
  cgroup v2's weight sharing to the files Yulius writes. The VM measurement follows the deployment,
  as with ADR-0039.
- An older binary (a rollback) never writes the sandbox weight. A graceful stop leaves 100. After a
  crash, a raised weight stays until the new server's first preparation of that sandbox, or until
  an operator writes 100 (docs/operations.md).
