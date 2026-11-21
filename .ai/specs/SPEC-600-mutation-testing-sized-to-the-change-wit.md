---
yulius-task: ba2b5b3f-2b04-4241-8917-82815c6f57af
status: in-progress
---

# SPEC-600 Disk build scratch and by-hand mutation checks

## Goal

Keep workspace copies and disposable Cargo targets on disk. Use by-hand mutation checks for every delivery. Run a project's mutation command only when the task asks for that campaign by name.

## Context

On 2026-11-21 the owner measured 105 minutes waiting for mutation tests in 22 Codex sessions (16% of agent time). Three sessions held 231 GiB on a 377 GiB host. Later that day the owner replaced the proposed size thresholds with the instance's by-hand policy and requested its exact wording from the installed skills. Parallel task #584 supplied the independent before-and-after measurements. [SPEC-601](SPEC-601-mutation-testing-sized-to-the-change-wit.md)

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
- [x] Deliver the by-hand mutation policy and bootstrap
- [x] Deliver disk build scratch, the tmpfs cap and on-request campaign cleanup
- [ ] Deliver disposable Cargo targets and request staging

## Acceptance criteria

- Concurrent agent and CI runs own separate disk scratch. Completed and forcibly ended runs release their copies. A session's ordinary /tmp reports the documented limit and a write past it fails with ENOSPC.
- The catalog and repository tell authors and reviewers when and how to check their tests by hand. The named on-request command uses disk scratch. An installed bootstrap carries the same rule.
- The spec records real elapsed time, tmpfs and disk allocations, limitations, and by-hand red/restored-green checks. The reviewed head merges into dev on green Yulius CI, followed by one staging request.

## Test plan

Exercise plain and joined bubblewrap arguments and the actual mount where user namespaces are available. Check scratch lifetime on success, failure and forced termination, including a concurrent live run. Verify Cargo jobserver descriptors and interactive and cached cancellation. Run the affected catalog, onboarding, process-host and repository contracts. Undo targeted behaviors to prove their tests fail and restore them before delivery.

## Decisions

ADR-0439 replaces the delivery default of ADR-0347 with the owner's by-hand rule. Disk scratch is private to the attempt and the command. Keep the matched workspace baseline for explicitly requested campaigns.

## Evidence

Delivery is split into three pull requests. The first carries the by-hand policy and bootstrap. The second carries the session limit and disk-backed campaign scratch. The third carries the disposable Cargo target launcher. Runtime observations below were collected on the implementation branch. The task requests staging once after all three merge.

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

Review of 18b5c6cd4 confirmed host recovery and found that the command's separate TempDir cleanup still failed on a read-only dependency. The command now uses the same pinned-directory cleanup from yulius-sandbox on normal completion and on early return. A filesystem test, with no campaign process, failed with EACCES on the former command cleanup. Switching it back to remove_dir_all by hand reproduced the failure. Restoring the shared cleanup removed both test directories before returning and preserved an external symlink target. All 59 host unit and 16 supervision tests passed, as did the two containment harness entry points with the previously recorded namespace limitation. Clippy passed for the command, sandbox and process host.

Review of 35d37017c found that terminal-generated Ctrl-C reached the foreground Cargo process group and bypassed the wrapper's cancellation handler. A real PTY test observed exit 130 and a surviving worker that ignored SIGINT and SIGHUP. The wrapper now holds the leader waitable until the disposable build group has no live workers. It also handles frontends that translate interruption to an ordinary exit status. A bounded shutdown that cannot confirm exit reports the retained target for session cleanup. Undoing the group check by hand failed both terminal-interrupt tests. The restored tests show the worker socket closed before wrapper return, the original exit code and an empty scratch parent. The failed-observation fixture retained its target and executed Cargo once. All 26 compiler-cache tests passed on Linux. The shared Rust cleanup passed Clippy on the aarch64-apple-darwin target. Python's older macOS kqueue exit path was not run on a Mac.

Review of 7815fa4c5 found that restoring a hung-up PTY masked the HUP exit status or the unconfirmed-shutdown diagnostic. Closing the real PTY master reproduced both failures. Restoring handling of terminal loss returned 129 after worker exit and preserved the build target when exit could not be verified. Undoing that handling by hand failed both checks again. All 28 compiler-cache tests passed after restoration.

The reviewer also found that the campaign command dropped its scratch guard after an unconfirmed stop. A command-free ownership test failed on the old handling. The command distinguishes an unconfirmed shutdown from a reaped error, reports the retained directory, and keeps the leader waitable through worker observation even on ordinary exit. An ordinary process fixture exited 7 and left a live worker on the previous implementation. The corrected supervisor stopped the worker and preserved 7. A stalled process-list fixture previously exceeded its observation deadline and now returns a timeout. These checks used filesystem and ordinary subprocess fixtures. Linux worker enumeration includes each thread. A fixture with a zombie main thread and a live worker reproduced an early return with the worker socket still open. Undoing thread enumeration and uncertain-shutdown retention by hand failed their two regressions, and restoring them passed. Clippy passed for all xtask targets on Linux and aarch64-apple-darwin.

