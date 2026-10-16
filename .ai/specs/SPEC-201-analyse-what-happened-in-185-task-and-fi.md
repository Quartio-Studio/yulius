---
yulius-task: 64fb05de-4847-4a1e-a1af-da782f8943ba
status: done
---

# SPEC-201: The in-sandbox Docker install brings buildx and compose

Status: done
Date: 2026-10-16
Owner: task #186
Scope authorization: task #186 (requested by MrFishPL): "Analyse what happened in #185 task and
fix this [a product sandbox with an engine but no buildx] and pls try to fix docker finaly".
Fixes #610 and #614 (part 2: the person's second screenshot, "Now it's worse than before: the
Docker engine has stopped"). Amends SPEC-199 DSY-003, DSY-004 and DSY-006, and ADR-0160.

## Goal

An agent that follows Yulius's Docker hint gets everything SPEC-187 promises (`docker`,
`docker compose`, buildx), not just an engine. An agent missing buildx or compose knows it
installs them in its own sandbox, without a person or a host change. Part 2: a sandbox's engine
starts again after `yulius.service` restarts, whatever state its cgroup leaf was left in.

## Context

- **What happened in #185.** Docker had been purged from the host on 2026-10-12, so bases built
  after that date have no `dockerd`. PR #608 (ADR-0160) made a sandbox's engine come from its own
  system and told agents without `dockerd` to run `sudo apt-get install -y docker.io`. #185 could
  not test that install path ("untried" in its summary).
