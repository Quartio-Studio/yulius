---
yulius-task: 98bb4310-8b5c-43c4-b97f-13977a7545f1
status: in-progress
---

# SPEC-279: Production CD runs get a Docker engine agents cannot reach

Status: in-progress
Date: 2026-10-23
Owner: task #265
Scope authorization: task #265, split out of task #264 (SPEC-278, ADR-0207). First user:
Quartio-Studio/product, whose production deploy ("prod: rollout to all others", its decision
0017; `scripts/cd/deploy.sh prod`, `npm run cd:rollout`) builds linux/amd64 images with
`docker buildx`, loads one with `docker load` and ships them over SSH.

## Goal

A production-stage CD run reaches a Docker daemon (buildx, `docker load`) that no agent can
reach: not the sandbox's engine, no socket bound into the sandbox, no `dockerd` from the
sandbox's system layer, storage outside every sandbox directory, its containers on a network no
agent shares. Every existing production guarantee stays.

## Context

Confirmed against `dev` at 2d9923df.

- **Production containment** (SPEC-134 review C1/H-A, round 3; `production_containment` in
  `crates/yulius-app/src/ci/cd.rs`): a production run joins its sandbox's holder for the network
  only (`host_system` join mode: the host's `/usr`, `/etc`, `/opt` read-only, empty `/var`,
  `/root`, `/home`, the sandbox's resolver), in the sandbox's `ci/` cgroup, and sets
  `docker_dir = None`. The process host drops `docker_dir` in host-system mode
  (`crates/yulius-ptyhost/src/containment.rs`).
- **Sandbox engines** (ADR-0151, ADR-0158, SPEC-187/196/199/201): the manager
  (`crates/yulius-sandbox/src/manager.rs`) keys everything by a name: holder state
  `runtime/sandboxes/<name>`, system layer and engine storage `sysroots/<name>/{upper,work,docker}`,
  cgroup `sandboxes/<name>/{system,agents,ci,engine}`. A Docker front beside each live holder
  serves `<state>/docker/docker.sock` and starts `dockerd` from the holder's merged system layer
  (base + upper) on the first connection, in the holder's user and network namespaces, with the
  engine view of `crate::engine::bwrap_argv` (data directory covered, the sandbox directory bound,
  its storage at `/var/lib/docker`). Idle engines and holders stop; adoption scans
  `runtime/sandboxes/*`; retirement (sandbox removal) stops the holder; the disk upkeep measures
  `sysroots/<name>/{upper,docker}` and prunes a running engine through its control socket.
- **Ids.** Every holder maps root to the service user and `1..` to the same subordinate range;
  what separates two sandboxes is their namespaces and the covered data directory, not the ids.
- **Agents' reach.** Agents are root of their sandbox's user namespace with `sudo apt`: they
  change the sandbox's system layer (`sysroots/<sid>/upper`) and its engine. The base
  (`system/base-<n>`, a copy of the host system) is a read-only lower layer for them.
- **Hosts.** `yulius host setup` installs Docker Engine on the host with
  `docker-buildx-plugin` (or Ubuntu's `docker.io` with `docker-buildx`) so bases copy `dockerd`;
  the host's own daemon is stopped. Production runs use the host's `/usr`, so the host's
  `docker` CLI and its buildx plugin.
- **Removal** (SPEC-206/215/276): a removed sandbox's `sysroots/<sid>`, `runtime/sandboxes/<sid>`
  and `cd/<sid>` are leftovers once nothing of it runs and its holder stopped; unknown names
  under those directories are `Unowned` leftovers.

## Requirements

- **PDE-001** Each sandbox has a production holder named `<sid>.prod` (a name no sandbox can
  have: `.` is outside the ids' alphabet): its own user, network and mount namespaces, its own
  `slirp4netns`, its own cgroup `sandboxes/<sid>.prod`, state `runtime/sandboxes/<sid>.prod`,
  system layer `sysroots/<sid>.prod/{upper,work}` over the current base, engine storage
  `sysroots/<sid>.prod/docker`. It has no forwarded ports (no relay to Yulius's internal port or
  the egress proxy) and no inbound dev ports.
- **PDE-002** Its Docker front serves its own engine, `dockerd` of the base (or of its own upper
  layer, which only that engine writes), never the sandbox's system layer; the same
  prerequisites as a sandbox's engine, and the same refusal with its reason when one is missing.
  Its engine's host name is `<slug>-prod` (cut to 63), so `docker info` tells it apart.
- **PDE-003** A production-stage run (a CD run of a `prod`-stage environment, and a production
  environment run of a delivery profile) prepares the production holder instead of the
  sandbox's, joins it in host-system mode, gets its Docker directory at `/run/yulius/docker` with
  `DOCKER_HOST` and `/run/docker.sock`, and joins its `ci/` cgroup. Nothing of the sandbox's
  holder, system layer, directory or engine is bound in. Dev-stage runs are unchanged.
- **PDE-004** Production runs' directories live in `<data>/cd/<sid>/runs/<run>`; the production
  engine binds `<data>/cd/<sid>/runs` (and nothing of the sandbox directory) at its host path, so
  `docker run -v "$PWD":…` in a production run mounts the run's files. A run directory a
  container wrote as a mapped id is removed as mapped root without following links.
- **PDE-005** The production holder holds a lease during a run, stops after the idle time like
  any holder (its engine first), is adopted after a server restart, and is retired with its
  sandbox (stopped once no run holds it).
- **PDE-006** Disk upkeep: its system layer and engine storage count toward the sandbox's quota
  (consumer `production`); a production run is refused while the sandbox is over the quota
  without that consumer (after the upkeep tried to free space), so production's own files never
  block the deploy that lets them be pruned; after each production run a sandbox over its quota
  gets the upkeep at once, which prunes the still-running production engine (build cache, unused
  images) through its control socket with the sandbox's engine.
- **PDE-007** Leftovers: `sysroots/<sid>.prod` and `runtime/sandboxes/<sid>.prod` belong to the
  sandbox `<sid>` (never `Unowned` while it lives) and go with a removed sandbox's files once its
  production holder stopped.
- **PDE-008** `yulius doctor --docker-smoke` also proves the production engine: a production
  run on the host's system reaches an engine named `<host>-prod`, builds with `docker buildx
  build --load`, round-trips an image through `docker save`/`docker load`, and a sandbox run
  sees neither that engine nor its socket.
- **PDE-009** Docs say it: `docs/architecture.md` (Containers and CD), `docs/operations.md`,
  `docs/remote-control.md`, the infra-bootstrap skill and `cd_status` lose "production runs have
  no Docker engine yet".

## Plan

- [x] Spec and ADR 0208: the production holder, compared with a per-run engine and the host's
- [ ] Production holder names, paths and validation (contracts, manager), retired with the sandbox
- [ ] Manager prepares the production holder: own engine view, no forwards, quota from the sandbox, measured and pruned
- [ ] Isolation port and wiring: prepare and release the production holder
- [ ] Production runs join the production holder with its Docker directory in host-system mode
- [ ] Production run directories under the CD directory, bound into the engine and removed as mapped root
- [ ] Leftovers know production trees and remove them with the sandbox
- [ ] Docker smoke proves the production engine (buildx, load, unreachable from the sandbox)
- [ ] Docs, cd_status and the infra-bootstrap skill say production runs have their engine
- [ ] Independent review, fixes, gates green, delivered into dev

## Acceptance criteria

- AC1 (PDE-001, PDE-002) The manager starts a holder `<sid>.prod` with its own paths and cgroup,
  no forwards and no inbound ports; its front's engine view has the production storage, the
  production runs directory and the production host name, and nothing of `sandboxes/<sid>` or
  `sysroots/<sid>`.
- AC2 (PDE-003) A production CD run's containment joins the production holder in host-system
  mode with the production Docker directory; the launched `bwrap` argv binds that directory at
  `/run/yulius/docker` and sets `DOCKER_HOST`, and binds nothing of the sandbox.
- AC3 (PDE-004) Production run directories are under `<data>/cd/<sid>/runs`; dev runs keep
  `<data>/runs/cd-<run>`.
- AC4 (PDE-005) Retiring a sandbox retires its production holder; a production holder is adopted
  by name; its relays are not served.
- AC5 (PDE-006) The sandbox's measurement includes the production trees as `production`; a
  production prepare is refused while the sandbox is over its quota without them; pruning
  reaches a running production engine, and a production run over the quota triggers it.
- AC6 (PDE-007) A live sandbox's `<sid>.prod` trees are not `Unowned`; a removed sandbox lists
  them, kept while the production holder runs.
- AC7 (PDE-008) The smoke has the production checks, and its unit tests pin their scripts.
- AC8 (PDE-009) No document or tool output says production runs have no Docker engine.

## Test plan

- Contracts: holder names validate (`<sid>.prod` yes; `.prod`, `a.b`, `x.prod.prod` no) in
  `HolderSpec`/`DockerFrontSpec`; the launch spec keeps `docker_dir` with `host_system`.
- Manager unit tests (fake holders as in the existing tests): production paths, retirement of
  both, no relays, engine view, quota refusal from the sandbox's reading, measurement consumers,
  prune of a fake production control socket.
- App unit tests: `production_containment` with the production join; run directory by stage.
- Wiring test: the launched argv of a production CD run.
- Integration (`crates/yulius/tests`): a CD production deploy against a recording isolation
  adapter prepares the production holder (not the sandbox's) and releases it; a removed
  sandbox's production trees in the leftovers.
- Real host: `yulius doctor --docker-smoke` with the production phase; what this sandbox cannot
  run is recorded in Evidence.

## Decisions

- ADR [0208](../../docs/decisions/0208-production-cd-runs-use-a-production-holder-with-its-own-docker-engine.md):
  a production holder per sandbox (chosen) over a per-run engine and the host's engine.
- The holder reuses the manager under a derived name (`<sid>.prod`) rather than a second
  manager: adoption, idle stop, retirement, egress restart, the front and mapped-tree removal
  come for free; the `.` keeps it apart from every sandbox id.
- Storage under `sysroots/<sid>.prod` (outside every sandbox directory, already a mapped tree the
  isolation removes) rather than `<data>/cd/<sid>`, which the service user removes; the run
  directories, which the server writes, are under `<data>/cd/<sid>/runs`.
- Same subordinate ids as the sandbox: the host offers one range; separation is by namespaces,
  as between two sandboxes. Recorded as a consequence.
- Quota shared with the sandbox (one budget per project), as production runs were already refused
  over it.
