# 0339. The engine runs below the group that holds its limits

## Status

Accepted. Specified in
[SPEC-483](../../.ai/specs/SPEC-483-deep-security-and-quality-audit-fix-ever.md) (finding C4,
slice 39), task #465. Amends
[0151](0151-a-sandbox-runs-its-own-docker-engine-under-an-engine-seccomp-profile.md) (the
engine's cgroup).

## Context

A sandbox's Docker engine ran in `sandboxes/<sandbox>/engine`, which was also the root of its
cgroup namespace and its `/sys/fs/cgroup`. The same group held the engine's limits: 3/8 of the
sandbox's tasks (SPEC-268 PID-002, so agents always keep a quarter) and a CPU weight of 100
beside `system`, `agents` and `ci`.

Root in the sandbox's user namespace is the service user on the host, and that user owns every
file of the delegated tree. The kernel's only check against a group's own limits is
`nsdelegate`, which refuses writes to the root of the writer's cgroup namespace. A process of
the engine can move to a child group and start a new cgroup namespace there. In a privileged
container, any Docker API caller in the sandbox can do this, a CI job included. From that
namespace, `engine/pids.max` and `engine/cpu.weight` are no longer its root, so it may write
them. Reproduced on Ubuntu 26.04 (kernel 7.0, `nsdelegate`). As the root of a bwrap user
namespace with Yulius's AppArmor profile, a direct write is refused, but from a nested
namespace `pids.max` becomes `max` and `cpu.weight` 10000. A real `dockerd` (run as root,
without the sandbox's user namespace) gave the same result: in its namespace,
`docker run --privileged --cgroupns=host … unshare -C` set both.

## Decision

- The engine runs in `sandboxes/<sandbox>/engine/inside` (`ENGINE_ROOT_LEAF`). That group is the
  root of its cgroup namespace and is what `bwrap` binds at `/sys/fs/cgroup`. The limits stay
  on `engine`, out of the engine's view.
- `cpu` and `pids` pass down from `engine` to `inside`, one write each, as the engine's group
  had them from the sandbox's before. runc passes them on to containers in threaded mode, so
  `--pids-limit` works and `--cpus` fails as it did (runc refuses a threaded subtree for it; seen
  on the guest in both layouts). `memory` cannot be threaded and never reached containers. An
  engine still running in `engine` itself, started from an earlier version's spec, keeps
  joining it until its front starts again: a group that passes only threaded controllers down
  takes processes.
- When systemd prunes an idle engine's empty groups at a restart, the front and the upkeep tick
  restore both `engine` and `inside` (and nothing above `engine`). The task cap returns with the
  sandbox's next preparation, as before.
- **An engine group from before is made plain again.** The first container an engine of the old
  layout ran made runc turn its subtree threaded: `engine/docker` threaded, `cpu` and `pids`
  passed down. A domain group created below it is then invalid, and no container starts in
  `inside` (seen on the guest). Each preparation, and each restore before a start, settles the
  group first. When no process runs in `engine` itself (and its `cgroup.procs` can be read),
  nor below it except in `inside`, the emptied leftovers go and the controllers pass down
  again. An old engine that still runs there is left alone.
- **A container is a populated group.** The idle check reads `cgroup.events` (`populated`) of
  the groups below the engine's root, not their `cgroup.procs`. runc's threaded groups cannot
  be read that way, so an old-layout engine was counted idle and stopped while its containers
  ran.

## Alternatives

- **Read-only limit files**: the owner (the service user, root inside) can always change the
  mode back, and `CAP_DAC_OVERRIDE` in the user namespace ignores it.
- **Refusing privileged containers, or cgroup namespaces, at the Docker front**: breaks Docker
  workflows people use (Docker-in-Docker, `--privileged` builds), and the rule would have to
  follow every way into the engine's mount namespace.
- **Limits only on the sandbox's group**: the per-leaf caps exist so that CI and containers
  cannot starve the agents. Dropping them gives up SPEC-268 PID-002.

## Consequences

- On a real kernel, a nested namespace's write now reaches only `inside` and its children. A
  fork past the engine's cap is still refused (`pids.events` `max` rises on `engine`). A real
  `dockerd` in this layout ran containers as before, with all their tasks counted on `engine`.
  The privileged container's nested write stayed inside its own view, and the engine kept
  `pids.max` 200.
- An engine started before the update stays in the old layout until its front restarts: the
  sandbox's next start or the host's next reboot. If systemd pruned an idle old engine's groups
  at the update, the server's upkeep may find its front's group gone and start the front again
  with the new spec at its next tick.
- Paths: the engine's spec, the front's supervisor and `sandbox-exec --engine` name
  `…/engine/inside`; the limit counters and the run-bind exclusion still look at `…/engine`.
