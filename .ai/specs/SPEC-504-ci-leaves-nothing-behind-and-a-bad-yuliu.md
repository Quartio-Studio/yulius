---
yulius-task: 6287c480-43ac-42a1-8b09-ab2af5830a44
status: in-progress
---

# SPEC-504: CI leaves nothing behind, and a bad yulius.toml never blocks CI

Status: in-progress
Date: 2026-11-17
Owner: Yulius task #486
Scope authorization: task #486, the person's goal of 2026-11-17 after two production incidents of
16 Nov 2026 ("fix both in principle, not by cleaning up by hand"). Builds on SPEC-187/SPEC-196
(the sandbox's engine and its Docker front, ADR-0151, ADR-0158), SPEC-494 (the front's gate,
ADR-0344), SPEC-497 HBA-003 (CI waits for Docker networks), SPEC-053/SPEC-268 (the sandbox's
`ci/` cgroup), SPEC-050 CI-110 and SPEC-217 VDP-001 (which jobs judge a pull request).

## Goal

1. A CI job leaves nothing running: the containers and networks it created are removed when it
   ends, whatever the outcome (passed, failed, cancelled, timed out, ended by a restart), and an
   agent's containers are never touched.
2. A bad `yulius.toml` never blocks a project's CI: a pull request is judged by the integration
   branch's gate, its own `yulius.toml` is checked as CI will read it after the merge, and an
   integration branch whose file became invalid anyway still lets its repair merge.

## Context

### What happened (production, 16 Nov 2026)

- **quartio-v2:** its sandbox's engine held 22 `quartio-browser` test containers (38 min to 16 h
  old) and 13 `quartio-test-*` networks from test runs killed or cancelled mid-test: superseded
  runs, server restarts, time limits. They took 18 of the engine's 31 networks, so six CI jobs
  waited for networks (SPEC-497 HBA-003) while no quartio-v2 job ran. The engine never went idle,
  so it kept Docker's 31-network pools instead of the 4,096 #1356 gives an engine at its next
  start. The operator restarted the front at 22:18.
- **autoresearch:** PR #4 merged a `yulius.toml` with both `[commands] gate` and `[[ci.jobs]]`.
  Yulius CI reads the gate from `dev` (`ci_configured_gate`) and refuses that pair, so it ran
  nothing for any pull request, PR #5 (the fix) included: `ci_gate_for_head` gives a proposed
  head no gate while `dev`'s configuration is broken. PR #4's own run had passed: its own file
  could not be read (`ci_jobs_at` → `Err`), so it silently ran `dev`'s previous jobs.

### What the code does today (verified)

- A CI job's processes join the sandbox's `ci/` cgroup leaf (`ci_join`, SPEC-053), shared by
  every job of the sandbox. Each run of the job has its own PID namespace, so the job's own
  processes end with it; its containers live in the engine (`engine/` group) and outlive it.
- The Docker front (`sandbox-host docker`) splices bytes between each client and the engine
  (`Gate::splice`); it never reads them. Nothing records who created a container.
- The upkeep prunes networks no container used for 10 minutes (SPEC-497 HBA-003): the leaked
  networks were in use by leaked containers, so nothing was freed.
- `ci_gate_for_head` runs a pull request's own `[[ci.jobs]]` when it changed them since its
  merge base (CI-110); an unreadable own file falls back to `dev`'s jobs without a word.

### In scope

- Per-job cgroup, labelling by the front, removal at the job's end and by a sweep, measures.
- Which jobs judge a commit on its way to the integration branch, the configuration check, the
  last valid gate, measures.
- Out of scope: volumes and images (never removed), containers an agent or a person creates,
  containers that a job's container creates through a socket it mounts (the engine's own view,
  not a CI caller; see Decisions).

## Requirements

### A CI job leaves nothing behind (pull request 1)

