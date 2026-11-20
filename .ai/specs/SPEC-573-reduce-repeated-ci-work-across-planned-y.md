---
yulius-task: 53a77e47-55cc-48b0-ba87-928491c17f74
status: in-progress
---

# SPEC-573 Planned installs and repeated CI execution

## Goal

Reduce duplicate CI execution during a planned instance install when a controlled comparison demonstrates a benefit at a bounded cost to serving the new version.

## Context

On 2026-11-20 the install request preceded daemon startup by 39 seconds. Five running gates were requeued. Of 55.95 summed job-minutes observed on three gates before shutdown, four completed jobs accounted for 9.20 minutes. The same four job IDs later passed again in 6.49 summed minutes. The separate browser train failed before the restart. Exact shutdown, first healthy serving time and counterfactual completion times are unknown.

Startup recovery ends orphaned hosts and requeues their runs. SQLite clears individual job results except path skips. The helper stops the service before backing up its database and installing the binary. A healthy install and a rollback both use ordinary startup recovery. [ADR-0232](../../docs/decisions/0232-an-update-waits-for-running-deploys-and-holds-new-ones.md) covers deployment admission and the existing urgent control.

## Requirements

- **PCI-001** Compare a bounded wait for executing runs with individual reuse within the same run. An individual result needs verifiable execution inputs, outputs and dependencies.
- **PCI-002** Bound any added install delay and permit an urgent install under existing deploy protections. Preserve ordinary queue priority, grant ownership and crash/rollback recovery.
- **PCI-003** Keep executed, skipped, failed and interrupted outcomes distinguishable and preserve the protected gate's proof.
- **PCI-004** Separate repeated completed jobs and their service time from interrupted starts, elapsed gate time and ready-to-serving latency. Exercise ordinary and mixed loads and a control requiring new execution.

## Plan

- [x] Trace restart losses and compare recovery options
- [x] Measure duplicate execution and install delay under controlled load
- [x] Prove bounded planned installation and the urgent path
- [x] Verify recovery, outputs and gate timing across workloads
- [ ] Review and deliver the measured change to dev

## Acceptance criteria

- **AC-1 (PCI-001, PCI-004)** Identical arrivals and work in paired fixtures show duplicate starts, repeated completed jobs, repeat duration, gate elapsed and ready-to-serving including the worst added wait. Relate observed duplicates to the limits of the evidence.
- **AC-2 (PCI-002)** With no CI, a short run, a long run, urgent proceed, cancellation or a running deploy, the update reaches the expected phase and admits eligible work under its established rules.
- **AC-3 (PCI-001, PCI-003)** A failed check retains its outcome on a completed gate. An incomplete gate with missing required output executes the producing check again. No previously skipped job supplies an executed pass.

## Test plan

Use controlled paired traces and the existing CI, self-update and helper fixtures. Keep the host budget fixed. Inspect the staged change and run its targeted tests, quick checks and security scan before independent review and Yulius CI. If the alternatives fail to justify a change, report the measurements and rationale.

## Decisions

A whole run that ends on the old executor can use its own artifacts and inputs throughout. Reusing an individual result across executors would need evidence beyond a stable job ID. Choose the smallest option supported by the paired measurements.

Use the first 30 seconds of the existing task drain for a planned version install. Wait only for a running gate with an executed pass, no queued job and at most one running job. This includes the interval before the executor commits the run verdict. The normal two task-ready observations accrue during this period. An urgent Proceed, ordinary same-build restart or failed CI-state read adds no CI wait. A new gate can still dispatch under its usual priority. The grace ends on the first eligible update pass that sees no qualifying gate or reaches the deadline. The helper owns stop, backup, health and rollback.

## Evidence

### Restart boundaries and incident accounting

