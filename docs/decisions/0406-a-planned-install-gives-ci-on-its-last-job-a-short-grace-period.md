# 0406. A planned install gives CI on its last job a short grace period

## Status

Accepted for [SPEC-573](../../.ai/specs/SPEC-573-reduce-repeated-ci-work-across-planned-y.md).

## Context

Startup recovery resets jobs of every running gate. Four previously passed checks in the November 20 install ran again for 6.49 summed job-minutes. A terminal run retains its verdict and can supply the usual merge evidence.

## Decision

During the first 30 seconds of the existing task drain, give a running CI gate time to finish if it already has an executed pass, no queued jobs and at most one job still running. This includes finalizing a verdict. Check again on the update loop's normal cadence. A slow task or deploy consumes the same interval. The deadline belongs to the persisted update phase and survives a daemon restart.

Continue without waiting bypasses CI subject to the existing deploy guard. A same-build restart proceeds under its existing rules. A failed CI-state read falls back to the ordinary install. This observation grants no capacity and imports no verdict.

## Alternatives

Waiting for every running gate increased both replayed work and delay in a saturated fixture. Holding new admissions would defer other projects. Copying individual passes would need a description of each job's mutable inputs and required outputs. The gate stores the command and commit, while the runner also reads current grants, quarantine, environment, cache and toolchain. Arbitrary shell commands can depend on services or files outside Git. A repeated exit code cannot attest to their availability.

## Consequences

The old executor can finish a gate with its current grants, outputs and siblings. A final job that runs beyond the deadline still undergoes full recovery, and concurrent work can lose an additional interval of service. The update can wait until the first normal observation at or after the deadline. Helper and deploy latency are measured separately in the specification.
