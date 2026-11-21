---
yulius-task: 0d5934fd-6afe-400b-8edd-0697cd160325
status: in-progress
---

# SPEC-598 Shorten the Yulius CI critical path

## Goal

Fixes #1627.

Shorten this project's pull request and full-suite runs while executing every existing check. Measure quality execution, test compilation and sibling admission on physical runs.

## Context

The owner's November 21 production sample puts median PR time at 9.4 minutes. Sequential quality stages took about 154 s for check, 115 s for lint, 154 s for macOS and 71–275 s for docs. Both test matrix entries spent about 195 s compiling. Their slot waits reached 94 s and e2e waited 34 s, with a maximum reported wait of 147 s. [SPEC-591](SPEC-591-yulius-gets-its-own-staging-instance-and.md) has since landed. PRs run quality and security and prove changed job definitions. Dev runs the full suite. [SPEC-595](SPEC-595-share-compiled-rust-across-worktrees-and.md) supplies the compiler cache.

Task 558 found siblings were already dispatched concurrently. After network and exclusive-turn checks, admission orders tickets by priority and arrival. The slot span includes all these waits. Job order alone does not identify the cause of a particular wait.

## Requirements

- CI-001 Run check, lint, macOS and rustdoc concurrently where their declared shares fit. Preserve doctor, repository contracts, checker tests, formatting, dependencies, locked builds and warning policies. Identify failures by pass.
- CI-002 Build the nextest binaries once for both hash partitions. Keep quarantine, no retries, CI failure output, test thread limits and one doctest pass. End a failed build before running either partition. Keep artifacts bounded and isolated from overlapping runs.
- CI-003 Admit runnable siblings without an artificial queue delay while respecting every instance slot, CPU, memory, network, priority and exclusive-turn condition. Cancellation releases reservations.
- CI-004 Preserve merge-first job selection and exit 78 for jobs that check nothing. Compare physical runs and record any remaining delays with their causes.

## Plan

- [x] Record production baselines and map the merge-first gate
- [x] Run independent quality passes in budgeted CI jobs
- [x] Run both test partitions from one build
- [x] Admit eligible siblings together within the host limits
- [ ] Verify and review the complete CI gate
- [ ] Measure the physical runs and deliver to dev

## Acceptance criteria

- AC1 Separate job records show overlapping quality passes, including diagnostics from an individual failing check and the full set of former quality commands.
- AC2 Both hash partitions use a single build of the exact revision. A build failure propagates, either partition can fail, and doctests run once. Local unpartitioned tests continue to work.
- AC3 On an idle host, eligible siblings with room in both budgets start promptly. A full slot or budget, an older overdue job or a protected turn still prevents admission.
- AC4 Before/after evidence distinguishes queue time, execution time and reused results. Reviewed code reaches dev through the configured gate and requests staging.

## Test plan

Use recording command fixtures to compare gate coverage and build counts and inject failures. Exercise deterministic admission cases and the real CI runner with a bounded fixture. Run focused checks and the repository quick gate. Compare physical PR and full-suite runs by commit and run ID.

## Decisions

Use independent quality jobs for their existing checkout, budget and failure reporting. Keep the historical quality key for the check job. Both original test matrix keys keep their after-merge settings. The first shard to acquire the lane lock builds a nextest archive for the exact run and SHA. Both run against their checkout concurrently. See [ADR-0438](../../docs/decisions/0438-quality-passes-have-own-jobs-and-test-partitions-share-a-build.md) for artifact lifetime and recovery. Use the existing project Cargo entry point for compilation. On the 112-CPU CI budget the full nine-job gate requests 111 CPUs in total: four default compiler shares of 12, two test shares of 21, 18 for e2e, two for deps and one for security. The former five-job gate gave each shard 22 CPUs. Host admission still uses the live slot, CPU and memory limits. Measure the protected gate and changed-definition checks separately during rollout.

## Evidence

- Physical PR run `7504f4a7-0812-45d3-b223-e8c4c94d8a39` at `cfcbf99` took 7m05s, including 7m04s for quality. PR run `c1f40e73-3135-404d-8b43-5154fd5b501e` at `fc7aa66` took 5m29s, including 5m27s for quality.
- Full-suite dev run `384a0ca8-0e2a-4cc4-ba9f-8e148ca277ac` at `86b2acb` took 7m13s. Quality waited 10s and ran 5m30s. Test 1 waited 35s and ran 5m18s, test 2 waited 10s and ran 4m15s, and e2e waited 10s and ran 7m01s. Security was disabled and skipped. Full-suite dev run `3ade1835-cfc3-438a-9b45-68d84a2cd567` at `dc08c1a` took 8m48s, with quality taking 8m47s and both test jobs waiting about one second.

- The quality regression failed on the old configuration for all five independent passes and passed after the split. The merge-first configuration test now requires all five PR quality passes, both original post-merge matrix keys, e2e and security.
- #582 corroborated the old gate on dev run `50a34b9c-fd17-4db6-9c70-ba047eb1e8ef` at `14e01f438`. Shard 1's job `6078ca9c-46ca-447c-9d47-6fb33b866e05` reported a 3m20s Cargo build. Shard 2 took 5m49s, including 147.679s in nextest for 2,928 tests. Quality took 5m43s. A local sequential-partition prototype built once in 3m49s. That prototype was stopped after testing the first partition so both production partitions could retain concurrent execution.
- The admission regression on the former scheduler woke the last of four tickets with three slots free. It left all three idle waiting for earlier tasks to wake. The updated scheduler granted the first three in order and immediately released an uncollected grant on cancellation. All seven resource tests passed, including host-load and overdue-ticket cases.
- A real task-local Yulius CI run with two idle slots, `33501500-48a9-4b6a-8f79-693f4c89f003`, already admitted both shards within one millisecond on the former scheduler. Production's aggregate wait spans also include network, exclusive-turn and genuine budget waits.

- Five Python gate tests passed. A two-party barrier proved the first arriving shard built once and both consumers ran concurrently. The failure fixture initially returned failure instead of the skip code for the peer that could run no tests. It now keeps the producer's failure and the peer's exit 78. A recovery fixture caught a potential false green when the producer retried after its peer skipped. A successful retry now checks the whole suite. Interruption, a new run on the same lane, another lane and a changed-definition twin each selected the expected archive bytes.
- The pinned nextest built one archive containing the xtask test binaries. In a separate checkout both hash partitions ran with workspace remapping (1 and 3 selected tests passed, respectively), and the consumer produced its own `target/nextest/ci/junit.xml`. The 13 xtask unit tests and 8 command-level tests passed, including the archive arguments for the quarantine listing, both partitions and one doctest command.

- A complete local archive built 45 binaries and packed 96 files in 13.29s after a 4m18s build. Parallel consumers in two checkouts extracted it in 2.78s and 2.53s. Their selected CI integration and configuration tests passed (1 and 4, respectively) without recompilation. Task-local physical CI run `a652b645-3955-4152-a902-ffc1eae5bdbf` recorded 0ms for both idle shard waits with ordered grants, matching the original idle runner. These runs cover available capacity and archive relocation. They do not attribute the supplied production wait tail to a specific deployed scheduler.
- A second queue fixture woke the first ticket of three while all three fit. The partial-grant implementation reserved just one. The complete admission pass reserves three whichever ticket wakes, keeps startup allowances on uncollected grants and respects the slot limit.
