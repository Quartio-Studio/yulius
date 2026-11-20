---
yulius-task: 7f21f67e-0c92-4473-b8e5-832c47e6bc63
status: done
---

# SPEC-569 Shorten required mutation verification

## Goal

Reduce the time an author spends running Yulius's required Rust mutation campaign while retaining workspace test evidence for every verdict.

## Context

The trusted entry point is `scripts/dev mutants`. It applies [ADR-0347](../../docs/decisions/0347-mutation-testing-runs-once-per-pull-request.md) with the workspace baseline from [SPEC-533](SPEC-533-restore-trustworthy-mutation-test-result.md). The runner in Quartio-v2 has a different build and test workload. The six campaigns in the request spent a median 41.7% of elapsed time in the baseline, and 87.0% of subsequent worker-phase time in builds.

## Requirements

- **MUT-001** Measure a real ordinary change on the trusted command, including elapsed time, build and test phases, occupancy and limitations on attributing delay to contention or the publication path.
- **MUT-002** Keep the real Git diff, mutant identities, workspace packages, features and selected peer tests, independent writable builds, campaign and test bounds, baseline failure and every outcome class.
- **MUT-003** Adopt a correction only after a matched comparison demonstrates shorter elapsed time and equivalent evidence. Any reusable artifact must respect source and configuration inputs, isolation, concurrent worktrees and restart. Every verdict requires a fresh execution.

## Plan

- [x] Profile a retained campaign and its representative revision
- [x] Compare isolated builds under a matched resource budget
- [x] Verify workspace controls and select the supported runner
- [x] Publish the independently reviewed measurements

## Acceptance criteria

- **AC-1** Report elapsed campaign time, phase totals, resource use and reached identities for matched revisions, diffs, tests, budgets and initial state. Record load and ordering limitations.
- **AC-2** A failing unmutated peer stops the clean baseline. Viable caught and missed mutants preserve their identities. Exercise the SPEC-565 weak and strengthened assertion if reproducible.
- **AC-3** Keep the trusted command when equivalent verdicts, bounded work or a reproducible saving cannot be established.

## Test plan

Inspect retained outcomes, phase commands and per-worker logs. Replay a fixed historical Git change in a detached worktree. Measure campaign wall time and process CPU separately, including setup cost when applicable. Use the xtask workspace fixture for a failing sibling baseline and caught and missed mutations. Compare sorted identities by outcome and record incomplete runs as such. Independently review the evidence and deliver through Yulius CI.

## Decisions

Keep the trusted command and the workspace evidence contract in ADR-0347. A campaign-local compiler cache reduced repeated registry compilation, but the direction of the end-to-end timing difference reversed between the two matched revisions. The experiment introduces no runner setting or new dependency.

## Evidence

### Retained campaigns

In #547's 14:06:41 UTC campaign, the clean build took 249.6 s and clean tests took 69.1 s of the 715.4 s elapsed. Three additional workers each compiled the same 257 external packages as the baseline. The 25 unviables consumed 949.8 summed build-seconds. The five catches consumed 491.4 build-seconds and 113.9 test-seconds. In #555 the 12:31:15 UTC run stopped at a clean test failure with zero mutants executed. The subsequent run reached 32, with six caught, one missed and 25 unviable.

### Paired replays

Run `scripts/dev mutants --base 3fae70d80` in a detached checkout of `ca765e8d0`. For the strengthened assertion, cherry-pick the 19-line test patch from `74629ff06` onto that head. Its Git tree is `c1c86b42fde7335b59472878215891e8b5206458`. The runner and entry point at the measured revision are byte-identical to dev `d809931d5`. [SPEC-565](SPEC-565-preserve-verdicts-when-a-reviewer-report.md)

Each trial started from an empty xtask build directory and three private mutation targets on tmpfs. All used Rust 1.99.0, cargo-mutants 27.1.0, nextest 0.9.146, three workers, 22 jobserver tasks, seven test threads per worker, a 300 s mutant test timeout and a 1,800 s campaign bound. The baseline and mutant commands retained the real diff, `--workspace`, `--all-features` and the revision's nextest filter. Registry sources were already present. OS page caches and other host workloads could vary.

The trial set `RUSTC_WRAPPER` to sccache 0.13.0 (Ubuntu package `0.13.0+ds-3build1`). Each campaign set `SCCACHE_SERVER_UDS` to a private Unix socket, `SCCACHE_DIR` to an empty directory on tmpfs and `SCCACHE_CACHE_SIZE=2G`. The server started with `SCCACHE_START_SERVER=1` and `SCCACHE_NO_DAEMON=1`, and the trial stopped and reaped it before the timer ended. GNU `time` measured command launch through completion, including foreground server startup, compiler children and shutdown. The rows are in execution order.

| Assertion | Runner | Wall s | Clean build / test s | Mutant build / test worker-s | CPU-s | Mean occupied cores | Host CPU pressure |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Weak | Trusted | 646.46 | 171.74 / 38.71 | 873.06 / 315.06 | 4,088.10 | 6.32 | 8.82% |
| Weak | Cached | 665.35 | 158.58 / 37.80 | 977.85 / 329.29 | 4,179.31 | 6.28 | 11.94% |
| Strengthened | Cached | 834.52 | 218.84 / 51.06 | 1,224.95 / 328.49 | 4,484.77 | 5.37 | 25.95% |
| Strengthened | Trusted | 957.81 | 282.44 / 46.27 | 1,338.46 / 348.88 | 4,615.41 | 4.82 | 40.24% |

