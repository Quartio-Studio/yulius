---
yulius-task: 18fcb52b-ce6f-4546-bab5-a771d949c52e
status: done
---

# SPEC-173: an isolated sandbox's agent is told there is no container engine, and what works instead

Status: done
Date: 2026-10-15
Owner: task #163
Scope authorization: the person's request (task #163): "i got this from a different project
[screenshot] fix". The screenshot is an agent of another project reporting: "There's no Docker
daemon in my sandbox. The `docker` command exists, but `/var/run/docker.sock` doesn't. The Deployer
needs Docker to build images and copy them to VMs, and a rollback needs the previous images kept on
this machine." Fixes #550; the engine itself is #551.

## Goal

An agent in an isolated sandbox knows before it designs anything that it has no container engine
there, in Yulius CI or in CD. It also knows what works instead, so it never builds a deploy, a test
setup or a rollback around a Docker daemon it cannot have.

## Context

- **Symptom.** An agent of another project designed a deployer that builds images in its sandbox,
  copies them to VMs and keeps the previous images there for rollback. Only then did it find that
  `docker` has no daemon.
- **Reproduced (in this task's own isolated sandbox).**
  - `docker version` fails: `failed to connect to the docker API at unix:///var/run/docker.sock`.
  - `dockerd` with its own data, exec root and socket under `/tmp` stops at `failed to create NAT
    chain DOCKER: iptables … Permission denied` (no `CAP_NET_ADMIN`).
  - `unshare -U`, `-n` and `-m` are refused with `Operation not permitted`.
- **Root cause.** ADR-22 (SPEC-002 §2.8, [ADR-0003](../../docs/decisions/0003-sandbox-isolation.md))
  runs every agent run, CI job and dev CD run with Docker's default capability set and a seccomp
  filter that refuses mount APIs and nested user namespaces, and that is by design. A container
  engine needs exactly those. CD production runs on the host's read-only system with an empty
  `/var` (ADR-0109). The `docker`, `dockerd` and `containerd` binaries are there only because the
  system-layer base copies the host's `/usr`, so the environment suggests a capability it does not
  have. Nothing an agent reads says otherwise: the briefing's environment facts stop at the
  dev-server ports.
- **Why tests missed it.** No test covers what the briefing says about the sandbox's environment.
  The isolation suite proves what is refused, not that agents are told.
- **Why not provide an engine in this fix.** An engine per sandbox would run in the namespace
  holder with its full user-namespace capabilities, and its socket would let any run start
  `--privileged` containers. That bypasses the per-run capability set and seccomp filter that §2.8's
  security properties rest on, so it needs its own spec, ADR and the host-level isolation suite
  (`YULIUS_ISOLATION_TEST=1`), which cannot run inside a sandbox. Filed as #551. The deployer in the
  screenshot would not be served by it either: production must not depend on images an agent can
  reach, and Docker's own remote engine (`DOCKER_HOST=ssh://…`) already builds on the target.
- **What else the cause could break.** Any work that assumes a local engine: `docker compose` test
  databases, testcontainers, image builds in CI, a Dockerfile-based dev CD script. The same
  briefing section covers them all.

## Requirements

| ID | Requirement |
| --- | --- |
| CTR-001 | When the sandbox's runs are isolated (the namespace holder of ADR-22 is wired), the briefing has a `## Containers` section saying there is no container engine in the sandbox, in Yulius CI or in CD runs, even where a `docker` client is installed. It names what works: an engine on another machine through the client (`DOCKER_HOST=ssh://…` or a `docker context`), images built where they run, rollback images kept there or in a registry, and `apt` for the services that tests need (started by the tests themselves). A briefing without isolation (legacy or host mode, where an engine may exist) has no such section. |
| CTR-002 | The section is the briefing's last mandatory part. Under the budget the hook leaves after the workflow instructions, it never pushes out the task header, the spec path or the architecture pointer that the briefing without it keeps. With extreme inputs every mandatory part stays and the section takes the optional parts' room. |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- `build_briefing` for an isolated input, within the default budget, contains `## Containers`,
  "no container engine", Yulius CI, `DOCKER_HOST=ssh://`, rollback and `apt`.
- The same input without isolation contains neither the section nor the claim.
- `scripts/dev ci` passes.

## Test plan

- Unit: `briefing::tests::an_isolated_sandboxs_briefing_says_there_is_no_container_engine` (fails
  before the fix: the section is missing).
- Unit (CTR-002): `the_containers_section_never_displaces_the_task_or_the_architecture` (goals of
  80–600 bytes at the Fix workflow's budget) and `huge_inputs_respect_the_budget_and_keep_mandatory_parts`
  (now also isolated). Both failed with the first, 430-byte section placed before the task.
- Integration: `agent_hooks::an_isolated_sandboxs_briefing_names_the_missing_container_engine`.
  With an isolation adapter installed the hook's briefing has the section, and without one it does
  not. It fails when `isolated` is hard-coded to `false`.
- Existing briefing tests prove the mandatory headings and the default budget still hold.
- Not testable here: the isolation suite on a real host. This change does not touch containment.

## Decisions

- [ADR-0140](../../docs/decisions/0140-isolated-sandboxes-have-no-container-engine-agents-are-told.md):
  isolated sandboxes run no container engine; the briefing says so and points to a remote engine.
  An engine per sandbox is #551.

## Evidence

- **Reproduction** (this task's isolated sandbox, 2026-10-15):
  - `docker version`: `failed to connect to the docker API at unix:///var/run/docker.sock`.
  - `dockerd --data-root /tmp/dk/data --exec-root /tmp/dk/exec -H unix:///tmp/dk/docker.sock`:
    `failed to create NAT chain DOCKER: iptables … Permission denied`, exit 1.
  - `unshare -U|-n|-m true`: `Operation not permitted`.
- **Regression test before the fix** (commit `8453ec08`): `cargo test -p yulius-domain --lib
  briefing::tests::an_isolated` → FAILED (`## Containers not in …`).
- **After the fix:** `cargo test -p yulius-domain --lib briefing::` → 16 passed.
- **Gate:** `scripts/dev ci` at `450b51ee` → exit 0; nextest 1848 tests run, 1848 passed, 18 skipped.
  After merging `origin/dev` (web-only #548), `python3 scripts/check_repository.py` passed.
- **Security:** `scripts/dev security` → exit 2, "Vidoc rejected the key (HTTP 401: Invalid or
  expired token)". The scan did not run (#470); per ADR-0104 that is unrun, not passed.
- **Review round 1** (fresh session, request changes): the 430-byte section placed before the task
  pushed the task header and `## Architecture` out at the Fix workflow's budget. Tests added first
  (commit after `e5c3802b`, both failing), then:
  - the section was cut to 257 bytes and moved after the architecture pointer;
  - the module doc, SPEC-002 §7.6 and the "client installed" wording were fixed;
  - the hook wiring test was added and shown to fail with the wiring removed.
  The overflow that exists without this change (a 600-byte goal under a Fix workflow already loses
  `## Architecture`) is #555.
- **Gate after the review fixes and a merge of `origin/dev` (#547):** `scripts/dev ci` at `da11cf61` →
  exit 0; nextest 1850 tests run, 1850 passed, 18 skipped.
- **Review rounds 2 and 3** (fresh sessions): round 2 approved `61e5fb3c` with one minor, a spec-path
  check that never matched (the briefing puts the path in backticks). It was fixed in `50482eb4`,
  with a guard that every check matches the plain briefing. Round 3 approved `81f0d175`, after a
  clean merge of `origin/dev` (#552).
- **Not run here:** the host-level isolation suite (`YULIUS_ISOLATION_TEST=1`) cannot run inside a
  sandbox. This change does not touch containment.
