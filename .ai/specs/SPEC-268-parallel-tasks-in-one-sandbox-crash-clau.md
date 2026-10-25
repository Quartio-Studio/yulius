---
yulius-task: 994ed95c-dd7d-430c-9bfa-b029a50f02dc
status: done
---

# SPEC-268: Parallel tasks in one sandbox crash Claude Code via pids.max

Status: done
Date: 2026-10-22
Owner: Yulius task #254
Scope authorization: task #254 (`/goal`): fix the crash, make a crash at the limit say so, size
build tools to the budget, docs and changes, deliver to `dev`. Amends SPEC-002 ADR-22 §5 (sandbox
limits), [SPEC-053](SPEC-053-ci-e2e-timeouts-under-agent-load-give-ci.md) CPU-201 (only the CPU went down to the leaves)
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
  the old 8192), and at most three quarters of the smallest `pids.max` of the delegated unit and
  its ancestors (systemd's `DefaultTasksMax` is 15 % of the host's table: 57 763 on the VM, so
  43 320), so a sandbox never starves the server and the other sandboxes before its own limit
  (review round 1). `YULIUS_SANDBOX_PIDS` still sets it, uncapped. Two saturated sandboxes can
  still fill the service between them; a common limit for all sandboxes is split off to task #260
  ([SPEC-272](SPEC-272-all-sandboxes-together-keep-the-server-a.md))
  (review round 3). Unreadable host values fall
  back to 8192.
- PID-002: When the sandbox is split into leaves, the `ci` and `engine` leaves each get
  `pids.max` = 3/8 of the sandbox's (at least 1): CI jobs and the engine with its containers
  together never take more than three quarters of the budget, and the agents' leaf (sessions,
  their tools, terminals, the Cockpit) always keeps at least a quarter. The `system` and `agents`
  leaves get no limit of their own. Without the split nothing changes. A leaf limit that cannot be
  written is logged and the sandbox runs on with its own limit.
- PID-003: A stopping server takes the task limits back from the leaves (`-pids`), as it takes the
  CPU back (CPU-205), so an older binary can join the sandbox group; the next preparation applies
  them again.
- PID-004: The isolation adapter reports a sandbox's limit counters. Kernel 6.8 counts a refused
  fork in the forking task's own group: `pids.events` `max` summed over the sandbox's groups
  (bounded to 4096) but the `ci` and `engine` subtrees, whose processes stop at their own leaf
  limits while sessions keep forking (review round 1); a paused run's `agents/run-*` group counts
  (round 2). Where the kernel has `pids.events.local` (6.13+, which counts where a limit was
  hit), the sandbox's own `pids.events.local` `max`. And `memory.events` `oom_kill` of the
  sandbox's group. None without the delegation. Amended by
  [SPEC-272](SPEC-272-all-sandboxes-together-keep-the-server-a.md) SHR-003: on 6.13+ the `.local`
  files of the same groups as on 6.8, and of the common `<unit>/sandboxes`; and by
  [SPEC-316](SPEC-316-limit-counters-count-forks-refused-at-th.md) LIM-001: also of `<unit>` (the
  service's own limit), never of an ancestor slice.
- PID-005: A run's counters are read when it starts (and when a restarted server takes it over).
  When it ends without Yulius ending it and not cleanly (`stopped_crashed`, `stopped_killed`), the
  counters are read again (only that attempt's start counters, left in place until its end's
  transition is won, review round 1): if the task counter rose, the attempt's `exit_reason` ends with
  `limit=pids`; else if the OOM counter rose, `limit=memory`. The exit sent to the task's viewers
  carries the same cause (`cause: "pids" | "memory"`).
- PID-006: The task's end banner and the attempt's state in the task's overview say the cause:
  "The agent stopped: the sandbox hit its process limit while it ran (too many processes and
  threads in this project's tasks at once)" and "The agent stopped: the sandbox ran out of memory
  while it ran" (overview: "the sandbox hit its process limit", "the sandbox ran out of memory"),
  from the exit frame or, after a load, the last run's exit reason (the banner is drawn again once
  the task's details arrive); without a cause they stay as they are.
- PID-007: Agent sessions, terminals, the Cockpit and CI jobs started where `yulius.toml` has no
  `[resources.env]` table (or there is no `yulius.toml`) get the default mapping
  `CARGO_BUILD_JOBS`, `NEXTEST_TEST_THREADS` and `RUST_TEST_THREADS` = `{cpus}`, except a
  variable whose knob the repository sets itself (`jobs = …` in `.cargo/config.toml` or
  `.cargo/config`, `test-threads = …` in `.config/nextest.toml`; the variable would override it,
  review round 1); a declared table,
  even an empty one, replaces it entirely, and an invalid one still sets only `YULIUS_CPUS` and
  `YULIUS_MEMORY_MB` (amends RES-004/RES-005).

## Plan

- [x] Spec and ADR state the cause and the design
- [x] Sandboxes get a task limit sized from the host
- [x] CI and the engine leave the agent sessions a reserve of the task limit
- [x] The isolation adapter reports a sandbox's limit counters
- [x] A session that ends at a limit records and sends why
- [x] The task view says the sandbox hit its process or memory limit
- [x] Repositories without `[resources.env]` get the Rust build and test defaults
- [x] Docs, gates, review and delivery

## Acceptance criteria

- AC-1 (PID-001): with `threads-max` 385 088 and `pid_max` 4 194 304 the default is 96 272; with
  unreadable values or a small host it is 8192; under a unit limited to 57 763 it is 43 320.
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

- `cgroup::tests` (fake cgroup trees): host-sized defaults within the service's limit; leaf limits written with the split, not without it; `-pids` on stop;
  counters for both kernel models, without CI's and the engine's subtrees.
- `supervision::a_session_that_ends_at_the_sandboxs_process_limit_says_so` with an isolation fake
  whose counters the test raises and a fake agent its own shell aborts (`/sh kill -ABRT $PPID`, SIGABRT as Bun): fails before PID-005.
- `supervision::an_agent_session_is_told_its_budget_in_the_repositorys_terms` extended for
  PID-007; `resources` unit tests for the default mapping.
- `isolation::isolation_limits` (opt-in, the VM's delegated unit) extended for AC-3; not runnable
  inside a sandbox, which has no cgroup mount.
- The UI: `task-done.e2e.ts` checks `endedMessage`/`limitCause` in the page and the rendered task
  view and overview of the mock's crashed task #3 (`limit=pids`).

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

## Evidence

Head before the review fixes: `415aa179`; commands run in this task's sandbox (Linux 6.8, 12 CPUs).

- Cause: not confirmable from here (no `/sys/fs/cgroup`, no apport log, quartio-v2's task and
  repository not reachable; see Context). The task's reproduction (Bun 1.4.3 aborts with 134 under a
  held process limit) and the code paths (one `pids.max` of 8192 for all of a sandbox's threads;
  134 classified as crashed) are the basis. Measured here: Claude Code runs 18 threads; the host's
  `threads-max` 385 088, `pid_max` 4 194 304.
- Tests that fail without the change, then pass:
  - `cgroup::tests::the_task_limit_is_sized_from_the_host`,
    `the_task_limit_stays_below_the_services`,
    `ci_and_the_engine_leave_the_agents_a_reserve_of_tasks` (failed: `+cpu` ≠ `+pids`),
    `without_the_split_the_leaves_get_no_task_limit`,
    `the_limit_counters_count_what_sessions_meet` (failed: 74 ≠ 6 before review fix 4).
  - `supervision::a_session_that_ends_at_the_sandboxs_process_limit_says_so` (failed:
    `exited code=None signal=Some(6)` had no `limit=pids`); waits for the exit frame.
  - `supervisor::tests::a_rise_of_a_limit_counter_is_the_runs_cause`,
    `a_runs_start_counters_are_its_own`.
  - `resources::tests::a_repository_without_a_mapping_gets_the_rust_defaults`;
    `supervision::an_agent_session_is_told_its_budget_in_the_repositorys_terms` (failed with the
    no-file default disabled: `ENV CARGO_BUILD_JOBS=set` missing).
  - e2e `task-done.e2e.ts`: "a session that ended at a sandbox limit says which", "a task that
    crashed at the sandbox's process limit says so in its banner and overview" and "the end
    banner names the limit once the task's details arrive after it" (holds the detail read; failed
    without `refreshBanner`: "observed: The agent stopped unexpectedly…"); 18/18 passed.
  - The counters test counts a paused run's `agents/run-t1` (13, the round-1 code gave 6) and reads `pids.events.local`.
