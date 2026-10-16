---
yulius-task: 0951753e-0028-44a0-8068-09f1892ed05f
status: in-progress
---

# SPEC-196: Docker just works in every sandbox and session, without a restart

Status: in-progress
Date: 2026-10-16
Owner: task #182
Scope authorization: task #182 (requested by MrFishPL): Docker must work for agents in every
sandbox of the instance with no reload, restart or new session; a stable path in every run, existing
holders gaining an engine in place, running sessions getting the socket, start on demand, no silent
failure, and a single host command that verifies it on the real VM. Amends SPEC-187 / ADR-0151.
Fixes #597.

## Goal

Every isolated run (task agent, Superagent, Shell, CI job; not a production CD run) can run
`docker …` at any time, whatever its holder's or its own age: against the sandbox's own engine, or
an answer that names why there is none and what the operator does about it. A person proves it on
the host with one command.

## Context

- **Symptom** (reproduced in this task's sandbox, 2026-10-16): `docker version` → "dial unix
  /var/run/docker.sock: connect: no such file or directory"; `/run/docker.sock`, `/run/yulius` and
  `DOCKER_HOST` absent. Agents of other sandboxes report the same after #583 was promoted (#593).
- **Root causes** (read on `dev` 56bf4bb8):
  1. *The engine is a property of the holder's start.* `holder::start` records `engine` once
     (`spec.engine.is_some() && docker`), and only the holder process serves the socket
     (`holder::serve_engine`). Holders survive updates and are adopted (`Manager::adopt`), and one
     with live sessions never idles, so a sandbox busy during the upgrade (or whose socket failed)
     keeps no engine for as long as it is used. `Manager::docker_for` encodes it: "a holder from
     before engines … serves none".
  2. *Runs get the socket only at spawn, as a file.* `containment::joined_argv` binds
     `<state>/docker.sock` at `/run/docker.sock` only when `join.engine_socket` is set at launch
     (`record.engine.then(…)`). Every run started before has nothing, a file bind would keep the
     old inode if the socket were recreated, and `DOCKER_HOST` is never set.
  3. *Silent opt-out.* Without a prerequisite (`YULIUS_SANDBOX_DOCKER=off`, no `dockerd`, no
     cgroup delegation, a writable executable) runs get no socket at all and no reason.
  4. *Never verified live.* `isolation_docker` is opt-in and needs a hand-made transient unit.
- **Why the tests missed it:** every test starts a fresh holder and a fresh run with the engine
  already decided; none adopts a holder or keeps a run across the change. The unit test
  `docker_is_told_per_sandbox_from_its_holder` asserted the defect as intended behaviour.
- **What a run's mount namespace allows.** A joined run's `bwrap` makes a mount namespace owned
  by the sandbox's user namespace, with its own `/run` tmpfs and the data directory covered by a
  tmpfs. The server owns that user namespace, so a helper that `setns`es into it has every
  capability there and may enter the run's mount namespace. A bind cannot cross mount namespaces
  with `mount(2)` (`check_mnt`), but a detached tree can: `open_tree(OPEN_TREE_CLONE)` where the
  source is visible (the holder's mount namespace), then `move_mount` inside the run's (Linux
  5.2+, what `machinectl bind` does). The seccomp filter binds runs, not this helper.

## Requirements

| ID | Requirement |
| --- | --- |
| DKL-001 | Every joined run that is not a production CD run binds the sandbox's Docker directory (`<state>/docker`, a directory) read-only at `/run/yulius/docker`, links `/run/docker.sock` (= `/var/run/docker.sock`) to `yulius/docker/docker.sock`, and has `DOCKER_HOST=unix:///run/yulius/docker/docker.sock`. The socket inside may disappear and be recreated without touching runs. |
| DKL-002 | A Docker front per live holder (`yulius sandbox-host docker`, a process of its own in the holder's user, network and mount namespaces and the sandbox's `system` group) serves `<state>/docker/docker.sock` in one of three modes: **engine** (the SPEC-187 supervisor: start on the first connection, idle stop), **relay** (a holder of SPEC-187 that serves an engine itself: bytes to its `<state>/docker.sock`), **refuse** (no engine possible: every request gets HTTP 503 with a Docker-API JSON `message` naming the reason and the operator's fix). It survives server restarts like the holder, exits when its holder or its socket is gone, and stops its engine on `SIGTERM`. |
| DKL-003 | The server gives every live holder a front: after starting a holder, when adopting one (whatever its age), and at each upkeep tick when the front died. It restarts a front whose mode no longer matches (refuse ↔ engine/relay, a new reason); an engine front gets the engine's storage when the holder predates engines; an engine's other settings change only with a new holder. Stopping a holder stops its front first. The front and its helpers never count as the sandbox's runs. Runs and their sessions are never restarted for any of this. |
| DKL-004 | Runs that are already running get the Docker directory without a restart: at each upkeep tick the server finds the mount namespaces of the sandbox's runs (processes in its network namespace, outside the engine's cgroup, the holder's and the front's own excluded) that have no `/run/yulius/docker` mount and are not production CD runs (`/var` a tmpfs), and `yulius sandbox-host bind-docker` moves a clone of `<state>/docker` there (read-only) and adds the `/run/docker.sock` link when nothing is at that path. A namespace is tried at most 3 times. |
| DKL-005 | Reasons and fixes are named, the same words in the socket's answer, the server's log and `yulius doctor`: the setting is off; the host has no `dockerd`; the server has no cgroup delegation; the `yulius` executable is writable by the sandbox's root; the holder's base predates Docker on the host; the sandbox's cgroup could not be prepared (a running engine is kept then). A failed engine start keeps SPEC-187's answer (engine log tail) and adds the operator's check (`yulius doctor --docker-smoke`). |
| DKL-006 | The briefing's `## Containers` section with an engine says `docker info` checks it and that an error names the cause; without one it says `docker info` says why. CTR-002 budgets hold. |
| DKL-007 | `yulius doctor --docker-smoke` on the host: as the data directory's owner, in a delegated transient unit (it re-runs itself through `systemd-run` when started as root outside one), with the current base, it starts a throwaway sandbox whose holder first gets no engine and a run started then; then, as an updated server would, adopts the holder with Docker on. The old run must reach the engine (`docker run --rm hello-world`, a `docker build`, a two-service `docker compose up`), keep working after the front is replaced (socket recreated), and a new run must have `DOCKER_HOST`. It prints one line per check and exits 0 only when all pass, and removes the sandbox. |
| DKL-008 | Isolation refusals of SPEC-187 hold (DKR-003/004/009): the directory holds only the socket; no host loopback, no other sandbox, no host Docker socket. |

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Write the fix spec and the regression test that fails
- [ ] Every joined run binds the sandbox's Docker directory and gets DOCKER_HOST
- [ ] A Docker front per sandbox serves the socket: engine on demand, relay, or an explanatory refusal
- [ ] The manager attaches fronts to new and adopted holders, restarts them and stops them with the holder
- [ ] Runs already running get the Docker directory without a restart
- [ ] Agents, doctor and the docker smoke command say how to check
- [ ] Host-level isolation test of adoption, live injection and socket recreation; ADR and docs
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green and delivered to dev

## Acceptance criteria

- An adopted holder whose record says it serves no engine gets a front in engine mode and its
  socket answers (manager test with a fake front); one with SPEC-187's engine gets a relay; with a
  missing prerequisite a refusal naming it.
- A front in refuse mode answers `docker`'s request with 503 and the reason; relay mode reaches the
  older socket byte for byte; a dead front is started again by the next tick and a refuse-mode
  front is replaced once Docker is possible.
- A joined run's argv binds the directory (not a file), links `/run/docker.sock`, sets
  `DOCKER_HOST`; a production CD run has none of them.
- The planner binds a run's namespace without the mount, skips one that has it and a production
  CD run's.
- `yulius doctor --docker-smoke` exists, is documented in `docs/operations.md`, and passes on the
  production VM after promotion (a person runs it; not runnable in a sandbox or in Yulius CI).
- `scripts/dev ci` passes.

## Test plan

- Unit (`yulius-ptyhost`): joined argv with the Docker directory, its link and `DOCKER_HOST`;
  none for a host-system run.
- Unit (`yulius-sandbox`):
  - the front's three modes over a real Unix socket (fake engine for engine mode);
  - the front exits when its socket is replaced;
  - the mount planner over `mountinfo` fixtures;
  - the manager: an adopted pre-engine holder gains an engine front, a SPEC-187 holder a relay,
    a missing prerequisite a refusal; a dead front is restarted; a mode change restarts it; the
    front is not counted as a run. Fronts are a fake executable (Python) here, holders the test
    process itself (`alive` holds for its own namespaces).
- Unit (`yulius`): doctor's reasons are the manager's; the smoke's re-exec argv.
- Unit (`yulius-domain`): the briefing variants and budgets.
- Host-level (opt-in, `crates/yulius/tests/isolation.rs`, `isolation_docker_live`): the smoke's
  scenario through the same code (`YULIUS_ISOLATION_TEST=1`, delegated unit, Docker installed).
  **Not runnable here:** this sandbox and Yulius CI are isolated runs whose seccomp filter refuses
  `setns`, mounts and namespaces.

## Decisions

- [ADR-0158](../../docs/decisions/0158-docker-reaches-every-run-through-a-front-and-a-directory-moved-into-live-runs.md),
  amending ADR-0151: a front process per holder instead of the holder's own supervisor; a
  directory bind plus `DOCKER_HOST`; live runs reached by `open_tree` + `move_mount` from a
  single-threaded helper. Rejected: restarting holders or sessions; a socket on the system layer or
  in the sandbox directory (writable by agents, so the server would have to defend every path
  against links); an abstract socket (not reachable at `/var/run/docker.sock`); the server process
  serving the socket (gone on every update, and an engine child of it would die with it).
- Reversible defaults: 3 injection attempts per mount namespace; the front's holder check every
  5 s; `--docker-smoke` uses `hello-world`, `busybox` and a `FROM busybox` build.

## Evidence

(Filled in at the end.)
