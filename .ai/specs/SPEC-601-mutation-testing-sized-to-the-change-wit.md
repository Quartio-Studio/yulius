---
yulius-task: ff25261d-6a45-4346-8f6a-949e2427e082
status: in-progress
---

# SPEC-601 Mutation testing sized to the change and measured on disk

## Goal

Measure the cost of mutation testing on a real Yulius change and verify the delivery that moves build scratch to disk and sizes campaigns to the change.

## Context

The owner measured 105 minutes of mutation waits across 22 sessions and 231 GiB of tmpfs use across three sessions on November 21. The current wrapper already selects committed Rust lines against the merge base. It starts at two workers on this host, allows 30 minutes and inherits the session's temporary directory. Tasks #583 and #584 received the same request. Task #583 owns the runtime, tooling, guidance and bootstrap changes. This task owns the independent measurement and verification record.

## Requirements

- **MUT-001** Agents and applicable CI jobs allocate mutation and ad-hoc build scratch on disk. Finished runs remove owned scratch. Session tmpfs has an explicit capacity and writes beyond it report a filesystem error.
- **MUT-002** The default selection covers touched lines. A documented size or risk threshold justifies wider selection. Both modes specify time and concurrency budgets and preserve evidence of survivors, timeouts and incomplete runs.
- **MUT-003** The project's verification guidance and default bootstrap for other projects give the same rule. Other repositories take this guidance through their own tasks.
- **MUT-004** Compare elapsed time and scratch memory on the same real Rust change with the old and delivered tools. Identify the workload, scope and limitations. Confirm the checked revision merged to dev.

## Plan

- [x] Measure a real mutation run on the current session tmpfs
- [x] Replay the same change with disk scratch and the new mutation budget
- [x] Measure the one-file rollback and restoration proof
- [ ] Verify containment, cleanup and bootstrap behavior at the delivery head
- [ ] Record the measurements and verify green delivery to dev

## Acceptance criteria

- **AC-1** The measured runs use the same changed product lines and tests. Each reports its mutant outcomes, elapsed time, peak tmpfs allocation and scratch remaining after exit.
- **AC-2** The containment and CI launch paths apply the declared bound and scratch location. A process at the limit observes the documented error. The wrapper's normal and incomplete outcomes have cleanup evidence.
- **AC-3** The repository and shipped bootstrap give an agent and reviewer the same scope, threshold, budgets and evidence obligations. Green Yulius CI and a dev merge identify the delivered revision.

## Test plan

Replay the terminal-redraw fix from PR #1586 in its real `yulius-contracts` crate. Limit the benchmark workspace to that crate so the baseline can run safely on the affected host. Measure tmpfs allocated blocks and elapsed wall time around the wrapper. Verify the scratch filesystem and inspect temporary directories after each exit. Check the delivered launcher, CI and bootstrap tests and Yulius delivery evidence.

## Decisions

The implementation in #583 uses one worker, changed lines and 10 minutes for routine changes. At 16 listed changed-line mutants, or when the change alters authorization, persistence or process lifecycle, a wider run can use touched-file scope, at most two workers and 30 minutes. Each mutant has a 300-second ceiling. CPU and memory budgets may further reduce concurrency. A reviewer checks the base, trigger, scope, budgets, an unmutated baseline and each mutant outcome. Risky changes exercise affected integration tests. A routine change in one product Rust file with a single behavior and 1–8 listed changed-line candidates may use a recorded rollback proof. The eligibility check lists candidates. The evidence names the changed behavior, the regression that failed with only the implementation reverted, and that same regression passing after restoration. The bootstrap asks each onboarded project to install the same rule in its guidance and mutation entry point.

Keep product edits and product delivery in #583. Use the complete crate and its ordinary test suite for both measurements. Compare single-run observations on this host and record compiler/process memory separately from tmpfs consumption.

## Evidence

### Method

Replay the complete `crates/yulius-contracts` directory from the parent of `a643b69064f95202077e35dbe6fc4c7c92b1fa01`, then apply that commit's `src/ptyhost/redraw.rs` change including its regression test. The benchmark Git workspace contains this crate as its sole member, its original workspace dependency versions and lock, and two commits. The wrapper runs `xtask mutants --base HEAD^` on the replay. Each campaign starts with fresh target directories and uses Cargo in offline mode, Rust 1.99.0, cargo-mutants 27.1.0, nextest 0.9.146, six CPUs and a declared 24,576 MiB budget.

