# 0298. Sessions live in their own unit

## Status

Accepted. Specified in [SPEC-458](../../.ai/specs/SPEC-458-urgent-yulius-service-cannot-restart-wit.md)
(task #440). Changes where SPEC-053's per-sandbox cgroups live; keeps ADR-0002 (a detached host
per run survives a restart of the server), ADR-0013 (the update helper) and ADR-0162 (one host
setup command).

## Context

On 2026-11-11 `systemctl restart yulius` took production down (Ubuntu 26.04.1, systemd 259.5,
kernel 7.0). The stop worked. `KillMode=process` left 87 session processes in
`system.slice/yulius.service/sandboxes/…`, as intended. Every start then failed with `Failed to
spawn executor: Device or resource busy` until every one of them was ended by hand.

- A cgroup v2 group that passes controllers to its children (`cgroup.subtree_control` not empty)
  takes no process itself (the no-internal-processes rule, `EBUSY`). The server's unit is
  delegated and passes `cpu memory pids` to `sandboxes/`, which survives the stop with its
  processes.
- systemd 255 forks the service's process in its own group and attaches it to `<unit>/server`
  (`DelegateSubgroup=`): no process ever enters `<unit>`. Since systemd 258 the manager starts
  `systemd-executor` with `CLONE_INTO_CGROUP` into the unit's own group, and only the executor
  moves on to the subgroup. The kernel refuses the clone, for `ExecStart=` and `ExecStartPre=`
  alike. Upstream `main` still does it.
- A stopped unit keeps the place of its group while one process is left there, whatever its
  `Slice=` says, so a new slice alone moves nothing.

The same unit restarts on Ubuntu 24.04 (systemd 255), which is why it was never seen. Shown with
the real unit in QEMU guests of both systems (SPEC-458, Evidence).

## Decision

- **A sessions unit.** For the server unit `<stem>.service`, `yulius host setup` writes
  `<stem>-sandboxes.service`: one keeper process (`sleep infinity`) whose delegated group holds
  every sandbox's groups (`sandboxes/<sandbox>/…`, unchanged below that). It is delegated to the
  service user (`User=`, `Delegate=yes`, `DelegateSubgroup=keeper`), while the keeper itself is
  root's (`ExecStart=+…`), so no signal of the service user reaches it. It keeps the default
  `KillMode`: stopping it ends every session, on purpose, since a unit that keeps processes over
  its stop is exactly what cannot start again.
- **One slice for both.** The server and the sessions unit live in `<name>.slice` (the stem
  with everything but letters and digits as `_`, because a dash nests slices). The sessions unit
  gives the slice's `cgroup.procs` to the service user at its start. The kernel asks for write
  access to `cgroup.procs` of the nearest common ancestor when a process moves between groups,
  and with it every join the server and `sandbox-exec` already do works across the two units,
  unchanged.
- **The server prefers it.** `cgroup::detect` returns the sessions unit beside the server's unit
  when its group and the slice's `cgroup.procs` are the service user's, else the server's own
  unit as before (a host not set up, development).
- **An adopt step before every start.** `<stem>-adopt.service` is a root oneshot that the
  server's unit `Wants=` and is ordered `After=` (a drop-in, `20-yulius-sessions.conf`, which
  also sets `Slice=`). It runs `yulius host adopt-sessions`: the groups an earlier layout left in
  the server's unit are made again in the sessions unit (same paths, limits, weights, frozen
  state, owner), their processes move, the emptied groups go, and the step waits until systemd
  names the old group no more. A dependency is the only thing that runs before a start on
  systemd 258: nothing inside the failing unit does. Nothing to move is success, so the step is
  the migration and a restart otherwise touches nothing of a session. Nothing is frozen for the
  move, so a group is read again until a read finds no new process (a fork between the read and
  the move leaves a child behind, and one process left is enough to keep the unit from
  starting). While the server runs the step moves nothing: `systemctl start` on an active unit
  starts its `Wants=` again. The unit has `TimeoutStartSec=120`, and systemd is asked with a
  bound.
- **No root unit for a binary someone but root could replace.** The adopt unit runs the
  server's binary as root. `yulius host setup` makes that binary root's first and writes the
  units only when the file and every directory above it are root's alone to write; a binary
  the server's unit names through a link is judged, and named in the adopt unit, at the
  link's target.
