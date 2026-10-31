---
yulius-task: 5d96b389-585c-42e7-b512-293f22c9f9a8
status: in-progress
---

# SPEC-362: A sandbox running CI jobs weighs more on the host than one that only builds locally

Status: in progress
Date: 2026-10-31
Owner: Yulius task #345 (follow-up 4 of #342)
Scope authorization: task #345. Builds on SPEC-053 / ADR-0039 (the per-sandbox `ci/` leaf) and
SPEC-082 / ADR-0065 (CI jobs' CPU budgets). The cgroup layout follows ADR-0201 and
`docs/architecture.md`, section CPU.

## Goal

1. A project sets how many CPUs ("slots") each of its CI jobs plans for and reserves, with
   today's value as the default.
2. While a sandbox's CI jobs run on a loaded host, they get more CPU than the agents' local builds
   in every sandbox. That includes other sandboxes, not only the agents of their own.

## Context

- **Symptom (2026-10-31).** The 12-CPU CI host ran at load 30–52 for hours. Each CI job of
  quartio-v2 planned for 2 CPUs, and every task's local cargo builds ran on top. quartio-v2 is
  cutting its jobs to 1 CPU on its side (its task #58).
- **"Slots" are the job's CPU budget.** A CI job waits for a budget out of the CI pools (SPEC-082
  RES-003). Its "job slot" wait records that budget (`cpus`) in the Pipeline tab, and the job is
  told it as `YULIUS_CPUS`. By default each job gets an equal share of the sandbox's capacity
  among its run's jobs, so a run of 6 jobs on 12 CPUs gets 2 each. The project already overrides
  this per job: `cpus` (1–1024) and `memory_gb` in `[[ci.jobs]]` (ADR-0065). It is parsed in
  `spec_format/config.rs`, applied in `Capacity::ci_job` and proved end to end by
  `local_ci::jobs_wait_for_their_cpu_budget_and_are_told_it`: a job declared `cpus = 3` runs
  with `YULIUS_CPUS=3`, its `[resources.env]` mapping `-j3`, and waits while the pool lacks 3
  CPUs. The instance-wide `YULIUS_CI_MAX_JOBS` slots (one per job) bound how many jobs run at
  once. They are not a per-job quantity.
- **CI only wins inside its own sandbox.** ADR-0039 splits each sandbox group into `system/`,
  `agents/` (100), `ci/` (400) and `engine/`. Every `sandboxes/<sandbox>` group keeps the default
  `cpu.weight` 100 under `<unit>/sandboxes`. cgroup v2 shares CPU by weight among the busy
  children of each group, so a sandbox whose agents build gets as much of the host as a sandbox
  running CI. ADR-0039 named this consequence: "one sandbox's agents can still slow another
  sandbox's CI". With CI and agents building in one sandbox and agents building in two others, CI
  gets 1/3 × 4/5 ≈ 27 % of what the sandboxes get and the local builds the other 73 %.

## Requirements

- **CSW-001 Slots per job.** A CI job's CPU budget is what its `[[ci.jobs]]` entry declares
  (`cpus`), else an equal share among its run's jobs (unchanged default). No second knob is
  added. The documentation names `cpus` as the place to cut a job's slots.
- **CSW-002 A sandbox weighs more while its CI jobs run.** With the `cpu` controller delegated,
  `<unit>/sandboxes/<sandbox>/cpu.weight` is 500 while at least one CI job of the sandbox runs,
  and 100 when none runs. It does not grow with the number of jobs (review round 1: 400 per job
  left two sandboxes that only build about 5 % each against 4 jobs). A job counts from when it has
  its slot and budget until its process ended and was reaped, whether it passed, failed, timed
  out or was cancelled.
- **CSW-003 Preparation keeps the weight; a start resets it.** Preparing a sandbox's groups (every
  run does) writes the weight for its jobs running now, so it never undoes a raised weight. A
  starting server writes 100 to every sandbox group, since no job runs in a new server and a
  crashed one left its weights. A sandbox whose group does not exist yet is skipped without a
  warning; its first preparation writes the weight.
- **CSW-004 Stop and rollback.** A stopping server writes 100 to every sandbox group with its
  split undo, so an older binary finds the default.
- **CSW-005 No delegation, no change.** Without delegation or the `cpu` controller nothing is
  written and CI runs as before.
- **CSW-006 Under load CI gets more than local builds.** With one sandbox running CI while its
  own agents and the agents of two other sandboxes build, the weights Yulius writes give CI more
  of the sandboxes' CPU than all agents together, under cgroup v2's hierarchical weight sharing.
  Each other sandbox keeps 1/7 however many jobs run.

## Plan

- [x] Spec and ADR for CI's host-wide CPU weight
- [x] A sandbox weighs more while its CI jobs run, in the cgroup adapter
- [x] CI jobs raise and lower their sandbox's weight through the isolation port
- [x] A test shows CI out-weighing local builds under load
- [x] Docs: architecture, operations, changes; per-job cpus documented as the slots
- [ ] Independent review, findings fixed test-first
- [ ] Gates green, pull request merged into dev on green CI

## Acceptance criteria

- A project's job declared with `cpus = N` runs with `YULIUS_CPUS=N` and waits for N CPUs
  (existing `local_ci::jobs_wait_for_their_cpu_budget_and_are_told_it`, rerun).
- A fake cgroup tree shows a sandbox's `cpu.weight` at 500 with one or more CI jobs and back at
  100 when they end. Preparation keeps it; a start and a stop write 100; a sandbox without a group
  yet is skipped.
- An integration test runs real CI jobs through the app and observes the counts the app tells the
  isolation: up while jobs run, down on every end (passed, timed out, cancelled by a superseding
  push), and 0 after the last.
- The share model over the written weights gives CI more of the sandboxes' CPU than the agents of
  three busy sandboxes (CSW-006), and each other sandbox 1/7 with 4 jobs. Without this change it
  gives CI less.

## Test plan

- `yulius-sandbox` unit tests on fake cgroup trees: weight by job count, preparation, start,
  stop, a missing group, no `cpu` controller.
- `yulius-sandbox` unit test: the cgroup v2 share model (weight divided among busy siblings, per
  level) over the tree that `Delegation` writes, with CI busy in one sandbox and agents in three.
- `yulius` integration test (`local_ci`): a fake isolation records the counts the app tells it
  while CI jobs run and after they end, including jobs cancelled by a superseding push. The
  adapter's `NamespaceIsolation::ci_jobs_running` is a two-line delegation to the tested
  `Delegation::set_ci_jobs` and runs in no test.
- No live kernel measurement: this sandbox has no cgroup mount (`/sys/fs/cgroup` is absent and
  mounting cgroup2 is refused). The VM measurement is left to the deployment, as with ADR-0039.

## Decisions

- [ADR-0247](../../docs/decisions/0247-a-sandbox-running-ci-jobs-weighs-more-on-the-host.md):
  a sandbox's own `cpu.weight` is 500 while its CI jobs run. Alternatives (a lower agents' leaf
  weight, a `cpu.max` cap on the agents, a host-wide CI group, 400 per job, a project-wide
  `[ci] cpus`) are compared there.