- **Symptom** (the person's screenshot, product sandbox): the engine answers (server 29.1.3,
  Ubuntu's `docker.io`), but `scripts/build-image.sh` fails because `docker buildx` is not a
  command. The agent concluded that the plugin had to come from the host or the base, and handed
  the person a host change.
- **Root cause.** On Ubuntu 24.04, `docker.io` 29.1.3 only *suggests* `docker-buildx` and
  `docker-compose-v2`, so `apt-get install` does not install them. Reproduced in this task's
  sandbox, whose engine works, inside `ubuntu:24.04`:
  - after `apt-get install -y docker.io`, `/usr/libexec/docker/cli-plugins` holds only
    `docker-trust`, and `docker buildx version` fails with "unknown command: docker buildx";
  - after `apt-get install -y docker-buildx docker-compose-v2`, buildx 0.30.1 and compose 2.40.3
    work.

  Every hint names only `docker.io`: the `DOCKER_NO_DOCKERD` refusal, the briefing, `yulius doctor`,
  the smoke test's reason, ADR-0160 and the docs. The host hint's "docker-ce or docker.io" has the
  same gap, because a base copies the host's CLI plugins.
- **Why the tests missed it.** They assert that the hint contains `docker.io`, and the hint's
  content was never run.
- **What else the cause breaks.** An operator who installs `docker.io` on the host gives every
  new base an engine without buildx or compose. The smoke test's `docker build` and
  `docker compose up` then fail.
- In a sandbox, an `apt` install writes to the sandbox's upper layer. It is kept across sessions,
  CI jobs and dev deploys (all joined runs, ADR-0158), so `~/.docker/cli-plugins` is never needed.

- **Part 2 symptom** (2026-10-16, after the instance updated at 18:10:49Z, the production
  deploy of #611). In the product sandbox (`6b76ccc7…`) and in this task's sandbox
  (`d62be476…`), `docker info` answered: "the engine exited (exit status: 1); engine log: …
  yulius: cannot join /sys/fs/cgroup/system.slice/yulius.service/sandboxes/<sandbox>/engine: No
  such file or directory". Both engines had stopped as idle before the update. The container
  "cgroup vanished" lines before it in the log are normal noise: containers that ended.
- **Part 2 root cause.**
  - `yulius.service` runs with `KillMode=process`, so holders, fronts and sessions survive a
    restart (operations §3).
  - When the unit stops, systemd prunes every *empty* group left in the unit's tree. The holder's
    leaf has processes and stays. An idle engine's `engine` leaf is empty, so it goes.
  - The front prepares the sandbox's groups once, when it starts (`Manager::start_front` →
    `cgroup_of`). After the restart the server adopts the holder and keeps its running front
    (`FrontAction::Keep`).
  - `Supervisor::spawn` then starts `yulius sandbox-exec --engine --cgroup <leaf>` with no
    check, so every engine start fails until a new run prepares the sandbox. A Claude session
    already running is not a new run.
  - Nothing in Yulius removes the leaf: `freeze::thaw_all` removes only `run-*` groups.
- **Why the tests missed it.** The supervisor tests run with an existing cgroup directory. The
  manager tests never take a group away from a live holder.
- **What else the cause breaks.** The `agents` and `ci` leaves are pruned too when they are
  empty. A run's preparation recreates them (`cgroup_of`), so only the engine, started by the
  front without a preparation, is stuck.

## Requirements

| ID | Requirement |
| --- | --- |
| DBX-001 | The `DOCKER_NO_DOCKERD` refusal and `yulius doctor`'s warning without a host `dockerd` tell an agent to run `sudo apt-get install -y docker.io docker-buildx docker-compose-v2` in the sandbox. |
| DBX-002 | Every host install hint (the refusal, doctor, the smoke test's reason) names the same install: `docker-ce` with `docker-buildx-plugin` and `docker-compose-plugin`. |
| DBX-003 | The briefing's `## Containers` section with an engine names the install for missing parts in this sandbox (`missing parts: sudo apt-get install -y docker.io docker-buildx docker-compose-v2 here`). The section stays within its 270 bytes, so the tight-budget renderings keep it whole: "own engine" goes, and the egress proxy is named by its `--add-host` flag alone. |
| DBX-004 | ADR-0160, `docs/operations.md`, `docs/architecture.md` and `docs/changes.md` name the full install. |
| DBX-005 | Before each engine start, the front recreates the engine's cgroup leaf when it is gone: that leaf only, under its existing parent (`cgroup::restore_leaf`). A missing parent is an error naming the group, not a recreated sandbox group without limits. |
| DBX-006 | At adoption and every upkeep tick, a kept engine front gets its engine leaf recreated when it is gone, and so does a relay front, whose SPEC-187 holder runs its own engine in the same leaf. This heals fronts and holders that run an older binary, which a server update never replaces. `restore_leaf` refuses a path with `..`, as `join` does. |
| DBX-007 | `docs/operations.md` names the prune on restart and the recovery. |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Reproduce the pruned engine cgroup after a restart and find its cause
- [x] Regression tests: the front and the tick restore a missing engine leaf
- [x] The front and the tick recreate the engine leaf; docs and gate green
- [x] Independent review of the engine-leaf fix, findings fixed test-first

## Acceptance criteria

- `DOCKER_NO_DOCKERD`, doctor's warning and the briefing contain
  `docker.io docker-buildx docker-compose-v2` (fails before: only `docker.io`).
- The host hints in the refusal, doctor and the smoke reason contain `docker-buildx-plugin` and
  `docker-compose-plugin`.
- The briefing names that install for missing parts "here" (in this sandbox). The tight-budget
  tests still find the section whole.
- Part 2: a supervisor whose engine leaf is missing starts the engine, and the leaf exists
  afterwards (fails before: the start fails). With the parent missing, the start fails and names
  the group. An adopted holder whose engine leaf was removed has it back after `keep_front`
  (fails before: still missing).

## Test plan

- Unit tests in `yulius-sandbox` (`manager.rs`), `yulius` (`doctor.rs`, `docker_smoke.rs`) and
  `yulius-domain` (`briefing.rs`) assert the full package set and the in-sandbox wording.
- Live check in this sandbox's engine, `ubuntu:24.04`: install `docker.io` alone, then with both
  plugins, and check buildx and compose.
- Part 2 tests in `yulius-sandbox`:
  - `engine_supervisor`: a start whose leaf is missing, with a command that fails without the
    group as `sandbox-exec` does, and a start whose parent is missing;
  - `manager`: `keep_front` after the leaf of an adopted holder's engine front is removed;
  - `cgroup`: `restore_leaf`.
- The full gate (`scripts/dev ci`).
- Not testable here: running the hint in the product sandbox, which this task cannot reach; a
  host install, which is a person's decision; a real systemd restart, since this sandbox cannot
  see the host's cgroup tree. The tests remove the leaf the way systemd does.

## Decisions

- **Name the full package set in every hint**, over:
  - (a) Yulius detecting a missing buildx per sandbox and telling the briefing. This needs a new
    per-sandbox probe in the briefing input. `apt-get install` of the full set is idempotent, so
    one command fixes a missing engine, a missing plugin or both on a sandbox whose engine is
    Ubuntu's. On a base copied from a `docker-ce` host apt refuses it and removes nothing
    (`containerd.io` conflicts with `containerd`, checked with `apt-get install -s` in this
    sandbox). The docs name `docker-buildx-plugin docker-compose-plugin` for that case, which is
    rare (`docker-ce-cli` recommends both).
  - (b) Yulius installing the plugins itself, rejected in ADR-0160 (files owned by the service
    user, a pinned download to maintain).
  - (c) Telling agents to drop the plugin into `~/.docker/cli-plugins`. That is a hand-made binary
    with no updates, and the system install already reaches every joined run.
- **Part 2: make the leaf again where the engine starts, and at the tick**, over:
  - (a) only at the tick: up to 30 s of failed starts after every restart;
  - (b) only in the front: fronts from an older binary are kept across an update and would stay
    broken until their holder restarts;
  - (c) `sandbox-exec` creating its `--cgroup` directory: a generic join that makes any missing
    group could make up a sandbox group without limits;
  - (d) keeping the leaf non-empty, or keeping systemd from pruning: the unit has no setting
    that stops it, and an idle process in the leaf would count as a container
    (`containers_running`).

  Only the leaf is made, never its parent, which holds the sandbox's limits. A leaf made again
  under a split parent gets the default `cpu.weight`, 100, which is the engine's weight. No ADR:
  this restores the layout ADR-0151 decided.
- ADR-0160 is amended in place, with a dated note. The decision (the sandbox's own system
  provides Docker) stands; only its install command was incomplete.

## Evidence

- **Reproduced** in this task's sandbox, whose engine works (Docker 29.9 from its `docker-ce`
  base): `docker buildx build` of a two-line Dockerfile succeeded, so the engine and plugin path
  are sound. In `ubuntu:24.04` on that engine, `apt-get install -y docker.io` (29.1.3) left only
  `docker-trust` in `/usr/libexec/docker/cli-plugins`, and `docker buildx version` answered
  "unknown command: docker buildx". After `apt-get install -y docker-buildx docker-compose-v2`,
  buildx 0.30.1 and compose 2.40.3 worked. `apt-cache show docker.io`: both plugins are only in
  `Suggests`. Filed as #610.
- **Tests that failed before the fix** (commit `6fdcf3ba`):
  - `manager::tests::the_front_mode_follows_the_holder_and_the_prerequisites`;
  - `doctor::tests::the_docker_check_names_what_sandboxes_lack`;
  - `docker_smoke::tests::the_host_install_it_asks_for_brings_buildx_and_compose`;
  - `briefing::tests::a_sandbox_with_an_engine_is_told_docker_works_and_how`.

  All four failed on the missing package names. The first longer briefing text (356, then
  311 bytes) also failed the section's 270-byte bound and two tight-budget tests, so the section
  was reworded to fit (266 bytes).
- **Gate:** `scripts/dev ci` exit 0 (2037 passed, 18 skipped, each time) at `4505a9dc`, at
  `dfac3e1e` after review round 1, and at `7f41c191` after review round 2. An intermediate run
  at `600f97d2` failed `cargo fmt --check` on the new smoke test, which was then formatted. The
  commit after `7f41c191` changes only this spec.
- **Security:** `scripts/dev security` exit 2: Vidoc refused the key (HTTP 401). The scan was
  **not run** (not passed). It sent nothing for the four changed code files.
- **Review round 1** (fresh session, at `4505a9dc`): APPROVE, with four low findings in own code
  and three nits.
  - L1: the briefing's `apt install` without `-y` aborts at the prompt with no terminal. It is now
    `apt-get install -y`, test first (449dd4eb).
  - L2: on a base copied from a `docker-ce` host, the full install does not apply. ADR-0160, the
    operations doc and the Decisions section now name `docker-buildx-plugin
    docker-compose-plugin` for that case.
  - L3: SPEC-199 now says it is amended by SPEC-201.
  - L4: this section was added, and the Plan is ticked.
  - Nit 5: the smoke's host advice now matches the refusal and doctor (`docker-ce` with its
    plugins), test first.
  - Nit 7: the long operations line is rewrapped.
  - Nit 6 was first left ("your ports"), for lack of room in the 270-byte budget, and was fixed in
    round 2.