| ID | Requirement |
| --- | --- |
| CLB-001 | Each CI job runs in its own cgroup `ci/ci-job-<job id>` under the sandbox's CI group (the sandbox's group where CPU is not split), made before the job starts and removed after it ends; where it cannot be made the job runs in the CI group as before (unlabelled). |
| CLB-002 | The Docker front tells a CI job's calls from everyone else's by the caller's cgroup (the connecting process, `SO_PEERCRED`, read from the host's `/proc` opened before the front joins the sandbox): a caller in `…/ci-job-<id>` is that job; any other caller is not a CI job. |
| CLB-003 | For a CI job's caller, the front adds the label `yulius.ci.job=<id>` to the body of every `POST [/v<version>]/containers/create` and `POST [/v<version>]/networks/create`, replacing a label of that name the caller set and keeping its other labels; every other byte passes unchanged. Requests are framed by HTTP/1.1 (`Content-Length`, `chunked`); an upgraded or hijacked call (attach, exec start, session, gRPC, `Upgrade`) and anything not parsed as a request pass raw from there on. A body that is not JSON or over 4 MiB passes unlabelled. Agents' calls are never parsed. |
| CLB-004 | When a CI job ends, whatever the outcome, Yulius removes the containers (forced, keeping their volumes) and then the networks of the sandbox's running engine labelled with that job. Images and volumes are never removed, and nothing starts a stopped engine. |
| CLB-005 | Each upkeep pass, and so first after a server starts, removes from each running engine the containers and networks labelled with a CI job whose cgroup no longer has a process (gone or empty). A container whose label came from its image (the image carries the same value) is not the job's and is kept. |
| CLB-006 | Every removal is recorded per sandbox (audit `sandbox.docker.ci_leftovers`: containers, networks, at the job's end or by the sweep), and `GET /api/analytics/docker` adds them per week and in total. |

### A bad yulius.toml never blocks CI (pull request 2)

| ID | Requirement |
| --- | --- |
| CFG-001 | While the integration branch has CI, every commit on its way there (a pull request's head, a merge candidate, a playground task's merge) is judged by the integration branch's jobs, never its own: a change never weakens the gate that judges it. This replaces CI-110 and SPEC-163 PCI-002's own-jobs rule. A branch with no CI and no broken file still lets a commit bring its own (bootstrap). |
| CFG-002 | A run of such a commit, and of the integration branch, whose `yulius.toml` as CI will read it after the merge is invalid has a failed job `yulius.toml` whose reason and output are the parser's message; the run fails, so it cannot merge. The file read is the head's for a merge candidate, a playground merge and a branch, and for a pull request the head's when it changed the file since its merge base, else the integration branch's. |
| CFG-003 | While the integration branch's `yulius.toml` is invalid, its gate is that of the newest first-parent commit (at most 100 back) whose file is valid: the last valid gate runs until a fix merges, and the CI page says so with the problem. Without one, CI runs nothing, as before. |
| CFG-004 | The minutes each sandbox's CI had no gate because of its configuration are counted per day and shown by `GET /api/analytics/ci-minutes`. |

## Plan

- [ ] Spec and plan written
- [ ] A CI job runs in its own cgroup (CLB-001)
- [ ] The Docker front labels a CI job's containers and networks (CLB-002, CLB-003)
- [ ] A job's leftovers go at its end (CLB-004)
- [ ] Ended jobs' leftovers are swept each upkeep and after a start (CLB-005)
- [ ] Leftovers are counted in the Docker analytics (CLB-006)
- [ ] Proof against a real engine: docker run, compose and a killed job through the front
- [ ] ADR, architecture, operations and changes for pull request 1
- [ ] Pull request 1 reviewed and approved
- [ ] A pull request is judged by the integration branch's jobs (CFG-001)
- [ ] An invalid yulius.toml fails its run with the parser's message (CFG-002)
- [ ] The last valid gate runs while dev's file is invalid (CFG-003)
- [ ] Config-blocked minutes are counted (CFG-004)
- [ ] ADR, architecture and changes for pull request 2
- [ ] Pull request 2 reviewed and approved

## Acceptance criteria

- A CI job that ran `docker run -d`, `docker network create` and `docker compose up -d` and then
  passed, failed, was cancelled or timed out leaves none of those containers or networks on its
  sandbox's engine; volumes and images stay.
- A server that starts finds labelled leftovers of jobs that ended while it was down and removes
  them at its first upkeep pass.
- An agent's containers and networks, created at the same time through the same front, are
  untouched, also when they run an image committed from a CI container.
- A pull request that changes `[[ci.jobs]]` runs the integration branch's jobs.
- A pull request whose `yulius.toml` has both `[commands] gate` and `[[ci.jobs]]` fails its run
  with the parser's message; the repair of a `dev` broken that way passes with the last valid
  gate and merges.

## Test plan

- Unit (`yulius-sandbox`): the request labeller over keep-alive, `Content-Length`, `chunked`,
  hijacked, upgraded and unparsable streams; creates with no, null and existing labels; bodies
  over the bound; the cgroup → job rule.
- Unit (`yulius-sandbox`): a front whose caller is a CI job labels its creates, another caller's
  bytes pass unchanged (fake engine capturing bytes).
- Unit (`yulius-sandbox`): leftovers removal against a fake engine socket: by job, by the sweep
  rule (running group kept, gone or empty group removed, image-inherited label kept), forced
  container removal without `v`, networks after containers, nothing when the engine is stopped.
- Integration (`yulius` tests): a CI job's cgroup is made before and removed after, and the
  isolation is told the job ended for every outcome (passed, failed, cancelled, timed out) and
  for jobs a restart ended.
- Real engine (in this sandbox, `dockerd` 29): a labelling relay in front of the real socket,
  the real `docker` CLI and `docker compose` through it; the removal and the sweep against the
  real engine; an agent-style unlabelled container kept.
- Configuration: `ci_gate_for_head` with changed own jobs; the `yulius.toml` job for invalid own
  and integration files; the last valid gate; counted minutes.

## Decisions

- **Caller by cgroup, not environment or socket per job.** The front already relays every call;
  the cgroup is set by Yulius before the job runs, cannot be left by the job (the cgroup tree is
  not writable from a run), and survives a server restart. An environment variable can be
  unset, and a socket per job needs the front and server to agree on job lifetimes. See
  ADR-0359.
- **Label, then remove by label.** Labels live in the engine with the resource, so the sweep
  needs no state of its own and works after a restart. A job's end is "its cgroup has no
  process": the same fact whether the server ran the job or a previous server did.
- **Only creates are parsed.** The front rewrites two request bodies of CI callers and passes
  everything else (and every agent byte) unchanged; a hijack or anything unparsed switches the
  connection to raw bytes, so interactive streams are never delayed.
- **Containers created by a job's container** through a mounted socket are not labelled (their
  caller is in the engine's cgroup, not a CI job's). The common case, a reaper like Ryuk, only
  removes. quartio-v2's harness should keep its own cleanup for a clean state between tests in
  one job; Yulius's cleanup is the backstop when the job is killed.

## Evidence

(Filled per slice.)
