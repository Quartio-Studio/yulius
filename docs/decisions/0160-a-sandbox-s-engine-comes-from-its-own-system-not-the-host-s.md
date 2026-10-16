# 0160. A sandbox's engine comes from its own system, not the host's

## Status

Accepted. Specified in [SPEC-199](../../.ai/specs/SPEC-199-i-got-this.md); task #185. Amends
[0151](0151-a-sandbox-runs-its-own-docker-engine-under-an-engine-seccomp-profile.md) and
[0158](0158-docker-reaches-every-run-through-a-front-and-a-directory-moved-into-live-runs.md).

## Context

ADR-0151 made "`dockerd` on the host" a prerequisite. The server read it once, at start
(`which("dockerd")`), and refused every sandbox's Docker without it. But the engine never runs the
host's `dockerd`: it runs the one in the sandbox's merged system layer, which is its base (a copy
of the host's system) plus its overlay upper layer, where `sudo apt-get install` writes.

On the reference VM, Docker was purged from the host on 2026-10-12 together with an unrelated CI
runner. After #602 every sandbox answered "the host has no `dockerd`", including sandboxes whose
base, copied on 2026-10-11, still has `dockerd`. An agent could not install Docker in its sandbox
to fix it, and the only fix it could suggest was a host change made by a person.

## Decision

- **The sandbox's own system decides.** A front serves an engine when the sandbox's system has
  `dockerd` (`usr/local/sbin`, `usr/local/bin`, `usr/sbin` or `usr/bin`). The decision is
  `manager::system_has_dockerd`:
  - an entry in the sandbox's upper layer decides;
  - with no entry there, the holder's base decides.

  `IsolationConfig.docker` carries only `YULIUS_SANDBOX_DOCKER`.
- **The upper layer is read as untrusted.** It is walked from one directory descriptor to the
  next (`openat` with `O_PATH | O_NOFOLLOW`, then `fstatat` without following), so no path is
  looked up twice and no link is followed, even if the sandbox swaps one in meanwhile:
  - a link at `dockerd` counts, since it resolves inside the sandbox;
  - anything else over a path (a whiteout, a directory, a link over a parent) hides the base's
    `dockerd`.

  The server therefore learns nothing about host paths from links the sandbox plants.
- **Installing Docker in a sandbox turns its engine on.** Without `dockerd` the refusal tells the
  agent to run `sudo apt-get install -y docker.io` (or an operator to install Docker on the host,
  for new bases). The front is asked at every tick (30 s), so it switches to an engine without a
  restart.
- **The host's `dockerd` is advice.** `yulius doctor` warns without it, since new bases then have
  none, and still checks the delegation and the kernel modules. The smoke test needs it, because
  its throwaway sandbox has only the current base.

## Alternatives

- **Re-read the host's `dockerd` at every tick.** This removes the restart, but it still refuses
  sandboxes that have `dockerd` and makes installs inside a sandbox useless.
- **Yulius downloads Docker's static binaries.** The files would belong to the service user,
  which is the sandbox's root, so an engine could rewrite them for every sandbox (ADR-0151's
  executable rule). It would also add a pinned download to maintain. Rejected.
- **Only tell the person to reinstall Docker on the host.** This is still the way to get Docker
  into every new base, but it leaves agents blocked on a person.

## Consequences

- An agent gets Docker in its sandbox on its own. A sandbox keeps its base's `dockerd` until its
  holder next starts on a newer base; one installed in the sandbox itself stays. A running engine
  is never cut because `dockerd` left the sandbox's system for a moment (a package swap): the
  front stays an engine until the holder starts again.
- An engine may now run a `dockerd` the sandbox installed itself. This grants nothing new:
  - the sandbox's root could already replace the base's `dockerd` in its upper layer;
  - agents can already run `--privileged` containers on their engine.

  The boundary stays the user namespace, the engine seccomp profile and the cgroup (ADR-0151).
- The host still provides the kernel modules, which a sandbox cannot load, and the cgroup
  delegation.
- Overlay opaque directories are not read. If an upper `usr/bin` is opaque and has no `dockerd`,
  the base's still counts. The engine then fails to start, and SPEC-187's answer (the engine log
  tail) names why.
