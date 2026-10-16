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
Fixes #610. Amends SPEC-199 DSY-003, DSY-004 and DSY-006, and ADR-0160.

## Goal

An agent that follows Yulius's Docker hint gets everything SPEC-187 promises (`docker`,
`docker compose`, buildx), not just an engine. An agent missing buildx or compose knows it
installs them in its own sandbox, without a person or a host change.

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

## Requirements

| ID | Requirement |
| --- | --- |
| DBX-001 | The `DOCKER_NO_DOCKERD` refusal and `yulius doctor`'s warning without a host `dockerd` tell an agent to run `sudo apt-get install -y docker.io docker-buildx docker-compose-v2` in the sandbox. |
| DBX-002 | Every host install hint (the refusal, doctor, the smoke test's reason) names the same install: `docker-ce` with `docker-buildx-plugin` and `docker-compose-plugin`. |
| DBX-003 | The briefing's `## Containers` section with an engine names the install for missing parts in this sandbox (`missing parts: sudo apt-get install -y docker.io docker-buildx docker-compose-v2 here`). The section stays within its 270 bytes, so the tight-budget renderings keep it whole: "own engine" goes, and the egress proxy is named by its `--add-host` flag alone. |
| DBX-004 | ADR-0160, `docs/operations.md`, `docs/architecture.md` and `docs/changes.md` name the full install. |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- `DOCKER_NO_DOCKERD`, doctor's warning and the briefing contain
  `docker.io docker-buildx docker-compose-v2` (fails before: only `docker.io`).
- The host hints in the refusal, doctor and the smoke reason contain `docker-buildx-plugin` and
  `docker-compose-plugin`.
- The briefing names that install for missing parts "here" (in this sandbox). The tight-budget
  tests still find the section whole.

## Test plan

- Unit tests in `yulius-sandbox` (`manager.rs`), `yulius` (`doctor.rs`, `docker_smoke.rs`) and
  `yulius-domain` (`briefing.rs`) assert the full package set and the in-sandbox wording.
- Live check in this sandbox's engine, `ubuntu:24.04`: install `docker.io` alone, then with both
  plugins, and check buildx and compose.
- The full gate (`scripts/dev ci`).
- Not testable here: running the hint in the product sandbox, which this task cannot reach, and
  a host install, which is a person's decision.

## Decisions

- **Name the full package set in every hint**, over:
  - (a) Yulius detecting a missing buildx per sandbox and telling the briefing. This needs a new
    per-sandbox probe in the briefing input. `apt-get install` of the full set is idempotent, so
    one command fixes a missing engine, a missing plugin or both on a sandbox whose engine is
    Ubuntu's. On a base copied from a `docker-ce` host it would replace `docker-ce`; the docs name
    `docker-buildx-plugin docker-compose-plugin` for that case, which is rare (`docker-ce-cli`
    recommends both).
  - (b) Yulius installing the plugins itself, rejected in ADR-0160 (files owned by the service
    user, a pinned download to maintain).
  - (c) Telling agents to drop the plugin into `~/.docker/cli-plugins`. That is a hand-made binary
    with no updates, and the system install already reaches every joined run.
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
- **Gate:** `scripts/dev ci` exit 0 at `4505a9dc` and, after review round 1, at `dfac3e1e` (2037
  passed, 18 skipped, each time). An intermediate run at `600f97d2` failed `cargo fmt --check` on
  the new smoke test, which was then formatted. The commit after `dfac3e1e` changes only this
  spec.
- **Security:** `scripts/dev security` exit 2: Vidoc refused the key (HTTP 401). The scan was
  **not run** (not passed). It sent nothing for the four changed code files.
- **Review round 1** (fresh session, at `4505a9dc`): APPROVE, with four low findings in own code
  and three nits.
  - L1: the briefing's `apt install` without `-y` aborts at the prompt with no terminal. It is now
    `apt-get install -y`, test first (449dd4eb).
  - L2: on a base copied from a `docker-ce` host, `docker.io` conflicts with `docker-ce`. ADR-0160,
    the operations doc and the Decisions section now name `docker-buildx-plugin
    docker-compose-plugin` for that case.
  - L3: SPEC-199 now says it is amended by SPEC-201.
  - L4: this section was added, and the Plan is ticked.
  - Nit 5: the smoke's host advice now matches the refusal and doctor (`docker-ce` with its
    plugins), test first.
  - Nit 7: the long operations line is rewrapped.
  - Nit 6 is left: the briefing says "your ports", not "ports in your range". The 270-byte budget
    has no room once `-y` is added. The `## Dev servers` section names the range.
- **Not testable here:**
  - running the hint in the product sandbox, which this task cannot reach (only this sandbox is
    visible under `/var/lib/yulius/sandboxes`);
  - a host install of Docker, which is a person's decision (the host has had no Docker since
    2026-10-12).
