---
yulius-task: 9fcfa555-1dd9-4085-85e3-684ef985949c
status: in-progress
---

# SPEC-053: CI jobs get the CPU first, whatever the agents build

Status: in progress
Date: 2026-10-13
Owner: Yulius task #45
Scope authorization: GitHub issue #283 and task #45 ("Make CI jobs get CPU priority over agent
commands in a sandbox, so CI is fast and deterministic whatever the agents build ... Keep agents'
own builds reasonably fast; don't starve them completely").

## Goal

A CI job in a sandbox gets the CPU it needs while agents in the same sandbox build. A red `e2e`
result means a real failure, not a busy machine, so the CI-only budgets and retries of the quick
patch (#286) can go.

## Context

- **The failures (#283).** On 2026-10-12 the `e2e` runs of #270 and #273 timed out after about
  19 minutes (a passing run takes 5–10). The `quality` job of the #273 run took 17.4 minutes
  instead of about 5. The load average reached about 50 on 12 CPUs, with 8 agents building in the
  `yulius` sandbox. The same tests passed locally in seconds.
- **The quick patch (#286).** In CI only, `e2e/e2e.config.ts` raised the action budget from 30 s to
  90 s, the assertion budget to 15 s and the test budget to 6 min, and retried each failed test
  twice.
- **What CI shares today.** Every process of a sandbox joins one cgroup,
  `<unit>/sandboxes/<sandbox>` (ADR-22 §5, `crates/yulius-sandbox/src/cgroup.rs`). That includes the
  holder, `slirp4netns`, every agent run, shell, browser and Cockpit, and every CI job. That group has
  `cpu.max` (every CPU but two), `memory.max` and `pids.max`. Inside it the scheduler shares the CPU
  per thread.
- **CPU-101 (SPEC-050, already deployed).** Agent Bash commands run under `nice -n 5` through
  `yulius --scrub-exec` (`crates/yulius-harness/src/scrub_client.rs`). In one cgroup, nice weights
  are per thread: nice 5 is weight 335 against 1024. Eight agents running `cargo` with
  `CARGO_BUILD_JOBS=4` give about 32 compiler threads, 32 × 335 ≈ 10 700 weight. Against them, an
  e2e job's three or four busy threads (Chromium, Node, Python), 4 × 1024 ≈ 4 100, get about 1.6
  of the group's 10 CPUs. That share falls with every agent that starts. The other agent processes
  are not niced: MCP servers, dev servers started outside Bash, Codex.
- **`CARGO_BUILD_JOBS`.** It is set once, when the session starts (`supervisor.rs`
  `agent_build_jobs`, a third of the CPUs, at least 2). It covers only Cargo.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| CPU-201 | With cgroup delegation and the `cpu` controller, a sandbox's group `<sandbox>` gets three leaves. `system` holds the holder and `slirp4netns`. `agents` holds agent runs, shells, browsers and the Cockpit. `ci` holds CI jobs. `cpu.weight` is 100 for `system` and `agents` and 400 for `ci`. Under full contention, with the nearly idle `system` leaf left out, CI gets 80 % of the sandbox's CPU and agents get 20 %. Idle CPU goes to whoever wants it. `cpu.max`, `memory.max` and `pids.max` stay on `<sandbox>`, so the leaves share them. | `cgroup::tests::a_sandbox_group_splits_into_weighted_leaves` |
| CPU-202 | Processes already in `<sandbox>` from before this change move to `agents` before the `cpu` controller is enabled for the leaves: holders and runs started before an update keep running. If enabling fails (a process raced in), the run still joins its leaf, without weights, and the next preparation tries again. | `cgroup::tests::processes_in_the_sandbox_group_move_to_agents_first`, `cgroup::tests::a_split_that_cannot_be_enabled_still_gives_leaves` |
| CPU-203 | A CI job's process (`/bin/sh` and everything it starts) joins the sandbox's `ci` leaf. Every other run joins `agents`. The holder joins `system`. | `ci_isolation::a_ci_job_joins_the_ci_group_and_an_agent_the_agents_group` |
| CPU-204 | Without delegation, or without the `cpu` controller, nothing changes: runs join the sandbox's group, or none at all, as before. CPU-101 (agent commands under `nice 5`) stays in either case. | `cgroup::tests::without_the_cpu_controller_there_is_no_split` |
| E2E-201 | In CI, `e2e/e2e.config.ts` uses the runner's default budgets and no retries, the same as everywhere else. | `e2e/e2e.config.ts`; Evidence |

## Acceptance criteria

- AC-1: on a host with delegation, `<sandbox>/cgroup.subtree_control` lists `cpu`. The leaves'
  `cpu.weight` files read 100, 100 and 400. A CI job's `/proc/<pid>/cgroup` ends in
  `/<sandbox>/ci`, and an agent run's ends in `/<sandbox>/agents`.
- AC-2: under several agents building, `e2e` passes in at most 12 minutes in 5 consecutive runs
  without retries (see Evidence for what was measured and where).
- AC-3: the CI-only budgets and retries are gone from `e2e/e2e.config.ts`.

## Plan

- [x] Spec and ADR: CI CPU priority by cgroup weight
- [x] Sandbox cgroup split: system, agents and CI leaves with weights
- [ ] CI jobs run in the sandbox's CI leaf
- [ ] e2e without CI-only budgets and retries, measured under agent load
- [ ] Docs, changes log and every gate green
- [ ] Independent review findings resolved

## Design

**Groups.** `Delegation::sandbox_group(sandbox, limits)` returns `SandboxGroups { system,
agents, ci }` instead of one path:

```
<unit>/sandboxes/<sandbox>          cpu.max, memory.max, pids.max (as before)
  ├── system   cpu.weight 100       holder, slirp4netns
  ├── agents   cpu.weight 100       agent runs, shells, browsers, Cockpit
  └── ci       cpu.weight 400       CI jobs
```

cgroup v2 allows processes only in leaves once a group passes controllers to its children (the
"no internal processes" rule). So the preparation goes in this order:
1. Create the three leaves.
2. Move what is still in `<sandbox>/cgroup.procs` to `agents` (at most 4096, as `prepare` does
   for the unit's root).
3. Write `+cpu` to `<sandbox>/cgroup.subtree_control`.
4. Write the weights.

Only `cpu` goes down to the leaves. Memory and task limits stay on the sandbox as a whole. The
preparation runs whenever a sandbox is prepared and is idempotent.

**Ports.** The manager's `Joined` and the app's `SandboxJoin` gain `ci_cgroup`. `join.cgroup` is
now the `agents` leaf, and the holder's spec gets `system`. `App::ci_containment` swaps
`join.cgroup` for `join.ci_cgroup`. The ptyhost contract is unchanged: a run still joins the one
directory it is given.

**Why 400.** CI runs most of the time when many tasks are open: two lanes, a run after every merge.
A weight of 1000 or more would leave 8 agents about one CPU between them for as long as CI is busy.
At 400, CI gets 8 of 10 CPUs under full contention, and the agents 2 or more. CI is then never more
than about 25 % slower than on an idle sandbox, whatever the agents run, and the agents keep moving.

## Test plan

- Unit tests in `yulius-sandbox` (`cgroup.rs`) on a fake cgroup tree in a temporary directory. They
  check the files the split writes, the order (procs moved before `subtree_control`), the fallback
  when enabling fails, and no split without `cpu`.
- An integration test in `crates/yulius/tests` with a fake `SandboxIsolation` that hands out
  distinct agent and CI groups: a CI job's launch plan joins the CI group, and an agent run's plan
  joins the agents group.
- The e2e suite on this branch's config (no CI overrides), run in the `yulius` sandbox on the VM
  while agents build: 5 consecutive runs, durations recorded under Evidence.

## Decisions

- **cgroup weight for CI jobs, not more `nice` and not an adaptive `CARGO_BUILD_JOBS`.**
  [ADR-0034](../../docs/decisions/0034-ci-jobs-get-the-cpu-first.md).
  - `nice` is per thread, so its effect shrinks with every compiler thread an agent adds. It
    reaches only what goes through `--scrub-exec`.
  - `CARGO_BUILD_JOBS` is fixed for a session's lifetime, so it cannot follow the load. It covers
    only Cargo, not npm, Go or a test runner.
  - A weight on a group is per group, so it holds whatever the agents run, in any language.
- **One CI leaf, not one per job.** The jobs of a run share the leaf as they shared the machine in
  the 5–10 minute baseline. A leaf per job (`e2e` not competing with `quality`) is possible later
  if measurements ask for it.
- **CPU-101 stays.** Inside `agents`, `nice 5` keeps the agents themselves responsive while their
  commands build.
- **Strays go to `agents`.** Processes found in `<sandbox>` after an update cannot be told apart
  cheaply. `agents` is the conservative choice: an old holder's `slirp4netns` shares the agents'
  weight until the holder restarts after its idle stop.
- **Other sandboxes are out of scope.** Sandboxes still have equal weight under `sandboxes/`. #283
  is about one sandbox's agents against its own CI.
