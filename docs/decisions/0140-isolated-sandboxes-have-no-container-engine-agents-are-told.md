# 0140. Isolated sandboxes have no container engine, and their agents are told so

## Status

Superseded by [0151](0151-a-sandbox-runs-its-own-docker-engine-under-an-engine-seccomp-profile.md) (task #172). Specified in [SPEC-173](../../.ai/specs/SPEC-173-i-got-this-from-a-different-projectfix.md);
task #163, #550. Builds on [0003](0003-sandbox-isolation.md) (ADR-22) and
[0109](0109-cd-runs-verified-commits-out-of-the-agents-reach.md).

## Context

An agent of another project designed a deployer around a local Docker daemon (build images, copy
them to VMs, keep the previous ones for rollback). Only afterwards did it find that `docker` exists in
its sandbox but no engine does. ADR-22's containment gives runs Docker's default capability set and a
seccomp filter without mount APIs or nested user namespaces, which an engine needs. Where the host
has Docker, the system-layer base copies its client and daemon binaries with `/usr`, so the client
is there and suggests an engine. The briefing told agents nothing about it.

## Decision

- Isolated sandboxes keep running no container engine, in agent runs, CI jobs and CD runs alike.
- When the namespace holder is wired, the session briefing has a short `## Containers` section that
  says so and names what works. It is the last mandatory part, so a tight budget cuts it before the
  task header or the architecture pointer:
  - an engine on another machine through a `docker` client (`DOCKER_HOST=ssh://…` or a
    `docker context`), with images built where they run and rollback images kept there or in a
    registry;
  - `apt` for the services that tests need, started by the tests or jobs themselves
    (`policy-rc.d` starts none).
- A briefing without isolation says nothing about engines.

## Alternatives

- **An engine per sandbox in the namespace holder** (rootless-Docker style). Its socket would let any
  run start `--privileged` containers past the per-run capabilities and seccomp filter that SPEC-002
  §2.8's security properties rest on. It needs its own spec, analysis and the host-level isolation
  suite: #551.
- **The host's engine bound into runs.** Root on the host for every agent: never.
- **Remove the `docker` client from the base.** It breaks the remote engine, the path that works,
  and a sandbox's own `apt install docker.io` would bring it back anyway.

## Consequences

- Agents design for a remote engine from the start. Production deploys never depend on images an
  agent can reach.
- Projects whose tests need `docker compose` or testcontainers still cannot run them in Yulius until
  #551. The briefing says so before anyone builds on it.
- The section costs about 260 bytes of the briefing's mandatory part, in isolated sandboxes only.
  With extreme inputs it takes the optional parts' room, as any mandatory part does.
