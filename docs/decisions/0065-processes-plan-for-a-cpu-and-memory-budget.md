# 0065. Processes plan for a CPU and memory budget; CI jobs wait for theirs

## Status

Accepted. Specified in
[SPEC-082](../../.ai/specs/SPEC-082-test-timeouts-and-cpu-memory-budgets.md). Builds on
[ADR-0039](0039-ci-jobs-get-the-cpu-first.md) (CI's CPU weight) and drops the default CPU quota
of the sandbox limits ([ADR-0003](0003-sandbox-isolation.md), SPEC-002 ADR-22 §5).

## Context

Every process Yulius started in a sandbox sized itself to the whole machine: two CI lanes of two
jobs each, every `quality` job at one test thread and one build job per CPU, and up to seven
agents running the same gate locally. On 12 CPUs the load reached 35–47 and tests that wait on
real processes timed out. The repository answered with a fixed thread count (#351), which made
its suite 2–3 times slower and fits no other repository. Yulius itself set `CARGO_BUILD_JOBS` for
every repository, and capped each sandbox at all CPUs but two, which throttled it in half of its
busy periods while those two CPUs idled. Repositories in other languages are next.

## Decision

- **A capacity.** A sandbox's capacity is its enforced CPU and memory limits within the host,
  else the host's CPUs and 2 GiB per CPU.
- **Budgets, told in the environment.** Every process group Yulius starts gets a budget and is
  told it as `YULIUS_CPUS` and `YULIUS_MEMORY_MB`, and in the variables the repository's
  `[resources.env]` in `yulius.toml` derives from them (`NEXTEST_TEST_THREADS = "{cpus}"`,
  `MAKEFLAGS = "-j{cpus}"`). Yulius sets no tool's variable itself.
- **CI jobs wait for their budget.** A `[[ci.jobs]]` entry declares `cpus` and `memory_gb`, else
  it gets an equal share of the capacity among its run's jobs. A job starts once its budget is
  free in pools that hold the capacity once for all lanes, in order.
- **Agents plan for a third.** Agent runs, terminals and the Cockpit get `YULIUS_AGENT_CPUS`, by
  default a third of the capacity (at least 2), memory in proportion.
- **No CPU quota by default.** A sandbox's CPU is shared by weight; `YULIUS_SANDBOX_CPUS` still
  caps it.

## Alternatives

- **A `cpu.max` quota per job or session.** It enforces, but it is a hard cap: a job alone could
  not use idle CPUs, a busy one would be throttled in stalls of up to a period, and inside the
  containment (own cgroup namespace, no cgroup mount) most runtimes size themselves by the CPUs
  they may run on, so they would still start one thread per CPU.
- **CPU affinity per job or session.** Runtimes see it, but it pins: a job could not use idle
  CPUs, and two jobs on the same CPUs would starve each other while others idle.
- **A make job server shared by all processes.** Only make, Cargo and Ninja speak the protocol; test
  runners and browsers do not, and a crashed client loses its tokens.
- **A lease service that sizes each command to the agents running now.** It needs the agents to
  ask for every heavy command and to wait without output; the budget and the weights cover the
  measured load (at most three test runs at once in 81 % of busy minutes) with nothing new to
  call.
- **Fixed thread counts in each repository** (#351). The machine's size belongs to Yulius, not to
  every repository; on a quiet machine they waste it, on a busy one they still oversubscribe.

## Consequences

- CI jobs never ask for more than the sandbox in total, whatever the number of lanes; a second
  run's jobs wait for the first run's instead of slowing everyone. Under agents' load CI keeps its
  weight (ADR-0039).
- A repository states its tools' variables once in `yulius.toml`; a new language needs no change
  in Yulius. The variables also reach the agent CLI's own process, which holds the provider
  credential, so what Yulius sets and what loads code or redirects requests (`PATH`, `CI`,
  `YULIUS_*`, `GIT_*`, `ANTHROPIC_*`, `CLAUDE_*`, `NODE_*`, `PYTHON*`, `*_PROXY`, `LD_*` and
  similar) cannot be set this way; a tool that needs such a variable reads `YULIUS_CPUS` or
  `YULIUS_MEMORY_MB` in the repository's own scripts.
- Agents keep running as many gates as they like; past three at once they share the CPU by time.
  Fairness between agents is cooperative: one that ignores its budget slows itself and the other
  agents, not CI.
- The sandbox can use every CPU of the host; Yulius's own group and the host's services keep their
  share by weight.
- Branches whose `yulius.toml` predates their repository's mapping get only the two `YULIUS_*`
  variables until they merge their integration branch.
