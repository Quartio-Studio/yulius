# 0158. Docker reaches every run through a front beside the holder and a directory moved into live runs

## Status

Accepted. Specified in [SPEC-196](../../.ai/specs/SPEC-196-docker-just-works-in-every-sandbox-and-s.md);
task #182, #597. Amends [0151](0151-a-sandbox-runs-its-own-docker-engine-under-an-engine-seccomp-profile.md).
Amended by [0160](0160-a-sandbox-s-engine-comes-from-its-own-system-not-the-host-s.md): whether a front serves an engine follows the sandbox's own system, not the
host's `dockerd`.

## Context

ADR-0151 made the engine a property of a holder's start: the holder served the socket, and a run
got `<state>/docker.sock` bound as a file at spawn, only if its holder served one. Holders
survive updates and are adopted, and one with live sessions never idles. So after the upgrade
every sandbox that was busy kept no engine for as long as it was used, and every session, Shell
and Superagent started before kept no socket, whatever the holder did later. Nothing told the
agent why. The person asked for Docker in every sandbox and session with no restart, no reload
and no new session.

What constrains a fix:

- A run's mount namespace is made by its `bwrap` and owned by the sandbox's user namespace. Its
  `/run` is a tmpfs of its own, and Yulius's data directory is covered by an empty tmpfs, so
  nothing the holder or the server later creates on the host becomes visible in it.
- The server owns the sandbox's user namespace. A process of the server's that `setns`es into it
  has every capability there, so it may enter the run's mount namespace (and the holder's). It is
  not under the run's seccomp filter.
- `mount(2)` cannot bind a path from another mount namespace (`check_mnt`). A detached tree can
  move: `open_tree(OPEN_TREE_CLONE)` where the source is visible, then `move_mount` where it is
  wanted (Linux 5.2+; `machinectl bind` works this way).
- `setns` into a user or mount namespace needs a single-threaded caller.
- A holder from before engines runs old code: it cannot be asked to start an engine.

## Decision

- **A Docker front per holder.** `yulius sandbox-host docker` is a process of its own, started by
  the server beside every live holder whatever its age: after starting one, when adopting one,
  and at every upkeep tick when it ended. It joins the sandbox's `system` group and the holder's
  user, network and mount namespaces, the view the holder's own supervisor had. It serves
  `<state>/docker/docker.sock`:
  - **engine**: the ADR-0151 supervisor and engine, unchanged;
  - **relay**: to a SPEC-187 holder's own `<state>/docker.sock`, which keeps its engine and
    storage;
  - **refuse**: a Docker-API 503 naming the reason (setting off, no `dockerd` in the sandbox's
    system, no delegation, a
    writable executable) and the operator's fix (`yulius doctor`, `yulius doctor
    --docker-smoke`).

  It lives detached like the holder, so it survives Yulius restarts. It exits when its holder is
  gone or its socket was replaced, and stops its engine on `SIGTERM`. Stopping a holder stops its
  front first: the front would otherwise keep the namespaces alive. The server replaces a front
  only when its mode or refusal changed. An engine's other settings change with the next holder,
  so a running engine and its containers are never cut for them. The front is never counted as
  a run (idle). New holders no longer serve the socket themselves; their spec keeps only
  `containers` (forwards on every address).
- **A directory, not a file.** Every joined run that is not a production CD run binds
  `<state>/docker` read-only at `/run/yulius/docker` (connecting to a socket needs no write
  access to its mount; runs cannot store anything there, outside the disk quota, nor remove the
  socket) and links `/run/docker.sock` (= `/var/run/docker.sock`)
  to `yulius/docker/docker.sock`, with `DOCKER_HOST=unix:///run/yulius/docker/docker.sock`. The
  engine's view has the same, for `-v /var/run/docker.sock`. A front that starts again recreates
  the socket inside the directory, and every run sees the new one.
