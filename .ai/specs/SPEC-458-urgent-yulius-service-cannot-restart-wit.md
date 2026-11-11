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

From the database and the code (the requester's correction, 2026-11-11): #439 was queued until
the clean start at 11:28:16, when its first run started, and that run lived until 11:47:35.
Two defects made it unusable meanwhile:

1. The start-up reconciliation ends with a sweep: every task that is starting, switching,
   stopping or running without a live run rests `stopped_server_lost`. The reconciliation
   itself starts queued tasks (each lost run frees a slot and drains the queue, in the
   background), and a launch is `Starting` with no run on record until it records one. The
   sweep took #439 for a dead server's task while its launch went on.
2. `yulius task restart` then moved the resting task to `Starting` before it looked for a run,
   was refused at `insert_attempt` ("task already has a live run"), and the refusal's error
   path wrote `failed_precondition` over the status, with the run still live. From then on a
   restart was refused the same way, a message "is failed_precondition", and Stop answered
   "task is …" without looking at the run.

## Requirements

### Layout (ADR-0298)

- **LAY-001** Sessions unit: `<stem>-sandboxes.service` for the server unit `<stem>.service`
  runs one keeper (`ExecStart=+/usr/bin/sleep infinity`) with `User=` the service user,
  `Delegate=yes`, `DelegateSubgroup=keeper`, `OOMScoreAdjust=-1000`, in the slice `<name>.slice`,
  where `<name>` is the stem with every character but a letter or a digit replaced by `_` (a dash
  would nest the slice). `User=` names who the group is delegated to; the `+` makes the keeper
  root's, so nothing of the service user can signal it (its end would end every session; shown
  on systemd 255 and 259). Its `ExecStartPre=+` gives the slice's `cgroup.procs` to the service
  user. It keeps the default `KillMode`: stopping it ends every session, deliberately.
- **LAY-002** `cgroup::detect` prefers the sessions unit: for a server in
  `<slice>/<stem>.service/<leaf>` it returns `<slice>/<stem>-sandboxes.service` when that
  directory, its `cgroup.procs` and `cgroup.subtree_control` and the slice's `cgroup.procs` are
  writable; else the server's own unit as before (hosts not set up, development).
- **LAY-003** The server's unit gets a second drop-in, `20-yulius-sessions.conf`: `Slice=`,
  and `Wants=` and `After=` for the sessions unit and the adopt unit.
- **LAY-004** Adopt unit: `<stem>-adopt.service`, a root oneshot (`Requires=` and `After=` the
  sessions unit) running `yulius host adopt-sessions --service <unit>` before every start of the
  server. For each group the server's unit may hold (the one systemd names as `ControlGroup`,
  `system.slice/<unit>`, `<slice>/<unit>`) it makes every child group but `server` again in the
  sessions unit (same relative paths, controllers passed down, and for a group it creates
  `cpu.max`, `cpu.weight`, `memory.high`, `memory.max`, `pids.max` and `cgroup.freeze`, owned
  by the service user), moves every process, removes the emptied groups and takes the
  controllers from the old unit. Where the unit changes its place, what the server left in
  `server/` moves to `legacy/` of the sessions unit, so that the old group empties. It then
  waits up to 5 s until systemd names the old group no more and removes what is left of it:
  systemd sweeps the emptied groups too and gives the unit's group up without removing it where
  that sweep crosses the step's own, so the group being there says nothing. A group that cannot
  be made or emptied is reported (the unit fails, which the server's `Wants=` survives) and the
  others still move. Bounded: 4096 groups, depth 8. Nothing to adopt is success.
- **LAY-005** A sandbox's `engine` group is not moved: its Docker engine runs in a cgroup
  namespace rooted there and could not manage containers from elsewhere. Adopt kills it
  (`cgroup.kill`), waits up to 10 s for it to empty and removes it; the engine starts again on
  demand as after a reboot. The sandbox's Docker front was started with the old group's path:
  the server replaces a running engine front when the group it was given is gone and the
  sandbox's engine group is another one now (a group that is still there may hold a running
  engine and is never given up). Said in the operations guide.
