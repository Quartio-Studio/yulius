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
- Second case (added 17 Nov 2026, 08:50 UTC, by the person): autoresearch PR #42 merged a valid
  multi-line TOML array (`paths = [` with one entry per line) into dev's `yulius.toml`; Yulius's
  own reader rejected it ("yulius.toml line 117: unsupported value"), CI ran nothing for any
  pull request ("No CI is configured") and `delivery_merge` refused. With a GitHub ruleset that
  requires `yulius/ci` on dev, the one-line fix (PR #50) could merge only through a person's
  bypass.
- Out of scope: volumes and images (never removed), containers an agent or a person creates,
  containers that a job's container creates through a socket it mounts (the engine's own view,
  not a CI caller; see Decisions).

## Requirements

### A CI job leaves nothing behind (pull request 1)

| ID | Requirement |
| --- | --- |
| CLB-001 | Each CI job runs in its own cgroup `ci/ci-job-<job id>` under the sandbox's CI group (the sandbox's group where CPU is not split), made before the job starts and removed after it ends; where it cannot be made the job runs in the CI group as before (unlabelled). |
| CLB-002 | The Docker front tells a CI job's calls from everyone else's by the caller's cgroup (the connecting process, `SO_PEERCRED`, read from the host's `/proc` opened before the front joins the sandbox): a caller in `…/ci-job-<id>` is that job; any other caller is not a CI job. |
| CLB-003 | For a CI job's caller, the front adds the labels `yulius.ci.job=<id>` and `yulius.ci.create=<a value of that create's own>` to the body of every `POST [/v<version>]/containers/create` and `POST [/v<version>]/networks/create`, under every `Labels` key in any case, replacing labels of those names the caller set and keeping its other labels; every other byte passes unchanged. Requests are framed by HTTP/1.1 (`Content-Length`, `chunked`) and their heads read line by line as they come; an upgraded or hijacked call (attach, an exec start that is not detached, session, gRPC, `Upgrade`), a broken body framing and anything that cannot become a request pass raw from there on. A body that is not JSON or over 4 MiB passes unlabelled, and the connection is still read after it. Agents' calls are never parsed. |
| CLB-004 | When a CI job ends, whatever the outcome, Yulius removes the containers (forced, keeping their volumes) and then the networks of the sandbox's running engine labelled with that job, while the job's group holds no process (asked for each one: a job id run again keeps its new attempt's). Images and volumes are never removed, and nothing starts a stopped engine. |
| CLB-005 | Each upkeep pass, and so first after a server starts, removes from each running engine the containers and networks labelled with a CI job whose cgroup no longer has a process (gone or empty). A container whose labels came from its image (its `yulius.ci.create` value is its image's) is not the job's and is kept, and so is one whose image cannot be read. |
| CLB-006 | Every removal is recorded per sandbox (audit `sandbox.docker.ci_leftovers`: containers, networks, at the job's end or by the sweep), and `GET /api/analytics/docker` adds them per week and in total. |

### A bad yulius.toml never blocks CI (pull request 2)

| ID | Requirement |
| --- | --- |
| CFG-001 | While the integration branch has CI, every commit on its way there (a pull request's head, a merge candidate, a playground task's merge) is judged by the integration branch's jobs; its own changed or added jobs run beside them (`<job> (this change)`), never instead: a change never weakens the gate that judges it, and a job it changes never lands untested. A branch's run reuses a same-tree passed run only when every job of its gate ran there. This replaces CI-110 and SPEC-163 PCI-002's own-jobs rule. A branch with no CI, or a file never valid, still lets a commit bring its own (bootstrap). |
| CFG-002 | A run of such a commit, and of the integration branch, whose `yulius.toml` as CI will read it after the merge is invalid has a failed job `yulius.toml` whose reason and output are the parser's message; the run fails, so it cannot merge. The file read is the head's for a merge candidate, a playground merge and a branch, and for a pull request the head's when it changed the file since its merge base, else the integration branch's. |
| CFG-003 | While the integration branch's `yulius.toml` is invalid, its gate is that of the newest first-parent commit (at most 100 back) whose file is valid: the last valid gate runs until a fix merges, and the CI page says so with the problem. Without one, CI runs nothing, as before. |
| CFG-004 | The minutes each sandbox's CI had no gate because of its configuration are counted: each episode is audited (`ci.config_blocked`, `ci.config_unblocked`) and `GET /api/analytics/merge-queue` sums them over its range (`config_blocked_minutes`). |
| CFG-005 | `yulius.toml` is read by a real TOML parser: multi-line arrays, trailing commas and comments inside arrays (and the rest of TOML 1.0) parse for `[[ci.jobs]]` and every other table; the checks on its values stay as they are. |
| CFG-006 | Every run judged by a last valid gate says so (the run's reason and the CI page: dev's problem, and the commit whose gate runs), and an invalid dev file never shows "No CI is configured" nor makes `delivery_merge` refuse a fix for want of CI. |

## Plan

- [x] Spec and plan written
- [x] A CI job runs in its own cgroup (CLB-001)
- [x] The Docker front labels a CI job's containers and networks (CLB-002, CLB-003)
- [x] A job's leftovers go at its end (CLB-004)
- [x] Ended jobs' leftovers are swept each upkeep and after a start (CLB-005)
- [x] Leftovers are counted in the Docker analytics (CLB-006)
- [x] Proof against a real engine: docker run, compose and a killed job through the front
- [x] ADR, architecture, operations and changes for pull request 1
- [x] Review round 1 of pull request 1 answered
- [x] Pull request 1 reviewed and approved
- [x] A pull request is judged by the integration branch's jobs (CFG-001)
- [x] An invalid yulius.toml fails its run with the parser's message (CFG-002)
- [x] The last valid gate runs while dev's file is invalid (CFG-003)
- [x] Config-blocked minutes are counted (CFG-004)
- [x] yulius.toml is read by a real TOML parser (CFG-005)
- [x] Runs on the last valid gate say so, and a fix is never refused for want of CI (CFG-006)
- [x] ADR, architecture and changes for pull request 2
- [x] Review round 1 of pull request 2 answered
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

### Pull request 1 (#1389): a CI job leaves nothing behind

2026-11-17, at the head reviewed (dev merged at `9261c728f`).

- `cargo nextest run -p yulius-sandbox`: 231 passed, among them the labeller's 9 tests
  (`docker_label::tests::*`: kept-alive creates, chunked and `Labels: null`, unlabellable bodies
  and a chunked one over 4 MiB passed as they came, six hijacking or upgrading calls, six
  unreadable starts and two cut-short requests passed unread, other bodies by length and chunked
  with the next request still read, stream bytes not held), the front's
  `a_ci_jobs_create_reaches_the_engine_labelled_and_no_one_elses`, the cgroup rules
  (`a_ci_job_group_is_named_after_the_job_and_read_back`, `a_ci_job_group_lives_as_long_as_its_job`),
  the removal (`a_jobs_end_removes_its_containers_then_its_networks`,
  `the_sweep_removes_only_ended_jobs_leftovers`) and the manager's
  `a_jobs_end_and_the_sweep_remove_what_ci_jobs_left`.
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & …'`:
  `local_ci::every_ci_job_is_grouped_and_its_end_told_whatever_the_outcome` (passed, failed, timed
  out and cancelled jobs each grouped before and told ended once, after; 9.9 s),
  `local_ci::a_job_a_restart_ended_is_told_ended`,
  `analytics::the_docker_read_counts_restarts_give_ways_and_needs_you_per_week`,
  `cockpit::the_instance_counts_docker_restarts_give_ways_and_needs_you_per_week`: passed.
