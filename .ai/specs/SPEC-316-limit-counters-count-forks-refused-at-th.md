---
yulius-task: 28319e5f-57d1-4318-955a-bc5a03ea6d40
status: in-progress
---

# SPEC-316: Limit counters count forks refused at the service's own task limit

Status: in-progress
Date: 2026-10-25
Owner: Yulius task #302
Scope authorization: task #302 (`/goal`), fixes #823. Amends
[SPEC-268](SPEC-268-parallel-tasks-in-one-sandbox-crash-clau.md) PID-004 and
[SPEC-272](SPEC-272-all-sandboxes-together-keep-the-server-a.md) SHR-003. Decision: amends
[ADR-0201](../../docs/decisions/0201-all-sandboxes-share-a-task-limit-below-the-services.md).

## Goal

On kernels with `pids.events.local` (6.13+), a session that aborts because a fork was refused at
`yulius.service`'s own `pids.max` (`TasksMax`) says the sandbox hit its process limit, as it
already does on 6.8, instead of "The agent stopped unexpectedly".

## Context

- **Symptom.** `Delegation::limit_counters` (`crates/yulius-sandbox/src/cgroup.rs`) is the only
  source of a run's `limit=pids` cause (SPEC-268 PID-004, PID-005): the run ends crashed or killed
  and the sandbox's `task_limit_hits` rose meanwhile.
- **Root cause.** The kernel refuses a fork at the first group up the tree whose `pids.max` is
  reached. On 6.8 it counts the refusal in `pids.events` `max` of the forking task's own group,
  which the counters read for every group of the sandbox, wherever the limit was. Since 6.13 the
  default mount counts it in `pids.events.local` `max` of the group whose limit was hit. The
  counters read the `.local` files of the sandbox's groups and of `<unit>/sandboxes` (SHR-003),
  never `<unit>/pids.events.local`. A fork refused at the unit's limit changes none of them.
- **Why tests missed it.** The fake trees (`fake_sandbox`) write `.local` files only for the
  sandbox's groups and `sandboxes/`; none wrote `u/pids.events.local`, and the VM runs 6.8.
- **How often.** Rarer since SPEC-272: `sandboxes/` stops at three quarters of the service's limit,
  so the unit's limit is reached only when the server itself (`<unit>/server`) uses more than its
  quarter. When it is reached, every fork in the unit, a sandbox's or the server's, is refused.
- **Layout.** `prepare` delegates `<unit>` (the service's group); its children are `server/` and
  `sandboxes/`. With `pids` enabled in its subtree the unit holds no task itself (cgroup v2's
  no-internal-process rule), so with `pids_localevents` its `.local` `max` stays 0.
- **Ancestors.** Above `<unit>` lie slices (`system.slice`, `-.slice`) that the delegation does not
  own. Their `.local` files are usually readable, but they count every service's forks refused at
  their limit, and they normally have none (`system.slice` has no `TasksMax` by default).

## Requirements

- LIM-001: Where the sandbox has `pids.events.local`, the limit counters also add
  `<unit>/pids.events.local` `max`: a fork refused at the service's own limit is a task-limit hit
  for every sandbox whose run ends crashed or killed meanwhile, like one refused at the common
  `sandboxes/` limit (SHR-003). Without `.local` (6.8) nothing changes.
- LIM-002: Limits above `<unit>` are not read: ancestor slices are outside the delegation and
  count other services' forks.
- LIM-003: SPEC-268 PID-004, SPEC-272 SHR-003, ADR-0201, the doc comment of `limit_counters` and
  `docs/architecture.md` say which limits are counted: on 6.13+ the sandbox's own, the common
  `sandboxes/` one and the service's; never an ancestor slice's.

## Plan

- [x] Spec and ADR-0201 record which limits are counted
- [ ] A fork refused at the service's limit counts for the sandbox
- [ ] Independent review answered and gates green

## Acceptance criteria

- AC-1 (LIM-001): in a fake tree with `pids.events.local`, `u/pids.events.local` `max 6` and
  every other `.local` file at 0, `limit_counters("s1").task_limit_hits` is 6; the test fails on
  dev 9e1a2870.
- AC-2 (LIM-001): on 6.8 (no `.local` file) `u/pids.events` is not read.
- AC-3 (LIM-002): a `.local` file in the unit's parent directory does not count.
- AC-4 (LIM-003): the documents name the three limits counted.

## Test plan

- Unit (fake cgroup tree, `crates/yulius-sandbox/src/cgroup.rs`):
  `a_hit_at_the_service_limit_counts_for_the_sandbox` covers AC-1 to AC-3; written first and run
  against the unfixed code.
- The existing `the_limit_counters_count_what_sessions_meet` and
  `a_hit_at_the_common_limit_counts_for_the_sandbox` keep passing.
- Gate: `scripts/dev ci`.
- Not testable here: a real 6.13+ kernel (the VM runs 6.8) and a real `TasksMax` hit; the fake tree
  and the kernel's documented `pids.events.local` semantics are the proof.

## Decisions

- **Hits at `<unit>` count** (ADR-0201, amended). The trade-off is a false attribution against a
  missed one. When the unit's limit is reached every fork in it is refused, the sandboxes' too, so
  a session that crashes meanwhile very likely died of it; saying "the sandbox hit its process
  limit" points at the task limit, the right knob (`TasksMax=`, docs/operations.md). The false
  case is a run that crashes for another reason while only the server's forks were refused: a
  coincidence, since the unit is reached only when the server uses more than its quarter and the
  sandboxes' forks are refused then too. SHR-003 already
  accepts the same over-attribution for the shared `sandboxes/` limit. Alternative rejected: not
  counting, and recording why. It keeps 6.13+ weaker than 6.8 for the case that matters most,
  a whole service out of tasks.
- **Ancestors are not read** (LIM-002): outside the delegation, shared with other services, and
  without a limit by default. A `TasksMax` on `system.slice` stays uncounted on 6.13+; that is an
  operator's own setting, and the kernel cannot tell which service forked from it.
- **On 6.8 the server's own refused forks are not attributed** (they count in `server/`, which is
  not read); on 6.13+ they are, through `<unit>`. Accepted: the asymmetry only adds hits when the
  whole service is at its limit.
