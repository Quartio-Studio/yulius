---
yulius-task: f0bdd2d9-5c1c-48c4-a929-c1c3586fe4fd
status: done
---

# SPEC-187: Docker works inside isolated sandboxes, through an engine of the sandbox's own

Status: done
Date: 2026-10-16
Owner: task #172
Scope authorization: task #172 (requested by MrFishPL): "Make Docker usable inside Yulius
sandboxes. Agents (tasks and the Superagent) and Yulius CI jobs should be able to run `docker
build`, `docker run` and `docker compose`", keeping ADR-0003 / SPEC-002 ADR-22, never mounting the
host's socket, preferring a rootless engine inside the sandbox's own namespaces, recording the
analysis in an ADR that amends ADR-0003/ADR-22, and reverting the "no container engine" briefing
of #163 (SPEC-173). Fixes #551.

## Goal

In an isolated sandbox, an agent session, the Superagent, a person's Shell and a Yulius CI job run
`docker run`, `docker build` and `docker compose` with the `docker` client as installed, against
an engine that belongs to that sandbox alone. Containers stay inside the sandbox's boundary: its
user, network and cgroup, its disk quota, its egress. They never reach the host's loopback,
another sandbox or the host's Docker.

## Context

- **The person's blocker.** A Superagent in another sandbox has been building a CD pipeline since
  2026-10-15. The sandbox has `/usr/bin/docker` (copied with the host's `/usr` into the base) but
  no engine: no socket, no `DOCKER_HOST`. #163 (SPEC-173, ADR-0140) only made the briefing say so.
- **Why there is none (measured in this task's sandbox, 2026-10-16).** A run has Docker's default
  capability set and the seccomp filter of ADR-22 §6, which refuses the mount APIs, `setns`,
  `pivot_root` and `CLONE_NEWUSER`. `dockerd` fails at its first `iptables` (no `CAP_NET_ADMIN`),
  `unshare -U|-n|-m` is refused. Every run starts with `NoNewPrivs: 1`.
- **What the holder already gives.** One namespace holder per sandbox (`holder.rs`): a user
  namespace (root = the service user, ids 1–65535 = its subordinate range), a private network
  namespace with `slirp4netns --disable-host-loopback` egress, a mount namespace holding the
  overlay system layer, and per-sandbox cgroup v2 limits (`sandboxes/<sid>/{system,agents,ci}`
  with `Delegate=yes`). The holder process itself runs with every capability of the sandbox's
  user namespace and no seccomp filter. Its mount namespace sees the whole host tree as the service
  user sees it (`--dev-bind / /`): `master.key`, the database and the other sandboxes. Runs see
  only their binds.
- **Host (production VM, as this sandbox sees it).** Ubuntu 24.04, kernel 6.8. Docker 29.9
  (`dockerd`, `containerd`, `runc`, `docker-proxy`, `rootlesskit`, buildx and compose plugins) is
  in the base. Loaded modules: `overlay bridge veth nf_tables nft_compat nft_chain_nat nf_nat
  nf_conntrack xt_MASQUERADE xt_addrtype xt_conntrack xt_nat ip_tables`, no `br_netfilter`.
  `apparmor_restrict_unprivileged_userns=1`.
- **What an engine needs that a run does not have.**
  - Mounts (overlay layers, `/proc`, `/sys`, binds), `pivot_root` and `setns` for `runc`.
  - `CAP_SYS_ADMIN` and `CAP_NET_ADMIN` in a user namespace that owns its network and mount
    namespaces: bridges, veths and NAT rules.
  - A writable cgroup v2 subtree.
  - Overlay storage on a filesystem that is not itself overlayfs.
  - A sysfs it may mount over and a writable `/proc/sys/net`.
- **Docker in an unprivileged user namespace** is a known-working setup: Docker in unprivileged
  LXC or Incus containers ("nesting"). `dockerd` runs as root of a user namespace, not the
  host's, with a delegated cgroup subtree, kernel overlayfs with `userxattr` (Linux 5.11+) and a
  private network namespace. A sandbox's holder already is such a namespace set.
- **Related work.** SPEC-173/ADR-0140 (the briefing this reverts), SPEC-008 and SPEC-059 (disk
  upkeep and the soft quota, `crates/yulius-app/src/disk.rs`), SPEC-053 (CPU weights of the
  sandbox's leaves), ADR-0121 (the Superagent joins its sandbox like a task run). Every contained
  run (task, Superagent, Shell, CI job) enters through `App::isolation_join`, so one change there
  covers all of them.

## Requirements

| ID | Requirement |
| --- | --- |
| DKR-001 | When a sandbox's runs are isolated, the host has `dockerd` (so the base has it), the cgroup delegation is on and `YULIUS_SANDBOX_DOCKER` is not `off`, the sandbox has a Docker engine of its own. Every contained run joined to the holder (agent session, Superagent, Shell, CI job; not a production CD run on the host's system) finds its socket at `/run/docker.sock` (= `/var/run/docker.sock`), so `docker`, `docker compose`, buildx and testcontainers work without settings. |
| DKR-002 | The engine starts on the first connection to that socket and is ready or answers why not (a Docker-API JSON error naming the engine log's last lines) within 60 s; clients queued behind a failed start share its answer for 5 s. The holder never follows a link at the engine's control socket, which the engine (and any container) can write. It stops after `YULIUS_SANDBOX_IDLE_MINUTES` with no client connection and no container process. It lives as long as the holder, so it survives a Yulius restart or update, and while it runs the holder is not idle. |
| DKR-003 | The engine runs in the sandbox's user, network and cgroup namespaces and never in the holder's mount namespace. It gets a run's file view (the system layer, the sandbox directory at its host path, a tmpfs over the data directory), plus only: its storage, its control socket's directory, a fresh read-only sysfs of the sandbox's network, the engine's own cgroup subtree at `/sys/fs/cgroup` (its cgroup namespace's root), the sandbox's Docker socket and the `yulius` executable read-only. No host Docker socket, no host `/sys`, no host cgroup tree, no `runtime/` or other sandboxes. |
| DKR-004 | The engine has every capability of the sandbox's user namespace under the engine seccomp profile. It is the run profile plus the mount APIs, `umount2`, `pivot_root` and `setns`, with the keyring calls answering `ENOSYS`. Nested user namespaces, `bpf`, `io_uring`, `perf_event_open`, modules, kexec, `userfaultfd`, `open_by_handle_at` and the other refusals stay. A seccomp filter is inherited and cannot be removed, so every container, `--privileged` included, is bound by it. Environment: cleared, then `PATH`, `HOME=/root`. Its read-only binds are not locked (same user namespace), so a server whose `yulius` executable the sandbox's root (the service user) owns gets no engine. |
| DKR-005 | Containers are limited by the sandbox's cgroup. The engine lives in `sandboxes/<sid>/engine` (CPU weight 100 beside `system`, `agents`, `ci`), and containers are created below it, so the sandbox's `memory.max`, `pids.max` and `cpu.max` bound them. Per-container flags (`-m`, `--cpus`) are not applied: no controller passes below a group holding the engine (documented). |
| DKR-006 | Engine storage (`sysroots/<sid>/docker`, outside the sandbox directory and never bound into runs) counts toward the sandbox's soft disk quota as the consumer "containers". When disk upkeep must free space (≥ 90 % of the quota or a refused start) and the engine runs, its build cache and the images no container uses are pruned through its own control socket, reported and audited like the other build caches. A stopped engine is never started for it, so a pressed sandbox never keeps an engine or its holder from idling. Containers and volumes are never pruned. |
| DKR-007 | Container traffic leaves through the sandbox's network namespace and its egress (`slirp4netns` with host loopback off), like a run's. With the engine on, the relays of the internal listener and `YULIUS_SANDBOX_FORWARD` listen on every IPv4 address of the sandbox's network, not only its loopback. A container then reaches the egress proxy and its secret injection at `host.docker.internal:<port>` (`--add-host=host.docker.internal:host-gateway`). Nothing outside the sandbox can connect in: `slirp4netns` forwards no port inward. |
| DKR-008 | A port a container publishes inside the sandbox's dev-server range is relayed to the host's loopback like a dev server's, because `docker-proxy` listens in the sandbox's network. |
| DKR-009 | From a container: the host's loopback (`10.0.2.2`, a host loopback port), another sandbox's dev server and the host's Docker are unreachable. The socket a container mounts is its own sandbox's engine. |
| DKR-010 | The briefing's `## Containers` section, in an isolated sandbox whose holder serves an engine (not merely the setting: a holder adopted from before engines, or whose socket failed, serves none), says Docker works there and how: its own engine, started on first use, bind mounts from the sandbox directory, images in the disk quota, `-p` in the port range for previews, `host.docker.internal` for the egress proxy. In an isolated sandbox without it, the section keeps saying there is no engine (SPEC-173 CTR-001). Without isolation, no section. It stays the last mandatory part (SPEC-173 CTR-002). |
| DKR-011 | `yulius doctor` reports whether sandboxes get Docker and, if not, why: no `dockerd`, no cgroup delegation, `YULIUS_SANDBOX_DOCKER=off`. With the engine on, it lists the kernel modules a sandbox cannot load itself (`overlay bridge veth nf_tables nft_compat nf_nat nf_conntrack xt_MASQUERADE xt_addrtype xt_conntrack`) that are neither loaded nor built in. |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The engine's seccomp profile and file view, built and tested as pure functions
- [x] The holder serves the sandbox's Docker socket and starts and stops the engine on demand
- [x] The server gives sandboxes an engine: runs see its socket, it gets its cgroup and storage
- [x] Engine images count toward the disk quota and are pruned under pressure
- [x] Agents are told Docker works; doctor and operator docs name the prerequisites
- [x] Host-level isolation tests: docker run, build, compose and the refusals
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- In a fresh sandbox on a host meeting DKR-001, as an agent and as a CI job:
  - `docker run --rm hello-world` succeeds;
  - `docker build` of a two-line Dockerfile succeeds;
  - `docker compose up` of a two-service file, one service fetching from the other by its service
    name, exits 0.
- From a container: a host loopback port and `10.0.2.2` are refused, another sandbox's dev server
  is unreachable, and `docker info` through a mounted socket names this sandbox's engine.
- A `--privileged` container's `uid_map` is the sandbox's, `unshare -U` fails in it, and a
  `cgroup2` it mounts shows only the engine's subtree.
- The engine's bwrap argv carries no holder state other than its two sockets, and no host `/sys`
  or cgroup tree. The engine filter allows `mount`/`setns`/`pivot_root`, answers `ENOSYS` for
  `keyctl` and refuses `CLONE_NEWUSER`, `bpf` and `io_uring_setup` (unit tests).
- A run's argv binds the socket at `/run/docker.sock` when the join names one, never for a
  host-system run (unit tests).
- The briefing says Docker works with the engine on, keeps "no container engine" without it,
  and the mandatory parts survive the budget tests.
- `scripts/dev ci` passes.

## Test plan

- Unit (`yulius-sandbox`):
  - the engine filter per call against the run filter;
  - the engine argv: binds, order, no host `/sys`, `--unshare-cgroup`, `--clearenv`, the `yulius`
    executable read-only;
  - `HolderSpec` with and without `engine` (validation, old records without the field);
  - the idle rule: connections, container cgroups, time;
  - the front proxy end to end with a fake engine: a socket server started by a fake command,
    a byte-exact round trip, a 503 JSON error when the command exits, the start bounded by a
    deadline, one start for concurrent clients;
  - the prune client against a fake API (paths, filters, `SpaceReclaimed` summed);
  - the `engine` cgroup leaf with and without the `cpu` controller;
  - forwards on `0.0.0.0` with the engine on.
- Unit (`yulius-ptyhost`): the socket bind in joined runs, absent for host-system runs and without
  an engine.
- Unit (`yulius-domain`): the briefing with the engine, without it, and without isolation; the
  CTR-002 budget tests with the new text.
- Unit (`yulius-app`): disk upkeep under pressure asks the port to prune the engine and reports
  the freed bytes; it does not without pressure.
- Unit (`yulius`): doctor's module check against fixture `/proc/modules` and `modules.builtin`.
- Host-level (opt-in, `YULIUS_ISOLATION_TEST=1`, `crates/yulius/tests/isolation.rs`, new test
  `isolation_docker`): the acceptance criteria above, in a delegated transient unit. **Not
  runnable in a sandbox or in Yulius CI:** both are themselves isolated runs whose seccomp filter
  refuses the namespaces and mounts the holder needs, so a person runs it on the host. The
  evidence says so.

## Decisions

- [ADR-0151](../../docs/decisions/0151-a-sandbox-runs-its-own-docker-engine-under-an-engine-seccomp-profile.md):
  the engine runs rootful-in-the-sandbox's-user-namespace (the LXC-nesting model), started on
  demand by the holder through `bwrap` with a run's file view. Its own seccomp profile bounds
  every container. The holder proxies a stable socket. Amends ADR-0003/ADR-22, supersedes ADR-0140.
  Rejected: the host's socket; rootless Docker or Podman nested in a second user namespace; an
  engine in the holder's mount namespace; a socket-filtering proxy; gVisor or microVMs.
- Reversible defaults:
  - `YULIUS_SANDBOX_DOCKER=on` when the prerequisites hold;
  - the engine's idle stop reuses `YULIUS_SANDBOX_IDLE_MINUTES`;
  - pruning only images no container uses and the build cache.

## Evidence

- **Reproduction** (this task's isolated sandbox, 2026-10-16): `docker version` fails with "failed
  to connect to the docker API at unix:///var/run/docker.sock". `/proc/self/status` shows
  `NoNewPrivs: 1`, `Seccomp: 2`, reduced capabilities, `uid_map` `0 999 1 / 1 231072 65535`. The
  base has Docker 29.9 (`dockerd`, `containerd`, `runc`, buildx and compose plugins).
  `/proc/modules` lists `overlay bridge veth nf_tables nft_compat nft_chain_nat nf_nat
  nf_conntrack xt_MASQUERADE xt_addrtype xt_conntrack`.
- **Tests written first, seen failing for the right reason**, then passing:
  - the engine seccomp profile (missing API);
  - the socket bind in joined runs (`a_joined_run_sees_the_sandboxs_docker_socket`: no bind);
  - the engine cgroup group (`SandboxGroups.engine` missing);
  - engine storage measured as "containers" (labelled "system" before);
  - disk upkeep pruning the engine (trait method missing);
  - the briefing variants and budgets (section cut at 440 bytes, then shortened to ≤ 257);
  - the doctor check.
- **Review round 1** (fresh session, request changes). Own code, each fixed test-first:
  - *High:* the holder followed a link at the engine-writable control socket. The regression test
    `a_link_at_the_control_socket_is_never_followed` failed with a plain `connect` (it reached the
    decoy). Fixed with `O_PATH|O_NOFOLLOW`, a socket check and `/proc/self/fd`.
  - *Medium:* the briefing came from the global setting. Now it comes from this sandbox's holder
    (`docker_is_told_per_sandbox_from_its_holder`).
  - *Low:*
    - queued clients restarted a failing engine (`clients_queued_behind_a_failed_start_share_its_answer`
      failed with 4 starts, now 1);
    - a stale socket file counted as served (now the readiness report says so,
      `the_readiness_line_says_whether_the_docker_socket_is_served`);
    - an executable owned by the sandbox's root (no engine, `an_executable_the_sandbox_owns_gets_no_engine`);
    - doctor text with a run of spaces (asserted);
    - the `--privileged` probe now uses `--cgroupns=host`;
    - per-container resource flags and the storage undercount are documented.
- **Review round 2** (fresh session, request changes). The round-1 fixes were all confirmed. Own
  code, fixed test-first:
  - *Medium:* pruning went through the sandbox's socket, so under disk pressure it started a
    stopped engine every 5 minutes and kept it and the holder from idling. Now it prunes only a
    running engine, through its control socket, with no link followed
    (`only_a_running_engine_is_pruned_through_its_control_socket` failed with `Some(1000)` for a
    stopped engine).
  - *Low:*
    - a group- or world-writable executable also gets no engine (the test failed for mode 775);
    - the docs say per-container CPU flags may fail, not that they are ignored;
    - the docs are rewrapped.
  - *Pre-existing:* `sysroots/<sandbox>` is never deleted for removed sandboxes: #582.
  - A flaky test of this change (`a_command_that_ignores_term_is_killed_after_the_grace` raced the
    shell's `trap`, gate run 1) was fixed to wait on a ready file, and passed 8 runs in a row.
- **Gate:** `scripts/dev ci` → exit 0 at `1ee6409e` (nextest 1942 passed, 18 skipped); rerun after
  the review fixes, see the pull request.
- **Security:** `scripts/dev security` → exit 2, "Vidoc rejected the key (HTTP 401: Invalid or
  expired token)". The scan did not run (#470); per ADR-0104 that is unrun, not passed.
- **Not run here:**
  - the host-level test `isolation_docker`, which needs `YULIUS_ISOLATION_TEST=1` and
    `YULIUS_ISOLATION_TEST_DOCKER=1` on the host in a delegated transient unit. This task's sandbox
    and Yulius CI are isolated runs whose seccomp filter refuses the namespaces and mounts the
    holder and engine need, so it skips there;
  - a real `docker run hello-world` in a fresh sandbox, which waits for a host running this
    version.

  A person runs both after the deploy.