- **Review round 2** (fresh session, at `05a9eb9b`): APPROVE, with one minor finding and one nit
  in own code, plus one pre-existing nit.
  - Minor: the docs said that on a `docker-ce` base `docker.io` "would replace" `docker-ce`. In
    fact apt refuses it and removes nothing (`apt-get install -s -y docker.io docker-buildx
    docker-compose-v2` in this sandbox: "containerd.io : Conflicts: containerd"). The three
    passages are corrected.
  - Nit: "ports in your range" is back, test first. "Egress proxy:" became "Proxy:" to stay within
    the budget (268 bytes).
  - Pre-existing nit: the briefing's `docker` flag is per server, not per sandbox (SPEC-199). The
    "missing parts … here" wording now covers a sandbox without `dockerd`, so no change was
    needed.
- **Not testable here:**
  - running the hint in the product sandbox, which this task cannot reach (only this sandbox is
    visible under `/var/lib/yulius/sandboxes`);
  - a host install of Docker, which is a person's decision (the host has had no Docker since
    2026-10-12).
- **Part 2, reproduced** in this task's sandbox: after the instance update at 18:10:49Z (CD run
  `185f8886`), `docker info` answered "the engine exited (exit status: 1); engine log: … yulius:
  cannot join /sys/fs/cgroup/system.slice/yulius.service/sandboxes/d62be476-…/engine: No such
  file or directory". The engine's last activity was at 18:54 local, so it had stopped as idle
  before the update. The person's screenshot shows the same in the product sandbox. Filed as
  #614.
