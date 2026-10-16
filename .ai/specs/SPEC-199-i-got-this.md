---
yulius-task: 23fb7ca3-7265-40ac-aaf9-6ab43b3e6893
status: done
---

# SPEC-199: A sandbox gets Docker from its own system, not from the host's

Status: done
Date: 2026-10-16
Owner: task #185
Scope authorization: task #185 (requested by MrFishPL): after #602 was promoted, `docker` in a
sandbox answers "the host has no `dockerd`; an operator installs Docker … on the host", and the
agent can only hand the person a host change. Amends SPEC-196 DKL-005 and ADR-0158. Amended by
[SPEC-201](SPEC-201-analyse-what-happened-in-185-task-and-fi.md) (task #186, #610): every install
hint (DSY-003, DSY-004, DSY-006) names buildx and compose with the engine.

## Goal

A sandbox whose own system has `dockerd` gets its engine, whatever the host has; one without it
is told how the agent installs it in the sandbox (no person needed) or how an operator gives it to
every new base. Installing Docker inside a sandbox turns the engine on within a tick, with no
restart.

## Context

- **Symptom** (reproduced in this task's sandbox, 2026-10-16, server at `06744993`): `DOCKER_HOST`
  and the socket are present; `docker info` answers "the sandbox's Docker engine is not available:
  the host has no `dockerd` …". Yet `/usr/bin/dockerd`, `containerd` and `runc` are in this
  sandbox's `/usr` (its base `base-3`, copied on 2026-10-11).
- **Why the host has none.** The host had `docker-ce` from 2026-10-10. On 2026-10-12 an operator
  session (imported, "Get familiar with the previous session …") purged Docker with the unrelated
  CI runner it came with. Bases built after that have no `dockerd`; older ones still do.
- **Root cause.** `wiring::sandbox::resolve` sets `IsolationConfig.docker` from
  `which("dockerd")` on the host, once at start, and `Manager::docker_unavailable` refuses every
  sandbox with `DOCKER_NO_DOCKERD` when it is missing. But the engine never runs the host's
  `dockerd`: `engine::bwrap_argv` runs `dockerd` from the sandbox's merged system layer (its base
  plus its overlay upper, `sysroots/<id>/upper`, where `sudo apt-get install` writes). So the host
  check is a proxy that is wrong both ways: it refuses a sandbox whose system has `dockerd` (this
  one), and it makes `sudo apt-get install docker.io` in a sandbox useless. It is also read only at
  start, so installing Docker on the host needs a server restart.
- `Manager::front_mode` already checks the holder's base for `dockerd` (`DOCKER_OLD_BASE`), but not
  the upper layer, and its reason assumes the host has Docker.

## Requirements

| ID | Requirement |
| --- | --- |
| DSY-001 | Whether a sandbox can have an engine no longer depends on the host's `dockerd`: `IsolationConfig.docker` carries only the `YULIUS_SANDBOX_DOCKER` setting, and `Manager::docker_unavailable` only the setting, the delegation and the executable. |
| DSY-002 | A front serves an engine when the sandbox's merged system has `dockerd` at `usr/local/sbin`, `usr/local/bin`, `usr/sbin` or `usr/bin`: an entry in the sandbox's upper layer decides (a file or a link counts, an overlay whiteout hides the base's), else the holder's base. The server never follows a link the sandbox could have planted in its upper layer (walked by directory descriptor with `O_NOFOLLOW`). A running engine front is kept when `dockerd` leaves the sandbox's system. |
| DSY-003 | A sandbox without `dockerd` gets a refusal (`DOCKER_NO_DOCKERD`) that names both fixes: install it in the sandbox (`sudo apt-get install -y docker.io`), after which the engine starts within a minute; or an operator installs Docker on the host and the sandbox gets it when its holder next starts on a newer base. Installing it in the sandbox switches the front to an engine at the next tick, without restarting anything. |
| DSY-004 | `yulius doctor` without a host `dockerd` still checks the delegation and the kernel modules, and warns that new bases have no `dockerd`, with both fixes. `yulius doctor --docker-smoke` still needs a current base with `dockerd` and says so. |
| DSY-006 | The briefing's `## Containers` section with an engine names the in-sandbox install (`sudo apt install docker.io` adds it) for a system without `dockerd`, short enough that tight-budget renderings keep the section whole. |
| DSY-005 | ADR-0160, `docs/operations.md`, `docs/architecture.md` and `docs/changes.md` say that a sandbox's engine comes from its own system. |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] A sandbox's Docker front decides from its own system layer, not the host's dockerd
- [x] The refusal, doctor and smoke name the in-sandbox and host fixes
- [x] ADR-0160 and the operations and architecture docs say the engine comes from the sandbox's system
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- With the host's `dockerd` absent, a manager with Docker on reports no unavailability, and a
  holder whose base has `dockerd` gets an engine front (fails before: `DOCKER_NO_DOCKERD`).
- A holder whose base lacks `dockerd` but whose upper layer has `usr/sbin/dockerd` gets an engine
  front; a whiteout in the upper layer over the base's `dockerd` gives the refusal; a link planted
  at `upper/usr` pointing to a directory with `bin/dockerd` is not followed.
- The refusal text names `sudo apt-get install` and the host install.
- Doctor without a host `dockerd` warns with both fixes and still reports missing modules.

## Test plan

- Unit tests in `yulius-sandbox` (`manager.rs`): the layered lookup (base only, upper file, upper
  link, whiteout, planted directory link) and `front_mode` for each.
- Unit tests in `yulius` (`wiring/sandbox.rs`, `doctor.rs`): `docker_setting` and `docker_check`.
- The full gate (`scripts/dev ci`). Not testable here: a live engine (this sandbox's seccomp
  refuses namespaces), and `apt-get install docker.io` in a fresh sandbox on the real host.

## Decisions

- **Per-sandbox, from the system the engine actually runs** (ADR-0160), over (a) re-reading the
  host's `dockerd` at every tick: it fixes the restart but still refuses this sandbox and blocks
  in-sandbox installs; (b) Yulius downloading Docker's static binaries itself: the files would be
  owned by the service user, which is the sandbox's root, so an engine could rewrite them for
  every sandbox (ADR-0151), and it adds a supply-chain pin to maintain; (c) only reporting that
  the host needs Docker: the person asked for Docker that works without them.
- An engine from a binary the sandbox installed itself grants nothing new: the sandbox's root can
  already replace the base's `dockerd` in its upper layer, and any agent can already run
  `--privileged` containers on its engine (ADR-0151's boundary is the user namespace, the engine
  seccomp profile and the cgroup).
- Overlay opaque directories are not read: an upper `usr/bin` made opaque with no `dockerd` would
  still count the base's; the engine then fails to start and SPEC-187's answer names it.

## Evidence

- **Reproduced** in this task's sandbox (server `06744993`): `docker info` → "the host has no
  `dockerd` …" while `/usr/bin/dockerd`, `containerd` and `runc` are in the sandbox's `/usr`
  (`base-3`, 2026-10-11). The host's purge of Docker on 2026-10-12 is in an imported operator
  session (dpkg log: `Removing docker-ce …`).
- **Tests that failed before the change:**
  - `manager::tests::a_sandbox_whose_own_system_has_dockerd_gets_an_engine` failed at "a link over
    usr hides the base's" (the upper layer was not read), then at `usr/local/sbin`;
  - `wiring::sandbox::tests::only_the_setting_turns_sandbox_docker_off` did not compile against the
    old signature (host `dockerd` argument);
  - `the_front_mode_follows_the_holder_and_the_prerequisites` (the `DOCKER_NO_DOCKERD` refusal and
    its fixes), `doctor::tests::the_docker_check_names_what_sandboxes_lack` (old text, early return
    before delegation and modules), `a_front_is_replaced_only_when_its_mode_changed` (a running
    engine replaced on `DOCKER_NO_DOCKERD`) and
    `briefing::tests::a_sandbox_with_an_engine_is_told_docker_works_and_how` (no install hint) failed
    on their new assertions.
- **Gate:** `scripts/dev ci` exit 0 at `52bdc990` (2036 passed, 18 skipped; earlier at `93ceee70`
  the same). An intermediate run at `cd850bbf` failed two tight-budget briefing tests (the longer
  section was cut); the hint was shortened to fit the section's budget, and the run passed.
- **Security:** `scripts/dev security` exit 2: Vidoc refused the key (HTTP 401), so the scan was
  **not run** (not passed). It sent nothing for the four changed code files.
- **Review round 1** (fresh session, at `93ceee70`): REQUEST CHANGES, three low findings in own
  code, all fixed:
  - L1: the docs claimed a base's `dockerd` survives the host's purge; reworded to "until the
    holder's next base";
  - L2: no test of an upper layer with `usr/bin` but no `dockerd`; added;
  - L3: the briefing told sandboxes without `dockerd` that Docker works; it now names
    `sudo apt install docker.io`.

  Nits, also fixed:
  - the walk is by directory descriptor (`O_PATH | O_NOFOLLOW`), not repeated `lstat`;
  - `usr/local/sbin` counts;
  - a running engine is kept when `dockerd` leaves the system;
  - the whiteout comment is accurate.

  Its pre-existing note, a base link resolved on the host, is left as is: the base is Yulius's
  own copy of the host.
- **Review round 2:** the verdict on the final head is recorded in the pull request.
- **Not testable here:**
  - a live engine (this sandbox's seccomp refuses namespaces);
  - `sudo apt install docker.io` in a fresh sandbox on the host, which also needs the kernel
    modules that `yulius doctor` lists;
  - `yulius doctor --docker-smoke`, which needs a base with `dockerd`, so Docker back on the host.