- `scripts/dev ci` at `415aa179`: green, 2435 tests passed, 17 skipped (after a fixed spec link).
  At `35564424` (round-1 fixes): green, 2437 passed, 17 skipped; `scripts/dev security` exit 0.
  At `81c430df` (round-2 fixes): green, 2438 passed; security exit 0. At `fc3f1b9b` (the split,
  `origin/dev` merged): green, 2446 passed, 17 skipped.
- Full e2e suite at `81c430df` (the web assets are unchanged since): 667 passed, 1 skipped, 1
  failed: `secrets.e2e.ts` "a failed copy shows no check…" (axe target-size on a Close button),
  run while this task compiled beside it; the file then passed 3 of 3 alone. It is the known
  flaky test #640 (about 1 run in 5), on a screen this change does not touch.
- `scripts/dev security` at `415aa179`: exit 0, no open critical/high, no medium in the change.
  The first scan flagged the fake agent's existing `/sh` (high, code not written here), sent only
  because this branch had added `/abort`; that addition was dropped (the test aborts the agent
  with `/sh kill -ABRT $PPID`), so the file is no longer in the change. 8 medium/low findings in
  untouched code are filed by CI.
- Not run: `isolation::isolation_limits` (opt-in, needs the VM's delegated unit; extended for
  AC-3: a CI forker stops below `ci/pids.max` while the agents' shell forks); `docker run
  --pids-limit` under a split sandbox (this sandbox's engine group shows no controllers:
  `--cpus` is refused and `--pids-limit` discarded here), so per-container task flags are
  documented as unsupported.
- Review round 1 (fresh session): REQUEST CHANGES with 3 major and 6 minor findings, all own code;
  each fixed test-first or documented (service limit, Docker pids flags, repository-set knobs,
  CI/engine hits, per-attempt counters, test wait, banner redraw and view test, docs).
- Review round 2 (fresh session): REQUEST CHANGES with 3 major (paused runs' groups not counted,
  `RUST_TEST_THREADS` over Cargo's `[env]`, no failing test for the banner redraw), 3 minor (the
  6.13 counter model, the service shared by two sandboxes, stale text) and a nit (no test that an
  explicit `YULIUS_SANDBOX_PIDS` is not capped: two lines in `install()`, left as is); fixed
  test-first, the text corrected.
- Review round 3 (fresh session): REQUEST CHANGES with 2 major findings, both in round 2's common
  `<unit>/sandboxes` limit (on 6.13+ its hits are not in the sandbox's counters; a stale value
  when the service's limit goes), plus 2 minor findings and 4 nits. After the third round the
  change is split: the common limit is reverted here and goes, with the banner-focus minor and the
  nits (dotted Cargo keys, run groups removed by a thaw, the explicit-limit test), to task #260.
  The rest of round 3 verified the round-2 fixes F1–F4 and F6.
