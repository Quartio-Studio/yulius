# 0354. A job that fails for the infrastructure runs again once by itself

## Status

Accepted. Specified in [SPEC-497](../../.ai/specs/SPEC-497-an-armed-pull-request-merges-in-about-on.md)
slice B (task #479), OFC-001 and OFC-003. Complements
[ADR-0350](0350-ci-jobs-are-admitted-by-what-ci-really-holds-on-the-host.md), which keeps most of
these failures from happening.

## Context

Of 20 quartio-v2 `quality` jobs that failed on 16 Nov 2026 between 12:00 and 17:00 UTC, few were
about the change: Docker had no address pool for a new network (3), and the same tests failed on
unrelated pull requests. Each such failure cost the pull request a whole cycle (an agent read the
run, decided it was not its own, asked `ci_rerun`), and a member whose own run failed dissolved
its merge candidate. 526 CI minutes that day went to failed runs and 114 to cancelled ones,
against 588 to passed ones; nothing told the two kinds of failure apart.

The workflow's rule stays: a failing test is never run again until it passes (it is reproduced
and fixed, or quarantined with an issue).

## Decision

- **What counts as the infrastructure's** (`yulius_domain::ci::infra`): only what the host or its
  Docker engine says about itself.
  - Docker's own words for a full address pool ("could not find an available, non-overlapping
    IPv4 address pool", "all predefined address pools have been fully subnetted").
  - A line naming `docker.sock` that says the connection ended (EOF, reset by peer, broken pipe,
    refused, "cannot connect to the Docker daemon").
  - A `SIGKILL` (or exit 137) while the kernel's OOM count of the job's sandbox grew.
  - A job whose process host was lost.
  Only the run's own output counts (what it printed after the line that started it again). A run
  in which any line reads as a failure in a test runner's or a tool's words (`fail`, `panic`,
  `assert`, `not ok`, `timeout`, `timed out`, `error`, `segmentation fault`, `core dumped`;
  Docker's own lines, Yulius's lines, passing summaries and `make`'s exit report left out), or
  with a failing test's block kept, is the change's, whatever ended it: a kill for memory or a
  lost host too. Otherwise Docker's words must be in its last 3 lines, the socket's only from the
  Docker client's own words (`Cannot connect to the Docker daemon`, or `error during connect`,
  `read unix`, `write unix`, `dial unix` with the connection's end), so a container's log naming
  the socket is not one. A missed rerun costs a cycle; a wrong one would break the rule that a
  failing test is never run again.
- **Run again once, by itself**: the job, not its run, starts again in its checkout with its slot
  and budget, its output saying why ("Docker had no free address pool for a new network: an
  infrastructure failure, not the change; the job runs again once by itself"). If it then passes,
  its reason says it passed when run again and why; if it meets the infrastructure again, it ends
  **error** saying so, never failed. A second run is never made after a test's failure.
- **Before the second run** after a full address pool, the job waits for room in its engine, at
  most 10 minutes, keeping its slot, budget and turn meanwhile; its output says so, a
  cancellation ends it cancelled with no second start, and past the bound it ends error saying
  its engine had no room, with no second run. A second start that cannot be made ends error for
  the infrastructure too, with the reason it could not. A cancellation that came while the first
  run ended starts no second one. The
  second run has its own time limit (a job can hold its slot up to twice its limit), the first
  run's measured use is not kept, and the stages of both runs stay on the job's span.
- **Counted**: the job keeps the kind, the milliseconds the thrown-away run took and whether its
  end itself was the infrastructure's (`ci_jobs.infra`, `infra_ms`, `infra_end`, migration 0064).
  When a run that started ends, its minutes are added to its sandbox's tally for the day
  (`ci_minutes`): runs are pruned to the newest few per sandbox, the tally stays. The merge
  queue's read
  (`GET /api/analytics/merge-queue`) and the Pipeline tab's Merge queue card count CI minutes by
  how runs ended: passed, failed, failed only for the infrastructure, cancelled, and the job
  minutes run again; the share outside the change is (failed outside + cancelled) / all, the
  target 15 %. A run counts as failed outside the change only when every job of it that failed
  ended for the infrastructure (`infra_end`).
- **The host's CI load** is kept once a minute (`ci_load`, 30 days): the host's CPUs busy (from
  `/proc/stat`), the CPUs running jobs reserved and measured, and the jobs waiting to start.
  `GET /api/analytics/ci-load` reads it by hour and the Pipeline tab's CI load card shows the
  job-minutes waited, the busiest hour and the host's busy share, so admission (ADR-0353) is
  measured before and after a change.

## Alternatives

- **Rerun the whole run**: costs every job of the run for one job's infrastructure failure.
- **Rerun any failed job once**: a flaky test would pass on its second try and never be
  quarantined; the rule forbids it.
- **An agent decides** (today's `ci_rerun`): a whole cycle per failure, and the candidate
  dissolved meanwhile.
- **Match more words** (any "connection reset", any "Killed"): a test about networking or one
  that kills a child would be run again for its own failure. The list stays narrow; a missed
  failure costs a cycle, as before.

## Consequences

- A pull request whose job met a full address pool, a Docker socket that went away, the OOM killer
  or a lost host passes on the second run without an agent's call, and its candidate is not
  dissolved for it.
- A job that met the infrastructure twice errors, which `delivery_merge` treats as before (not a
  failure of the change; it blocks the merge until a run passes).
- The runs before this change have no `infra`: their failures count as "failed" in the minutes.
