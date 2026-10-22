---
yulius-task: 47d3f1c2-e4c1-43f1-8c9c-9f20104fd91a
status: in-progress
---

# SPEC-272: All sandboxes together keep the server a share of the service's tasks

Status: in-progress
Date: 2026-10-22
Owner: Yulius task #260
Scope authorization: task #260 (`/goal`), the follow-up split off task #254 after its third review
round: a common task limit for all sandboxes, and the round-3 leftovers. Amends
[SPEC-268](SPEC-268-parallel-tasks-in-one-sandbox-crash-clau.md) PID-001 and PID-004. Decision:
[ADR-0201](../../docs/decisions/0201-all-sandboxes-share-a-task-limit-below-the-services.md).

## Goal

However many sandboxes are busy, the server in `<unit>/server` keeps a quarter of
`yulius.service`'s task limit and can fork; a session that dies at that common limit still says
the limit was hit; and the small defects left by #254's review round 3 are fixed.

## Context

- **The gap.** Since SPEC-268 a sandbox's default `pids.max` is a quarter of the host's task table,
  at most three quarters of the service's own `pids.max` (systemd's `DefaultTasksMax`, 15 % of the
  table: 57 763 on the VM, so 43 320 per sandbox). Nothing bounds the sandboxes together: two
  saturated ones fill the service's 57 763 and the server can no longer fork (ADR-0198,
  Consequences).
- **The reverted attempt.** #254's round 2 wrote three quarters of the service's limit to
  `<unit>/sandboxes/pids.max` (`Delegation::cap_sandboxes`). Round 3 found two defects: on kernels
  with `pids.events.local` the counters never saw a hit at that limit, and with no service limit
  the old value stayed. It was reverted (`0a90db43`) and split off to this task.
- **How the kernel counts (read from `kernel/cgroup/pids.c` at v6.13).** `pids_try_charge` walks
  from the forking task's group up and refuses at the first group over its `pids.max`.
  `pids_event` then, by default, adds one to `pids.events.local` `max` of that over-limit group and
  to `pids.events` `max` of it and every ancestor; with the cgroup2 mount option
  `pids_localevents` (and on every kernel before 6.13, which has no `.local` file) `max` in both
  files counts the forks refused to tasks of that group itself, wherever the limit was. Under the
  defaults every sandbox's limit equals the common one (43 320 on the VM), so while any other
  sandbox holds a process, `sandboxes/` is the group at its limit, and a hit there shows in no file
  of the sandbox on a 6.13+ kernel. Ubuntu 24.04's HWE kernels are 6.14+; the VM runs 6.8.
- **Survives restarts.** `yulius.service` stops with `KillMode=process`: holders stay, so
  `<unit>/sandboxes` and its `pids.max` survive a restart, and the next server has to write the
  value it wants, `max` included.
- **Round-3 leftovers.** (a) `taskview.js` calls `terminal.refreshBanner()` on every detail read
  (resync, steps, status changes); it rebuilds the end banner and its Continue / Try again buttons,
  and a keyboard user on one of them loses focus to `<body>`. (b) `resources.rs` `sets_own` only
  sees `key =` lines: `RUST_TEST_THREADS.value = "1"` in Cargo's `[env]`, `build.jobs = 2` at the
  top, `[env.RUST_TEST_THREADS]` and `profile.default.test-threads = 1` are missed, so the default
  variable overrides the repository's own setting. (c) On 6.8 the counters add up `pids.events` of
  the sandbox's groups, `agents/run-*` among them; `freeze::thaw_all` removes empty run groups, so a
  group with hits removed during another run makes the end total fall below the start total and
  can hide a smaller real rise. (d) No test shows that an explicit `YULIUS_SANDBOX_PIDS` is not
  capped by `within_service`.

## Requirements

- SHR-001: With the `pids` controller delegated, the server writes `<unit>/sandboxes/pids.max` at
  every start: three quarters of the service's limit (`tasks_max_above`, at least 1), or `max` when
  the service has none (`TasksMax=infinity`), so a value left by an earlier server never stays
  stale. A write that fails is logged and the server runs on.
- SHR-002: The per-sandbox default stays as SPEC-268 PID-001 sets it. An explicit
  `YULIUS_SANDBOX_PIDS` is not capped by the service (`within_service` is not applied to it); when
  it is above the common limit, the server logs a warning naming both figures, since the common
  limit then binds each sandbox too. operations.md says so.
