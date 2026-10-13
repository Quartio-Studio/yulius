---
yulius-task: none
status: done
---

# SPEC-082: tests do not time out under load; every process plans for its CPU and memory budget

Status: implemented. On the VM after the next deployment: the budget lines in CI job output and
the load during a busy hour are still to be observed.
Date: 2026-10-14
Owner: engineer A of the 2026-10-13 coordination round (branch `fix/test-timeouts`)
Scope authorization: the person's words (F 2026-10-13): "There were 2 very obvious test bugs
keeping showing. Tests kept getting timed out ... Fix the root cause and not just patch. We want
to fully utilise our RAM and CPU but keep everything stable and robust ... we'll onboard other
repos soon (studio, product) and they might need slightly different testing." The coordinator's
brief: tests time out in Yulius CI runs and in agents' local runs while many agents work; a
resource model so the machine is fully used but never oversubscribed, which this repository and
other repositories consume through `yulius.toml`; the racy queued-message test fixed at its root.

## Problem and evidence

All from the VM (12 CPUs, 47 GiB, the `yulius` sandbox) on 2026-10-11..13: the `ci_jobs` table
(stored output tails), the agents' Claude Code transcripts in the sandbox, the sandbox's cgroup
files and the kernel log.

**1. A third of the suite's time was a fixed 45 s sleep in the product (test bug).**
- Every Yulius CI `quality` run (52 runs with test output) had 10 to 17 tests over nextest's 30 s
  slow mark. Their durations did not depend on load: each varied by under 2 s across all runs.
  `task_done::a_restart_keeps_done_tasks_done_and_applies_goals_achieved_meanwhile` took
  104.2–106.0 s in 49 runs, against nextest's kill at 120 s (`terminate-after = 4` × 30 s);
  `a_pending_background_agent_…` 66.4–67.9 s; seven more 46–60 s.
- The cause: a goal task's goal is typed once Claude Code's SessionStart hook has arrived and the
  screen is quiet, or, for a CLI without hooks, after half of `GOAL_READY_TIMEOUT` (45 s). The fake
  agent ran SessionStart only when a test asked for it (`session_start: true`), which nine tests
  did. Every other goal test waited the hookless fallback; one with a restart waited it twice.
  Every supported Claude Code runs SessionStart.
- The 17 such tests added up to about 980 s of the suite's 1 742 s of test time (sum of per-test
  medians). On a loaded machine the 105 s test crossed 120 s: nextest killed it in agents' gates
  on 10-12 at 20:00 and 10-13 at 11:57 (UTC), and `a_goal_the_tui_dropped_is_submitted_again`
  failed at its 90 s wait on 10-12 at 14:53.

**2. The queued-message test raced the product's queue (test bug).**
`task_done::a_message_queued_before_the_start_reaches_the_agent` failed in agents' gates with
`Conflict("task is running")` (10-13 12:31 and 14:14 UTC; "about one run in three", said the task
that patched it). The test created a task, moved it to `queued` itself while session slots were
free, queued a message and started it. But a queued task with a free slot is what the queue
starts by itself, and the previous task's stop and its Done (SES-101) each run a pass of the queue
in the background. The patches since (#355: a start of a running task answers OK; #360: the test
accepts a conflict) left the window before the message: one queue pass there starts the task and
the message is refused ("The task has started: type into its session instead"), shown
deterministically by calling `drain_queue_now()` at that point. The product is right; the test's
premise was wrong.

**3. Every process sized itself to the whole machine.**
- Two CI lanes ran two jobs each: up to 4 CI jobs at once (52 minutes over the two days, 3 jobs
  in 39 more), each `quality` with `NEXTEST_TEST_THREADS=num-cpus` (12 threads) and Cargo's
  default of 12 build jobs.
- Agents ran the same gate locally: up to 7 foreground test runs at once (from the transcripts;
  background runs come on top), each at one thread per CPU until #351 hard-coded 4.
- #351's 4 threads made the agents' local suite 2–3 times slower without making it more robust:
  full runs took 120–200 s before it and 273–417 s after (23 runs from 13:12 UTC). The suite is
  mostly waiting (finding 1), so fewer threads mainly lengthened the waits.
- Yulius set `CARGO_BUILD_JOBS` (a third of the CPUs) in every agent session: a Rust variable in
  product code, for every repository.
