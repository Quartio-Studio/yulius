# 0151. A sandbox runs its own Docker engine, bounded by an engine seccomp profile

## Status

Accepted. Specified in [SPEC-187](../../.ai/specs/SPEC-187-docker-works-inside-sandboxes-for-tasks.md);
task #172, #551. Amends [0003](0003-sandbox-isolation.md) (SPEC-002 ADR-22). Supersedes
[0140](0140-isolated-sandboxes-have-no-container-engine-agents-are-told.md).

## Context

Projects build and deploy container images, and their tests use `docker compose` and
testcontainers. Isolated sandboxes had a `docker` client (copied with the host's `/usr`) but no
engine. ADR-22 gives runs Docker's default capability set and a seccomp filter without mount APIs,
`setns`, `pivot_root` or nested user namespaces, and an engine needs all of those.

What each part of ADR-22 means for an engine:

- **User namespace.** Root of the sandbox is the service user, ids 1–65535 are its subordinate
  range, and the holder has every capability there. An engine that is root of this namespace,
  like Docker in an unprivileged LXC container, can create network, mount, PID and cgroup
  namespaces owned by it and needs no nested user namespace or `newuidmap`. Image files owned by
  ids above 65535 cannot be stored (rare; rootless Docker has the same limit).
- **Mount namespace.** The holder's sees the whole host tree as the service user sees it. A run's
  sees only its binds. Docker resolves `-v` paths in its own mount namespace, so an engine there
  would let any run read `master.key` through `docker run -v /var/lib/yulius:/x`. The engine
  therefore gets a run's view, plus only what it needs.
- **Overlay system layer.** Overlayfs cannot be an upper layer, so engine storage lives on the
  data volume (`sysroots/<sid>/docker`), outside the sandbox directory and never bound into runs.
  Kernel overlayfs works in a user namespace with `userxattr` (Linux 5.11+).
- **cgroup v2.** The delegated `sandboxes/<sid>` gets an `engine` child. The engine sees it as
  `/sys/fs/cgroup`, the root of its own cgroup namespace, and containers go below it. The
  sandbox's limits cover them. Showing the engine the host's cgroup tree would let it write other
  sandboxes' limits, which the service user owns.
- **Seccomp and capabilities.** A seccomp filter is inherited and cannot be removed. The engine
  starts under its own profile: the run profile plus the mount APIs, `umount2`, `pivot_root` and
  `setns`, with keyrings answering `ENOSYS` (what `runc` and LXC expect). Every container,
  `--privileged` included, keeps that bound. Docker's own profile narrows regular containers
  further. Nested user namespaces, `bpf`, `io_uring`, perf, modules, kexec, `userfaultfd` and
  `open_by_handle_at` stay refused. `bwrap` sets `no_new_privs`, so setuid binaries and file
  capabilities do not raise privileges inside containers either.
- **Network.** The engine shares the sandbox's network namespace. Its bridges and NAT live there
  and its traffic leaves through `slirp4netns` with host loopback off. Containers reach no more
  than a run: not the host's loopback, not other sandboxes, not the host's Docker.
- **sysfs and `/proc/sys`.** `runc` mounts a sysfs per container, which the kernel allows only
  where a sysfs is already fully visible. The holder mounts a fresh read-only sysfs of the
  sandbox's network for the engine (never the host's `/sys`, which carries the host's cgroup
  tree). The engine uncovers `/proc/sys`, where root of a user namespace can write only that
  namespace's own settings.

## Decision

- **One engine per sandbox, started on demand.** The holder serves `docker.sock` in its state
  directory. Runs (agents, the Superagent, Shells, CI jobs) see it at `/run/docker.sock`. The
  first connection makes the holder start `yulius sandbox-exec --engine` → `bwrap` (the engine's
  file view, `--unshare-pid/ipc/uts/cgroup`, `--cap-add ALL`, `--clearenv`) →
  `yulius sandbox-host engine` → `dockerd`. Bytes are then spliced to the engine's own socket. A
  failed start answers a Docker-API JSON error that names the engine log. The engine stops after
  the sandbox's idle time with no connection and no container process. It lives as long as the
  holder, so it survives Yulius restarts.