Sample allocated blocks on the session's private /tmp using `statvfs` at roughly 200 ms intervals. Record the peak change relative to immediately before launch. Measure elapsed time from launch through exit and the largest child-process RSS with `getrusage(RUSAGE_CHILDREN)`. RSS is the high-water mark of the largest individual child.

### Filesystem limit

A disposable container on the development host ran `dd if=/dev/zero of=/tmp/build bs=1048576 count=1025` against a tmpfs mounted with `size=1073741824`. The first 1,024 MiB were written. The next write returned “No space left on device” and dd exited 1. The current agent's namespace refused nested bubblewrap. The container verified Linux's write behavior at the configured mount size. At `fb9d5a07b`, `cargo test -p yulius-ptyhost --lib containment::tests` passed nine tests including plain and joined mount arguments. `each_agent_and_ci_run_owns_only_its_build_scratch` passed for distinct agent and CI directories and verified guard cleanup.

### Result of the committed campaign

| Metric | Old wrapper `14e01f438` | Disk-scratch wrapper `fb9d5a07b` |
| --- | ---: | ---: |
| Workers / campaign limit | 2 / 30 min | 1 / 10 min |
| Baseline tests passed | 56/56 | 56/56 |
| Mutants caught | 7/7 | 7/7 |
| Elapsed wall time | 39.611 s | 127.762 s |
| Sampled peak increase in private /tmp | 903,884,800 B | 0 B |
| Sampled peak allocated in dedicated scratch | 904,015,872 B (tmpfs) | 581,419,008 B (ext4) |
| Largest individual child RSS | 874,444 KiB | 849,368 KiB |
| Entries remaining in scratch parent | 0 | 0 |

The updated wrapper received an ext4 `YULIUS_BUILD_SCRATCH` directory while ordinary `TMPDIR` remained `/tmp`. The full campaign used 88.151 s more elapsed time in this trial. The shared host's one-minute load average reached 287 during the updated run, and its unmutated build took 33 s compared with 17 s in the original run. The workload and host conditions limit timing comparisons to these trials.

### Scratch-only control

On the original wrapper, setting only `TMPDIR` to the private ext4 directory kept the two-worker default. This run caught the same seven mutants in 50.424 s with zero sampled growth on the session tmpfs. Its maximum observed disk allocation was 1,037,381,632 bytes. This control isolates the effect of relocating scratch on tmpfs allocation. Each timing is one observation on a shared host.

### Command cleanup and validation

With `scripts/cargo --scratch-target metadata --no-deps --format-version=1`, Cargo reported a target within the supplied ext4 scratch root. The target was absent after exit. An invalid metadata format returned exit 1 and also left the parent empty.

The timing table uses completed, sequential campaigns. Discarded exploratory runs exposed a Cargo configuration parse error, overlapping report rotation and an incomplete disk-usage sample while Cargo removed an object. The observer subsequently skipped partial disk samples and continued to sample tmpfs with statvfs.

With seven changed-line candidates, `--scope files` without a risk declaration returned a policy error. With `--risk lifecycle --scope files` the same test fixture listed 15 candidates, selected two workers and a 30-minute limit, and accepted SIGTERM after cargo-mutants started. The wrapper returned exit 1 with “mutation run interrupted” and its disk scratch parent was empty. The interrupted run supplied cancellation evidence.

Interrupting the initial ad-hoc target implementation left its directory behind. After the fix, the same foreground process-group SIGTERM test returned exit 143 and the target was absent. The child created an artifact and announced its path before the signal, proving cleanup of a populated target.

### Routine one-file proof

The wrapper's `--check-proof` mode counted one product Rust file and seven candidates in 0.468 s. It created zero workspace copies and used zero sampled additional tmpfs bytes. The committed regression `ptyhost::redraw::tests::a_codex_erase_from_home_proves_a_repaint_across_read_boundaries` checks the fix's behavior when Codex clears from home across reads.

For the proof, a fresh disk checkout kept the new tests and temporarily replaced the implementation with the parent revision. `cargo test --offline --lib -p yulius-contracts ptyhost::redraw::tests::a_codex_erase_from_home_proves_a_repaint_across_read_boundaries -- --exact` failed at “split at 0” with exit 101 in 29.552 s on a cold target. Restoring the committed implementation made the same command pass its single test with exit 0 in 8.828 s. The three measured commands took 38.848 s combined. With the rollback and restoration already recorded during development, the extra eligibility check took 0.468 s. The targeted test supplied the behavioral proof.
