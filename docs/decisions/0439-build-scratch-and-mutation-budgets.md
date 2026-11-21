# 0439. Build scratch lives on disk and changed lines determine the mutation budget

## Status

Accepted for SPEC-600.

## Context

Workspace copies under the session tmpfs consumed 231 GiB across three sessions. Even short diffs launched independent workspace builds.

## Decision

Give contained agents and CI jobs a run-owned directory inside the sandbox disk quota for disposable builds. Cap the ordinary /tmp mount at 1 GiB. Use one mutation worker for up to 10 minutes on routine diffs. At 16 changed-line mutants, or when the change alters authorization, persistence or process lifecycle, allow up to two workers for 30 minutes. The latter boundaries require their relevant integration tests alongside the workspace baseline. Keep the diff and individual verdicts in the evidence.

## Alternatives

Increasing the session memory allocation increases the maximum loss per concurrent build. A shared mutable target can confuse the baseline and mutant binaries. An unbounded campaign on a changed file spends the review budget on untouched behavior.

## Consequences

A full /tmp fails writes with ENOSPC. Parallel runs use independent writable disk targets. Tests that exceed the campaign budget produce incomplete evidence and keep their outstanding mutants visible.
