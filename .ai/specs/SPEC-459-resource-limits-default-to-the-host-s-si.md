---
yulius-task: daf7a54d-e960-47e4-81cd-8c61824a49b3
status: in-progress
---

# SPEC-459: Resource limits default to the host's size and adapt after a move

Status: in-progress
Date: 2026-11-11
Owner: Yulius task #441
Scope authorization: task #441, asked by the project's owner ("Resource limits size themselves to
the host by default, and adapt when the instance lands on a different host").

## Goal

An instance's resource limits fit the machine it runs on without anyone editing a unit file: their
defaults come from the host's CPUs, memory and data disk, a superadmin changes them in Settings →
Instance, and a number pinned for another machine is noticed after a move and replaced by the
host-sized one, by the migration flow itself.

## Context

- Six instance limits decide how much work a host takes: live agent sessions
  (`YULIUS_MAX_SESSIONS`), CI jobs at once (`YULIUS_CI_MAX_JOBS`), CI runs at once per sandbox
  (`YULIUS_CI_LANES`), a sandbox's memory (`YULIUS_SANDBOX_MEMORY_GB`), its soft disk quota
  (`YULIUS_SANDBOX_DISK_GB`) and the disk bound of a CD or environment run
  (`YULIUS_CD_RUN_DISK_GB`). Only the memory follows the host (host memory minus an eighth,
  `Limits::defaults`); the others are fixed clap defaults (8, 4, 2, 100 GB, 20 GB) in
  `crates/yulius/src/serve.rs` and `crates/yulius/src/wiring/sandbox.rs`, tuned on a 12-CPU VM.
- Every one of them is read from the immutable start configuration (`AppConfig`, `CiSettings`,
  `IsolationConfig`), so a change needs a unit edit and a restart, and nothing says where a value
  came from.
- Real case (2026-11-11): the old host (12 CPUs, 47 GiB, 387 GB disk) had drop-ins
  `MAX_SESSIONS=8`, `SANDBOX_MEMORY_GB=38`, `SANDBOX_DISK_GB=300`. Copied as-is to the new host
  (64 CPUs, 755 GiB, a 3.5 TB data mirror) they kept 9 tasks queued on an idle machine. Set by hand
  to `MAX_SESSIONS=32`, `SANDBOX_DISK_GB=1000`, `CI_MAX_JOBS=12`, `CI_LANES=4`, memory back to the
  default.
- A setting that wins over the unit exists for the Jev and Vidoc keys (`jev_key.rs`,
  `vidoc_key.rs`): saved in the `instance` row, superadmin-only, applied at once, audited.
- `yulius instance import` (SPEC-329) puts a copy on a new host and today tells the person to copy
  the old unit's limits by hand. Task #442 (continuing #439) builds the one-command move on it and
  expects the limits to be re-sized for the target by this task.
