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

1. ~~CI's processes are not waiting for CPUs~~ (removed 17 Nov 2026, see the amendment below).
2. **The worst case is bounded.**
   - The CPUs the running jobs may use at once (their `cpus` or default shares), with this job's,
     are at most three times the CI budget (`CI_OVERSUBSCRIPTION`; twice until 17 Nov 2026).
   - Were every job to use all it may at the same moment, each would still get a third of it.
     Measured use (rule 3) stops new jobs long before that.
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
goes first. A job that has waited 10 minutes (`OVERTAKE_LIMIT`) holds back the jobs after it in
the order (its priority and lower) until it starts or leaves; a job of a higher priority still
goes first, so a branch's job can wait longer while merges and pull requests keep coming.

## Alternatives

- **The reservations as a floor** (ADR-0350: the larger of reservations and measured use): what
  made the queue on 16 Nov, with the host 25–30 % busy while jobs held 50–56 of 64 CPUs.
- **Larger pools** (reservations up to twice the host, the stopgap): nothing but reservations
  stopped a start, so five jobs ran and CI's processes waited for a CPU 10–25 % of the time at
  compile peaks; quartio-v2's timing tests are its flaky ones. Rule 2's three times the budget
  differs in that measured use (rule 3) stops new jobs first.
- **The `ci` groups' CPU pressure as the gate**: first adopted as rule 1. Removed: a job pinned to
  a few CPUs showed 15–20 % pressure while the host was 45 % busy (the amendment below).
- **The host's load as the gate**: never adopted; the agents' own builds would hold CI back.
- **Strict first-come order**: one whole-budget job at the front kept every job behind it waiting
  7–37 min. A bounded overtake keeps the order fair without that.
- **Learn each job's need from its history**: exact per job, but a container is not tied to the
  job that started it, and a job's need differs by change. The groups' total needs no
  attribution.

## Consequences

- **The host's idle CPUs are used.** A job is held back only:
  - by contention the kernel measures (until the amendment below: no longer);
  - by the bounded worst case;
  - by measured use.

  A job that cannot start costs the others nothing.
- **A whole-budget job no longer empties the host.** It starts as soon as the readings leave a
  quarter of the budget free and the worst case allows. Otherwise jobs pass it for 10 minutes;
  then the host drains only as far as it needs.
- **Merges come first**: a merge train's run no longer queues behind pull requests' runs.
- **Contention stops admissions and never stops a running job.** A job admitted between two peaks
  can still meet the next one. The worst-case bound keeps that within three times the CPUs. (Since
  the amendment below, measured use alone stops admissions.)
- **The job's environment is unchanged**: `YULIUS_CPUS` and the variables derived from it are
  still its budget.
- **Projects' `cpus` still matter.** They bound the worst case and set a job's parallelism, but they
  no longer decide how many jobs run.
- **`memory_gb` no longer reserves memory for a job's whole run.** The worst-case bound is on CPUs;
  a job's memory counts at its start as its budget (at most a quarter of the CI budget's), then as
  the anonymous memory measured. Jobs that peak late in memory together can meet the sandbox's
  memory limit; a memory bound was left for when it is seen.

## Amendment, 17 Nov 2026: CPU pressure holds no job back

The first rule held every job while any sandbox's `ci` group showed more than 10 % CPU pressure.
The night after it shipped, the product repository's CI pinned each job's `node --test` workers
to the job's own CPUs (`scripts/ci/bounded.sh`, `taskset`), three of them for a 21-job run.

- Those workers waited for their three CPUs: 15–20 % pressure in that sandbox's `ci` group, while
  every other sandbox showed 0 %.
- The host was 45 % busy, and 117 jobs of every project queued behind the rule.
- Pressure cannot tell a job that confines itself from a host that is full. Measured use (rule 3)
  already stops new jobs when CI really fills the host, and the worst case (rule 2) bounds what
  a burst can do.

So the rule is gone. The pressure is still read, for diagnosis (logged at `debug` with each
reading, as `cpu_pressure_permille`), and a test keeps a pinned job's pressure from holding others
back again.

## Amendment, 17 Nov 2026: the worst case is three times the budget

At twice the budget, the worst case was the limit that held jobs back all night, not the host:
- 00:13 UTC: the running jobs' budgets were 108 of 112, with the host 60–77 % busy and CI's
  processes waiting for a CPU 1–2 % of the time.
- 01:25 UTC: three quartio-v2 jobs of 30 CPUs held a fourth back (90 + 30 > 112) with the host 55 %
  busy. Once that job had waited 10 minutes, it held back 105 smaller jobs behind it.

Measured use (rule 3) is what stops jobs when CI really fills the host. The worst case only bounds
a burst, and three times still guarantees each job a third of what it may use.

## Amendment, 21 Nov 2026

A waiter that wakes reserves slots and budgets for the eligible tickets in priority and arrival order until capacity is full or an overdue ticket blocks the rest. The same admission checks apply to each grant against the updated totals. A ticket owns its grant until its job collects it or cancellation releases it. Its startup allowance covers that interval and the usual window after collection. This removes a scheduling delay when several tickets fit and the later task wakes first. [SPEC-598](../../.ai/specs/SPEC-598-shorten-a-yulius-ci-run-parallel-quality.md)
