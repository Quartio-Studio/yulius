---
yulius-task: 994ed95c-dd7d-430c-9bfa-b029a50f02dc
status: in-progress
---

# SPEC-268: Parallel tasks in one sandbox crash Claude Code via pids.max

Status: in-progress
Date: 2026-10-22
Owner: Yulius task #254
Scope authorization: task #254 (`/goal`): fix the crash, make a crash at the limit say so, size
build tools to the budget, docs and changes, deliver to `dev`. Amends SPEC-002 ADR-22 §5 (sandbox
limits), [SPEC-053](SPEC-053-ci-cpu-priority.md) CPU-201 (only the CPU went down to the leaves)
and [SPEC-082](SPEC-082-test-timeouts-and-cpu-memory-budgets.md) RES-005 (Yulius set no tool's
variable). Decision: [ADR-0198](../../docs/decisions/0198-sandbox-task-limits-are-sized-from-the-host-and-leave-sessions-a-reserve.md).

## Goal

The parallel tasks of one sandbox cannot use up its process budget so that the agent sessions
die; and when a session still ends at a limit, the task says so instead of "The agent stopped
unexpectedly".

## Context

- **The incident.** quartio-v2 (sandbox `7c46cdac-…`), task #10 became `stopped_crashed` at
  2026-10-22 08:22:48Z, about 59 s into a foreground Bash call (`cargo build --all-targets …;
  scripts/gate all …`) with no tool result recorded. Tasks #8, #11, #12 and #13 of the same
  sandbox were working at the same time: several cargo builds and test runs whose tests start
  Docker containers (Chromium, GreenMail, a warm pool). Host: 12 CPUs, 47 GB, no swap, load ~100.
- **One budget for everything.** Every sandbox has one cgroup, `sandboxes/<id>`, with
  `pids.max` = 8192 (`YULIUS_SANDBOX_PIDS`, `Limits::defaults` in
  `crates/yulius-sandbox/src/cgroup.rs`). Only the CPU is passed down to its leaves (`system`,
  `agents`, `ci`, `engine`), so `pids.max` is one pool. It counts tasks, i.e. threads: every
  rustc and test thread of every agent's tools, every CI job, the sandbox's Docker engine and all
  its containers share 8192. A Claude Code process alone has 18 threads (measured in this
  session: `ps -o nlwp` of `claude`), each `cargo build` runs `CARGO_BUILD_JOBS` rustc processes
  with their own thread pools, and a Rust test binary runs one thread per CPU, each test with its
  own runtime; Chromium is hundreds of threads.
- **How it kills a session (reproduction, given in the task).** Claude Code 2.1.292 runs on Bun
  1.4.3. Under `prlimit --nproc` with little headroom, both `claude -p` and a Bun 1.4.3 script
  that mimics a Bash tool call (a piped `bash -c` child forking processes that hold their slots
  while the parent keeps doing fs/spawn/fetch work) die with `panic(main thread): abort() called`
  ("oh no: Bun has crashed"), exit 134 (SIGABRT, `bwrap` reports 128 + 6). A limit hit only
  briefly (children fail and exit at once) is survived. So the session dies when the budget stays
  exhausted while its own process needs a thread or a process, whoever holds the slots.
- **Why "crashed", not "killed".** `exit_event` (`crates/yulius-app/src/supervisor.rs`) treats
  signals 1/2/9/15 and codes 129/130/137/143 as kills; 134 is `stopped_crashed`. A cgroup OOM kill
  is SIGKILL (137, `stopped_killed`) and `vm.overcommit_memory` is 0, so the status points at the
  task limit, not memory. Nothing recorded which limit was hit: the UI said "The agent stopped
  unexpectedly".
