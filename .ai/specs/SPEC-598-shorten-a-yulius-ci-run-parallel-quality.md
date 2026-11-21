---
yulius-task: 0d5934fd-6afe-400b-8edd-0697cd160325
status: done
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
- [x] Verify and review the complete CI gate
- [x] Measure the physical runs and deliver to dev

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

- Task 582 compared both old build logs in physical dev run `56c6c4b0-f909-49a9-8425-b75608669bfe` at `cf492c608`. Shard 1 compiled for 5m00s and shard 2 for 4m54s, totaling 9m54s across concurrent checkouts. Passed merge train `c34dfbd2-6bb2-406d-a004-3be25e75cdf4` at `5af25af8a` waited 1m01s for quality, then ran it for 11m51s. The four Cargo passes ran for 2m27s (check), 2m42s (lint), 2m34s (macOS) and 3m54s (docs), totaling 11m37s in sequence.
- The quality regression failed on the old configuration for all five independent passes and passed after the split. The merge-first configuration test now requires all five PR quality passes, both original post-merge matrix keys, e2e and security.
- #582 corroborated the old gate on dev run `50a34b9c-fd17-4db6-9c70-ba047eb1e8ef` at `14e01f438`. Shard 1's job `6078ca9c-46ca-447c-9d47-6fb33b866e05` reported a 3m20s Cargo build. Shard 2 took 5m49s, including 147.679s in nextest for 2,928 tests. Quality took 5m43s. A local sequential-partition prototype built once in 3m49s. That prototype was stopped after testing the first partition so both production partitions could retain concurrent execution.
- The admission regression on the former scheduler woke the last of four tickets with three slots free. It left all three idle waiting for earlier tasks to wake. The updated scheduler granted the first three in order and immediately released an uncollected grant on cancellation. All seven resource tests passed, including host-load and overdue-ticket cases.
- A real task-local Yulius CI run with two idle slots, `33501500-48a9-4b6a-8f79-693f4c89f003`, already admitted both shards within one millisecond on the former scheduler. Production's aggregate wait spans also include network, exclusive-turn and genuine budget waits.

- Five Python gate tests passed. A two-party barrier proved the first arriving shard built once and both consumers ran concurrently. The failure fixture initially returned failure instead of the skip code for the peer that could run no tests. It now keeps the producer's failure and the peer's exit 78. A recovery fixture caught a potential false green when the producer retried after its peer skipped. A successful retry now checks the whole suite. Interruption, a new run on the same lane, another lane and a changed-definition twin each selected the expected archive bytes.
- The pinned nextest built one archive containing the xtask test binaries. In a separate checkout both hash partitions ran with workspace remapping (1 and 3 selected tests passed, respectively), and the consumer produced its own `target/nextest/ci/junit.xml`. The 13 xtask unit tests and 8 command-level tests passed, including the archive arguments for the quarantine listing, both partitions and one doctest command.

- A complete local archive built 45 binaries and packed 96 files in 13.29s after a 4m18s build. Parallel consumers in two checkouts extracted it in 2.78s and 2.53s. Their selected CI integration and configuration tests passed (1 and 4, respectively) without recompilation. Task-local physical CI run `a652b645-3955-4152-a902-ffc1eae5bdbf` recorded 0ms for both idle shard waits with ordered grants, matching the original idle runner. These runs cover available capacity and archive relocation. They do not attribute the supplied production wait tail to a specific deployed scheduler.
- A second queue fixture woke the first ticket of three while all three fit. The partial-grant implementation reserved just one. The complete admission pass reserves three whichever ticket wakes, keeps startup allowances on uncollected grants and respects the slot limit.