- **LAY-006** The needrestart rule also names the sessions unit, so a package install never
  restarts it.

### Host setup, check and doctor

- **HST-001** `yulius host setup` writes the two units and the drop-in when they differ,
  reloads systemd and starts the sessions unit; a running server is not restarted, and a manual
  line says the next restart moves the sessions and keeps them. The item is unsafe: `host check`
  only reports it (failed, "run `sudo yulius host setup`"), since the update helper cannot write
  unit files: that sessions live in the server's own group when a file is missing, and only
  which files differ when all are there. `deploy/systemd` holds the three files as the setup
  writes them for `yulius.service`, the user `yulius` and `/usr/local/bin/yulius` (pinned by a
  test), for the first restart on a host that has no binary with this change yet.
- **HST-002** `yulius doctor` has a line "restart with sessions": ok with the sessions unit in
  use; a warning naming `sudo yulius host setup` when sessions live in the server's unit, which
  says that a restart fails while sessions run when systemd is 258 or later.

### Restart and Update now

- **RST-001** `restart_safety::blocker`: a restart of the server's unit would fail when systemd
  is 258 or later, `sandboxes` below one of the unit's groups (LAY-004's list) has processes,
  and the unit does not `Wants=` its adopt unit. Pure over the facts (the first line of
  `systemctl --version`, `cgroup.events`, `LoadState`, `ControlGroup` and `Wants` of `systemctl
  show`). A systemd that cannot be asked blocks nothing.
- **RST-002** The update helper checks it before it stops the service, for `restart` and
  `install`: it fails the request with the reason and stops nothing.
- **RST-003** The server checks it when Restart or Update now is asked: `409` with the reason,
  before any task is paused or anything is built.

### Export

- **EXP-001** `yulius instance export` freezes the sessions unit together with the server's
  unit and thaws both; a stopped server whose sessions unit is active is frozen there instead
  of being refused; `--move` leaves both frozen.

### A task that rests while its run is on record (#439)

- **RUN-001** The start-up sweep rests only tasks the server before this one left live, whose
  run this server does not hold again and which it is not launching.
- **RUN-002** A start (Continue, Restart) of a resting task with a live run on record changes
  nothing when it is refused: while this server holds the run it is refused before the task
  moves ("the task still has a live run: stop it first"). A run no server holds is settled
  first, as a restarted server does (ADR-0050): exited, ended and counted; host or agent
  alive, terminated and then lost; gone, lost. A run that cannot be ended still refuses the
  start.
- **RUN-003** Stop on a resting task with a live run on record ends that run (held: asked to
  shut down, with the escalation armed; not held: settled as in RUN-002) instead of answering
  "task is …".

## Plan