- **Evidence not reachable from this task.** This task runs inside its own sandbox: no
  `/sys/fs/cgroup` (`find` fails: "No such file or directory"), no `/var/log/apport.log`, and the
  quartio-v2 task is not in this project's store (`task_get` → not found), its repository not
  readable (`gh api` → 404). The attempt's exit code (expected 134), the sandbox's `pids.events`
  `max` counter and apport's log are therefore unconfirmed; the design rests on the reproduction
  and on the limits in the code. The host's task limits are readable: `kernel.threads-max` =
  385 088, `kernel.pid_max` = 4 194 304.
- **Budgets for tools.** Agent sessions get `YULIUS_CPUS` (a third of the sandbox: 4 of 12 here)
  and the variables of the repository's `[resources.env]` (SPEC-082 RES-004/005). Since SPEC-082,
  Yulius sets no tool variable itself: a repository without `[resources.env]` gets no
  `CARGO_BUILD_JOBS`, so each of five parallel tasks builds with 12 jobs. This repository maps it
  (`CARGO_BUILD_JOBS=4` in this session); quartio-v2's `yulius.toml` could not be read.
- **cgroup v2 facts used.** Limits nest: a fork is refused when any ancestor is at its
  `pids.max`, so a leaf's `pids.max` below the sandbox's caps that leaf alone. `pids.events`
  `max` counts refused forks (kernel 6.8 counts them in the forking task's own group; 6.13+ also
  hierarchically), `memory.events` `oom_kill` is hierarchical. The sandbox group holds no process
  once split (CPU-202), so `+pids` can be enabled for its leaves like `+cpu`; without the split
  (no `cpu` controller) runs live in the sandbox group itself and its leaves cannot get limits.

## Requirements

- PID-001: The default `pids.max` of a sandbox is sized from the host: a quarter of the smaller
  of `kernel.threads-max` and `kernel.pid_max`, at least 8192 (96 272 on the 47 GB VM, 12 times
  the old 8192). `YULIUS_SANDBOX_PIDS` still sets it. Unreadable host values fall back to 8192.
- PID-002: When the sandbox is split into leaves, the `ci` and `engine` leaves each get
  `pids.max` = 3/8 of the sandbox's (at least 1): CI jobs and the engine with its containers
  together never take more than three quarters of the budget, and the agents' leaf (sessions,
  their tools, terminals, the Cockpit) always keeps at least a quarter. The `system` and `agents`
  leaves get no limit of their own. Without the split nothing changes. A leaf limit that cannot be
  written is logged and the sandbox runs on with its own limit.
- PID-003: A stopping server takes the task limits back from the leaves (`-pids`), as it takes the
  CPU back (CPU-205), so an older binary can join the sandbox group; the next preparation applies
  them again.
- PID-004: The isolation adapter reports a sandbox's limit counters: the sum of `pids.events`
  `max` over the sandbox's group and its descendants (bounded to 4096 groups), and
  `memory.events` `oom_kill` of the sandbox's group. None without the delegation.
- PID-005: A run's counters are read when it starts (and when a restarted server takes it over).
  When it ends without Yulius ending it and not cleanly (`stopped_crashed`, `stopped_killed`), the
  counters are read again: if the task counter rose, the attempt's `exit_reason` ends with
  `limit=pids`; else if the OOM counter rose, `limit=memory`. The exit sent to the task's viewers
  carries the same cause (`cause: "pids" | "memory"`).
- PID-006: The task's end banner and the attempt's state in the task's overview say the cause:
  "The sandbox hit its process limit while the agent ran (too many processes and threads in this
  project's tasks at once)" and "The sandbox ran out of memory while the agent ran"; without a
  cause they stay as they are.
- PID-007: Agent sessions, terminals, the Cockpit and CI jobs started where `yulius.toml` has no
  `[resources.env]` table (or there is no `yulius.toml`) get the default mapping
  `CARGO_BUILD_JOBS`, `NEXTEST_TEST_THREADS` and `RUST_TEST_THREADS` = `{cpus}`; a declared table,
  even an empty one, replaces it entirely, and an invalid one still sets only `YULIUS_CPUS` and
  `YULIUS_MEMORY_MB` (amends RES-004/RES-005).

## Plan

