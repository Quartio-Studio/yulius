---
yulius-task: ff25261d-6a45-4346-8f6a-949e2427e082
status: done
---

# SPEC-601 Measuring disk-backed mutation scratch and verifying by-hand delivery checks

## Goal

Verify disk-backed build scratch, an enforced session tmpfs limit, and the owner’s November 21 rule for by-hand delivery checks and explicitly requested campaigns.

## Context

The owner measured 105 minutes of mutation waits across 22 sessions and 231 GiB of tmpfs use across three sessions on November 21. The current wrapper already selects committed Rust lines against the merge base. It starts at two workers on this host, allows 30 minutes and inherits the session's temporary directory. Tasks #583 and #584 received the same request. Task #583 owns the runtime, tooling, guidance and bootstrap changes. This task owns the independent measurement and verification record. After the full measurements below, the owner superseded the change-sizing request. The installed skill passages are the wording source for the new rule. The active follow-up campaign was interrupted when that instruction arrived. No campaign was started under the new instruction.

## Requirements

- **MUT-001** Agents and applicable CI jobs allocate mutation and ad-hoc build scratch on disk. Finished runs remove owned scratch. Session tmpfs has an explicit capacity and writes beyond it report a filesystem error.
- **MUT-002** For every delivery, undo the behavior the new tests hold, observe the failure, restore the behavior and record the result. Run a project mutation campaign only when a task requests it by name. Re-review checks the test evidence and each prior finding.
- **MUT-003** Preserve the installed mutation, re-review and staging passages in the four catalog skills. Align the project guidance, generated skills and tests. An ADR supersedes the per-PR default in ADR-0347 and retains ADR-0141’s same-PR test repair. After a merge, one staging request completes delivery. Other repositories take changes through their own tasks.
- **MUT-004** Compare elapsed time and scratch memory on the same real Rust change with the old and delivered tools. Identify the workload, scope and limitations. Use Yulius delivery evidence for the checked revision’s green dev merge and its one staging request.

## Plan

- [x] Measure a real mutation run on the current session tmpfs
- [x] Replay the same change with disk scratch and the new mutation budget
- [x] Measure the one-file rollback and restoration proof
- [x] Verify disk cleanup and the owner’s by-hand policy against the installed skills
- [x] Obtain independent approval of the measurement and policy evidence

## Acceptance criteria

- **AC-1** The measured runs use the same changed product lines and tests. Each reports its mutant outcomes, elapsed time, peak tmpfs allocation and scratch remaining after exit.
- **AC-2** The containment and CI launch paths apply the declared bound and scratch location. A process at the limit observes the documented error. The wrapper's normal and incomplete outcomes have cleanup evidence.
- **AC-3** The repository and shipped bootstrap carry the installed by-hand, campaign opt-in, re-review and staging rules. The on-request mutation tool remains available. Green Yulius CI and a dev merge identify the delivered revision.

## Test plan

Replay the terminal-redraw fix from PR #1586 in its real `yulius-contracts` crate. Limit the benchmark workspace to that crate so the baseline can run safely on the affected host. Measure tmpfs allocated blocks and elapsed wall time around the wrapper. Verify the scratch filesystem and inspect temporary directories after each exit. Check the delivered launcher, CI and bootstrap tests and Yulius delivery evidence.

## Decisions

The November 21 owner instruction replaces the earlier campaign-sizing policy. A delivery records the by-hand rollback, failing test and passing restoration. The command `scripts/dev mutants` remains available on explicit request. The author makes one staging request after merge and finishes the task. The four installed skills specify the wording for the catalog.

Keep product delivery in #583 and measure the complete affected crate on #584. Single-run timings describe this shared host. Report compiler RSS and tmpfs use separately.

## Evidence

### Method

Replay the complete `crates/yulius-contracts` directory from the parent of `a643b69064f95202077e35dbe6fc4c7c92b1fa01`, then apply that commit's `src/ptyhost/redraw.rs` change including its regression test. The benchmark Git workspace contains this crate as its sole member, its original workspace dependency versions and lock, and two commits. The wrapper runs `xtask mutants --base HEAD^` on the replay. Each campaign starts with fresh target directories and uses Cargo in offline mode, Rust 1.99.0, cargo-mutants 27.1.0, nextest 0.9.146, six CPUs and a declared 24,576 MiB budget.

Sample allocated blocks on the session's private /tmp using `statvfs` at roughly 200 ms intervals. Record the peak change relative to immediately before launch. Measure elapsed time from launch through exit and the largest child-process RSS with `getrusage(RUSAGE_CHILDREN)`. RSS is the high-water mark of the largest individual child.

