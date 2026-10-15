# 0127. A job's resource use comes from its process host

## Status

Accepted. Specified in [SPEC-154](../../.ai/specs/SPEC-154-fix-488-ci-jobs-and-cd-runs-record-their.md);
task #129, issue #488. Extends [0112](0112-stages-are-spans-written-with-the-state-they-measure.md)
(stages are spans) and keeps [0039](0039-ci-jobs-get-the-cpu-first.md)'s cgroup layout unchanged.

## Context

SPEC-137 measures every stage of tasks, CI and CD, but a CI job's or CD run's span held only its
declared `cpus` and `memory_mb` budget, not what it used. The request asked for "resource use (CPU,
memory, disk) where available", read "from a per-job cgroup (…) or the process host's accounting",
with unknown use left unknown, never 0.

A sandbox's CI jobs share its `ci/` cgroup leaf (ADR-0039), so the cgroup cannot tell one job from
another. Hosts without systemd delegation have no sandbox cgroups at all, and agent sandboxes, where
Yulius CI runs the project's own tests, have no cgroup filesystem.

The process host (`yulius-ptyhost`) already runs exactly one job, observes its exit, kills what is
left of its group and session, and reaps it.

## Decision

- **The process host measures the job's process tree when it reaps it**: after the reap it reads
  `getrusage(RUSAGE_CHILDREN)`, which the kernel has added up over every process reaped down the
  child's chain (bubblewrap's init reaps the job's orphans inside the containment). It records
  `cpu_ms` (user + system), `max_rss_bytes` (the largest peak resident set of one process) and, on
  Linux with task I/O accounting (`/proc/self/io`), `written_bytes` (`ru_oublock` × 512) in
  `exit.json` as `usage`. Nothing is recorded when the child never ran.
- **`exit.json` carries it, the `Exit` frame does not**: the server reads it from the file, which the
  host writes before it sends the frame. The socket protocol is unchanged, so a host and a server of
  different versions still understand each other; an older host's file reads as unknown.
- **Stored with the end, added by the end trigger**: `ci_jobs` and `cd_runs` get `cpu_ms`,
  `memory_peak_mb` and `disk_written_mb` (`0030_job_resource_use.sql`, schema version 31), and the end triggers patch them into the
  `job` / `run` span with `json_patch`, which leaves a `NULL` out: unknown stays absent.
- **A job Yulius killed has no figures**: cancelling, a timeout or too much output kills the job's
  whole group and session at once, so its processes are reaped by init, outside the host's chain,
  and the host would measure almost nothing (an 8-second busy loop read as 3 ms; review round 1).
  Its use is unknown rather than a near-zero figure that would pull the percentiles down.
- **Reserved names**: a job's `::yulius-measure` cannot set the three figures.
- **Shown as p50/p90**: each stage row of the summary has the p50 and p90 of CPU time and peak
  memory of its spans that measured them; the Pipeline page's stage table shows them beside the
  durations, and a waterfall row its own figures.

## Alternatives

- **A per-job cgroup leaf (`ci/<job>`)**: exact totals (`cpu.stat`, `memory.peak`, `io.stat`), but it
  changes ADR-0039's layout, needs the `memory` and `io` controllers delegated down to the leaves,
  every job's leaf created and removed, and gives nothing where there is no delegation, including
  the sandboxes where Yulius CI itself runs.
- **Sampling `/proc` for the job's processes**: the sum of processes at once, but a scan of every
  process per tick on a busy host, short peaks missed, and the job's processes are in their own pid
  namespace and session under bubblewrap.
- **Figures printed by the job (`::yulius-measure`)**: a job could claim anything (ADR-0112).

## Consequences

- Every host measures CPU time and peak memory, with or without cgroups; disk written is known on
  Linux with task I/O accounting.
- Peak memory is the largest single process's peak, not the sum of the processes running at once:
  a parallel build's total is higher. The docs and the Pipeline page say so.
- A process that leaves the job's reaping chain is not counted: a background process still running
  when the job ends (the host kills what is left of the group and session), a daemon reparented
  outside the containment, every process of a killed job (whose figures are therefore unknown).
- The peak memory has a floor of a few MiB: Linux counts the forked host's memory before the exec
  in the child's peak. On macOS the host's own `sysctl` and `ps` helpers count as the job's.
- A child subreaper host that reaps the orphans of a killed job before reading its use could give
  killed jobs figures too; not done, as a killed job's figures matter less than its reason.
- A per-job cgroup can still replace or complement this later behind the same figures.
