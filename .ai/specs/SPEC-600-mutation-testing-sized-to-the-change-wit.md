---
yulius-task: ba2b5b3f-2b04-4241-8917-82815c6f57af
status: in-progress
---

# SPEC-600 Disk build scratch and by-hand mutation checks

## Goal

Keep workspace copies and disposable Cargo targets on disk. Use by-hand mutation checks for every delivery. Run a project's mutation command only when the task asks for that campaign by name.

## Context

On 2026-11-21 the owner measured 105 minutes waiting for mutation tests in 22 Codex sessions (16% of agent time). Three sessions held 231 GiB on a 377 GiB host. Later that day the owner replaced the proposed size thresholds with the instance's by-hand policy and requested its exact wording from the installed skills. Parallel task #584 supplied the independent before-and-after measurements.

## Requirements

- **MTS-001** Give contained agent and CI build commands private disk scratch. Reclaim finished campaigns and run-owned alternate targets, including after cancellation. Bound each contained session's /tmp at 1 GiB. A write past capacity returns ENOSPC.
- **MTS-002** Undo the lines a delivery's tests should hold, observe the tests fail, restore the lines and record the failure. The reviewer may undo a doubtful line. Keep ADR-0141's requirement to fix a test that cannot fail in the same PR. Preserve scripts/dev mutants as an on-request command.
- **MTS-003** Carry the installed policy into the four catalog skills, local instructions and skills, review and verification documents and their enforced contracts. The bootstrap gives this guidance to fresh projects. Existing external repositories receive changes only through their own tasks. After merge, request staging once and end the task.
- **MTS-004** Record elapsed time and measured memory before and after on a real mutation run, distinguishing tmpfs from process memory. Use the independent measurements. Run no new campaign for this delivery.

## Plan

- [x] Bound session tmpfs and provide run-owned disk scratch
- [x] Reap interrupted builds and verify current-run baseline evidence
- [x] Record independent before-and-after measurements
- [x] Install the instance's by-hand delivery policy and bootstrap contract
- [x] Preserve Cargo cancellation and inherited jobserver tokens
- [x] Reclaim attempt scratch after forced host termination
- [ ] Verify by hand and deliver the reviewed change

## Acceptance criteria

- Concurrent agent and CI runs own separate disk scratch. Completed and forcibly ended runs release their copies. A session's ordinary /tmp reports the documented limit and a write past it fails with ENOSPC.
- The catalog and repository tell authors and reviewers when and how to check their tests by hand. The named on-request command uses disk scratch. An installed bootstrap carries the same rule.
- The spec records real elapsed time, tmpfs and disk allocations, limitations, and by-hand red/restored-green checks. The reviewed head merges into dev on green Yulius CI, followed by one staging request.

## Test plan

Exercise plain and joined bubblewrap arguments and the actual mount where user namespaces are available. Check scratch lifetime on success, failure and forced termination, including a concurrent live run. Verify Cargo jobserver descriptors and interactive and cached cancellation. Run the affected catalog, onboarding, process-host and repository contracts. Undo targeted behaviors to prove their tests fail and restore them before delivery.

## Decisions

ADR-0439 replaces the delivery default of ADR-0347 with the owner's by-hand rule. Disk scratch is private to the attempt and the command. Keep the matched workspace baseline for explicitly requested campaigns.

## Evidence

The original containment argument tests failed when required to show a size on /tmp. All 46 process-host unit tests passed after the mount change. The ownership test passed for both agent and CI launches. A disposable Linux container reported exactly 1,073,741,824 bytes and returned ENOSPC to a 1,073,741,825-byte allocation. Nested bubblewrap was denied by this session, so the live contained-agent test skipped its mount assertions.

### Matched real mutation runs

Task #584 replayed the full yulius-contracts crate and the redraw change and regression test from a643b69064f95202077e35dbe6fc4c7c92b1fa01. Both runs used fresh targets, six CPUs, a 24,576 MiB budget, Rust 1.99.0, cargo-mutants 27.1.0 and nextest 0.9.146. Both baselines passed the same 56 tests, both caught all seven changed-line mutants, and both left zero scratch entries after exit.

| Runner | Workers | Wall time | Peak sampled session tmpfs growth | Peak scratch allocation | Maximum single-child RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| Before, 14e01f438 | 2 | 39.611 s | 903,884,800 B | 904,015,872 B on tmpfs | 874,444 KiB |
| Disk campaign, fb9d5a07b | 1 | 127.762 s | 0 B | 581,419,008 B on ext4 | 849,368 KiB |

The disk campaign was measured on an intermediate implementation before the owner replaced change-sized campaigns with by-hand checks. The final on-request tool keeps the pre-existing worker and time settings. No elapsed time for a final-head campaign is claimed.

The samples used statvfs at about 200 ms intervals and getrusage for child RSS. A disk-only control with the old two-worker settings took 50.424 s, caught all seven mutants and used 1,037,381,632 B on ext4 with zero sampled tmpfs growth. A disk-only one-worker control took 77.790 s. A preliminary disk run took 120.198 s. These sequential trials shared a loaded host; the one-minute load average reached 287 during the fb9d5a07b replay. The measured campaign reduced tmpfs use and took longer on this change.


