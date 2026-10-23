# 0208. Production CD runs use a production holder with its own Docker engine

## Status

Accepted. Specified in [SPEC-279](../../.ai/specs/SPEC-279-production-cd-runs-get-a-docker-engine-a.md);
task #265. Extends [0109](0109-cd-runs-verified-commits-out-of-the-agents-reach.md),
[0151](0151-a-sandbox-runs-its-own-docker-engine-under-an-engine-seccomp-profile.md) and
[0158](0158-docker-reaches-every-run-through-a-front-and-a-directory-moved-into-live-runs.md);
lifts the "no Docker engine in production" limit of
[0207](0207-cd-runs-restore-and-save-agent-states-under-one-hold.md).

## Context

A production CD run (SPEC-134 review C1/H-A) joins its sandbox's holder for the network only and
runs on the host's read-only system, so nothing an agent installed or wrote runs with
production's secrets. It had no Docker engine: the sandbox's engine is the agents' (they run its
`dockerd` from a system layer they change with `sudo apt`, they can read and replace its images,
and its socket is bound into their runs). Deploys that build images with `docker buildx`, load
one with `docker load` and ship it over SSH (Quartio-Studio/product's production rollout) could
not run from Yulius CD.

## Decision

- **A production holder per sandbox.** The sandbox manager runs a second holder for each sandbox
  under the name `<sid>.prod` (`.` is outside the ids' alphabet, so no sandbox has it), started
  when a production-stage run prepares it and held by its lease: its own user, network and mount
  namespaces with its own `slirp4netns` (no forwarded ports, no inbound dev ports), its own
  cgroup `sandboxes/<sid>.prod`, state `runtime/sandboxes/<sid>.prod`, a system layer
  `sysroots/<sid>.prod/{upper,work}` over the current base, and engine storage
  `sysroots/<sid>.prod/docker`. No agent run ever joins it.
- **Its own engine and front.** Its Docker front starts `dockerd` from that layer: the base's
  (a copy of the host system, read-only to every sandbox) or what only this engine wrote, never
  the sandbox's layer. Same prerequisites and refusals as a sandbox's engine; the engine is named
  `<slug>-prod`. It binds `<data>/cd/<sid>/runs`, where production runs' directories now live, and
  nothing of the sandbox directory.
- **Production runs join it.** In host-system mode as before, with its Docker directory at
  `/run/yulius/docker` (`DOCKER_HOST`, `/run/docker.sock`) and its `ci/` cgroup; the host's
  `docker` CLI and buildx plugin come with the host's `/usr`.
- **Lifecycle and disk.** It stops when idle like any holder, is adopted after a restart, is
  retired with its sandbox, and its trees go with the removed sandbox's once it stopped. Its layer
  and storage count toward the sandbox's quota (consumer `production`); the upkeep prunes a
  running production engine with the sandbox's, and runs right after each production run when
  the sandbox is over its quota. Production's own files never refuse a production deploy (only
  the rest of the sandbox's usage does), so they can always be pruned.

## Alternatives

- **A fresh engine per production run** (a throwaway holder and storage for each deploy). The
  strongest isolation between deploys, but every deploy pays a holder and engine start and
  rebuilds and re-pulls every layer (no build cache, no base images), and a cancelled run's
  storage needs its own cleanup. A production holder keeps the cache between deploys of the same
  project while staying out of the agents' reach; a per-run storage can be added later if a
  project needs it.
- **The host's engine** (the host's `dockerd`, or a socket of it). Its socket is root of the host
  (any container can bind `/`), it is shared by every project on the instance, its storage is
  the host's `/var/lib/docker`, and `yulius host setup` stops it on purpose: it would break the
  read-only host system production runs rely on and put every project's production in one engine.
- **The sandbox's engine.** Agents control it: its binary, its images, its build cache and its
  socket. Rejected by SPEC-134 for the same reason as the sandbox's system layer.
- **A second manager or a new directory layout** (`<data>/cd/<sid>/engine/…`). Would duplicate
  adoption, idle stop, retirement, egress restart, the front and mapped-tree removal; the derived
  name reuses them, and `sysroots/` and `runtime/sandboxes/` are already outside every sandbox
  directory and removed as mapped trees.

## Consequences

- Production deploys can build, load and run images; a production run's containers run on a
  network no agent shares, and its images and build cache are out of agents' reach.
- The production holder maps the same ids as the sandbox's (the host offers one subordinate
  range): the boundary between it and the agents is namespaces and the covered data directory,
  as between two sandboxes. An escape from a sandbox's containment reaches it, as it reaches
  every other sandbox.
- Each sandbox may run two holders (and two engines) at once; the production one only during and
  shortly after a production run. Its disk is the project's: a sandbox the agents filled refuses
  production deploys, as before; production's images are pruned when the sandbox is over its
  quota, so a deploy may rebuild layers it had cached. Production run files a failed removal
  left are listed as ended runs.
- A production run's directory moved from `<data>/runs/cd-<run>` to `<data>/cd/<sid>/runs/`.
- The smoke test (`yulius doctor --docker-smoke`) proves the production engine on a host.