- At admission the old executor holds a turn for an exclusive job, a resource reservation and a slot. SQLite records a job as running after these are acquired. A queued job has no durable grant. On normal completion the executor drops its proxy credential, cleans job-scoped engine resources and the private job host/log, records exit status and bounded output, and releases the budget. The worktree and per-checkout cache can survive or be reused by a different attempt.
- The install request starts the helper protocol. It checks the staged build and restart prerequisites, then stops the old service before backing up the binary and SQLite files. Grants, cancellation channels and resource ownership disappear with the old process. A detached job host can outlive it. At startup `ci_recover` terminates leftover hosts, asks isolation to clean their job resources and calls `requeue_ci_run`. One transaction clears starts, ends, verdicts, output and usage for jobs of running runs except path skips. Completed runs keep their verdicts. A helper health failure restores the stopped-service backup and the previous binary.
- The supplied comparison of identical job IDs establishes four second executions, totaling 6.49 job-minutes after 9.20 job-minutes in their first passes. The three measured gates contributed 23.64 (quartio-v2 #1483, including 7.01 passed), 20.12 (Yulius #1507) and 12.19 (Product #686) job-minutes before shutdown. Five runs were requeued. The quartio-v2 browser train had already failed its latency test. The incident's 39 seconds ran from the installing audit event to daemon startup. The old daemon answered at +25 seconds. Neither event timestamps the systemd stop or the first healthy response.
- The Yulius task's `ci_status` returned the final #1507 gate at 8e32066, passed in 20m29s with its security job skipped. The quartio-v2 run was outside this project's readable CI history. For quartio-v2 quality and Product shared, broker and mcp/browser, the snapshots prove two successful executions, but supply no artifact inventory, mutable-input fingerprint or remaining sibling duration at install readiness. All four therefore get zero individual reuse credit. A gate that actually completes under the old executor keeps all its results and any dependencies were consumed in that execution.

### Paired timing fixtures

Run `python3 crates/yulius/tests/local_ci/planned_install_replay.py` to reproduce these pairs.

One-second discrete events, fixed job costs, FIFO within equal priority, common arrivals and budget in each pair. The helper is serving 39 seconds after its request in the fixture, and stops execution at +25 seconds. These are scenario inputs. Task readiness is observed at +2 and +4 seconds from drain entry, and every two seconds thereafter. Recovery reruns each unfinished gate from the beginning. A completed gate keeps its outcome. The old and proposed install both leave CI admission open. For the all-runs alternative, the fixture holds new admissions from readiness until serving.

| Fixed load | Jobs started twice | Previously passed jobs repeated | Second service of repeated passes (s) | Second service of all jobs started twice (s) | Gate elapsed per run (s) | Drain entry to serving (s) |
| --- | --- | --- | --- | --- | --- | --- |
| Ordinary, two runs, no update | 0 → 0 | 0 → 0 | 0 → 0 | 0 → 0 | 55/60 → 55/60 | n/a |
| One run near completion | 3 → 0 | 2 → 0 | 103 → 0 | 216 → 0 | 224 → 113 | 43 → 69 |
| Near completion, long run, later arrival | 6 → 3 | 3 → 1 | 150 → 50 | 592 → 380 | 223/381/116 → 112/407/107 | 43 → 69 |
| One long final job | 2 → 2 | 1 → 1 | 40 → 40 | 310 → 310 | 388 → 414 | 43 → 69 |
| Saturated, four runs | 5 → 5 | 1 → 1 | 100 → 100 | 905 → 905 | 438/393/498/463 in both | 43 → 43 |
| Empty at readiness, later arrival | 0 → 0 | 0 → 0 | 0 → 0 | 0 → 0 | 25 → 25 | 43 → 43 |

The mixed fixture uses five slots and three gates, arriving at 0, 0 and 90 seconds with job durations [35,65,112], [50,270] and [60]. Drain starts at 68. The 60-second all-runs drain cut repeat service for completed jobs to 50 seconds but moved the long gate to 437 seconds and serving to 99. On saturated load it increased repeat service from 100 to 250 seconds and reached serving at 99 seconds. A long running job alone gives no estimate of time remaining.

For a timing sweep, use executed-job durations in quality, test 1, test 2, e2e order from Yulius train #1523 [248,318,315,392], PR #1523 [552,447,436,437], PR #1521 [267,344,346,419] and the post-recovery #1507 [334,282,210,396]. Exclude skipped security. Only job durations come from history. Compare interruptions every five seconds up to the ordinary gate's end, running each vector separately in four slots (354 pairs) and together in six slots with arrivals [0,45,200,360] (230 pairs). Lost partial service and all second-attempt service are distinct from the repeated completed jobs shown here.

| Mean per interrupt | Repeated completed jobs | Second service of repeated passes (s) | Last gate elapsed (s) | Drain entry to serving (s) |
| --- | --- | --- | --- | --- |
| Solo | 0.73 → 0.56 | 233.80 → 177.53 | 684.37 → 661.17 | 43 → 46.94 |
| Mixed | 1.93 → 1.76 | 642.92 → 579.25 | 1473.23 → 1456.90 | 43 → 52.50 |

Median elapsed-gate change was zero in both sweeps. Worst observed additional install wait was 26 seconds (30 seconds from drain entry including the four seconds the control already waited). The 15-second window saved 20.38 seconds of repeat-pass service on mixed load with 4.60 seconds of additional mean wait. A 60-second window saved 131.25 seconds with 18.26 seconds additional mean wait and up to 56 seconds extra. At the selected 30 seconds, moving the synthetic stop to +0 or +38 seconds still reduced mixed repeat-pass service by 63.67 or 70.26 seconds per interruption. The fixture does not predict production wall-clock savings, admission competition or the historical fate of a particular gate.

For a comparison with individual reuse, give every synthetic completed pass immutable inputs, a verdict-only output and no downstream artifact. Keep those passes in their exact gate across the modeled restart and rerun interrupted jobs. This idealized policy removed all 150 seconds of completed-pass replay in the mixed three-gate fixture, while the longest gate still ended at 381 seconds and serving took 43 seconds. It removed all 642.92 mean seconds of completed-pass replay in the mixed timing sweep and moved the last gate forward 106.12 mean seconds. No such provenance is recorded for the incident's four jobs.


### Executable verification

The initial `cargo test --locked -p yulius --test main local_ci::planned_install::` against the eager installer failed the three planned-install cases at the assertion that the helper had interrupted the consumer. The same-build restart control passed. In the changed implementation the producer's result, checked artifact and final green or red gate survive when the consumer finishes before installation. The deadline test drops the old Tokio executor, removes the actual producer file, and starts recovery against the same store and Git clone. Both commands must execute again with the same run ID. An individual retained pass would leave the consumer waiting for a file that never arrives.

The indexed SQLite observation uses the existing partial active-run index and per-run job index. `EXPLAIN QUERY PLAN` on the schema with the obsolete single-lane index removed showed searches on `ci_runs_active` and `ci_jobs_run`. CI query failure or timeout leaves the existing update path available. No migration is needed.


`cargo nextest run --locked -p yulius -p yulius-store -E 'test(/local_ci::planned_install::|self_update::|a_finishing_ci_run_has_an_executed_pass|a_requeued_run_keeps_its_paths_skip|a_job_a_restart_ended_is_told_ended|a_tests_failure_is_never_taken_for_the_infrastructure|update_helper::tests::/)'` ran 72 tests, all passed. This includes the existing deploy guard, cancellation, staged build and failed-health rollback, four real CI executor fixtures and eight eligibility scenarios. In the missing-output case the fixture observed positive held job weight before disposal and zero immediately afterwards. The new app answered the helper's version endpoint while the persisted run still read running, then acquired fresh grants, rebuilt the missing file and recorded both results under the original job IDs.