- [x] Spec and ADR state the cause and the design
- [ ] Sandboxes get a task limit sized from the host
- [ ] CI and the engine leave the agent sessions a reserve of the task limit
- [ ] The isolation adapter reports a sandbox's limit counters
- [ ] A session that ends at a limit records and sends why
- [ ] The task view says the sandbox hit its process or memory limit
- [ ] Repositories without `[resources.env]` get the Rust build and test defaults
- [ ] Docs, gates, review and delivery

## Acceptance criteria

- AC-1 (PID-001): with `threads-max` 385 088 and `pid_max` 4 194 304 the default is 96 272; with
  unreadable values or a small host it is 8192.
- AC-2 (PID-002, PID-003): in a fake split sandbox with `pids.max` 64, `ci/pids.max` and
  `engine/pids.max` are 24 and `agents` has none; stopping writes `-pids`; without the split no
  leaf limit is written.
- AC-3 (PID-002, real kernel, opt-in `isolation_limits`): forks in `ci` are refused at its leaf
  limit while a shell in `agents` still forks.
- AC-4 (PID-004, PID-005): an agent run that aborts (SIGABRT, like Bun's panic) while the
  sandbox's task counter rose ends `stopped_crashed` with `exit_reason` ending `limit=pids` and an
  exit frame with `cause: "pids"`; an abort without a rise records no cause; an OOM rise gives
  `limit=memory`.
- AC-5 (PID-006): the banner and the overview show the cause's sentence.
- AC-6 (PID-007): a session in a worktree without `[resources.env]` gets `CARGO_BUILD_JOBS`; one
  with a table that does not map it does not.

## Test plan

- `cgroup::tests` (fake cgroup trees): host-sized defaults; leaf limits written with the split, not
  without it; `-pids` on stop; counters summed over the subtree, bounded.
- `supervision::a_session_that_ends_at_the_sandboxs_process_limit_says_so` with an isolation fake
  whose counters the test raises and the fake agent's new `/abort`: fails before PID-005.
- `supervision::an_agent_session_is_told_its_budget_in_the_repositorys_terms` extended for
  PID-007; `resources` unit tests for the default mapping.
- `isolation::isolation_limits` (opt-in, the VM's delegated unit) extended for AC-3; not runnable
  inside a sandbox, which has no cgroup mount.
- The UI sentence: `status.js` unit-level through the e2e suite's task view where a mock run ends
  with a cause.

## Decisions

- **Sized from the host, not a fixed 8192.** The limit guards the host's task table, not the
  sandbox's memory (kernel stacks are charged to its `memory.max`). A quarter of the host's table
  leaves room for Yulius, the host and other sandboxes, and is 12 times what five Rust tasks with
  containers exhausted. A fixed bigger number would be wrong on small and large hosts alike.
- **Leaf limits for CI and the engine, not per run.** The session that died and the tools that
  took the slots share one process tree: Claude Code starts every Bash command itself, inside the
  sandbox, which has no cgroup mount, so neither a per-run group nor a group for tool commands can
  keep a session's own process apart from its tools without the server moving each command
  (a new call per command). CI jobs and the engine's containers are the work that runs apart from
  any session and can take slots for long (a warm pool, a hung job); capping them leaves the
  sessions a reserve the incident's containers could not take. Three eighths each: CI and the
  engine are each still 3.75 times the old whole budget.
- **Attribution by counters, not by exit code.** Exit 134 is any abort; the counters say whether
  the limit was actually hit while the run lived. A rise caused by a sibling is still the right
  thing to say: the sandbox was at its limit when the session needed it. A restart loses the start
  value, so a server that takes a run over reads it again (a rise before the restart is missed).
- **A default mapping only where the repository says nothing.** Most repositories never add
  `[resources.env]`; a Rust one then builds and tests with one job and thread per host CPU in every
  task. The three Rust variables change nothing for other languages; a repository that declares a
  table, even an empty one, keeps full control (ADR-0065's principle stands for every declared
  mapping).
