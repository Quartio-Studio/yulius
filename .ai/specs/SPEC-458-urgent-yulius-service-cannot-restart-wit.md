---
yulius-task: 43a4ea9a-8f2b-4c87-9cb9-9376e150eb5f
status: draft
---

# SPEC-458: The server restarts with live sessions on systemd 258 and later

Status: draft
Date: 2026-11-11
Owner: Yulius task #440
Scope authorization: task #440 ("URGENT: yulius.service cannot restart with live sessions on
systemd 259 (Ubuntu 26.04)") and its addition about task #439 ("Include recovery from an
externally killed run in the fix"). Decision: [ADR-0298](../../docs/decisions/0298-sessions-live-in-their-own-unit.md).
Builds on SPEC-053 (per-sandbox cgroups), SPEC-203 (`yulius host setup`), SPEC-002 ADR-18 (the
update helper), SPEC-329 (the export's freeze) and SPEC-065 (ADR-0050).

## Goal

Restarting `yulius.service` keeps every agent session, on Ubuntu 24.04 (systemd 255) and on
Ubuntu 26.04 (systemd 259): sessions and sandboxes live in their own delegated unit that a
restart of the server never touches. A host that is not set up for it is detected by `yulius
doctor` and `yulius host check`, and Restart and Update now refuse there instead of taking the
instance down. A run whose processes were killed behind the server's back no longer leaves its
task stuck.

## Context

### What happened

2026-11-11, 11:24 to 11:28 UTC, production (Ubuntu 26.04.1, systemd 259.5, kernel 7.0):
`systemctl restart yulius` stopped the server; `KillMode=process` left 87 processes in
`/sys/fs/cgroup/system.slice/yulius.service/sandboxes/…`. Every start failed with `Failed to spawn
executor: Device or resource busy` until every one of them was killed by hand.

### Root cause

A cgroup v2 group that passes controllers to its children (`cgroup.subtree_control` not empty)
takes no process (the no-internal-processes rule, `EBUSY`). The server's unit is delegated
(`Delegate=yes`, `DelegateSubgroup=server`) and passes `cpu memory pids` to `sandboxes/`, which
survives the stop with its processes.

- systemd 255 forks the service's process in its own group and then attaches it to
  `<unit>/server`: no process ever enters `<unit>`.
- Since systemd 258 the manager starts `systemd-executor` with `CLONE_INTO_CGROUP` into the
  unit's own group (`<unit>`), and only the executor moves on to the subgroup. The kernel
  refuses the clone, for `ExecStart=` and for `ExecStartPre=` alike. Upstream `main` still does
  it. It is not 256: 256 and 257 spawn into the subgroup.

Reproduced with a scratch unit in QEMU VMs (Ubuntu 24.04, systemd 255.4; Ubuntu 26.04, systemd
259.5): same unit settings, one survivor in `sandboxes/x/agents`; the restart works on 255.4 and
fails on 259.5 with the production messages and state (`type domain`, `subtree_control = cpu
memory pids`, no process in the unit's root, no `server` child).

### What the experiments settled

- A second delegated unit with one keeper process keeps its group whatever the server does; a
  oneshot with `RemainAfterExit=yes` does not (systemd removes its empty group).
- The service user cannot write a process of the server's unit into another unit: the kernel
  wants write access to `cgroup.procs` of the common ancestor, `system.slice`, which is root's.
  With both units in one slice of their own whose `cgroup.procs` the service user owns, the
  plain write works on both versions, so every existing join (`cgroup::join`, `move_pid`) is
  unchanged. `AttachProcessesToUnit` over D-Bus works too, but every joiner would need the
  system bus, and `sandbox-exec` joins after it entered the sandbox's namespaces.
- Nothing inside the server's unit can heal it on 259 (`ExecStartPre=+` fails like the start).
  A root oneshot unit that the server's unit `Wants=` and is ordered `After=` runs before every
  start, also one by `Restart=on-failure`, and can move the survivors out: the restart then
  works from the old layout, across the `Slice=` change, and systemd removes the old group.

### Task #439

After the cleanup its task was `failed_precondition` with a live run on record: Restart said
"task already has a live run", a message "continue the task first", Stop changed nothing. A
start that finds a live run on record fails at `insert_attempt` and nothing looks at that run
again; a resting task's Stop is refused before it looks either. Which path left the record
live could not be read from production (no access); the code leaves it live when a launch fails
after its spawn (`attach_run` or a store write), and nothing recovers it later.

## Requirements

### Layout (ADR-0298)

- **LAY-001** Sessions unit: `<stem>-sandboxes.service` for the server unit `<stem>.service`
  runs one keeper (`/usr/bin/sleep infinity`) as the service user with `Delegate=yes`,
  `DelegateSubgroup=keeper`, `OOMScoreAdjust=-1000`, in the slice `<name>.slice`, where `<name>`
  is the stem with every character but a letter or a digit replaced by `_` (a dash would nest
  the slice). Its `ExecStartPre=+` gives the slice's `cgroup.procs` to the service user. It keeps
  the default `KillMode`: stopping it ends every session, deliberately.
- **LAY-002** `cgroup::detect` prefers the sessions unit: for a server in
  `<slice>/<stem>.service/<leaf>` it returns `<slice>/<stem>-sandboxes.service` when that
  directory, its `cgroup.procs` and `cgroup.subtree_control` and the slice's `cgroup.procs` are
  writable; else the server's own unit as before (hosts not set up, development).
- **LAY-003** The server's unit gets a second drop-in, `20-yulius-sessions.conf`: `Slice=`,
  and `Wants=` and `After=` for the sessions unit and the adopt unit.
- **LAY-004** Adopt unit: `<stem>-adopt.service`, a root oneshot (`Requires=` and `After=` the
  sessions unit) running `yulius host adopt-sessions --service <unit>` before every start of the
  server. It mirrors `<old unit>/sandboxes` into the sessions unit (same relative paths,
  controllers passed down, `cpu.max`, `cpu.weight`, `memory.max`, `memory.high`, `pids.max` of
  groups it creates, owned by the service user), moves every process, removes the emptied groups
  and takes the controllers from the old unit. The old unit is the one systemd names
  (`ControlGroup`), else `system.slice/<unit>` and `<slice>/<unit>`. Bounded: 4096 groups, depth
  8. Nothing to adopt is success.
- **LAY-005** A sandbox's `engine` group is not moved: its Docker engine runs in a cgroup
  namespace rooted there and could not manage containers from elsewhere. Adopt kills it
  (`cgroup.kill`), waits up to 10 s for it to empty and removes it; the engine starts again on
  demand as after a reboot. Said in the operations guide.
- **LAY-006** The needrestart rule also names the sessions unit, so a package install never
  restarts it.

### Host setup, check and doctor

- **HST-001** `yulius host setup` writes the two units and the drop-in when they differ,
  reloads systemd and starts the sessions unit; a running server is not restarted, and a manual
  line says the next restart moves the sessions and keeps them. The item is unsafe: `host check`
  only reports it (failed, "run `sudo yulius host setup`"), since the update helper cannot write
  unit files.
- **HST-002** `yulius doctor` has a line "restart with sessions": ok with the sessions unit in
  use; a warning naming `sudo yulius host setup` when sessions live in the server's unit, which
  says that a restart fails while sessions run when systemd is 258 or later.

### Restart and Update now

- **RST-001** `restart_safety::blocker`: a restart of the server's unit would fail when systemd
  is 258 or later, `<unit's group>/sandboxes` has processes, and the unit does not `Wants=` its
  adopt unit. Pure over the facts (version text, `cgroup.events`, `systemctl show`).
- **RST-002** The update helper checks it before it stops the service, for `restart` and
  `install`: it fails the request with the reason and stops nothing.
- **RST-003** The server checks it when Restart or Update now is asked: `409` with the reason,
  before any task is paused or anything is built.

### Export

- **EXP-001** `yulius instance export` freezes the sessions unit together with the server's
  unit and thaws both; a stopped server whose sessions unit is active is frozen there instead
  of being refused; `--move` leaves both frozen.

### A run killed behind the server (#439)

- **RUN-001** A start (Continue, Restart) that finds a live run on record which this server is
  not attached to settles it first, as a restarted server does (ADR-0050): exited, ended and
  counted; host or agent alive, terminated and then lost; gone, lost. A run that cannot be
  ended still refuses the start.
- **RUN-002** Stop on a resting task with such a run settles it the same way instead of
  answering "task is …".

## Plan

- [ ] The server uses the sessions unit when the host has one (cgroup detection)
- [ ] `yulius host adopt-sessions` moves the sessions of the old layout out of the server's unit
- [ ] Host setup writes the sessions unit, the adopt unit and the drop-in; host check and the doctor report it
- [ ] Restart and Update now refuse where the restart would fail (helper and server)
- [ ] The export freezes the sessions unit too
- [ ] A start or a stop settles a live run whose host is gone (#439)
- [ ] Proof on real hosts: restarts, the helper's restart and rollback, the first restart that installs the fix (systemd 255 and 259)
- [ ] ADR-0298, the operations guide, the architecture document and the change entry
- [ ] Gates and an independent review of the final head

## Acceptance criteria

- On systemd 259.5 and 255.4, with the layout set up, `systemctl restart <unit>` and a `kill -9`
  of the server with `Restart=on-failure` keep every session process; the server comes back
  active and attaches to them.
- On a host with the old layout and live sessions, after `host setup` from the new binary the
  first restart moves the sessions and succeeds on both versions.
- With the new binary and the old layout on systemd 259, Restart and Update now are refused
  with the reason, by the server and by the helper, and nothing is stopped.
- The helper's restart, install and rollback paths end healthy with sessions alive on both
  versions.
- `yulius doctor` and `host check` name the missing setup.
- Task #439's state (a resting task, a live run on record, no process) is left by Continue or
  Stop.

## Test plan

- Unit: `cgroup::detect` (sessions unit preferred, each missing permission falls back),
  the unit names; adopt on a fake cgroup tree (mirror, moves, limits, engine killed, bounds,
  nothing to do); `restart_safety::blocker` table; host setup item with the fake host (writes,
  idempotent, dry run, check mode, needrestart rule); helper refuses before stopping; freeze
  with the sessions unit (frozen, thawed, stopped server, move); supervisor: start and stop
  with an unattached live run (gone, alive, cannot be ended).
- Real hosts (QEMU, Ubuntu 24.04 and 26.04, the real binary as `yulius.service` with real
  sessions): the acceptance list above; logs kept in the Evidence section.
- Not testable here: production itself (the first restart is a person's, by the operations
  guide), and the exact path that left #439's run live.

## Decisions

- Sessions move to their own unit rather than healing the server's unit before each start:
  with the old layout every restart would depend on a root step that moves every process, for
  ever; with their own unit the adopt step is only the migration, and a restart touches nothing
  of a session.
- One slice with the service user's `cgroup.procs` rather than `AttachProcessesToUnit`: no code
  path of a join changes, and nothing in a sandbox needs the system bus. What the service user
  can move is still only its own processes, among groups of its own slice.
- The guard lives in the helper (root, authoritative, before the stop) and in the server (an
  answer at once); the doctor only warns, since hosts without systemd 258 are fine.
- The engine is killed at the migration rather than moved broken; it is a one-time cost of the
  first restart on a host that had the old layout.
- #439 is fixed where it shows (start and stop look at a live run nobody is attached to)
  rather than at the unknown path that left it: every such path is covered.
- One pull request although it passes 400 lines: the layout, its setup, the adopt step and the
  guard are one fix that is unsafe in parts; the #439 recovery was asked for in the same fix.
