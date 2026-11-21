---
yulius-task: ba2b5b3f-2b04-4241-8917-82815c6f57af
status: in-progress
---

# SPEC-600 Disk scratch and change-sized mutation testing

## Goal

Keep workspace copies and temporary Cargo targets on disk. Give routine changes a short, line-scoped campaign and preserve mutation evidence at high-risk boundaries.

## Context

On 2026-11-21 the owner measured 105 minutes waiting for mutation tests in 22 Codex sessions (16% of agent time). Three sessions held 231 GiB on a 377 GiB host. The existing workspace peer baseline and outcome accounting are specified in SPEC-533 and SPEC-569. Parallel task #584 supplies independent benchmark evidence.

## Requirements

- **MTS-001** Give agent and CI build commands private disk scratch. Reclaim finished campaigns and run-owned alternate targets. Bound each contained session's ordinary /tmp at 1 GiB. A write past capacity returns ENOSPC.
- **MTS-002** Default to changed lines with one worker and a 10-minute campaign limit. At 16 listed changed-line mutants or an explicit risk declaration (authorization, persistence or process lifecycle), permit the wider budget of 30 minutes and at most two workers within the existing resource ceilings. Risky changes exercise affected integration tests, defaulting to all when no filter is supplied. Explicit file scope covers the touched Rust files. Record scope, trigger, limits, baseline and outcomes. Follow-up fixes use the reviewed head. A routine one-file change with one behavior and 1–8 changed-line candidates may use the list-only eligibility check with a recorded failing rollback and passing restoration.
- **MTS-003** Put the rule in the repository and its bootstrap workflow, skill and review guidance. Existing external repositories receive edits only through their own tasks.
- **MTS-004** Record elapsed time and measured memory before and after on a real mutation run. Preserve comparable test and mutation outcomes, distinguish scratch from process memory, and identify measurement limits.

## Plan

- [x] Specify scratch ownership and the mutation budgets
- [x] Bound session tmpfs and provide run-owned disk scratch
- [x] Apply change-sized mutation scope and clean campaign build trees
- [x] Install the same policy through bootstrap and verify its evidence
- [x] Reap interrupted builds and verify current-run baseline evidence
- [x] Verify the rollback-proof path for a routine one-file fix
- [ ] Measure the real campaign and deliver the reviewed change

## Acceptance criteria

- A contained run sees the documented capacity and a disk build path. Concurrent runs own distinct scratch. Finishing or failing a build releases its copies while retaining diagnostic results.
- A small diff uses a single job. The size boundary, declared risk and smaller CPU or memory budgets choose reproducible limits. Baseline failures, misses, timeouts and unreached mutants remain visible.
- Fresh bootstrap instructions describe the same thresholds and reviewer evidence. The measured result and reviewed head reach dev through Yulius CI.

## Test plan

Test the plain and joined bubblewrap arguments and the actual mount where user namespaces are available. Exercise scratch lifetime on successful, failing and bounded campaigns, the policy boundaries, and the real workspace peer fixture. Check the built-in catalog guidance and repository contracts. Compare a fixed change before and after with the pinned tool.

## Decisions

Keep /tmp for small temporary files. Allocate build scratch per run and per campaign, retaining only the campaign report. Use changed-line mutant count to budget builds and a declared risk to broaden the tests for those lines. Preserve the existing workspace baseline for every verdict.


## Evidence

The original containment argument tests failed when required to show a size on /tmp. The budget test failed when eight changed-line mutants were required to use one worker. All 46 process-host unit tests passed after the mount change. The ownership test passed on the quota-accounted scratch location for both agent and CI launches. A disposable Linux container reported exactly 1,073,741,824 bytes and returned ENOSPC to a 1,073,741,825-byte allocation. Nested bubblewrap was denied by this session, so the live contained-agent test skipped its mount assertions.

The real xtask peer fixture rejected a broken baseline, reported two missed mutants, then caught both after the peer asserted the result. A file-scope run included an unchanged function and caught all three. A timed campaign reported unreached mutants. Forcing the fixture to check its ordinary temp root failed while it saw the build directory. Passing the temporary Cargo configuration restored /tmp for nextest while the copies stayed on disk. The catalog guidance test and the disposable Cargo target success/failure check passed.

For fresh projects the onboarding prompt and infra-bootstrap workflow point to the rule in the built-in infra-bootstrap skill. That skill installs the thresholds, disk scratch and evidence requirements into the project's own guidance and mutation command. The bundled delivery, TDD and review skills use the same thresholds. Changes to existing external repositories belong to tasks in those repositories.

### Matched real mutation runs

Task #584 replayed the full yulius-contracts crate and the redraw change and regression test from a643b69064f95202077e35dbe6fc4c7c92b1fa01. Both runs used fresh targets, six CPUs, a 24,576 MiB budget, Rust 1.99.0, cargo-mutants 27.1.0 and nextest 0.9.146. Both baselines passed the same 56 tests, both caught all seven changed-line mutants, and both left zero scratch entries after exit.

| Runner | Workers | Wall time | Peak sampled session tmpfs growth | Peak scratch allocation | Maximum single-child RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| Before, 14e01f438 | 2 | 39.611 s | 903,884,800 B | 904,015,872 B on tmpfs | 874,444 KiB |
| Disk campaign, fb9d5a07b | 1 | 127.762 s | 0 B | 581,419,008 B on ext4 | 849,368 KiB |

The samples used statvfs at about 200 ms intervals and getrusage for child RSS. A disk-only control with the old two-worker settings took 50.424 s. A disk-only one-worker control took 77.790 s. A preliminary disk run took 120.198 s. These sequential trials shared a loaded host; the one-minute load average reached 287 during the fb9d5a07b replay. The measured campaign reduced tmpfs use and took longer on this change. The list-only rollback-proof path addresses the fixed build cost for eligible routine changes with existing behavioral evidence.

### Lightweight proof and review corrections

On the same real redraw fix, `scripts/dev mutants --check-proof --base HEAD^` listed the seven eligible candidates in 0.572 s and started zero workspace builds. With the new regression test present, replacing only the implementation of Redraw::process with its parent revision made `cargo test -p yulius-contracts a_codex_erase_from_home_proves_a_repaint_across_read_boundaries` fail at split 0 (exit 101, 29.394 s including cold compilation). Restoring the implementation passed the identical test (exit 0, 7.637 s). Eligibility plus both commands took 37.603 s. A previously recorded equivalent rollback and restoration require only the eligibility check at review.

Independent review of fb9d5a07b reproduced a retained Cargo target and a live backend after SIGTERM with a foreground PTY. The PTY regression failed with a retained target before the fix, then read interactive input, forwarded termination, reaped the child and observed an empty scratch parent. The exact reviewed runner also credited a stale caught.txt when the current process ignored TERM; the regression failed after 8.10 s on that head. Both discovery and execution now keep a dedicated POSIX session, use the campaign deadline and escalate after three seconds. The current run rotates old reports and requires its own recorded successful baseline. Its summary includes the filter, baseline state and mutant test-phase timeout. The pinned tool applies its 300 s timeout to testing; the whole-campaign deadline covers discovery, builds and tests.
