# 0353. CI jobs start by what CI uses, within a bounded worst case, and never wait behind a job that does not fit

## Status

Accepted. Amends [ADR-0350](0350-ci-jobs-are-admitted-by-what-ci-really-holds-on-the-host.md) and
[SPEC-497](../../.ai/specs/SPEC-497-an-armed-pull-request-merges-in-about-on.md) HBA-002:

- The CI budget, the Docker networks, the sandbox's weight, the lanes and the wait's reason stay as
  decided there.
- What changes: when a job may start, and in which order waiting jobs start.

## Context

Measured on production on 16 Nov 2026 (64 CPUs, 755 GiB; `yulius.db` wait and job spans,
`/proc/stat`, the sandboxes' `cpu.pressure`):

- **Reservations, not load, made the queue.**
  - From 17:00 to 20:00 UTC, the running jobs held 50–56 of 64 CPUs by their declarations or
    default shares. The host was 25–30 % busy and 51 of 755 GB of memory was used.
  - Since 12:52 a run's jobs waited a median 10–11 min to start, against a median 5 min of work
    (yulius). quartio-v2's p90 wait was 21 min.
  - Jobs waited 648 job-minutes in the 18:00 hour and 904 in the 19:00 hour.
  - A job's budget is what it may use at once (its compilers' parallelism), not what it uses over
    its run: checkouts, links, tests and browsers leave most of it idle.
- **One job held up every other.**
  - A run with one job and no `cpus` gets the whole budget as its share. autoresearch's `gate`
    (clippy and tests, 12 s of work) asked for all 64 CPUs.
  - Waiters were served strictly in order, and its project pushed every 3–10 minutes. Each new run
    therefore put it back at the front, where it waited 7–37 min for the host to empty, and every
    job behind it waited too. That began at 18:26, the hour the waits exploded.
- **Doubling the pools as a stopgap showed the other limit.** With reservations allowed to add up to
  twice the host, five jobs ran instead of two and the host was ~68 % busy. But at compile peaks:
  - the load reached 96 on 64 CPUs;
  - CI's processes waited for a CPU 10–25 % of 10-second windows;
  - quartio-v2's Docker engine (its test containers) waited 22 %.

  quartio-v2's flaky tests are timing tests. More jobs by a larger factor would have traded speed
  for flakiness.
- **A waiting job held a job slot.** It took its slot before its budget, so a slot could be held by
  a job that could not start.

## Decision

A waiting job starts when all of these hold (`yulius_domain::resources::admit`,
`App::ci_reserve`):

1. **CI's processes are not waiting for CPUs.**
   - The sandboxes' `ci` groups' `cpu.pressure` `some avg10` is at most 10 % (`CI_PRESSURE_LIMIT`).
   - This is the kernel's own measure of contention, so no job starts into a compile peak and
     tests keep the CPU they get.
2. **The worst case is bounded.**
   - The CPUs the running jobs may use at once (their `cpus` or default shares), with this job's,
     are at most twice the CI budget (`CI_OVERSUBSCRIPTION`).
   - Were every job to use all it may at the same moment, each would still get half of it, so a
     test's time limits keep their margin.
   - Declarations bound the worst case; they no longer reserve the host.
3. **What CI uses leaves room.** The sum of these fits in the CI budget, in CPUs and in memory:
   - what the sandboxes' `ci` and `engine` groups were measured to use;
   - what jobs started in the last minute may still bring;
   - this job's start.

   A starting job counts as its budget but at most a quarter of the CI budget
   (`CI_START_SHARE`), until the readings show it. Where nothing is measured, the reservations
   decide as before.
4. **A slot is free.** The slot is taken when the job is admitted, not while it waits.

Waiting jobs are ordered by what waits for them, then by how long they have waited
(`CiPriority`):

- first the jobs of merge trains and playground merges, which a merge waits for;
- then pull requests';
- then branches'.

A job that cannot start lets later ones that can start before it, and one before it that can start
goes first. A job that has waited 10 minutes (`OVERTAKE_LIMIT`) holds back everything behind it
until it starts or leaves, so it waits at most about one more job's length.

## Consequences

- **The host's idle CPUs are used.** A job is held back only:
  - by contention the kernel measures;
  - by the bounded worst case;
  - by measured use.

  A job that cannot start costs the others nothing.
- **A whole-budget job no longer empties the host.** It starts as soon as the readings leave a
  quarter of the budget free and the worst case allows. Otherwise jobs pass it for 10 minutes;
  then the host drains only as far as it needs.
- **Merges come first**: a merge train's run no longer queues behind pull requests' runs.
- **Contention stops admissions and never stops a running job.** A job admitted between two peaks
  can still meet the next one. The worst-case bound keeps that within twice the CPUs.
- **The job's environment is unchanged**: `YULIUS_CPUS` and the variables derived from it are
  still its budget.
- **Projects' `cpus` still matter.** They bound the worst case and set a job's parallelism, but they
  no longer decide how many jobs run.