- [ ] The server uses the sessions unit when the host has one (cgroup detection)
- [ ] `yulius host adopt-sessions` moves the sessions of the old layout out of the server's unit
- [ ] Host setup writes the sessions unit, the adopt unit and the drop-in; host check and the doctor report it
- [ ] Restart and Update now refuse where the restart would fail (helper and server)
- [ ] The export freezes the sessions unit too
- [ ] Start-up never rests a launching task; a start or a stop settles a run left on record (#439)
- [ ] Proof on real hosts: restarts, the helper's restart and rollback, the first restart that installs the fix (systemd 255 and 259)
- [ ] ADR-0298, the operations guide, the architecture document and the change entry
- [ ] Gates and an independent review of the final head

## Acceptance criteria

- On systemd 259.5 and 255.4, with the layout set up, `systemctl restart <unit>` and a `kill -9`
  of the server with `Restart=on-failure` keep every session process, and the server comes back
  active.
- On a host with the old layout and live sessions, after `host setup` from the new binary the
  first restart moves the sessions and succeeds on both versions.
- The first restart that installs the fix, done by the pre-fix helper with the three units in
  place, keeps every session on both versions. Without the units it fails on systemd 259 as it
  did on production, and the guide's way back restores the server with its sessions.
- With the new binary and the old layout on systemd 259, Restart and Update now are refused
  with the reason, by the server and by the helper, and nothing is stopped.
- The helper's restart, install and rollback paths end healthy with sessions alive on both
  versions.
- `yulius doctor` and `host check` name the missing setup.
- A task launched during the start-up reconciliation runs, shown as running. A resting task
  with a live run on record (#439's state) keeps its status when a start is refused, and Stop
  ends its run; then it continues.

## Test plan

- Unit: `cgroup::detect` (sessions unit preferred, each missing permission falls back),
  the unit names; adopt on a fake cgroup tree (mirror, moves, limits, engine killed, bounds,
  nothing to do); `restart_safety::blocker` table; host setup item with the fake host (writes,
  idempotent, dry run, check mode, needrestart rule); helper refuses before stopping; freeze
  with the sessions unit (frozen, thawed, stopped server, move); supervisor: the
  reconciliation during a launch; a refused start and a stop with a run this server holds; a
  start and a stop with a run no server holds (host killed, host alive).
- Real hosts (QEMU guests, Ubuntu 24.04 and 26.04): the pre-fix binary set up by its own
  `yulius host setup`, the update helper installed as the operations guide says, the builds
  staged by hand where the helper would build them, and stand-in session processes placed as
  the server places a sandbox's (by the service user from the server's group, in the groups
  `cgroup.rs` makes): the acceptance list above; results in the Evidence section.
- Not testable here: production itself (the first restart is a person's, by the operations
  guide); real agent sessions, sandboxes and Docker engines in the guests (they need a GitHub
  App, a Claude subscription and a sandbox base), so reattaching to a session's terminal and an
  engine starting again after the move are covered by unit tests and unchanged code only.

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
- #439 is fixed at its cause (the sweep) and where it shows (a start and a stop look at the
  run on record first), so another path that leaves a resting task with a run is recoverable
  too. The launch marks its task instead of the sweep waiting for the launch lock: a start-up
  never waits for a slow launch, and a test can hold a launch and reconcile without a
  deadlock.
- The keeper is root's: the service user owns the unit's group and could signal a keeper of
  its own, and the keeper's end is the end of every session.
- The first restart cannot be made safe by code alone: the helper that installs this build is
  the build before it, and nothing of a failing unit runs before its start on systemd 258. So
  the three files ship in `deploy/systemd`, the guide gives the steps for before the update and
  the way back that keeps the sessions, and both were run in the guests.
- Adopt asks systemd whether it let go of the old group instead of waiting for the group to
  vanish: found on 26.04, where the first restart with two sandboxes left the emptied group
  behind and the step reported a failure that was none.
- One pull request although it passes 400 lines: the layout, its setup, the adopt step and the
  guard are one fix that is unsafe in parts; the #439 recovery was asked for in the same fix.

## Evidence

### Real hosts (2026-11-11)

Two QEMU guests from Ubuntu's cloud images, each run several times from one prepared disk:
Ubuntu 24.04.5 (systemd 255.4, kernel 6.8) and Ubuntu 26.04.1 (systemd 259.5, kernel 7.0).
Prepared as production was: the pre-fix binary (`911e3127`, the merge base) installed as
`/usr/local/bin/yulius` and set up by its own `yulius host setup --public-url http://vm.test
--no-smoke`, which wrote and started `yulius.service`; the update helper installed from
`deploy/systemd` as the guide says. The build with the fix is this branch at `16607772` (the
merge of `dev`; later commits change no code that ran here unless said below).

What stands in for what, because it cannot be had in a guest:

- Sessions are stand-in processes (`sleep`) started by the service user from inside the
  server's group, in the groups, with the limits and controllers `cgroup.rs` gives a sandbox:
  `sandboxes/<sandbox>/{system,agents,agents/run-a1 (frozen),agents/run-a2,ci,engine/init,engine/docker/c1}`.
  "Kept" below means the same process ids are alive.
- A build is staged by hand in `/var/lib/yulius-update/staged/` where the helper's own build
  would put it. The request file is written as the server writes it, by the service user.

Results (twelve runs; every one ended and no session process was lost where one should be kept):