On that same historical redraw fix, task #584 kept the regression test and undid only the implementation. The test failed at “split at 0” in 29.552 s including cold compilation, then passed in 8.828 s after restoration. Those two commands took 38.380 s.

### Delivery checks by hand

Reverting the by-hand paragraph in the local verification skill made the repository policy test fail on that file. Restoring the paragraph passed the same test. The installed mutation, re-review and staging passages matched their relevant catalog copies byte for byte. All 22 skill tests passed. A workspace campaign already started under the original brief had stopped at the unmutated baseline with zero mutants tested. Its catalog test found a 1,541-byte workflow exceeding the existing 1,444-byte limit. The shortened bootstrap points to the owning skill. No campaign was started after the owner's policy change.

Removing the two tmpfs size arguments and the scratch ownership receipt by hand failed three host tests: the plain mount arguments, the joined mount arguments, and forced termination retaining its build tree. Restoring those lines restored the checks. Forcing the direct Cargo launcher to close inherited file descriptors made the jobserver token test fail with EBADF. Restoring inheritance passed the same test. The cached, noninteractive cancellation regression failed on the nested supervisors with a live Cargo child after termination. The corrected single supervisor reaped that backend and removed the disposable target. All 23 compiler-cache tests passed, including foreground terminal input and cancellation.

Requested campaigns retain reports separately from their disk workspace copies. Before the policy change, a real peer fixture failed its unmutated baseline, reported two misses, then caught both with the restored peer assertion. A timed-out command that left stale verdicts had previously claimed catches from its predecessor. Its regression failed on the previous runner and passed with report rotation, a verified current baseline and reaped cancellation. Security scanning is disabled for this project.


The process-host suite at the first review of the revised policy passed 49 unit tests and 16 supervision tests. The two containment test entry points passed with the live nested mount limitation recorded above. The supervisor reaped a real process-group leader after SIGKILL and removed its populated scratch while another attempt retained its build. Reconciliation retained a starting host, reclaimed a previous-boot attempt and refused to delete a replacement directory. Both shipped workflow tests and all 13 onboarding tests passed. The repository contracts and formatting passed.


Independent review of f82f0a1e6 found that the host's normal and early exits still used pathname-only TempDir cleanup. The added host teardown regression failed on that revision with “host removed another build tree, close=true”. The host now keeps the recorded device, inode and path in memory through exit. This also reclaims CI build trees when CI has already removed the run record. Both teardown paths preserve a replacement; an old host also preserves a new attempt's receipt at a reused run directory. Manually removing the two identity comparisons failed the host teardown, recovery and new-receipt tests. Restoring both comparisons passed all 52 host unit tests, 16 supervision tests and the two containment harness entry points. Clippy passed for all host targets. The selected CD agent-join and contained-delivery product integration tests both passed.

Review of 23e720535 found a delayed waiter could operate on a newer launch and deletion could follow a replaced pathname after checking its inode. Two deterministic regressions failed on that head. The server now captures the host PID, launch token hash and build receipt at the ready handshake. A delayed waiter checks only its captured ownership, waits for the old child when necessary and sends no signals. Startup failure recovery matches the token and PID before cleanup. The host registers the receipt through an open control-directory descriptor, so a reused control path receives only its own registration. Receipts stay with the protected run metadata. Recursive deletion uses the opened original directory and its child descriptors. The root ownership marker is removed after the payload. A concurrent launch keeps its marker and its build files.

Undoing the launch match and the descriptor-relative traversal by hand failed both the delayed-callback and the mid-cleanup directory-exchange tests. Restoring the changes passed all 56 host unit tests, 16 supervision tests and the two containment harness entry points. The ready-handshake test removed and recreated the control directory and confirmed that the old receipt still reclaimed only the old build. Clippy passed for every host target.

Review of d8ee4cbf1 found that clearing the advertised scratch erased its ownership marker and that CI could discard the receipt after a failed startup. The regression tests emptied and repopulated the exported directory before forced cleanup and killed a fixture host between publishing its receipt and reaching readiness. Both failed on that revision. The exported directory now sits below the ownership marker. The server pins the build directory and reclaims a confirmed pre-ready exit before returning the startup error. Undoing these two changes by hand failed three host tests, including both regressions. Restoring them passed 58 host unit tests, 16 supervision tests and the two containment harness entry points. Clippy passed for all host and xtask targets. Repository contracts and formatting passed.

Review of 8c1532d77 reproduced EACCES while reclaiming a populated mode-0555 dependency directory with service permissions. The regression drops discretionary-access overrides on its test thread, creates populated mode-0555 and mode-000 directories, and checks an external symlink target keeps its mode and content. It failed with permission denied on the reviewed code and again when owner-write restoration was undone by hand. Restoring access on the pinned directory passed the check.
