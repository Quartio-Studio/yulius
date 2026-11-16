# 0350. CI jobs are admitted by what CI really holds on the host

## Status

Accepted. Specified in [SPEC-497](../../.ai/specs/SPEC-497-an-armed-pull-request-merges-in-about-on.md)
slice A (task #479). Amends SPEC-082 RES-003 (the CI pools) and SPEC-362 CSW-002 (the sandbox's
weight); [ADR-0065](0065-processes-plan-for-a-cpu-and-memory-budget.md) and
[ADR-0247](0247-a-sandbox-running-ci-jobs-weighs-more-on-the-host.md) stay in force otherwise.

## Context

On 16 Nov 2026 quartio-v2 ran four `quality` runs at once 42 % of the time between 12:00 and
15:27 UTC (`YULIUS_CI_LANES=4` on a 64-CPU host). Each declared, or got as its share, about 16
CPUs, so the CI pools (the sandbox capacity, 64 CPUs) let all four start. Each then took a median
18.8 min against about 10 alone, Docker ran out of address pools for new networks in 3 runs, and
timing tests failed on unrelated pull requests. With `cpus = 30` two fitted and took 13.5 min
each, and this repository's pull request waited 8 min 27 s for a slot behind them.

The pools counted declarations only. A job's budget is told to its tools (`YULIUS_CPUS`); its
compilers, browsers and the Docker containers it starts (in the sandbox's `engine` group) use what
is free. The pools also left nothing for the server and the agents' sessions, and a sandbox's
weight (500 with any job running) gave its CI less than its jobs held once several other
sandboxes' agents built. Each sandbox's Docker engine had Docker's default pools: 31 networks.

## Decision

- **One CI budget for the instance**: the capacity less an eighth for the server and the agent
  sessions (at least 1 CPU and 2 GiB; 56 of 64 CPUs here). Every sandbox's jobs come out of it
  (`Capacity::ci_budget`).
- **Admitted by what CI holds**: a job starts when its reservation fits beside the larger of the
  running jobs' reservations and what the `ci` and `engine` groups of the sandboxes running them
  were measured to use (CPU time per second over at least 5 s, anonymous memory). Readings are
  taken at each job's start, at every dispatcher pass and while a job waits, under the
  admission's lock so they apply in order; a base older than 30 s is a new base and the next job
  waits one look for a recent rate. With no job running anything starts; a job larger than the
  budget runs alone. Waiters keep their order (`yulius_domain::resources::admit`,
  `App::ci_reserve`).
- **A job's default share is of the CI budget** (`cpus` not declared): an equal share of the
  budget among its run's jobs, so a run's jobs start together on an idle host and `YULIUS_CPUS`
  says what the job may use (18 CPUs each for three jobs on 64 CPUs, 21 before).
- **Docker networks are part of the budget per engine**: each engine makes /24 networks out of
  Docker's own default ranges (4,096 instead of 31; the engine runs in the sandbox's network
  namespace, so these ranges are as free there as Docker's defaults were). The engine's own
  `/info` says how many its pools hold, so one started before keeps 31 until its next start. A
  job waits while its sandbox's running engine can make fewer than 32 more (a quarter of a
  smaller engine's, 7 of 31); networks no container used for 10 minutes are pruned first, so a
  network a job or an agent made a moment ago stays. No engine running: no wait, and nothing
  starts one. The networks are looked at before the job waits for a slot and its budget, so a
  job held by one engine holds nothing the other sandboxes need.
- **The wait says why**: a queued job's reason is what it waits for ("waits for the host's CI
  budget: CI holds 50 of the host's 56 CPUs for CI (32 reserved, 50 measured); this job needs
  16", or the Docker networks), cleared when it starts; `ci_status` and the CI page show it.
- **Agents' heavy commands take what CI does not hold**: a sandbox's weight among sandboxes is
  100 plus 100 for each CPU its running jobs hold (at least 500, at most 10,000), so against `n`
  sandboxes whose agents build it gets `(1 + c) / (1 + c + n)` of the host: 30 held CPUs get 45
  of 64 against four building sandboxes (28 with the flat 500). Agents keep their own weight
  and nice level, so idle CPUs still go to them. The price: while CI uses more than it reserved,
  another sandbox's building agents keep about 2 of 64 CPUs (about 9 with the flat 500); CI's
  runs are what everyone waits for.
- **`YULIUS_CI_LANES` keeps its host-sized default** (one per 32 CPUs plus 2, at most 8) and
  only bounds how many runs of one sandbox take a checkout at once. It no longer sizes the load:
  four lanes of 16-CPU jobs now run as the budget allows, the others waiting with their reason.
  `YULIUS_CI_MAX_JOBS` stays the count of jobs at once.

## Alternatives

- **Measure each job and learn its need from its history**: exact per job, but a container is
  not tied to the job that started it (the engine serves the whole sandbox) and a job's history
  differs by change. The groups' total needs no attribution, and the reservations still cover a
  job that just started.
- **Admit by the host's load or pressure**: the agents' local builds would then hold CI back,
  the opposite of what CI needs.
- **Cap the agents with `cpu.max` while CI runs**: a hard cap wastes idle CPUs and needs a
  number per sandbox; the weight gives CI its share under contention and nothing when idle.
- **Fewer lanes**: a lane count cannot tell a 2-CPU job from a 30-CPU one, and a project's own
  `cpus` would still decide the load.
- **A larger Docker range than Docker's defaults**: other private ranges (10.0.0.0/8) can meet
  the sandbox's link or what a project reaches; Docker's own ranges were already in use there.

## Consequences

- A project's `cpus` and the lanes can no longer overload the host: jobs that use more than they
  declared make the next one wait, with the measured figure in its reason.
- A waiting job looks again every 5 s and at each job's end; the reading of a few cgroup files
  per sandbox with running jobs costs nothing measurable.
- Engines started before this change keep Docker's default pools until their next start; the
  network wait holds their jobs back at 7 free, with its reason, before a run fails for it.
- A project's networks are /24s from its engine's next start: the first one a project makes is
  172.17.1.0/24 (`docker0` keeps 172.17.0.1), not 172.18.0.0/16. A project that wrote a gateway
  such as 172.18.0.1 into its files, or puts more than 253 containers on one network, sets its
  own `subnet` in its compose file.
- A sandbox running agents' Docker containers beside its CI counts them as CI's: its next job
  waits a little more. Containers of sandboxes running no CI job are not counted.
