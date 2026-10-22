# 0198. Sandbox task limits are sized from the host and leave sessions a reserve

## Status

Accepted. Specified in [SPEC-268](../../.ai/specs/SPEC-268-parallel-tasks-in-one-sandbox-crash-clau.md);
task #254. Amends [0003](0003-sandbox-isolation.md) (SPEC-002 ADR-22 §5: a fixed 8192 tasks per
sandbox), [0039](0039-ci-jobs-get-the-cpu-first.md) (only the CPU went down to the leaves) and
[0065](0065-processes-plan-for-a-cpu-and-memory-budget.md) (Yulius set no tool's variable).

## Context

Every sandbox had one `pids.max` of 8192 for all its threads: every agent's builds and tests, CI
jobs, the Docker engine and its containers. Five tasks of one project building Rust and running
tests with Chromium and mail containers used it up, and a Claude Code session that needed a thread
or a process at that moment died: Bun aborts (exit 134) when a fork or thread creation keeps
failing. Yulius showed it as "The agent stopped unexpectedly", with nothing saying that a limit
was hit. Repositories without `[resources.env]` also built with one job per host CPU in every
task.

## Decision

- **Sized from the host.** A sandbox's default `pids.max` is a quarter of the smaller of
  `kernel.threads-max` and `kernel.pid_max`, at least 8192 (96 272 on a 47 GB host), and at most
  three quarters of `yulius.service`'s own `pids.max` (systemd's `DefaultTasksMax`, 15 % of the
  table: 57 763 on that host, so 43 320), and all sandboxes together get the same three quarters
  on `<unit>/sandboxes`, so the server keeps a quarter however many are busy.
  `YULIUS_SANDBOX_PIDS` still sets a sandbox's.
- **CI and the engine leave the sessions a reserve.** With the CPU split, the sandbox also passes
  `pids` to its leaves; `ci` and `engine` each get 3/8 of the sandbox's limit, so together they
  leave the `agents` leaf at least a quarter. A stopping server takes `pids` back with the CPU.
- **A crash at a limit says so.** The adapter reports the forks refused to the sandbox's sessions:
  on kernel 6.8 `pids.events` `max` of its groups but CI's and the engine's (whose leaf limits
  stop them, not the sessions), on kernels with `pids.events.local` the sandbox's own limit hits;
  and its `memory.events` `oom_kill`. A run that
  ends crashed or killed while one of them rose records `limit=pids` or `limit=memory` in its exit reason and in the exit its viewers
  get, and the task view says the sandbox hit its process limit or ran out of memory.
- **A default mapping where a repository has none.** Without a `[resources.env]` table, processes
  get `CARGO_BUILD_JOBS`, `NEXTEST_TEST_THREADS` and `RUST_TEST_THREADS` = `{cpus}`, but not one
  whose knob the repository sets in its tools' own files (`jobs` in `.cargo/config.toml`,
  `test-threads` in `.config/nextest.toml`); a declared table, even an empty one, replaces it.

## Alternatives

- **Only a bigger fixed number.** Wrong on small hosts (a fork bomb takes the host's table) and on
  large ones (still small), and it leaves no reserve when containers or CI leak threads.
- **A group per agent run, or for its tool commands.** Claude Code starts every Bash command
  itself inside the sandbox, which has no cgroup mount: a run's group would hold the session and
  its own build together, and moving each command into a tools group needs the server to move it
  on a call per command. A per-run group only splits sessions from each other while overcommitting
  the sandbox's limit, so it reserves nothing; kept for later if per-run attribution is needed.
- **Partitioning the whole limit (agents, CI, engine each a fixed third).** Guarantees each its
  share but wastes two thirds when only agents work, the common case.
- **Treating exit 134 as a limit.** Any abort exits 134; only the counters say a limit was hit.

## Consequences

- The incident's load fits five times over (43 320 on the VM's default unit; twelve times with
  `TasksMax=` raised); a leak in CI or in containers stops at its leaf
  without taking the sessions' quarter. A session's own tools can still use up the agents' share
  and kill it; the task then says the limit was hit.
- The engine's containers see `pids` among their available controllers. Per-container task flags
  (`--pids-limit`, compose `pids_limit`) join the CPU flags as unsupported: no controller passes
  below the engine's group, which holds `dockerd` (SPEC-187 DKR-005), so they may make a
  container fail to start where they were discarded before.
- Sandboxes together stop at three quarters of the service's limit, so the server keeps a
  quarter; under systemd's default `TasksMax` one saturated sandbox can still take the whole of
  the sandboxes' share from the others.
- A server restart loses a run's starting counters; the server that takes the run over reads them
  again, so a limit hit before the restart is not attributed.
- Rust repositories without `[resources.env]` build and test with their budget, not the machine.