- **Its own cgroup.** `sandboxes/<sid>/engine` has CPU weight 100 beside `system`, `agents` and
  `ci`.
- **Its own storage.** `sysroots/<sid>/docker` counts toward the soft quota as "containers". Under
  disk pressure, a running engine's unused images and build cache are pruned through its own
  control socket. A stopped engine is never started for that. Containers and volumes are never
  pruned.
- **The egress proxy for containers.** With the engine on, the forward relays listen on every IPv4
  address of the sandbox's network, so containers reach the egress proxy at
  `host.docker.internal:<port>`.
- **Off switch and prerequisites.** `YULIUS_SANDBOX_DOCKER=off` turns it off. It needs `dockerd` on
  the host and the cgroup delegation. `yulius doctor` names what is missing, including kernel
  modules a sandbox cannot load.

## Alternatives

- **The host's Docker socket in runs.** Root on the host for every agent: never.
- **Rootless Docker or Podman nested in a second user namespace** (`rootlesskit`, `newuidmap`
  inside). It needs `CLONE_NEWUSER`, which is the entry point of most kernel privilege escalations
  and which ADR-22 refuses. It also needs a second id range carved from the 65,536 ids a sandbox
  has, plus `slirp4netns` or `pasta` per engine. It gains nothing: the sandbox's user namespace
  already is the "rootless" boundary.
- **The engine in the holder's mount namespace.** Simplest, but `-v` reaches the host tree as the
  service user (other sandboxes, `master.key`).
- **A proxy that filters the Docker API** (refusing `--privileged`, host binds). A large surface to
  keep correct across API versions. The seccomp bound and the file view already limit what
  `--privileged` gains (DKR-003, DKR-004).
- **An engine started with every holder.** About 70 MB per sandbox for projects that never use
  Docker. Starting on demand costs one start (a few seconds) on first use.
- **gVisor or microVMs.** gVisor slows the file- and syscall-heavy work of agents. There is no KVM
  on the host (ADR-22).

## Consequences

- Agents, the Superagent, Shells and CI jobs use `docker`, compose, buildx and testcontainers as
  documented. The briefing says so (amends SPEC-173). A sandbox without the prerequisites keeps
  the "no container engine" briefing.
- The kernel attack surface grows, for the engine and its containers only. Mount APIs, `setns`,
  `pivot_root` and `CAP_SYS_ADMIN`/`CAP_NET_ADMIN` are reachable inside the sandbox's user
  namespace. Kernel bugs reachable that way are in scope of the existing "not a defence against
  kernel exploits" (ADR-22, §8). gVisor remains the upgrade.
- The engine's read-only binds (`/sys`, the `yulius` executable) are made by `bwrap` in the
  sandbox's own user namespace, so they are not locked: the engine could remount them read-write.
  This is safe only for files the sandbox's root does not own. Host-root-owned sysfs files and the
  root-owned installed executable qualify. A server whose executable the service user owns,
  or which its group or anyone may write, therefore gets no engine.
- Per-container resource flags (`-m`, `--cpus`) are not supported: memory flags are discarded,
  and CPU flags may make the container fail to start. The engine lives in its own group, and
  cgroup v2 passes no controller below a group that holds processes. The sandbox's limits still
  bound every container.
- A `--privileged` container is as strong as the engine (root of the sandbox with the engine's
  view and filter), never more. Paths outside the engine's view (another user's home in a Shell,
  run directories) do not exist for `-v`.
- Containers stop when the holder stops (it is idle only once the engine has stopped). A
  container with a restart policy comes back when the engine starts again.
- Untestable in a sandbox or in Yulius CI, which are isolated runs themselves. The host-level
  isolation suite covers it (`YULIUS_ISOLATION_TEST=1`, `isolation_docker`).