Review of b7e4e88af reproduced a live nextest test worker outside Cargo's initial process group after the launcher had returned 143 and discarded its target. Foreground and cached noninteractive fixtures using that process layout both failed on the reviewed code. The Linux disposable-target launcher now adopts orphaned workers as a child subreaper, ends them and waits until it has no children. By-hand undo of the adopted-worker check failed both regressions. On systems without Linux subreapers, an uncertain terminal-session change preserves and reports the target. The Cargo launcher is tracked separately from campaign scratch and the by-hand delivery policy.

First-PR review of 0b19c4970 reproduced deletion under a newer scratch receipt version and retention under a reused host PID when status was absent. Both added regressions failed on that head. Receipt writers now stamp RECORD_VERSION and readers reject newer formats before opening a build tree. The receipt records the host's kernel birth identity. Missing status recovery distinguishes a confirmed process-group or birth mismatch from a failed observation. A genuine group-leading host and an unknown birth keep their builds. Older runs without a receipt remain readable. By-hand undo of the newer-version rejection and birth comparison failed both regressions, and restoration passed them. All 61 host unit tests and 16 supervision tests passed, alongside the two containment harness entry points. Host Clippy passed for Linux and the macOS compilation target.

Review of 70ebaed7f shifted only the recorded wall time and reproduced reclamation while the real host and child were still running. The regression covers inspection, explicit termination and the reaped-host waiter. On the same boot, recovery checks the receipt's host birth and group and the child's session and birth before deleting scratch. Unconfirmed termination reports failure and retains the tree. An optional child birth in status.json distinguishes a reused child PID after restart. Older statuses without birth retain scratch while their recorded session leader could still own it. The regression also verifies a replacement child survives termination of the old record. Undoing the recorded-exit guard by hand failed the regression with termination reporting success while ownership was live. Restoration passed it. All 56 contract tests, 62 host unit tests and 16 supervision tests passed. The two containment harness entries passed with the earlier limitation. Clippy passed for the host and contracts on Linux and the macOS compilation target.

The independent policy review approved 9e9d04340. Its documentation finding was addressed by identifying the superseded between-round campaign clause in ADR-0362 and ADR-0439. All seven affected catalog passages matched the installed wording byte for byte. All 43 skill, onboarding and workflow tests passed on this delivery. Repository contracts and formatting passed. Vidoc was disabled.

The by-hand policy and bootstrap merged in PR #1636 at 3b72387f1. Yulius CI run cee3a80d-33ec-4cbe-bc9e-dd0f124ac72f passed on its own head c9bf17b1b. [Delivery evidence](https://yulius.quartio.ai/api/tasks/ba2b5b3f-2b04-4241-8917-82815c6f57af/delivery/evidence/3b72387f15de9d60d61af21555e8ab3fac9090bf)

On the disk runtime branch, an ordinary child that started a second POSIX session survived its leader’s exit on the earlier supervisor. The new Linux monitor registers as a child subreaper and waits for the last adopted process before confirming cleanup. Undoing subreaper registration by hand reproduced the live worker. A missing shutdown acknowledgement preserves the build tree. The filesystem regression removed a populated mode-000 dependency and preserved an external symlink target’s contents and mode. By-hand removal of permission recovery failed with EACCES. The Darwin descriptor-relative recovery algorithm passed the same ordinary-permissions test when selected on Linux. Native macOS was unavailable.

The restored Linux process fixture checked both normal exit and cancellation with a worker in a separate session. All five supervisor tests passed. An ordinary command inherited its jobserver descriptor, and cancellation recorded before launch prevented a command from starting. The scoped runtime run passed 56 contract tests, 62 host unit tests, 16 supervision tests and both integration tests for contained delivery and the CD agent join. Both containment harness entry points passed with the nested namespace limitation recorded above. Clippy passed for the four affected crates on Linux and the aarch64-apple-darwin target.

Independent review at e2d183d1b reproduced a detached child on the portable supervisor. Its original session was empty even though a worker still held the socket. The portable supervisor now checks that every remaining process group has a surviving process identity from before the command. Unknown groups retain the build and produce an unconfirmed-shutdown error. The ordinary detached-worker test failed with the earlier command's exit status 7. Undoing the group comparison by hand reproduced the same failure. After restoration, all six process checks passed on Linux, including the portable path for normal leader exit and cancellation. macOS uses the kernel's process unique identifier. An unrelated new group can conservatively retain local scratch. [XNU process identity API](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/proc_info_private.h)

Independent review approved the disk runtime at 917559091. The reviewer independently passed six supervisor tests and two filesystem tests and verified that both forms of detached-worker shutdown retain scratch on the portable path. Exact-head CI and merge are recorded in the task’s delivery evidence.