Phase totals come from `outcomes.json`. Worker phases overlap. CPU-s is accounted user plus system CPU time. Mean occupied cores is CPU-s divided by wall time. Host CPU pressure is the fraction of elapsed time with runnable work waiting for CPU (Linux PSI `some`). Mean busy host cores were 40.75, 44.08, 56.16 and 56.06 of 64 respectively. Peak sampled process-tree RSS ranged from 14.4 to 16.3 GiB and can count shared pages more than once.

In the weak trusted run, registry rustc processes were observed from command seconds 11.02 to 21.04 during the 171.74 s baseline build, overlapping workspace compilation. The corresponding interval across the two additional workers was 221.29 to 239.37 s. Across the command, samples accounted for 2,857.21 workspace rustc CPU-s. The two additional workers' first viable builds took 183.9 and 185.3 s. Later viable builds in warm targets included two at about 23 s. One-second process samples observed 33.17 registry rustc CPU-s in the baseline and 112.48 across the other two workers. That rustc subtotal omits native C and assembly work and processes that exit between samples.

The weak cache trial served 456 Rust, 34 C and 28 assembly cache hits. Across the command and its nested builds, sccache declined 862 incremental invocations and 167 requests for unsupported crate types. The store ended at 192 MB. Sampled registry rustc time fell from 145.65 to 53.95 CPU-s, while the complete command consumed 91.21 more CPU-s and took 18.89 s longer. The first viable builds in the two additional workers took 197.1 and 204.1 s.

The strengthened cache trial finished 123.29 s sooner. Contention differed even at similar host occupancy. Sampling `/proc/<pid>/task/<tid>/schedstat` observed 1,321.4 summed thread-seconds waiting to run in the cache trial and 2,084.1 in the trusted trial. These waits can coincide. The clean build alone differed by 63.60 s, before the extra workers could benefit from the cache. The experiments establish neither a repeatable elapsed saving nor a change in peer throughput. The 29, 48 and 36 minutes from approval to publication in #514, #518 and #520 also include concurrent review and author work. The replays measured command duration, without a paired publication event.

### Outcome identity

All four campaigns listed the same 23 raw mutant records and executed each exactly once. Comparing multisets included package, file, span, function, genre, name, replacement and the actual saved diff. Within each pair the baseline and each mutant had identical phase arguments, verdicts, failing test names and Rust error codes for unviables.

- Both weak baselines passed 3,209 selected tests across 41 binaries. Both commands exited 1, with 14 caught, one missed, eight unviable, zero timed out and zero unreached. The escaped mutation replaced `==` with `!=` in `start_review_subagent` at `crates/yulius-store/src/coordination.rs:421:31`.
- Both strengthened baselines passed 3,210 selected tests across 41 binaries. Both commands exited 0, with 15 caught and eight unviable. The added `a_first_reviewer_start_is_active_only_when_it_links_its_launch` assertion failed on the formerly missed mutation. Every other outcome held.
- For an independent comparison, sort UTF-8 lines of `outcome<TAB>identity<LF>` across `caught.txt`, `missed.txt`, `timeout.txt` and `unviable.txt`, using the file stem as the outcome. Their SHA-256 is `85cd2f38b26a6b4bf6414004df12d80f9c71ce3ef9b8ea159af532e03c89e654` with the weak test and `93196f635d5b2304bc6489fe785241d85b1d7991d623bee09cad9aadf5b5d424` with the strengthened test.

### Bounded controls

The existing xtask regression `a_real_run_finds_the_line_no_test_holds` passed in each real baseline. It stops on a deliberately failing unmutated sibling, reports a viable miss, catches it after the sibling asserts the result, and verifies that a whole-run deadline names unreached mutants.

A two-package Git fixture ran the repository's xtask entry point with two workers pinned to the same two CPUs, a 5 s mutant test bound and a 60 s campaign bound. The unmutated peer asserted one value and parked indefinitely when another changed. All four raw identities were reached once in both orders. `held` was caught, `escape` missed, `completes` timed out and `value` failed to compile. Each command exited 1. Trusted runs took 5.84 and 5.83 s. Cached runs took 5.91 and 6.11 s. The deliberate timeout dominates these fixture times.

A separate Cargo fixture exercised actual compiler-cache reuse. Each test started with an empty writable build directory. The peer asserted values compiled from local source, a locked Git dependency, a Cargo feature, a rustc cfg flag, a build-script environment value and the compiler version. The unchanged rebuild produced three Rust cache hits, and a fresh server reopening the same cache produced three more. All eleven test invocations passed, including the changed inputs, a switch from Rust 1.99.0 to 1.100.0 and concurrent worktrees with different expected values. The restarted server recorded nine Rust hits and 18 misses in total. These are fixture results for the tested inputs and a graceful server restart.

`spec_check`, `scripts/dev quick` and `git diff --check` passed. On the documentation head, `scripts/dev mutants` reported no changed Rust code against `origin/dev`. `scripts/dev security` reported Vidoc disabled for this project or instance.

Independent review of `17748ea9d` verified the saved diffs, execution multiplicities, phase arguments, outcomes, timings, resource samples and retained controls. It approved with one documentation correction. The baseline compilation intervals above now use the retained process samples.
