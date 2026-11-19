---
yulius-task: 54c4a0e3-d269-47d0-9355-2c4ed893ac81
status: in-progress
---

# SPEC-550 Wait for actual host exit when settling a stopped run

## Goal

Stop a detached run left by an earlier server and verify that its host has exited before recording the run as lost.

## Context

Issue #1449 reports the host still running after stop on CI run `0c4393e8`. PR #1313 fixed #1190 by recognizing a zombie as exited. A live ptyhost can also linger to deliver the child's exit frame. The CI log contains no process state or run-directory inspection to distinguish those cases. On the base revision the supervisor inspected the run before requesting termination, and the adapter inspected it again. If the child exited between those inspections, the second read preferred its exit record and termination returned success while the host was still serving.

## Requirements

- **HST-001** Exercise an actual live host between the decision to terminate and the proof of termination.
- **HST-002** Check host exit under a bound. Preserve identity checks, failure on a surviving target and the single terminal attempt.

- **HST-003** On Linux, a host whose main thread has exited still counts as live while another thread runs. An exited, unreaped host counts as gone.

## Plan

- [x] Specify the delayed host exit regression
- [x] Reproduce the failure with a controlled host
- [x] Verify settlement against actual host exit
- [ ] Prove whole-process exit when the Linux leader dies first
- [ ] Review and deliver the fix to dev

## Acceptance criteria

- **AC-1** Delay at the termination boundary until the real host publishes its child's exit and observe whether the original assertion fails (HST-001).
- **AC-2** With the same delay, the stopped task retains its server-lost status, the one attempt is lost, and the host has exited when the loss is recorded (HST-002).

- **AC-3** A Linux thread-group leader in state Z with a worker held on a pipe counts as live until the worker exits (HST-003).

## Test plan

Run the existing stop integration test with a controlled child exit during termination. Check the host independently at loss recording and wait on a bounded exit condition. Run the neighboring start and zombie cases and the existing termination failure and identity tests.

## Decisions

Use the detached host's existing exit-frame linger and a scoped test hook. The previous zombie test covers a child that has already exited. This probe covers a host that is still executing after its child exits.

Explicit termination now reads the versioned process identities from `status.json` and uses the existing boot, PID, start-time and leadership checks followed by the two-second wait for survivors. Normal outcome inspection still prefers the child's exit record. A malformed status fails explicit termination. An absent record retains its previous behavior. This applies the existing [ADR-0050](../../docs/decisions/0050-a-run-ends-only-when-its-host-is-proven-gone.md) contract.

On Linux a PID file descriptor becomes readable when the last thread exits. A zero-time poll fits the existing bounded termination loop and recognizes an unreaped zombie. [ADR-0390](../../docs/decisions/0390-linux-host-exit-is-observed-for-the-whole-process.md)

## Evidence

- At `ae1da7d8f`, the probe delayed termination until the detached host published its child's exit. The original integration test reached its final assertion and failed with “the run's host still runs” (nextest run `c0b53058`, 3.156 s). The adapter test for malformed status also failed on the original implementation.
- Seven focused nextest cases passed (run `816cb2d3`), covering ordinary stop, delayed host exit at the loss transition, restart, an unreaped zombie, surviving termination, stale process identity and an unreadable record.
- `scripts/dev quick` passed. `scripts/dev security` reported Vidoc disabled for the project.
- Independent review approved `252fbd507`. The reviewer observed that deleting the status file between inspection and termination discards its identities on the base revision too. This separate case was recorded on #1449.

- Stress on the first fix failed in iteration 1 of 10. A diagnostic run captured a zombie leader, two threads in the host and a second `waitpid(WNOHANG)` returning `StillAlive` at the loss transition. A syscall trace captured the kill and the host becoming waitable after the loss. #1190's fixture had a single-threaded, fully exited child.
- The deterministic Linux worker test failed at `908a53098` with “the worker was still running.” With the PID file descriptor check, all three process unit tests passed, including the existing fully exited zombie case.
