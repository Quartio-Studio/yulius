# 0034. CI jobs get the CPU first through a weighted cgroup

## Status

Accepted. Specified in [SPEC-053](../../.ai/specs/SPEC-053-ci-e2e-timeouts-under-agent-load-give-ci.md).
It extends the per-sandbox limits of SPEC-002 ADR-22 §5 and keeps SPEC-050 CPU-101 (agent commands
under `nice 5`).

## Context

Yulius CI runs a sandbox's jobs on the same VM as its agents (ADR-21), and in the same cgroup.
Eight agents building in the `yulius` sandbox pushed the load to about 50 on 12 CPUs. `e2e` jobs
on correct pull requests then timed out after 19 minutes instead of passing in 5–10 (#283), and a
quick patch (#286) gave CI longer budgets and retries. Agent commands already run under `nice 5`,
but nice weights are per thread inside one cgroup. Every compiler thread an agent adds therefore
shrinks CI's share, and processes outside `--scrub-exec` are not niced at all.

## Decision

A sandbox's cgroup is split into three leaves:
- `system`: the holder and `slirp4netns`, `cpu.weight` 100;
- `agents`: agent runs, shells, browsers and the Cockpit, `cpu.weight` 100;
- `ci`: CI jobs, `cpu.weight` 400.

Only the `cpu` controller goes down to the leaves. `cpu.max`, `memory.max` and `pids.max` stay on
the sandbox, so the leaves share them. Under full contention CI gets 80 % of the sandbox's CPU and
the agents 20 %. When CI is idle, the agents get everything. Processes found in the sandbox group
from before the split move to `agents` first, as cgroup v2 requires.

## Alternatives

- **More `nice` (for example 19) for agent commands.** Simple, but still per thread. Its effect
  depends on how many threads the agents run, it covers only Bash commands, and it would slow the
  agents' own builds against the agents themselves.
- **An adaptive `CARGO_BUILD_JOBS`.** It is fixed when a session starts, so it cannot follow the
  load. It covers only Cargo.
- **A higher CI weight (1000 or more).** CI would win almost completely. While two CI lanes are
  busy, which is most of the time with many open tasks, 8 agents would share about one CPU.
- **A leaf per CI job.** `e2e` would not compete with `quality` either. It needs a group per job
  and its cleanup. The 5–10 minute baseline already includes that competition.

## Consequences

CI's speed no longer depends on what the agents run. A CI run is at most about 25 % slower than on
an idle sandbox. Agents slow down only while CI is busy, and never below a fifth of the sandbox.
Without delegation or the `cpu` controller nothing changes, and `nice 5` remains the only priority.
Sandboxes still share the host equally with each other, so one sandbox's agents can still slow
another sandbox's CI.