- **Part 2 tests that failed before the fix** (commit `9168d8cb`):
  - `engine_supervisor::tests::a_start_recreates_the_engine_leaf_a_restart_pruned`: "the engine
    exited (exit status: 1); engine log: cannot join …/sandbox/engine";
  - `manager::tests::a_kept_engine_front_gets_its_pruned_leaf_back`: "the engine's leaf is back".

  `cgroup::tests::a_pruned_leaf_is_restored_under_its_parent` was written with `restore_leaf`.
- **Part 2 gate:** `scripts/dev ci` exit 0 at `771f9dc4`, and at `8411e71f` after review round 1
  (2040 passed, 18 skipped, each time). An earlier
  run at `21fe1657` passed every test but failed `cargo doc`: the new doc comment linked the
  private `LEAVES`. The link became plain code.
- **Part 2 review round 1** (fresh session, at `d143d10f`): REQUEST CHANGES, all findings in own
  code and fixed test first:
  - Major: holders of SPEC-187, which run their own engine behind a relay front, use the same
    leaf, and the tick only restored it for engine fronts. The test now covers both modes; the
    relay case failed before (`"relay: the engine's leaf is back"`). The tick now restores it for
    relay fronts too.
  - Low: the missing-parent assertion could not tell the refusal from the fake join's own error.
    It now requires the start's "the engine's cgroup " prefix.
  - Nit: `restore_leaf` refuses a path with `..`, as `join` does (test first).
  - Nit: the review box had been ticked early. It is ticked in the head sent to round 2, the
    commit after `8411e71f`, which changes only this spec. Round 2's verdict on that head is in
    the pull request.
  - Nit left: a failure that persists warns at every tick. A live holder keeps the parent, so this
    is unlikely.

  The reviewer confirmed the systemd prune (`unit_prune_cgroup` → `cg_trim`, delegated units
  included). It also found that `adopt()` at startup calls `keep_front`, so the leaf comes back
  right after a restart. The front can write the delegated tree (`--dev-bind / /`, the service
  uid). Both tests at `9168d8cb` fail without the fix.