- **Engines end at the move.** A sandbox's Docker engine runs in a cgroup namespace rooted at
  its `engine` group and could not manage containers from another place. The step kills it
  (`cgroup.kill`, the one way to end a whole group without racing its forks and reused process
  ids); it starts again on demand, and a Docker front that was given the old group is replaced
  so that it does.
- **A guard.** A restart would leave the server down when systemd is 258 or later, the unit
  holds processes outside the server's own subgroup (its `sandboxes`, or what an earlier server
  left), and either the unit does not pull in its adopt unit or the sessions unit, which the
  adopt unit requires, does not run. The
  update helper checks this before it stops the service (restart and install), and the server
  when Restart or Update now is asked (409, before a task pauses or anything is built). `yulius
  doctor` has a line for it, and `yulius host check` reports the missing units.
- **The first restart.** The build that brings this arrives through the helper of the build
  before it, which has no guard. On a host with systemd 258 or later the three units are
  therefore put in place first (`deploy/systemd`, or `sudo yulius host setup` of a binary that
  has this change); the start that follows the install runs the adopt step with the new binary.
  `docs/operations.md` §8 has the commands and the way back.

## Alternatives

- **Heal the server's unit before each start** (`ExecStartPre=+` that moves the sessions aside).
  It cannot run: on systemd 258 the pre-start command is refused like the start.
- **Keep sessions in the server's unit and move them out and back around every restart** by a
  root unit. Every restart would depend on moving every process, for ever, and a crash
  (`Restart=on-failure`) as well. With their own unit the move happens once.
- **Take the controllers back at stop** so that the unit's group may hold a process again. A
  killed server takes nothing back, and the sandboxes' limits would be gone while it is down.
- **`AttachProcessesToUnit` over D-Bus** instead of the shared slice. It works, but every
  joiner would need the system bus, and `sandbox-exec` joins after it entered the sandbox's
  namespaces.
- **A transient scope per sandbox.** Starting one needs the bus and a polkit rule for the
  service user, or a user manager; the sandbox's groups would no longer be the server's to make.
- **A oneshot with `RemainAfterExit=yes` and no process.** systemd removes the group of a unit
  without processes; shown in the guests.
- **A keeper run by the service user.** Anything of that user could signal it, and its end is
  the end of every session.
- **Wait for a systemd fix, or pin systemd 255.** Upstream still starts the executor that way,
  and Ubuntu 26.04 ships 259.

## Consequences

- A restart of the server keeps every session on systemd 255 and 259: `systemctl restart`, a
  crash with `Restart=on-failure`, the helper's restart, an install and a rollback (SPEC-458,
  Evidence).
- Stopping or restarting `<stem>-sandboxes.service` ends every session. The needrestart rule
  names it, so a package install never restarts it.
- A host set up before this needs `sudo yulius host setup` once (it restarts nothing), or the
  three files. Until then the layout is the old one: fine on systemd 255, refused by the guard
  on 258 and later while sessions run.
- At the move every running Docker engine is killed once, with its containers and without a
  shutdown. A holder from before ADR-0158 that serves an engine itself keeps the old group: its
  sandbox has no Docker until it restarts.
- The server prepares the sessions unit's groups when it starts. After that unit was stopped or
  restarted (which ends every session), the server has to be restarted as well.
- The keeper is safe from the service user's signals, not from the service user: the group is
  delegated to it, and whoever owns a group can end what is in it through the group's files.
- Memory a process used before the move stays charged to its old group until it is freed
  (cgroup v2 moves no charges), so a sandbox's `memory.max` counts less for a while. One time.
- The service user can move its processes between the two units of its slice. Nothing else
  lives in that slice.
- A rollback to a build without `host adopt-sessions` leaves the adopt unit failing, which the
  server's `Wants=` survives. That build places new sessions in the server's unit again, so on
  systemd 258 and later it must not be restarted while they run: update again instead.
- cgroup paths in commands and documents change from
  `/sys/fs/cgroup/system.slice/yulius.service/sandboxes/…` to
  `/sys/fs/cgroup/yulius.slice/yulius-sandboxes.service/sandboxes/…`.
- The guard knows whether the adopt unit is pulled in and the sessions unit runs, not whether
  the adopt step will succeed: a binary that cannot run it, or a group it cannot move, still
  leaves the server down on systemd 258 and later, and the unit then says why
  (`systemctl status yulius-adopt`).