| Scenario | 24.04 (systemd 255) | 26.04 (systemd 259) |
| --- | --- | --- |
| The incident: `systemctl restart yulius` with sessions in the server's unit, pre-fix binary | restarts, sessions kept | `Failed to spawn executor: Device or resource busy`, restart counter 18 and rising, the unit's group as on production (`subtree_control = cpu memory pids`, no process in its root, no `server`); back only after every session process was ended |
| The first restart that installs the fix: the three files of `deploy/systemd` put in place, then an `install` request handled by the pre-fix helper | `done`; "moved 10 processes in 11 groups … (2 Docker engines ended)"; 10 of 10 kept; server in `yulius.slice/yulius.service/server` | the same |
| Then `systemctl restart`, `kill -9` of the server (`Restart=on-failure`), the helper's `restart` | 15 of 15 kept each time (a third sandbox added in the sessions unit by the service user) | the same |
| An install whose build reports another commit, and one whose build cannot run at all (the adopt step fails with it) | both `rolled_back`, previous binary back and healthy, 15 of 15 kept | the same |
| The fix installed without the units, sessions then running in the server's unit: the helper's `restart` and `install` | both go through (this systemd restarts it), sessions kept; the host check names the setup | both `failed`: "yulius.service would not start again: its sessions live in its own group, where systemd 259 starts no unit while one runs. Run `sudo yulius host setup` on the host once, then try again; nothing was stopped."; same server process, sessions kept |
| The same host, the server's own answers (superadmin session) | Update now: 409 "no update is on offer yet" (not the guard); Restart: 202, restarted, sessions kept | Update now and Restart: 409 with the sentence above; same server process |
| `yulius doctor` and `yulius host check` there | warn "… (this host: systemd 255; this systemd still restarts it): run `sudo yulius host setup`"; check fails the item | warn "… (this host: systemd 259; sessions run there now, so Restart and Update now are refused) …"; check fails the item |
| `sudo yulius host setup`, then Restart asked of the server | units written, server untouched; 202, "moved 5 processes in 6 groups", sessions kept; doctor and check ok | the same |
| The adopt step alone (server stopped) | `cpu.max`, `memory.max`, `pids.max`, the owner and `frozen 1` of the paused run are the same in the sessions unit; engine processes gone; old group gone; the server's start then thaws the run (no update is open) | the same |
| Update now without the units while sessions run | goes through (255) | `failed`: "Could not start yulius.service … The rollback failed too … Restore by hand from …", sessions still alive; the guide's way back (units, the staged build's `host adopt-sessions`, start) brings the previous version back with 5 of 5 kept; the second install is `done`, 5 of 5 kept |
| After a rollback: the pre-fix binary with the units, sessions in `yulius.slice/yulius.service/sandboxes`; then the update again | the adopt unit fails with the pre-fix binary ("unrecognized subcommand 'adopt-sessions'") and the server starts regardless; the install is `done`, "moved 5 processes in 6 groups from …/yulius.slice/yulius.service …" (the unit in place), 5 of 5 kept, also over a `systemctl restart` after it | the same |
| A host without the unit, set up by the build with the fix | units and server written and started; server in `yulius.slice`, "per-sandbox limits on cgroup=…/yulius-sandboxes.service"; restart and crash keep the sessions; a second setup changes nothing | the same |

Found by these runs and fixed before the last one: the adopt step reported a failure on 26.04
when systemd gave up the emptied old group without removing it (commit `cdeb3f6e`); a Docker
front would have kept the old group's path (`30f7ebd1`); one sandbox that cannot be moved
ended the whole move (`50a5b6ea`). Three probes with scratch units back the design: a root
keeper with `User=` keeps the delegation and cannot be signalled by the user; systemd 255 and
259 let go of a stopped unit's emptied group at once, also while its start job waits; the old
layout restarts on 255 and not on 259.

Not shown, and why: production itself; real agent sessions, sandboxes and Docker engines (a
guest has no GitHub App, no Claude subscription and no sandbox base), so that the server
attaches to a session's terminal again and that an engine starts again after the move rest on
unit tests and on code this change does not touch; the helper's own `build` step.
