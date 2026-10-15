---
yulius-task: 34fa1e0a-cc4f-4c63-b55f-205765d773f6
status: in-progress
---

# SPEC-154: CI jobs and CD runs record their CPU, memory and disk use as stage figures

Status: in-progress
Date: 2026-10-15
Owner: Yulius task #129 (part of task #118, SPEC-138)
Scope authorization: issue [#488](https://github.com/Quartio-Studio/yulius/issues/488), the
follow-up of [SPEC-137](SPEC-137-rich-measurements-of-every-stage-tasks-c.md): "Each CI job's and
CD run's span gets `cpu_ms`, `memory_peak_mb` and, where cheap, disk written. They are read from a
per-job cgroup (…) or the process host's accounting at the job's end, and recorded with the job's
end like the other figures. Hosts without cgroup v2 leave them unknown, never 0. The Pipeline
page's stage table and the waterfall tooltip show the p50/p90 of peak memory and CPU time."

## Goal

A CI job's `job` span and a CD run's `run` span carry what the job actually used: its CPU time
(`cpu_ms`), its peak memory (`memory_peak_mb`) and the bytes it wrote to storage
(`disk_written_mb`), measured by Yulius's process host when it reaps the job, written with the
job's end. People see the p50 and p90 of CPU time and peak memory beside the duration in the
Pipeline page's stage table, and one job's figures in its waterfall row.

## Context

- **Spans** (SPEC-137, ADR-0112): a CI job's span opens and ends through triggers on `ci_jobs`
  (`spans_ci_job_start`, `spans_ci_job_end`, migration 28); a CD run's through `cd_runs`
  (`spans_cd_run_insert`, `spans_cd_run_end`). The end triggers copy `exit_code` and `reason` into
  the span's `detail`. The figures a job prints (`::yulius-measure`) cannot use reserved names.
- **Process host** (`yulius-ptyhost`, SPEC-002 §7.1): one `ptyhost` process per run spawns the
  job (`/bin/sh` → bubblewrap when contained), observes its exit, kills what is left of its group
  and session, reaps it and writes `exit.json` / the `Exit` frame (`ExitRecord`). The app turns it
  into `RunExit`; `ci_watch` follows CI jobs and CD runs alike.
- **cgroups** (ADR-0039): a sandbox's CI jobs share its `ci/` leaf; only the `cpu` controller goes
  down to the leaves. Hosts without delegation (and every agent sandbox, where Yulius CI itself
  runs: `/sys/fs/cgroup` is not mounted) have no cgroup at all.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| USE-001 | The process host measures the job's process tree when it reaps it: CPU time (user + system of every reaped descendant), peak resident memory (the largest process's peak, the kernel's `ru_maxrss`) and, on Linux with task I/O accounting, the bytes written to storage (`ru_oublock` × 512). It is in the exit record (`exit.json`, `Exit` frame) as `usage`, absent when the child never ran or the reading failed. An `exit.json` without it (an older host) reads as unknown. | ptyhost test; contracts round-trip test |
| USE-002 | A CI job's end stores its use (`ci_jobs.cpu_ms`, `memory_peak_mb`, `disk_written_mb`, migration 30) and the end trigger adds them to the `job` span's figures in the same transaction; unknown values are absent from the figures, never `0` or `null`. A job Yulius killed (cancelled, timed out, over its output) has unknown use: its processes were reaped outside the host's chain (review round 1). | store trigger test; integration `local_ci` (a measured job, a killed job) |
| USE-003 | A CD run's end does the same on its `run` span (`cd_runs` columns, `spans_cd_run_end`). | store trigger test |
| USE-004 | `cpu_ms`, `memory_peak_mb` and `disk_written_mb` are reserved: a job's `::yulius-measure` cannot set them. | domain test |
| USE-005 | The stage summary gives each row the p50 and p90 of CPU time and peak memory of its spans that have them (`cpu_p50_ms`, `cpu_p90_ms`, `memory_peak_p50_mb`, `memory_peak_p90_mb`; absent when none has). | app unit test; integration `stages` |
| USE-006 | The Pipeline page's stage table shows CPU time and peak memory (p50 · p90) beside the durations, `—` when unknown; a waterfall row's tooltip, title and table show the span's own CPU time, peak memory and bytes written. | e2e `pipeline.e2e.ts` |
| USE-007 | docs/api.md "Stages", SPEC-137 and ADR-0127 describe the figures and where they come from. | docs |

## Plan

- [x] The process host measures a reaped job's CPU, peak memory and disk writes
- [x] CI jobs and CD runs store their use and their spans carry it
- [x] The stage summary gives the p50 and p90 of CPU time and peak memory
- [x] The Pipeline page and the waterfall show the use
- [x] Docs, ADR and changes updated
- [ ] Independent review approves the exact head
- [ ] Every gate green and the merge into dev armed on Yulius CI

## Acceptance criteria

- AC1 (USE-001, USE-002): a CI job run through the process host that burns CPU has `cpu_ms > 0`
  and `memory_peak_mb ≥ 1` on its `job` span (integration test in `crates/yulius/tests/local_ci.rs`).
- AC2 (USE-002, USE-003): a job or CD run ended without a measurement has none of the three
  figures in its span's detail (not `0`, not `null`).
- AC3 (USE-005, USE-006): the Pipeline page shows a job row's CPU time and peak memory p50/p90
  beside its duration, and a job's waterfall row its own figures (e2e).
- AC4 (USE-007): docs/api.md "Stages" and SPEC-137 list the new figures.

## Test plan

- ptyhost unit test: a child that burns CPU and allocates memory is measured; an exit record
  without `usage` deserializes (compatibility).
- Domain tests: conversion to MiB and milliseconds (rounding, unknown disk), reserved names.
- Store tests: the job and CD run end triggers add the figures, and leave them out when unknown.
- App unit test: summary percentiles of CPU and memory, rows without figures.
- Integration: `local_ci::a_job_records_the_cpu_and_memory_it_used` through the real process host;
  `stages::` summary over the API.
- e2e: `pipeline.e2e.ts` with the mock's figures; axe.
- Gates: `scripts/dev ci`, `scripts/dev security`, the e2e suite.

## Decisions

- **The process host's accounting, not a per-job cgroup** (ADR-0127). The host already reaps the
  job's whole tree, and one host runs one job, so `getrusage(RUSAGE_CHILDREN)` after the reap is
  the job's use on every host, with no cgroup layout change, no cleanup of per-job groups and no
  dependence on delegation (Yulius CI's own sandboxes have no cgroup filesystem, so a cgroup
  reading would be unknown exactly where the integration test runs). The cost: peak memory is the
  largest single process's peak, not the sum of processes at once (a parallel build's total is
  higher), and a process that escapes the job's reaping chain (a daemon reparented outside the
  containment) is not counted. Both are stated in the docs. Alternatives: a per-job leaf
  `ci/<job>` with `memory` enabled down to it (exact totals, but changes ADR-0039's layout, needs
  `memory` delegated to the leaves and cleanup of every leaf, and is unknown without delegation);
  sampling `/proc` for the job's processes (a scan per tick, misses short peaks, needs the
  bubblewrap pid namespace mapped).
- **Stored on the state rows, added by the end triggers** (ADR-0112's rule: a span follows the
  state it measures). `json_patch` with the three values drops a `NULL` value, so unknown stays
  absent.
- **Units**: CPU in whole milliseconds; memory and disk in MiB rounded to the nearest, at least 1
  for a non-zero memory peak (a running process is never 0 MiB); a disk figure of 0 is a real
  measurement (the job wrote nothing).
- **The usage travels in `exit.json`, not in the `Exit` frame** (ADR-0127): the frame's decoder
  refuses unknown flag bits, so a new field there would break a server and a host of different
  versions (a detached host outlives an upgrade, ADR-0002). The host writes `exit.json` before it
  sends the frame, so the server reads the usage from the file when the frame arrives.
- **The waterfall shows one span's figures** (a single job has no percentile); the p50/p90 are in
  the stage table, the place that aggregates.