- CI `e2e` ran one worker (the runner's default in CI) and took 435–1 114 s, longer than
  `quality` in most runs: the critical path of every CI run, on a machine with idle CPUs at night.

**4. The sandbox was capped below the machine and throttled.**
The sandbox's `cpu.max` was every CPU but two (10). Its `cpu.stat`: 900 801 of 1 792 320 busy
periods throttled (50.3 %), 148 959 s of throttled time. A throttled group stops all of its threads
until the next 100 ms period, which is a wall-clock stall for every waiting test, while the two
CPUs left out stayed nearly idle (Yulius's own group used 231 CPU-seconds over the same days).

**5. Memory pressure.** The sandbox hit `memory.max` 22 441 048 times and had 64 OOM kills
(kernel log: `rustc`, `clippy-driver`, `rust-lld`; 10-12 21:53–22:15 and 10-13 03:07–03:54 and
14:16–14:49 CEST), with 1.36 billion page-cache refaults.

Already fixed before this change: the sandbox memory limit (38 GiB), a rust-analyzer plugin that
used 20 GB, the CI split by CPU weight (SPEC-053, deployed 10-13 20:21 UTC), and the wall-clock
assertions of `wiring::jev` and `yulius-github::client` (bounded by their fakes' answers since).

## Scope

In scope: the two test bugs at their root; a CPU and memory budget for every process group Yulius
starts in a sandbox (CI jobs, agent runs, terminals, the Cockpit), told the same way in any
language and scheduled for CI; the sandbox's CPU shared by weight instead of capped; this
repository's `yulius.toml`, nextest configuration and e2e configuration consuming the budget.

Not in scope:
- one cgroup per agent session (fairness between agents enforced by the kernel); the budget
  makes agents plan for a share, the CI split keeps CI first (SPEC-053), and the measurements
  after deployment decide whether per-session groups are needed;
- a lease or job server that sizes a command by the agents running at that moment;
- per-job cgroups for CI jobs (one CI leaf stays, SPEC-053);
- declaring `cpus` per job in this repository's `yulius.toml`: the deployed parser refuses the new
  keys until this change is deployed, and this pull request is tested with its own `[[ci.jobs]]`.
  The default equal share applies meanwhile.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| TST-001 | The fake agent runs Claude Code's SessionStart hook whenever it runs hooks, as every supported Claude Code does; a test of an older CLI leaves it out (`no_session_start`). The hookless fallback keeps its product default (half of 90 s); its own test shortens the bound through `AppConfig::goal_ready_ms` instead of sleeping 45 s. | `task_done::*`, `supervision::goal_mode_types_the_goal_once_the_session_is_ready` (durations in Evidence) |
| TST-002 | `task_done::a_message_queued_before_the_start_reaches_the_agent` makes the wait real: another task holds the only session slot, so the session limit queues the task, a queue pass leaves it queued, the message is accepted, and the holder's stop lets the queue start it with the message. No state is set by hand. | the test, 10 of 10 in a row; the old test fails at once with one queue pass injected |
| RES-001 | A sandbox's capacity is its enforced CPU and memory limits (the isolation adapter's `cpu.max` and `memory.max`, only with cgroup delegation) within the host's CPUs; without a memory limit, 2 GiB per CPU. | `resources.rs`; `local_ci::jobs_wait_for_their_cpu_budget_and_are_told_it` (limits 4 CPUs, 8 GiB) |
| RES-002 | A `[[ci.jobs]]` entry may declare `cpus` (1–1024) and `memory_gb` (1–4096) as whole numbers; `yulius.toml` accepts whole numbers. Invalid values are configuration errors with their line. The declaration is stored with each job (schema 20), so a restart keeps it. | `spec_format::config::tests::jobs_declare_their_needs_and_the_budget_maps_to_variables`, `precise_errors_with_line_numbers`, `render_round_trips`; store `runs_jobs_transitions_and_one_running_run_per_sandbox` |
| RES-003 | A CI job leaves `queued` only once a job slot and its budget are free: its declared CPUs and memory, else an equal share of the capacity among its run's jobs (at least one CPU; memory in proportion to the CPUs). The pools hold the capacity once, for all lanes and sandboxes, so CI jobs never ask for more CPUs or memory than the sandbox has. Waiters are served in order; a budget larger than the pool takes the whole pool; a job cancelled while it waits ends cancelled. The job's output starts with `[yulius] budget: N CPUs, M MiB …`. | `local_ci::jobs_wait_for_their_cpu_budget_and_are_told_it` (fails without the wait: three jobs ran at once), `local_ci::runs_jobs_in_parallel_publishes_statuses_and_gates_merges` |
| RES-004 | Every process group Yulius starts is told its budget in `YULIUS_CPUS` and `YULIUS_MEMORY_MB`, and in the variables of `[resources.env]` (`NAME = "template"`, placeholders `{cpus}`, `{memory_mb}`, `{memory_gb}`) of the `yulius.toml` where it starts: a CI job's checkout, an agent run's worktree, a terminal's or the Cockpit's folder. Names are `[A-Z_][A-Z0-9_]*`. What Yulius sets and what changes how the agent CLI, Git, TLS, proxies, shells or interpreters run is refused (`PATH`, `HOME`, `CI`, `YULIUS_*`, `GIT_*`, `ANTHROPIC_*`, `CLAUDE_*`, `NODE_*`, `PYTHON*`, `*_PROXY`, `LD_*` and similar): the variables reach the agent CLI's own process, which holds the provider credential. At most 32 entries of at most 256 bytes. A missing or invalid table sets only the two variables. | `resources::tests::*`, `local_ci::jobs_wait_for_their_cpu_budget_and_are_told_it` (`MAKEFLAGS=-j3`), `supervision::an_agent_session_is_told_its_budget_in_the_repositorys_terms` |
| RES-005 | Agent runs, terminals and the Cockpit get the agent budget: `YULIUS_AGENT_CPUS`, else a third of the capacity rounded up, at least 2, never more than the capacity; memory in proportion. Yulius sets no tool's variable of its own: the `CARGO_BUILD_JOBS` it set for every repository (SPEC-020 ENV-003) is now the repository's mapping. | `resources::tests::an_agent_plans_for_a_third_of_the_sandbox`, `supervision::an_agent_session_is_told_its_budget_in_the_repositorys_terms` (`CARGO_BUILD_JOBS` unset without a mapping) |
| RES-006 | A sandbox has no CPU quota by default: its CPU is shared by weight (the CI split of SPEC-053 inside, Yulius's own group and the host's services beside it). `YULIUS_SANDBOX_CPUS` still sets `cpu.max`, and the capacity follows it. | `cgroup::tests::defaults_leave_room_for_the_host` |
| RES-007 | This repository consumes the budget: `[resources.env]` maps it to `CARGO_BUILD_JOBS`, `NEXTEST_TEST_THREADS` and `RUST_TEST_THREADS`; `.config/nextest.toml` has no fixed thread count (one per CPU outside Yulius); the `quality` job no longer forces one thread per CPU; the e2e runner uses half the budget as workers (its own default is half the cores). | `yulius.toml`, `.config/nextest.toml`, `e2e/e2e.config.ts`; this pull request's CI |

## Design and contracts

**The budget model.** Domain `yulius_domain::resources` (pure): `Capacity`, `Budget`, `Need`,
`Capacity::ci_job`, `Capacity::agent`, `EnvTemplate`, `check_env_template`, `budget_env`. App
`yulius_app::resources`: `App::capacity` (from `SandboxIsolation::limits` and the host's CPUs),
`App::agent_budget`, `App::agent_resource_env(dir)`, `App::ci_reserve(budget)` (two FIFO
semaphores in `CiState`, CPUs and MiB, sized from the capacity at the first job).

```
capacity (sandbox limits within the host, else host CPUs and 2 GiB per CPU)
├── CI pools: every CI job of every lane holds its CPUs and MiB while it runs
│     need: [[ci.jobs]] cpus / memory_gb, else capacity / jobs in its run
└── agent budget: per agent run, terminal, Cockpit (YULIUS_AGENT_CPUS, else capacity / 3, ≥ 2)
environment: YULIUS_CPUS, YULIUS_MEMORY_MB, [resources.env] of the yulius.toml where it starts
```

**Why the environment, not a cgroup quota per process.** A quota is a hard cap: a job alone at
night could not use the idle CPUs, and inside the sandbox's containment (its own cgroup
namespace, no cgroup mount) most runtimes do not see it anyway; they size themselves by the CPUs
they are allowed to run on, all of them. The environment works in every language and the
repository decides what it means for its tools. Priority stays with the kernel: CI's weight
(SPEC-053) and the sandbox's share of the host.

**Why the CI pools hold the whole capacity.** When agents are idle, CI uses the machine. When they
build too, CI keeps at least 80 % by weight (SPEC-053), so a CI job's threads run at most about
1.25 times slower; the jobs never ask for more than the sandbox in total, lanes included.

**Why a third for agents.** Three agents running a gate at once then fill the sandbox. The
transcripts show at most three foreground test runs at once in 81 % of the minutes with any
(1 295 of 1 591). More
at once share by time; with tests that wait on conditions (TST-001), that slows them without
failing them. The operator can set `YULIUS_AGENT_CPUS`.

**Lifecycle.** A job's budget is decided when it is about to start (its run's job count is known)
and released when its process has ended (the permits drop with the job's task). Restarts: queued
and requeued runs keep their jobs' declarations in the store; the pools start empty.

**Persistence.** Migration 20 adds `ci_jobs.cpus` and `ci_jobs.memory_mb` (nullable). Older rows
read as "nothing declared".

## Failure and compatibility

- A `yulius.toml` an older Yulius reads: `[resources.env]` is an unknown table and ignored. The
  new `[[ci.jobs]]` keys are refused by older binaries (strict keys), so a repository declares
  them only once its Yulius is updated (this repository waits for the deployment).
- A worktree whose `yulius.toml` predates `[resources.env]` gets only `YULIUS_CPUS` and
  `YULIUS_MEMORY_MB`: Cargo then builds with all CPUs until the branch merges the integration
  branch.
- The pools are sized once per process. A budget above a pool (the capacity read later is
  larger) is clamped to the pool, so a job never waits forever.
- A job stuck waiting for its budget is still bounded: cancelling or superseding its run ends the
  wait; it does not count against `YULIUS_CI_JOB_TIMEOUT_MIN` before it starts.
- Rollback: the previous binary refuses schema 20 like any newer schema; the update helper
  restores the database with the binary.

## Delivery and acceptance

- AC-1 (TST-001): the goal tests run without the 45 s fallback; the slowest test of `task_done`
  takes under half of nextest's kill bound.
- AC-2 (TST-002): the queued-message test passes 10 of 10 in a row; the previous version fails at
  once when a queue pass runs where the stop's or Done's background pass can run.
- AC-3 (RES-003): with a capacity of 4 CPUs and two jobs of 3 CPUs per run in two lanes, at most
  one job runs at a time, every job's output states its budget, and `MAKEFLAGS` reaches the job
  as `-j3`.
- AC-4 (RES-004, RES-005): an agent session started in a worktree whose `yulius.toml` maps
  `MAKEFLAGS = "-j{cpus}"` has `YULIUS_CPUS`, `YULIUS_MEMORY_MB` and `MAKEFLAGS` set, and no
  `CARGO_BUILD_JOBS`.
- AC-5 (after deployment, on the VM): CI job outputs start with their budget line; during a
  busy hour the load average stays near the CPU count instead of 35–47, and no test is killed at
  120 s.

## Open decisions

- Per-session cgroups for agents (kernel-enforced fairness between agents): owner the
  coordinator; default not now; revisit if AC-5 shows agents still starving each other.
- Per-job `cpus` for this repository (for example `quality` 8, `e2e` 4 on 12 CPUs): after the
  deployment; the equal share (6 and 6) applies until then.

## Decisions

[ADR-0065](../../docs/decisions/0065-processes-plan-for-a-cpu-and-memory-budget.md).

## Evidence

All on this branch, on a 10-CPU Mac (other engineers building at the same time), unless said.

**TST-001: the goal tests without the fallback.** CI medians on the VM (10-13, 12 threads) against
this branch (`task_done::` and the supervision test, 6 threads, one run):

| Test | CI before (runs) | After |
| --- | --- | --- |
| `task_done::a_restart_keeps_done_tasks_done_and_applies_goals_achieved_meanwhile` | 105.0 s (49) | 19.3 s |
| `task_done::a_pending_background_agent_keeps_the_task_working_until_it_finishes` | 66.9 s (48) | 24.6 s |
| `task_done::a_monitor_that_never_reports_counts_until_it_expires` | 66.9 s (3) | 22.4 s |
| `task_done::a_switch_waits_for_a_background_agent_launched_in_the_turn` | 64.2 s (3) | 22.5 s |
| `task_done::a_done_task_whose_shell_ends_frees_its_slot_at_once` | 58.7 s (3) | 16.8 s |
| `task_done::a_stop_a_hook_continued_does_not_leave_the_task_waiting` | 57.9 s (48) | 15.5 s |
| `task_done::a_goal_not_met_does_not_leave_the_task_waiting` | 56.9 s (48) | 15.4 s |
| `task_done::a_live_monitor_keeps_the_task_working_until_it_ends` | 55.5 s (3) | 12.7 s |
| `task_done::a_background_shell_keeps_the_task_working_until_it_ends` | 54.3 s (51) | 20.0 s |
| `task_done::a_goal_not_met_never_reads_idle_between_turns` | 53.8 s (2) | 11.9 s |
| `task_done::a_continued_turn_that_dies_without_a_stop_leaves_the_task_waiting` | 53.0 s (49) | 17.1 s |
| `task_done::a_failed_goal_leaves_the_task_waiting` | 52.3 s (3) | 10.0 s |
| `task_done::an_achieved_goal_with_no_shell_left_makes_the_task_done` | 49.5 s (51) | 6.9 s |
| `task_done::a_done_task_can_be_archived` | 46.8 s (52) | 11.0 s |
| `task_done::a_done_task_can_be_deleted_its_session_is_closed_first` | 46.8 s (52) | 5.2 s |
| `task_done::a_turn_closed_without_a_verdict_settles_as_before` | 46.3 s (3) | 5.0 s |
| `supervision::goal_mode_types_the_goal_once_the_session_is_ready` | 46.1 s (52) | 12.4 s |

Together about 980 s before and 250 s after. The whole `task_done::` file (28 tests) and the
supervision test ran in 63 s.

On the VM, this pull request's first Yulius CI run (f136336, on the deployed binary, one test
thread per CPU): nextest `Summary [76.190s] 1440 tests run: 1440 passed, 16 skipped`, no test over
30 s. The 52 runs before took 121–193 s with 10–17 tests over 30 s each. That run failed later
in `cargo doc` on a link to a crate-private item, fixed in the next commit.

**TST-002: the queued-message test.**
- Red: the previous test with one `app.drain_queue_now()` after its manual move to `queued` fails
  at once with `Conflict("The task has started: type into its session instead")`.
- Green: the new test passed 10 of 10 in a row (`--stress-count 10`), 11 s each. Its first version
  took 33 s: it stopped the prompt task while it was still starting (the fake writes its launch
  file before the spawn), which waits for the whole stop escalation; it now waits for `running`.

**RES-002..005.**
- Red: `local_ci::jobs_wait_for_their_cpu_budget_and_are_told_it` with the reservation taken out
  fails with `jobs over the budget ran at once: ["build", "check", "build"]`; green with it.
- `supervision::an_agent_session_is_told_its_budget_in_the_repositorys_terms` passes: the session
  sees `YULIUS_CPUS`, `YULIUS_MEMORY_MB` and `MAKEFLAGS`, and `CARGO_BUILD_JOBS` is unset.
- Unit tests: `resources::tests::*` (shares, the agent's third, templates, refused names) and the
  parser's `jobs_declare_their_needs_and_the_budget_maps_to_variables`; the store keeps a job's
  declaration.
- This repository's `yulius.toml` read by the new parser: `quality` and `e2e` without
  declarations, three variables; on 12 CPUs and 38 GiB a job gets `YULIUS_CPUS=6`,
  `YULIUS_MEMORY_MB=19456`, `CARGO_BUILD_JOBS=6`, `NEXTEST_TEST_THREADS=6`,
  `RUST_TEST_THREADS=6`.

**Gates.**
- `cargo nextest run -p yulius-domain -p yulius-store -p yulius-sandbox -p yulius-app`: 520
  passed. The doctests of `yulius-domain` (9, 4 new) pass.
- `cargo nextest run -p yulius` on the suites this touches (`task_done`, `supervision`,
  `first_message`, `local_ci`, `shells`, `cockpit`, `session_sync`, `pool_supervision`,
  `subscription_pool`, `update_supervision`, `supertask`, `queued_messages`): 133 passed in 137 s
  at 6 threads, one test over 30 s (41.6 s).
- `cargo fmt --all`, `cargo clippy --workspace --all-targets --all-features -- -D warnings` and
  `python3 scripts/check_repository.py` pass.
- Not run here: the e2e suite (only its worker count changed; no `node_modules` on this machine)
  and the whole gate, which Yulius CI runs on the pull request.