### Filesystem limit

A disposable container on the development host ran `dd if=/dev/zero of=/tmp/build bs=1048576 count=1025` against a tmpfs mounted with `size=1073741824`. The first 1,024 MiB were written. The next write returned “No space left on device” and dd exited 1. The current agent's namespace refused nested bubblewrap. The container verified Linux's write behavior at the configured mount size. At `fb9d5a07b`, `cargo test -p yulius-ptyhost --lib containment::tests` passed nine tests including plain and joined mount arguments. `each_agent_and_ci_run_owns_only_its_build_scratch` passed for distinct agent and CI directories and verified guard cleanup.

### Disk-scratch measurements under the original request

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

The one-worker budget in this historical trial belonged to the superseded sizing proposal. The updated wrapper received an ext4 `YULIUS_BUILD_SCRATCH` directory while ordinary `TMPDIR` remained `/tmp`. The full campaign used 88.151 s more elapsed time in this trial. The shared host's one-minute load average reached 287 during the updated run, and its unmutated build took 33 s compared with 17 s in the original run. The workload and host conditions limit timing comparisons to these trials.

### Scratch-only control

On the original wrapper, setting only `TMPDIR` to the private ext4 directory kept the two-worker default. This run caught the same seven mutants in 50.424 s with zero sampled growth on the session tmpfs. Its maximum observed disk allocation was 1,037,381,632 bytes. This control isolates the effect of relocating scratch on tmpfs allocation. Each timing is one observation on a shared host.

### Command cleanup and validation

With `scripts/cargo --scratch-target metadata --no-deps --format-version=1`, Cargo reported a target within the supplied ext4 scratch root. The target was absent after exit. An invalid metadata format returned exit 1 and also left the parent empty.

All completed campaigns in this record preceded the owner’s replacement instruction. The timing table uses completed, sequential campaigns. Discarded exploratory runs exposed a Cargo configuration parse error, overlapping report rotation and an incomplete disk-usage sample while Cargo removed an object. The observer subsequently skipped partial disk samples and continued to sample tmpfs with statvfs.

After the owner changed the rule, the active follow-up campaign at `fd1f761f8` received SIGTERM and returned exit 1 with “mutation run interrupted”. Its disk scratch parent was empty. Its partial result is excluded from the table.

Interrupting the initial ad-hoc target implementation left its directory behind. At commits `fd1f761f8` and `f82f0a1e6`, the same foreground process-group SIGTERM test returned exit 143 and the target was absent. The child created an artifact and announced its path before the signal, proving cleanup of a populated target.

### By-hand regression evidence

A fresh disk checkout retained the new test and restored the parent implementation of `Redraw::process`. `cargo test --offline --lib -p yulius-contracts ptyhost::redraw::tests::a_codex_erase_from_home_proves_a_repaint_across_read_boundaries -- --exact` failed at “split at 0” with exit 101 in 29.552 s on a cold target. Restoring the fix made the identical command pass its single test with exit 0 in 8.828 s.

At `f82f0a1e6`, `cargo test --offline -p yulius-ptyhost --lib` passed all 49 tests, including plain and joined mount arguments, run-owned scratch, and cleanup after forced termination with another attempt still live.

### Bootstrap and repository checks

At `f82f0a1e6527fa56415a617e51c17dee36daa5b1`, all seven designated passages matched the installed delivery-pipeline, om-code-review, om-fix and test-driven-development skills. This included the by-hand paragraph, the relevant re-review sentences and the delivery-pipeline staging bullet. Development guidance and the source, Claude and Codex copies of the repository verify/review skills also carried the rule. The onboarding skill contained the by-hand default. ADR-0439 superseded ADR-0347’s delivery campaign default and preserved ADR-0141’s same-PR repair rule.

In an isolated copy of the committed policy files, restoring only om-code-review’s former campaign paragraph made the comparison fail with six of seven passages present. Restoring the committed paragraph made it pass with seven of seven. The original implementation head `fd1f761f8` failed all seven comparisons.

`spec_check` validated this spec. `scripts/dev quick` passed the repository contracts and Rust formatting checks. Before the owner’s new instruction, the evidence branch’s mutation command found zero changed Rust lines. Vidoc was disabled for this project.

The independent reviewer approved measurement head `3b942ac5150122bc782bee399984166c46755b15` after verifying the historical trial provenance, the seven installed passages and the SIGTERM cleanup at both named revisions. Yulius records the actual CI run, dev merge and staging request for the evidence branch on task #584 and the implementation on task #583.
