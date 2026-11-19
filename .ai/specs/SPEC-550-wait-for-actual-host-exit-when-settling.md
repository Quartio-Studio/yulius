---
yulius-task: 54c4a0e3-d269-47d0-9355-2c4ed893ac81
status: in-progress
---

# SPEC-550 Wait for actual host exit when settling a stopped run

## Goal

Stop a detached run left by an earlier server and verify that its host has exited before recording the run as lost.

## Context

Issue #1449 reports the host still running after stop on CI run `0c4393e8`. PR #1313 fixed #1190 by recognizing a zombie as exited. A live ptyhost can also linger to deliver the child's exit frame. The CI log contains no process state or run-directory inspection to distinguish those cases. The supervisor inspects the run before requesting termination, and the adapter inspects it again.

## Requirements

- **HST-001** Exercise an actual live host between the decision to terminate and the proof of termination.
- **HST-002** Check host exit under a bound. Preserve identity checks, failure on a surviving target and the single terminal attempt.

## Plan

- [x] Specify the delayed host exit regression
- [x] Reproduce the failure with a controlled host
- [ ] Verify settlement against actual host exit
- [ ] Review and deliver the fix to dev

## Acceptance criteria

- **AC-1** Delay at the termination boundary until the real host publishes its child's exit and observe whether the original assertion fails (HST-001).
- **AC-2** With the same delay, the stopped task retains its server-lost status, the one attempt is lost, and the host has exited when the loss is recorded (HST-002).

## Test plan

Run the existing stop integration test with a controlled child exit during termination. Check the host independently at loss recording and wait on a bounded exit condition. Run the neighboring start and zombie cases and the existing termination failure and identity tests.

## Decisions

Use the detached host's existing exit-frame linger and a scoped test hook. The previous zombie test covers a child that has already exited. This probe covers a host that is still executing after its child exits.