- SHR-003: The limit counters (SPEC-268 PID-004) see a fork refused at the common limit. Where the
  sandbox has `pids.events.local`, they add `pids.events.local` `max` of every group of the sandbox
  but the `ci` and `engine` subtrees (bounded to 4096 groups, as on 6.8), and of
  `<unit>/sandboxes`. By default that counts the sandbox's own limit and the common one; with
  `pids_localevents` it counts the forks refused to the sessions' own groups, like 6.8. The common
  group is shared, so its hits are attributed to every sandbox whose run ends crashed or killed
  meanwhile. Without `.local` (6.8) nothing changes: the forking group counts, wherever the limit.
- SHR-004: A run group whose `pids.events` or `pids.events.local` `max` is above 0 is not removed by
  `freeze::thaw_all`, so the counters never fall while the server lives (systemd removes empty
  groups when the service stops; a restarted server reads a run's counters again, PID-005).
- SHR-005: A default variable is left out (SPEC-268 PID-007) when its key appears as a component
  of a dotted key path in the tool's file: before `=` on a key line, or in a `[table]` header,
  bare or quoted (`RUST_TEST_THREADS.value = "1"`, `env.RUST_TEST_THREADS = "1"`,
  `[env.RUST_TEST_THREADS]`, `build.jobs = 2`, `profile.default.test-threads = 1`). Values and
  comments are not keys.
- SHR-006: A detail read that redraws the end banner keeps keyboard focus on the same control of
  the banner (`keepFocus`).

## Plan

- [x] Spec and ADR state the sandboxes' common task limit
- [x] All sandboxes together stop at three quarters of the service's tasks
- [x] A session that ends at the common limit says so
- [ ] A removed run group no longer hides a rise in the counters
- [ ] Dotted keys in Cargo and nextest files keep their knob
- [ ] The end banner keeps keyboard focus when details arrive
- [ ] Docs, gates, review and delivery

## Acceptance criteria

- AC-1 (SHR-001): a fake delegated tree gets `sandboxes/pids.max` = 43320 for a service limit of
  57 763, and `max` again when the service then has none; without the `pids` controller nothing is
  written.
- AC-2 (SHR-002): with an explicit 50 000 under a 57 763 service, the sandbox's limit stays 50 000
  and the common one is 43 320 (the server warns); the default is capped to 43 320.
- AC-3 (SHR-003): in a fake tree where only `sandboxes/pids.events.local` records a hit (the
  sandbox's own `.local` file says 0), the sandbox's task counter rises; with `pids_localevents`
  counts in `agents/` count; CI's and the engine's do not.
- AC-4 (SHR-004): `thaw_all` keeps an empty run group with hits and removes an empty one without.
- AC-5 (SHR-005): each dotted form above leaves its default out; a value or a comment naming the key
  does not.
- AC-6 (SHR-006): in the browser, focus on the banner's Continue / Try again button stays on it
  after a resync redraws the banner.

## Test plan

- `cgroup` unit tests in `yulius-sandbox` with fake trees: the common limit and its reset (AC-1),
  the counters with only `sandboxes/` recording the hit and in local-events mode (AC-3).
- A unit test of the wiring's task-limit choice in `crates/yulius/src/wiring/sandbox.rs` (AC-2).
- A `freeze` unit test (AC-4); a `resources` unit test (AC-5).
- An e2e test in `e2e/tests/task-done.e2e.ts`: focus the banner's button on the mock's task #3,
  `resyncNow()`, wait for the banner to be rebuilt, and check focus (AC-6).
- Each test is run before its fix and fails. Not testable here: a real 6.13+ kernel (the VM runs
  6.8); the counters on it rest on the kernel source read above.

## Decisions

- **Three quarters of the service, written at every start, `max` without one** (ADR-0201). The same
  share as the per-sandbox cap, so the default sandbox loses nothing alone, and the server keeps a
  quarter (14 443 on the VM) whatever the sandboxes do. A fixed number, or a share of the host,
  would not follow `TasksMax=`.
- **Counting the common group's hits for every sandbox** rather than none: a run that ended crashed
  while the shared limit was hit most likely died of it, and "the sandbox hit its process limit"
  points the operator at the right knob; attributing a hit to the exact sandbox would need a
  group per sandbox above the common one, which is the sandbox's own group again.
- **Local files for the whole walk on 6.13+**, not only the sandbox's own: the same code then
  covers the default mode and `pids_localevents` (round 3's nit), at the cost of a walk the 6.8
  path already does.
- **Keep run groups with hits** rather than compare counters per group: the port's
  `LimitCounters` stays two totals, and such groups are rare (a run paused for an update that met a
  limit) and go when the service stops.
- **A warning for an explicit limit above the common one** rather than capping it: the setting
  stays what the operator wrote, and the log says what binds.
- **`keepFocus` around the banner's redraw** rather than skipping redraws: the exit actions can
  change with the details too (an archived task, a retry), so the banner is redrawn and the focus
  restored, as the delivery strip does.