- Out of this task: `YULIUS_SANDBOX_PIDS` (already sized from the host's task table and checked
  against the service's `TasksMax` at every start, ADR-0198, ADR-0201) and `YULIUS_SANDBOX_CPUS`
  (an optional cap with no default).

## Requirements

- HSL-001: without a pin, each limit is sized from the host (integers, rounded down; `cpus`
  logical CPUs, memory in GiB, the data directory's filesystem in GB):
  - live sessions: `cpus / 2 + 2`, at most one per 2 GiB of memory, at least 2;
  - CI jobs: `cpus / 6 + 2`, at most 64;
  - CI runs per sandbox: `cpus / 32 + 2`, at most 8;
  - sandbox memory: the memory minus an eighth (at least 4 GiB kept back), at least 2 GiB
    (unchanged);
  - sandbox disk quota: a quarter of the data disk, at least 100 GB but never more than half the
    disk;
  - CD run disk: a twentieth of the data disk, at least 20 GB but never more than half the disk.
  A fact the host does not report leaves that limit at its earlier fixed default. The 12-CPU VM
  keeps 8, 4, 2, 100 GB, 20 GB; the 64-CPU host gets 34, 12, 4, 875 GB, 175 GB.
- HSL-002: a limit's value in force is, in this order: a value saved in Settings; else, with
  "host-sized" saved in Settings, the host-sized value (the unit's number is ignored); else the
  unit's `YULIUS_*` value; else the host-sized value. Its source reads `settings`, `unit` or
  `default`.
- HSL-003: every pinned number is recorded with the host (CPUs, memory, data disk) it was
  configured on: a unit value when the server first sees it, a Settings value when it is saved.
  A unit value seen again with the same number keeps its record, also after the unit dropped it
  for a while.
- HSL-004: a pin no longer fits when a fact its formula uses differs from this host's by a quarter
  or more (either way) and its number differs from this host's host-sized value. The instance says
  so at start (a warning naming each such limit, its number, the host it was configured on and
  the host-sized value), in the limits view and on Settings → Instance, where one action takes the
  host-sized values for all of them and another keeps them (recording them for this host).
- HSL-005: the migration flow applies the host-sized values by itself: `yulius instance import`
  marks the copy (`<data>/.yulius-limits-adapt`), and the first start that finds the mark saves
  "host-sized" for every pin that no longer fits, recorded unit pins included, so a drop-in copied
  later is already overruled. It is audited (`limits.adapted`, by `migration`) and the mark is
  removed. The mark on an unchanged host changes nothing.
- HSL-006: a superadmin reads the limits (`GET /api/admin/limits`), saves a number or "host-sized"
  for one (`PUT /api/admin/limits/{key}`), removes its setting (`DELETE`), takes the host-sized
  values for every pin that no longer fits (`POST /api/admin/limits/adapt`) or keeps them
  (`POST /api/admin/limits/keep`). Everyone else gets 403. A number outside the limit's range, or
  a sandbox memory above the host's, is refused with the range named. Each change is audited.
- HSL-007: a change applies without a restart for live sessions (a raise starts queued tasks at
  once), CI jobs (running jobs finish; the count in force never exceeds the larger of the old and
  the new number), CI runs per sandbox, the sandbox disk quota and the CD run disk bound (from the
  next run). The sandbox memory applies at the next restart, and the view says so while the saved
  and the running value differ.
- HSL-008: Settings → Instance shows a "Resource limits" card: the host's size, and per limit its
  value, its source, the formula in words with the host-sized value, the unit's number when it is
  overruled, who saved a setting and when, and the controls of HSL-006.
- HSL-009: saved settings and records survive a restart; a database of an earlier version starts
  with none (every unit value is then recorded for the host of that first start).

## Plan

- [x] Spec, ADR-0294 and the failing formula tests
- [x] Host-sized formulas, precedence and the pinned-on-another-host rule in the domain
- [x] The limits record in the store (migration 0051) and the start-up load that records unit pins with their host
- [x] Sessions, CI lanes and the CD run disk read the live limits, and CI job slots resize while jobs run
- [x] The sandbox disk quota changes live, and memory and disk settings reach the isolation at start
- [x] Superadmin API: read, set, host-sized, remove, adapt and keep, audited
- [x] `instance import` marks the copy and the first start adapts the limits by itself
- [ ] Settings → Instance card with values, sources, formulas and the moved-host notice
- [ ] Docs: operations, API, architecture, product summary and changes
- [ ] Gates green and an independent review in a fresh session

## Acceptance criteria

- On a host reporting 12 CPUs, 47 GiB and 387 GB the limits without pins are 8, 4, 2, 41 GiB,
  100 GB and 20 GB; on 64 CPUs, 755 GiB and 3.5 TB they are 34, 12, 4, 660 GiB, 875 GB and 175 GB;
  on 2 CPUs, 4 GiB and 40 GB they are 2, 2, 2, 2 GiB, 20 GB and 20 GB.
- A server started with `YULIUS_MAX_SESSIONS=8` on the small host and then on the large one reports
  the limit as `unit`, 8, not fitting, configured on 12 CPUs and 47 GiB, host-sized 34; "adapt"
  makes it 34 from `default` with the unit's 8 shown as overruled, and "keep" instead leaves 8
  without the notice.
- With the import's mark present, the same second start ends with 34 in force without any call,
  an audit entry `limits.adapted` and no mark; the old host's `SANDBOX_DISK_GB=300` copied into
  the unit afterwards stays overruled.
- With the session limit at 1, one task running and one queued, saving 2 in Settings starts the
  queued task without a restart; a non-superadmin's save is refused (403); `0` sessions and 65 CI
  jobs are refused with the range.
- With one CI job slot held, lowering the jobs from 2 to 1 lets no second job start until the
  first ends; raising to 3 lets two more start at once.
- After a restart on the same store the saved settings, sources and records are the same.
- The Instance page shows the card with each value's source and formula; on a moved host its
  notice lists the limits that do not fit, and "Use host-sized values" clears it.

## Test plan

- Domain unit tests (`yulius-domain::limits`): the formulas at 1, 2, 4, 12, 64 and 256 CPUs with
  small and large memory and disks, unknown facts, the clamps; precedence; the quarter threshold
  either way, per fact; a pin equal to the host-sized value is never flagged.
- Store test: migration 0051 on an earlier database, the record's round trip.
- Application tests on a real store with a fake system probe (`crates/yulius/tests/limits.rs`):
  the acceptance criteria above (start on two hosts, mark, adapt, keep, settings over unit,
  refusals, audit, restart, the queued task starting).
- Unit tests of the CI job slots (shrink while held, grow) and of the sandbox manager's quota
  changing live; `serve` argument tests that the five options have no fixed default.
- Import: the mark is written for the service user at the end of a successful import.
- Browser tests on the mock (`e2e/tests/instance-limits.e2e.ts`): the card, a saved value, the
  notice and both of its actions.
- Gates: `scripts/dev ci`, the e2e suite, `yulius security-scan` (Vidoc is off for this project:
  reported as disabled).
- Not testable here: a real move between two machines; the detection is tested with a fake probe
  reporting the two real hosts' sizes.

## Decisions

- ADR-0294 records the lasting ones: the formulas and their calibration on the two real hosts,
  "host-sized" as a setting of its own (so an adapted limit keeps following the host instead of
  being pinned again), per-pin host records, the quarter threshold, the import's mark, and memory
  applying at a restart (the CI budget pools and each sandbox's `memory.max` are sized at start).
- The limits are one JSON document in the `instance` row (`limits`), not six columns: they are
  read and written together and get new members without a migration.
- No new CLI command: the server knows the unit's pins only while it runs, so the adaptation
  happens at its start, and the migration flow's part is the mark its import leaves.
- One pull request although it is past 400 lines outside tests: defaults, sources, settings and
  the moved-host handling share one record and one view, and none of them is usable alone.