- **Live runs get the directory.** At each tick the server finds the mount namespaces of the
  sandbox's runs: processes in its network namespace, not the holder's mount namespace, not the
  front, not in the engine's cgroup. It reads their `mountinfo` and keeps those without
  `/run/yulius/docker` whose `/var` is not a tmpfs (a production CD run's is). For them, `yulius
  sandbox-host bind-docker` (single-threaded) opens their namespaces through the host's `/proc`,
  joins the sandbox's user namespace, and for each run clones `<state>/docker` in the holder's
  mount namespace and moves it to `/run/yulius/docker` in the run's, read-only. It adds the
  `/run/docker.sock` link when nothing is there. Each namespace is tried at most 3 times. Nothing
  restarts and the run's environment is unchanged: `docker`'s default path is the link.
- **Old holders' forwards.** A holder from before engines listens for its forwards on loopback
  only. Its front serves them for containers on the engine's default bridge gateway
  (`172.17.0.1`, `host-gateway`) once the engine runs.
- **One host command proves it.** `yulius doctor --docker-smoke` runs the upgrade in a throwaway
  sandbox on the host: a holder without an engine and a run started then, adoption with Docker on,
  `hello-world`, a build and a two-service compose in the old run, a recreated socket, and
  `DOCKER_HOST` in a new run. Started as root it runs itself as the data directory's owner in a
  delegated transient unit (`systemd-run`).

## Alternatives

- **Restart holders or sessions** after an upgrade. That is the failure the person reported.
  Sessions hold agents' context, and a holder cannot restart under its runs.
- **The socket on the system layer** (for example `/var/lib/yulius-docker/docker.sock`, visible in
  every run through the overlay), or **in the sandbox directory.** No mount namespace is entered.
  But runs (root of the sandbox) can write both places, and the front sees the whole host. Every
  path would need defending against links planted to make it remove or bind files elsewhere
  (`master.key`). Old runs would still need `/var/run/docker.sock`, in their private `/run`.
- **An abstract Unix socket** in the sandbox's network namespace. Every run reaches it already,
  but `docker`'s default path is a file, and old runs' environment cannot be changed.
- **The server process serves the socket.** It is gone on every update, and an engine started
  as its child would die with it, taking the containers.
- **The holder starts the engine on request** through a control channel. Holders from before
  engines run code that has no such channel.
- **A file bind of the socket** (ADR-0151). It cannot follow a recreated socket, and it exists
  only for runs started after the socket.

## Consequences

- Every sandbox and session gets Docker within one upkeep tick (30 s) of an updated server
  adopting it, or the reason why not. Agents told "no engine" by an older briefing find
  `docker` working when they try; the new briefing says `docker info` checks it.
- The server now enters runs' mount namespaces, with every capability of the sandbox's user
  namespace. It does only `mkdir`, `move_mount` of the Docker directory and a `symlink`, in a
  namespace the run controls anyway. The helper is not in the run's PID namespace, so the run
  cannot see or trace it. The moved tree's root is the directory: `..` leads to the run's own
  `/run/yulius`, and `open_by_handle_at` stays refused.
- **Engine storage and old bases.** A holder from before engines has no
  `sysroots/<sandbox>/docker`: the server makes it before an engine front starts. A sandbox whose
  system has no `dockerd` gets a refusal saying how to install it in the sandbox or on the host
  (ADR-0160).
- **Rolling back** to a server from before this decision: it does not know fronts, counts a
  front as a run and never stops it, so those holders no longer go idle. Their runs keep the
  socket. A person stops the fronts (`pkill -f 'sandbox-host docker'`) after such a rollback.
- The engine's read-only bind is not locked (same user namespace, as ADR-0151 says of its
  other binds), so a `--privileged` container could remount the directory writable, as it can
  already write `<state>/engine`. That is the engine's own sandbox, not runs.
- One more detached process per live holder (a few MB). Its log is
  `<state>/docker-front.log`.
- Not runnable in a sandbox or in Yulius CI (both are isolated runs). The unit tests cover the
  modes, the planner, the argv and the manager with a fake front; the host test
  `isolation_docker_live` and `yulius doctor --docker-smoke` cover the namespaces.