- Independent review at `a1d73a32e` found that the docs and archive jobs could invoke Cargo before entering the pinned toolchain on a clean CI host. With Cargo absent from the initial PATH, the direct launcher reproduced ENOENT. The public-entry regression also failed on both missing commands. Both jobs now enter through `scripts/dev`; a clean-home fixture runs the real xtask with rustup available only under the home and checks that docs, archive and archived partition execution all reach the pinned Cargo. Older checkout definitions use their own aggregate quality or test command. Vidoc was disabled. The scoped Clippy check passed.
- The public-entry fixture also reproduced a skipped peer becoming exit 1 at the xtask boundary. The same subprocess handoff used by security now preserves the archiving job’s exit code. The fixture proves both the producer’s failure and the peer’s exit 78. Nine xtask CLI tests passed, including explicit archive paths for both partitions.
- Round two independently approved `165700e39eec65f2c8b13e2a5af03bcc9802c163`, reran the nine xtask CLI and five Python gate tests and traced gate coverage, cancellation, resource limits and merge-first selection. Scoped xtask Clippy and `scripts/dev quick` passed.
- Manual undo and restore of the lint job, build reuse, changed-definition isolation, failed-peer skip, retry coverage, archived workspace remapping, docs invocation and batching made their respective assertions fail. Restoring the implementation passed all 5 gate, 9 xtask CLI and 8 admission tests.
- A paired task-local Yulius CI experiment delayed only the older of two eligible tickets by 2s after enqueue. The real runner executed both shell shards and recorded their wait spans. Before, at `cf492c608` in run `4b531bc5-037a-4b33-ac14-38a6e01d60da`, the older and younger tickets waited 2,001ms and 2,002ms. After, at `165700e39` in run `d6df90f7-55ac-4851-a91a-604243f4fb2c`, they waited 2,002ms and 1ms. Both runs passed with two free slots and the same injected delay. This isolates sibling ordering from host load, Docker networks and exclusive jobs.

### Hosted measurements on PR 1632

These rollout runs exercised both the protected sequential quality definition and the proposed jobs. Times below are minutes and seconds. Job columns measure execution after admission. The total includes queue waits and the protected checks.

| Run ID | Total | Protected quality | Check | Lint | macOS | Docs | Deps | Archived shards 1 / 2 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| c570a842-d104-440e-a751-10a748f6e4ee | 8:37 | 7:59 | 2:17 | 2:22 | 2:15 | 0:50 | 0:02 | 6:44 / 6:03 |
| 13f3c5a0-c2f6-4083-a992-e877204e4504 | 10:13 | 8:36 | 2:29 | 2:36 | 2:25 | 0:52 | 0:03 | 8:14 / 6:37 |
| b740521f-1360-41c0-b8c9-4db7642b4726 | 9:04 | 8:27 | 2:00 | 2:08 | 1:57 | 0:40 | 0:02 | 6:03 / 5:04 |
| 4dd578d7-07ce-46ab-adb3-bc68242b2a0f | 6:43 | 6:42 | 1:50 | 1:54 | 1:48 | 0:34 | 0:02 | 6:31 / 4:43 |
| ba96672f-2bfe-43e6-ae5a-ed5f6e546311 (20:47 UTC train) | 10:29 | 6:21 | 2:29 | 2:17 | 2:20 | 0:47 | 0:02 | 5:59 / 3:48 |

The longest new quality pass took 1m54s–2m36s across these runs. The full-suite dev baselines above spent 5m30s–8m47s on sequential quality. In PR run c570a842, shard 2 built 45 binaries in 3m09s and packed 96 files in 0.82s. Shard 1 reused that archive. Each extracted it for quarantine listing and execution in 1.84–1.90s per extraction. The old dev run at cf492c608 paid 5m00s and 4m54s for separate builds. Both proposed partitions passed in every row.

On the train, the changed test jobs waited 3m05s and 4m36s. Those hosted spans include contention and used the deployed scheduler. The paired runner experiment above measures the scheduler change itself. Quality checker fixtures now capture their own stdout after the first hosted run exposed synthetic test-build markers in the quality log. The focused checker suite emitted markers before the fix and none after, with all five tests passing.