- Real engine (this sandbox's dockerd 29.9.3, Compose v5.7.0):
  `YULIUS_REAL_DOCKER_SOCKET=/run/yulius/docker/docker.sock cargo nextest run -p yulius-sandbox
  --run-ignored all -E 'test(/real_engine_a_jobs/)'` passed (1.7 s). Through a front labelling
  for a job: `docker volume create`, `docker run -d -v …` with its own label, `docker network
  create`, `docker compose up -d` and `docker commit`; through an agent's front: a container, and
  one from the committed image (which inherits the label). The job's container, Compose's
  container, its network and Compose's network carried the job's label, the container its own
  label too; the agent's none. The job's end removed exactly 2 containers and 2 networks; the
  agent's two containers, the volume and the image stayed; the sweep then removed a later
  container of the ended job (1, 0). Nothing of the proof was left (`docker ps -a`, `network ls`,
  `volume ls`).
- e2e: `npx e2e run tests/pipeline.e2e.ts`: 18 passed, with the Docker card's new tile.
- `scripts/dev macos`: passed.
- Not provable in this sandbox: it has no cgroup file system (`/sys/fs/cgroup` is absent, and a
  container's is read-only), so the caller's cgroup on a real kernel is proven on a QEMU guest:
- **Real host** (a throwaway QEMU TCG guest from the Ubuntu 26.04.1 cloud image, kernel
  7.0.0-38, glibc 2.43; the branch's release build): `sudo yulius host setup --public-url …`
  installed Docker, built the base and passed the contained doctor. Its Docker smoke failed at
  its first run (`no started … within 90s`), and so did dev's own binary (`f5807b28b`) on the
  same guest. The smoke run's stderr, kept by a diagnostic build, said `cannot start bwrap:
  bubblewrap must be named by its absolute path`: since ADR-0314 (#1241) `sandbox-exec` refuses
  a bare `bwrap`, and the smoke still passed one, so every smoke run failed to start on every
  host. Fixed in this pull request, test-first
  (`docker_smoke::tests::a_smoke_run_names_bubblewrap_by_its_host_path`); the smoke now keeps
  each run's stderr in `<probe>/<script>.err`. With `f69a076f5`, `yulius doctor --docker-smoke`
  passed all 17 checks, the new ones among them: `a CI job's container and network carry its
  label` (`label=docker-smoke-job net-label=docker-smoke-job`), `an agent's container beside it
  carries none` (`label=.`), `a CI job's end removes them and keeps the agent's`
  (`container=yulius-smoke-agent` only). This proves the front's caller check on a real kernel:
  the front joined the holder's namespaces, read the job's pid by `SO_PEERCRED` and its cgroup
  through the host's `/proc`, and labelled the job's create calls. The removal went through the
  real engine's control socket.

### Pull request 1, review round 1 (request changes; every finding own code)

Reviewer (Fable 5.2) on `16b22577a`: 2 major, 8 minor, 4 nits; CI run `e156c2bd` passed; the
`security` job ended skipped (exit 78: Vidoc refused the instance key, HTTP 401, as on dev's own
runs), so the security scan is **unrun**, not passed. Each finding was fixed test-first:

1. *Major, a head not ending in CRLF CRLF was held until the client closed.* Heads are read line
   by line as the bytes come; the moment they cannot be a request line or a CRLF-ended header
   line, what came goes on and the connection turns raw.
   `what_is_not_a_request_passes_while_the_client_waits` (a client that keeps its side open:
   `\r\n\n`, bare LF, `hello\n`, `hello`, a header without CR, `HTTP/2.0`).
2. *Major, nothing failed when a job was not launched in its own group.*
   `local_ci::a_contained_ci_job_is_launched_in_its_own_group`: contained, through the
   recording session host, every job's launch joins `/cg/s1/ci/ci-job-<job id>`; with no job
   group it joins `/cg/s1/ci`, with no CI group `/cg/s1/agents` and no group is asked for.
   Mutation: `join.ci_cgroup = Some(group)` removed → this test failed.
3. *Broken chunk framing dropped bytes.* What was read passes and the connection turns raw:
   `broken_chunk_framing_passes_what_came` (on a create and on `/build`, the next request
   arriving).
4. *A call that only might hijack turned the connection raw.* An exec start's body is read;
   `"Detach": true` keeps the connection read: `a_detached_exec_start_keeps_the_connection_read`.
5. *Removals made before a failure were lost from the measures.* `remove_ci_leftovers` returns
   what it removed with the first error; a failed call skips only its own container or network:
   `removals_before_a_failure_count`.
6. *The image-inherited rule.* Each labelled create gets `yulius.ci.create` with a value of its
   own; a container is inherited only when its value is its image's, so a job's container from
   an image committed from its own container goes; a container whose image cannot be read is
   kept (ADR-0359 says both): `a_jobs_end_removes_its_containers_then_its_networks`,
   `each_create_gets_its_own_value_and_keeps_its_bytes_when_unlabellable`, and the real-engine
   proof again (2 containers, 2 networks removed; the agent's and the inherited one kept).
7. *Recovery awaited each removal before CI started.* The recovery removes in tasks of its own:
   `a_job_a_restart_ended_is_told_ended` now also runs with an isolation whose removal never
   ends, and the run is still queued again. Mutation: the removal awaited again → this test
   failed (61 s).
8. *A reused job group kept its old mtime.* A group made again for the same job id is made
   fresh: `a_job_group_made_again_is_fresh_for_the_sweep`.
9. *The smoke's sixth phase could hang.* A run that never says done is killed before it is
   waited for.
10. *Documents.* `docs/api.md` names the new fields and the audit action; ADR-0359 says the
    per-create value, the line-by-line head, and the consequences: a container another job or
    an agent uses goes with its creator, anonymous volumes stay, a container whose image cannot
    be read stays.

Mutation testing (`scripts/dev mutants` on the answered head, after merging dev at
`dff3c34ae`): `mutants: 316 listed, 129 caught, 0 missed, 0 timed out, 12 unviable, 175 not
reached (bound 30 min, took 1801 s)`. Nothing that ran survived; the 175 not reached are beyond
the 30-minute bound (ADR-0347).

Nits: a lowercase `labels` key is the one labelled; an unlabellable create passes byte for byte
wherever its `Content-Length` is; the gate takes its caller when it is made
(`Gate::telling_jobs`); every sandbox's emptied job groups are swept, with or without a serving
engine.

### Pull request 1, review round 2 (request changes; own code, all fixed)

Reviewer (Fable 5.2) on `1b8490201`: every round-1 finding fixed; CI run `496b40ae` passed;
security unrun (Vidoc 401). New:

1. *Major: the recovery's removal by job id could delete a network of the requeued attempt*
   (`requeue_ci_run` keeps job ids). A job's removal now takes each container and network only
   while the job's group holds no process, asked for each one (`Jobs::OneEnded`); a running
   attempt's stay and go at its own end. `manager::tests::a_jobs_end_and_the_sweep_remove_what_ci_jobs_left`
   now tells a job id whose group holds a process ended: nothing is removed (it failed before
   the change: 3 deletes instead of 2).
2. *The spec's CLB-003 and CLB-005 rows* say the create value, the detached exec start and the
   unreadable image (CLB-004 the per-item rule).
3. *A chunked create over 4 MiB turned the connection raw.* Its chunk and the rest are copied,
   and the connection is still read: `an_unlabellable_create_passes_as_it_came` now sends a
   create after it, labelled.

Nits: every `labels` key in any case is labelled (`Labels` and `labels` both); `job_label` is
gone (one `label` helper); `fake_sandbox`'s doc comment is back on it; ADR-0359 says "made over
10 minutes ago", the lone-surrogate and `Expect` limits and the per-item rule; architecture and
operations name `yulius.ci.create`.

### Pull request 1, review round 3 (approve)

Reviewer (Opus 5.5) on `1ea7e5973`: approve; every round-2 fix verified; CI run `547c9d82`
passed (quality, test 1/2, test 2/2, e2e); security unrun (Vidoc 401). Its notes, answered
after the approval within the follow-up allowance:

- *Minor: the job's-end removal raced the ptyhost's own exit*, so it often fell to the sweep.
  `Manager::ci_job_ended` waits up to 5 s for the job's group to empty first;
  `a_jobs_end_and_the_sweep_remove_what_ci_jobs_left` now reports the end of a job whose group
  empties 300 ms later, and its container is removed at the end (`job-end`), which failed before
  the change.
- *Nit: `serde_json` sorts a labelled body's keys*, so a body that spells one key two ways may
  change meaning to Go's last-wins decoder: said in ADR-0359's Consequences (only the job's own
  unusual bodies are affected).

### Pull request 2: a bad yulius.toml never blocks CI

2026-11-17, on this branch after PR #1389 merged (dev at `87c08e62a`).

- `cargo nextest run -p yulius-domain`: 565 passed. Among them:
  - `spec_format::config::tests::valid_toml_parses_whatever_its_layout`, which failed with the
    old reader: autoresearch's multi-line `paths = [` array with comments and a trailing comma,
    a multi-line basic string, `4_0`, an inline-table job and an unknown table of any shape;
  - `precise_errors_with_line_numbers`: invalid TOML is refused with the parser's message at
    its line, including a broken multi-line array at the line of its bad entry;
  - this repository's own `yulius.toml` parsed by `the_repositorys_own_cd_is_defined` and
    `the_repositorys_own_security_job_alone_is_granted_the_vidoc_key`;
  - the bounded nesting (65,000 `[` refused, "cannot recurse further");
  - `merge_queue::tests::blocked_minutes_add_up_episodes_within_the_range`.
- `cargo nextest run -p yulius-app -p yulius-contracts`: passed. CI-110's unit test is removed
  with the rule.
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & (test(/^local_ci::/) |
  test(/^merge_train::/) | test(/^playground::/))'`: 194 passed. New tests:
  - `a_pull_request_that_changes_its_jobs_is_judged_by_the_integration_branchs` (CFG-001);
  - `a_pull_request_with_an_invalid_yulius_toml_fails_with_the_parsers_message` (CFG-002,
    autoresearch's PR #4: `gate` and `[[ci.jobs]]` together). It fails at once with the
    parser's message, a failed job `yulius.toml` and its jobs skipped, and its gate result is
    failed.
  - `an_invalid_integration_branch_runs_its_last_valid_gate_until_a_fix_merges` (CFG-003,
    CFG-006), in four stages:
    1. dev breaks after a valid commit; the overview names the problem, `gate_fallback` and the
       source `last_valid_yulius_toml`;
    2. a pull request that leaves the file alone fails, naming dev's problem;
    3. the repair passes on the last valid gate, and its run says so;
    4. once the repair merges, dev's own gate is in force again.
  - `a_repair_of_a_never_valid_configuration_is_tested_with_its_own_jobs` (CFG-003 for a file
    never valid, and CFG-004: 30 blocked minutes counted by the merge-queue read).

  Rewritten to the new rule:
  - the secrets test: a pull request's own grants and commands never run;
  - the security-job test: bootstrap only;
  - the run-again test: the same integration jobs;
  - the two scan-switch tests: the scanning job is the integration branch's;
  - the playground test: a task that changes a job is judged by dev's.
- Other integration files that read the CI configuration or its problems (analytics,
  scope_overlaps, leftovers, stages, delivery, playground_move, pr_watch, cockpit): 213 passed.
- e2e: `npx e2e run tests/ci.e2e.ts`: 21 passed. The new test covers the banner of the last
  valid gate (`ciconfig=fallback`), never "CI is not running", the gate card's source and the
  run's note.
- Mutation testing (`scripts/dev mutants`): `mutants: 171 listed, 89 caught, 0 missed, 0 timed
  out, 82 unviable, 0 not reached (bound 30 min, took 1563 s)`.
- Not measured here: production's minutes before and after. The incidents give the "before"
  (autoresearch's CI ran nothing from PR #4's merge to PR #5's, and from PR #42's merge until a
  person bypassed the ruleset for PR #50). The "after" is `config_blocked_minutes`, which counts
  only a file never valid once this is deployed; promotion and deployment are a person's.

### Pull request 2, review round 1 (request changes; every finding own code)

The reviewer (Fable 5.2) read `ccc36172e`, then the fixes of two failed CI runs, which arrived
mid-review:
- `ecdbabddb`: the repository check refused `toml` in the domain. The checker and
  `docs/engineering/architecture.md` now allow it by name, with ADR-0360.
- `29a96c920`: `remote_mcp::a_broken_integration_ci_is_named_when_merging` pinned the old "tests
  nothing" refusal of a repair. It is now
  `a_repair_of_a_broken_integration_ci_waits_for_its_own_checks`.

The security job ended skipped (Vidoc 401, as on dev): **unrun**. Answers, each fixed
test-first:

- *Direction and Major 1: a valid but failing job merged untested, then judged its own repair,
  and the same-tree reuse recorded dev green for jobs that never ran.*
  - A change's own changed or added jobs now run beside dev's, never instead
    (`dom::with_changed_jobs`; never under the trusted scan's key;
    `ci::tests::a_changes_own_jobs_run_beside_the_gates`).
  - A branch reuses a same-tree verdict only where every job of its gate ran with the same
    command (`a_branch_never_reuses_a_run_that_did_not_run_its_jobs`). Removing the check fails
    it.
  - `a_pull_request_that_changes_its_jobs_is_judged_by_the_integration_branchs` now also changes
    a job to `exit 1`: the run fails on `check (this change)`.
  - ADR-0360 records the reversal: "Only the integration branch's jobs" is now a rejected
    alternative.
- *Major 2: `LastValid` secrets untested.* `only_the_integration_branch_jobs_grant_secrets` now
  shows that `LastValid` grants for the same name and command, and not for another.
- *Major 3: the configuration check was proven for pull requests only.* Each of these fails at
  once with a failed job `yulius.toml` and the other jobs skipped:
  - `merge_train::a_candidate_with_an_invalid_yulius_toml_fails_with_the_parsers_message`;
  - `playground::a_task_with_an_invalid_yulius_toml_fails_its_merges_run`;
  - dev's own run at its invalid head, in
    `an_invalid_integration_branch_runs_its_last_valid_gate_until_a_fix_merges`.
- *Minors.*
  1. Fallback without a gate: `an_invalid_file_with_no_earlier_gate_names_no_fallback`.
  2. Episodes across a restart: the state is read from the audit first.
     `an_episode_that_ended_while_the_server_was_down_is_closed` counts 30 minutes, not the
     range's 90.
  3. A head refused for dev's file runs again once it is valid:
     `a_head_refused_for_the_branchs_file_runs_again_once_it_is_valid`.
  4. A table inside a known one is refused as `NestedTable` ("is not a table Yulius reads"),
     and the newly strict corners are in ADR-0360.
  5. The stale text is fixed: `docs/api.md` "Runs", the architecture's sequence diagram and the
     code comments.
  6. The store reads an unknown source as the integration branch's
     (`a_run_of_an_unknown_source_still_reads`), and ADR-0360 gives the rollback SQL.
  7. The remaining remote refusal is tested:
     `a_change_that_does_not_repair_a_never_valid_ci_is_refused`.
  8. A Git error checks the head's own file, and an unwalkable history gives no verdict, never
     own jobs. The test `LocalGit` port gained `parents` and `merge_base`, as production's Git
     has.
- *Nits.* The parser is named TOML 1.1, the dead test arm is gone, a reason no longer repeats
  "yulius.toml", and the comments in merge_train.rs and playground.rs are updated.
  `EnvironmentsConflict` stays as a guard of the profile's rule, said in its comment.

### Pull request 2, review round 2 (request changes; every finding own code)

The reviewer (Fable 5.2) on `0120363ca` (`304ca1cae` adds only a plan line) confirmed every
round-1 fix. CI run `fae842a9` passed; security unrun (Vidoc 401). It found the following, each
fixed test-first:

1. *Major: a pull request behind dev ran dev's old and removed jobs as "(this change)".* Own jobs
   are compared with the jobs at the head's merge base as well as with the gate (unknown base:
   with the gate alone), and a profile's gate gets none.
   - `a_branch_behind_is_tested_with_the_integration_branchs_jobs_of_now` now asserts the run's
     exact jobs, with a changed and a removed job on dev. It failed before the change with
     `plain (this change)` and `gone (this change)`.
   - `ci::tests::a_changes_own_jobs_run_beside_the_gates` covers the base and the profile gate.
2. *Major: an `exclusive` job and its twin ran at once.* The turn is keyed by the job's
   definition and matrix value: `an_exclusive_job_and_its_changed_twin_share_a_turn`. It failed
   before the change; `an_exclusive_job_waits_its_turn_outside_its_time_limit` still passes.
3. *Minor: no verdict on a Git error read as no CI.* `ci_gate_result` answers pending while the
   walk is unknown: `a_history_git_cannot_walk_leaves_a_head_pending_not_unconfigured`. With a
   commit object of dev missing from the clone, it was `NotConfigured` before. A failing
   `merge_base` in the configuration check takes the "changed" path, which the pull-request
   tests cover.
4. *Minor: twins moved the gate's job to a cold checkout.* A twin checks out at `<key>.change`
   (`matrix_siblings_get_their_own_checkouts`), and `checkout_job_key` reads it as the job's.
5. *Minor: changed jobs past `MAX_JOBS` were dropped silently.* None is dropped (the run holds
   at most twice the bound), and ADR-0360 says a scanning job's change gets no twin.

Nits:
- a reused run matches a job's verdict by command too;
- an episode's state is kept only once its audit entry is written, and the doc is corrected;
- a twin is told the job's own name in `YULIUS_CI_JOB`;
- ADR-0360 says a matrix-only twin keeps the entry's grant;
- `Head` is removed.


### Pull request 2, review round 3 (request changes; every finding own code)

The reviewer (Fable 5.2) on `38d960201` confirmed every round-2 answer by reading the code and
its tests. It found the following, each fixed test-first:

1. *Major: a change to a job's time limit or resources got no twin.* `with_changed_jobs`
   compared key, matrix and command only, where dev's earlier rule compared the whole job. The
   reviewer showed it with a scratch domain test: a job with `timeout_minutes` 30 changed to 1
   ran only the gate's job. The whole `Job` is compared now, and the reuse rule's "every gate
   job ran" check covers need, timeout and exclusive too.
   - `a_change_to_a_jobs_time_limit_runs_its_twin` (integration) and the timeout-only case in
     `ci::tests::a_changes_own_jobs_run_beside_the_gates` failed before the change.
2. *Minor: two round-2 fixes had no test.*
   - `a_reused_verdict_comes_from_the_job_that_ran_the_command`. Mutation check: dropping the
     command match from the reused verdict fails it.
   - The time-limit test also asserts the twin's `YULIUS_CI_JOB`. Mutation check: dropping the
     strip fails it.
3. *Nit: a twin's checkout never went stale.* `checkout_job_key` read `<key>.change` as
   `<key>`'s. It is its own name now, and it is used while a recent run had that job's twin.
   `unused_ci_checkouts_go_while_no_run_runs` keeps `gate.change` and removes `old.change.2`.
   Mutation check: counting twins as their job's key fails it.

Commands on `7b21063b6`:
- `cargo test -p yulius-domain`: 566 + 26 passed.
- `cargo test -p yulius-app --lib -- --test-threads=1`: 599 passed.
- `cargo test -p yulius --test main -- leftovers:: local_ci:: playground:: merge_train::`: 231
  passed.

Outside this change: in a parallel `cargo test -p yulius-app --lib`, one of the `sweep::tests`
failed in each of two runs (`over_its_cap_…`, then `a_cross_target_profile_s_lock_counts_too`).
The host's load average was 14 to 19 at the time. Each passes alone (3 of 3) and in the
single-threaded run. This branch does not touch `sweep.rs`, and Yulius CI ran these tests green
on `38d960201`.

### Pull request 2, review round 4 (request changes; every finding own code)

The reviewer (Opus 5.5) on `9febbf056` confirmed every round-3 answer by reading the code and
running its tests. It also checked that job keys cannot contain `.`, so no real key collides
with `<key>.change`. It found the following:

1. *Blocker: CI run `b9d60ec8` failed in `quality` on `cargo fmt`.* `c198699cf` formats the
   line; `cargo fmt --all -- --check` and `clippy -D warnings` on the changed crates are clean.
2. *Minor: ADR-0360 no longer matched the code.*
   - A twin that changes only its time limit keeps the command, so it gets the integration
     branch's grant. That is safe, because the grant always comes from the integration
     branch's entry, and the ADR now says so.
   - The reuse rule now names need, time limit and `exclusive` too, as does
     `docs/architecture.md`.
3. *Minor: the extended reuse check had no test.*
   `a_branch_never_reuses_a_run_that_ran_its_job_with_another_time_limit`. Mutation check:
   dropping the time limit from the check fails it.
4. *Nit: the reused verdict was looked up by fewer fields than the reuse check.* Both now use
   one predicate, `ran_as`. `a_reused_verdict_comes_from_the_job_that_ran_with_the_time_limit`
   failed before the change: it copied `passed` from the old job instead of the twin's
   `skipped`.
5. *Nit: the leftovers test missed the reported case.* `unused_ci_checkouts_go_while_no_run_runs`
   now has `lint` run recently without a twin, and asserts that `lint.change` is stale.
   Mutation check: making `checkout_job_key` strip `.change` again fails it.
