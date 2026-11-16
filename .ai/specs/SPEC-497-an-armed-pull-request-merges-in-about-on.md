---
yulius-task: 29fb50db-a8f1-40ea-bf43-8677399be0fe
status: in-progress
---

# SPEC-497: an armed pull request merges in about one CI run

Status: in-progress
Date: 2026-11-16
Owner: Yulius task #479
Scope authorization: the request of task #479 (MrFishPL, 2026-11-16): measure why the path to
merge got slower in quartio-v2 after SPEC-493, find the causes, and make an armed pull request
merge in about one CI run in every project. Builds on SPEC-493 fixes 3 to 6 and ADR-0285 as
amended; redoes none of them. Fast mode stays off. quartio-v2's own part (its flaky tests, its
Docker network range, how its tests create networks) is that project's: named in an issue or a
message to its Superagent, not waited for.

## Goal

An armed pull request merges about one CI run after it is armed, in every project. Read on the
instance a week after the last slice merged:

- the median from armed to merged is at most 25 min in quartio-v2 and 15 min in yulius;
- at most 15 % of CI minutes go to runs that fail for reasons outside the change, or are
  cancelled;
- at least 80 % of merge candidates merge.

## Context

### What was measured (production, 16 Nov 2026, from the request)

quartio-v2: 31 merge candidates formed, 10 merged, 21 dissolved. Since 12:00 UTC 10 candidate
runs were cancelled ("the merge train was dissolved: #N's own …", "`dev` moved") and 10 pull
request runs superseded. CI minutes: 588 on passed runs, 526 on failed runs, 114 on cancelled
runs. From opening to merge the median was 389 min (00:00 to 08:00) and 250 min (after 16:00);
yulius 30 min, 3 of 3 candidates merged. Four `quality` runs at once 42 % of the time between
12:00 and 15:27 (`YULIUS_CI_LANES=4`): 18.8 min each against about 10 alone, and Docker ran out
of address pools in 3 runs. With `cpus = 30` from 15:27, two at once 96 % of the time, 13.5 min
each. Of 20 failed quality jobs between 12:00 and 17:00 few were about the change: Docker
address pools (3), the same tests failing on unrelated pull requests (`operator_step_dots…` 3,
`person_lang…` 3, `browser images_and_proxy…` 2, two of `browser covers_and_names` 2 each,
`share_dialog_said…` 2), and candidates failing on how members combine (formatting after the
merge 2, `every_default_file_is_listed_with_todays_version` when two pull requests change the
same default skill file). An agent's own test run used 18 cores at once while CI ran.
Baseline 12-13 Nov: merged on its own run 17 min from opening; with a candidate 127 min.

What this task measured itself (the instance's `ci_status` of this project, 16 Nov): pull
request #1354's run waited 8 min 27 s for a job slot and its budget before `quality` started
(ran 12 min 5 s), while quartio-v2's two `cpus = 30` jobs held 60 of the host's 64 CPUs.

### How the parts work today (with their code)

- **The host.** 64 CPUs, 755 GiB (`nproc`, `free`). Each sandbox's processes run in
  `sandboxes/<id>/{system,agents,ci,engine}` (`yulius-sandbox/src/cgroup.rs`); CI's leaf weighs
  four times the agents' (ADR-0039), and a sandbox running CI jobs weighs 500 among sandboxes
  (100 otherwise), however many CPUs its jobs hold (SPEC-362, ADR-0247).
- **Admission.** A run takes a lane of its sandbox (`ci_dispatch`, `YULIUS_CI_LANES`, host-sized
  "one per 32 CPUs plus 2, at most 8"). Each job then waits for an instance-wide job slot
  (`YULIUS_CI_MAX_JOBS`, `ci/slots.rs`) and for its budget out of the CI pools
  (`App::ci_reserve`, `resources.rs`): its declared `cpus`/`memory_gb` or an equal share of the
  sandbox capacity among the run's jobs. The pools are sized once from the sandbox capacity (the
  host's CPUs, `memory.max`). The budget is only told to the job (`YULIUS_CPUS`); nothing checks
  what it really uses. A job's Docker containers run in its sandbox's `engine` group, outside
  its budget.
- **Docker networks.** Each sandbox runs its own engine in its own network namespace
  (`engine.rs` `dockerd_argv`), with Docker's default address pools: 172.17-31.0.0/16 in /16s
  and 192.168.0.0/16 in /20s, 31 networks in all.
- **A job's end.** Its exit decides its state; nothing tells an infrastructure failure from a
  test failure. `ci_rerun` (an agent's call) queues the whole commit again.
- **The merge queue** (ADR-0285, SPEC-493 MQC): a member whose own run fails dissolves the
  candidate and cancels its run; a passed candidate waits for its members' own runs; a moved
  integration branch cancels the candidate.

### Scope

In scope: this repository, in slices, each its own pull request, in the request's order, each
measured. Not in scope: quartio-v2's own code and settings (issue or Superagent message); fast
mode.

## Requirements

### Slice A: CI never takes more than the host has

- **HBA-001** One CI budget for the whole instance: CPU and memory of the host, less a reserve
  for the server and agent sessions (an eighth of each, at least 1 CPU and 2 GiB). Every
  sandbox's jobs are admitted from it.
- **HBA-002** A job is admitted when its reservation fits beside what CI holds now, where CI
  holds the larger of the running jobs' reservations and what every sandbox's `ci` and `engine`
  groups were measured to use over the last sampling window (CPU time per second; anonymous
  memory). A job that uses more than it declared, or Docker containers it starts, therefore
  hold the host back from the next job. Waiters are served first come, first served; a job
  larger than the budget runs alone. A job's default share (no `cpus`) is a share of the CI
  budget, so a run's jobs start together on an idle host.
- **HBA-003** Docker networks are part of the budget per engine. Each engine gets address
  pools of /24 networks within Docker's own default ranges (4,096 networks instead of 31), read
  back from the engine's `/info` (an engine started before keeps 31). A job of a sandbox whose
  engine has fewer than 32 free networks (a quarter of a smaller engine's) waits, saying so;
  networks no container used for 10 minutes are pruned first.
- **HBA-004** Agents' heavy local commands take what CI does not hold: a sandbox's weight among
  sandboxes grows with the CPUs its running jobs hold, so under contention its CI gets about its
  reservation; agent processes keep their own weight and nice level.
- **HBA-005** `YULIUS_CI_LANES` stays the bound of runs at once per sandbox, with its host-sized
  default; it no longer sizes the load, the budget does, so neither the lanes nor a project's
  `cpus` can overload the host. The job's wait says which part of the budget it waits for.

### Slice B: a failure outside the change is handled as such

- **OFC-001** A job that ended for an infrastructure reason (no free Docker address pool, the
  Docker socket's EOF or reset, killed for memory, its process host lost) is run again once by
  itself, its output saying why; a second infrastructure failure ends it error.
- **OFC-002** A test that fails in the runs of two different pull requests within 48 hours
  while the integration branch's last run passed it is flaky: Yulius quarantines it at once (its
  failure alone no longer fails a job, the job's output says so), files one issue in the
  project's repository and tells the project. It is never rerun until green.
- **OFC-003** CI minutes are counted by outcome (passed, failed in the change, failed outside
  it, cancelled) per project and day, and read with the merge queue's numbers.

### Slice C: the merge queue does not throw work away

- **MQW-001** A member whose own run fails leaves without cancelling a candidate run that still
  decides the others.
- **MQW-002** A candidate whose run passed merges, even while a member's own run still goes.
- **MQW-003** Members whose combination conflicts in derived files are handled without a wasted
  cycle (decision recorded in the slice).
- **MQW-004** Why each candidate was dissolved is counted and shown.

### Slice D: a CI run is shorter

- **SHR-001** A job can be split into shards that run side by side where the budget allows.
- **SHR-002** A candidate's run reuses what its members' own runs built. The gain is measured.

### Slice E: measured before and after

- **MSR-001** Armed-to-merged per project, candidates formed, merged and dissolved with their
  reasons, and CI minutes by outcome, before each slice and after it, in the Evidence.

## Plan

- [x] Slice A: the host budget sampled from every sandbox's CI and engine groups
- [x] Slice A: jobs admitted by the larger of reserved and measured use
- [x] Slice A: Docker networks per engine in the budget, larger address pools
- [x] Slice A: a sandbox's weight follows the CPUs its jobs hold
- [x] Slice A: documents and ADR
- [ ] Slice A: review rounds answered
- [ ] Slice B: infrastructure failures rerun once by themselves
- [ ] Slice B: flaky tests quarantined with an issue, the project told
- [ ] Slice B: CI minutes by outcome read and shown
- [ ] Slice B: documents, ADR and review
- [ ] Slice C: a failed member leaves without cancelling the candidate
- [ ] Slice C: a passed candidate merges while own runs go
- [ ] Slice C: derived-file conflicts handled without a wasted cycle
- [ ] Slice C: dissolve reasons shown; documents, ADR and review
- [ ] Slice D: sharded jobs
- [ ] Slice D: a candidate reuses its members' builds; review
- [ ] Slice E: numbers before and after, quartio-v2's part told

## Acceptance criteria

- Slice A: with jobs whose measured use exceeds their reservation, the next job waits until the
  measured use leaves room; with a full engine address pool, the job waits with a reason; the
  weight of a sandbox follows its jobs' CPUs. Tests at the domain rule and with the real
  executor (`local_ci`).
- Slice B: an infrastructure failure reruns that job once and says so; a test failing in two
  pull requests' runs is quarantined with one issue; CI minutes by outcome read per project.
- Slice C: a failing member does not cancel the candidate's run; a passed candidate merges with
  an own run still going; dissolves are shown with their reasons.
- Slice D: a sharded job runs its shards side by side and passes only when every shard passed.
- Nothing merges without a passed Yulius CI run on the exact head or the exact merged result; a
  failed test is never rerun until green; ADR-0285's gains stay.

## Test plan

- Domain units for the admission rule, the infrastructure classifier, the flaky rule and the
  shard plan.
- `local_ci` (real Git, real jobs, fake isolation measurements) for admission, reruns and
  shards; `merge_train` for the queue changes; `analytics` for the reads.

## Decisions

- **The budget counts measured use, not only declarations** (slice A): a job's `cpus` is a
  promise the job does not keep (its tools and containers use what is free), so admitting by
  declarations let four 16-CPU jobs take 18.8 min each. Measuring per job would need each
  container mapped to the job that started it; measuring the CI and engine groups of every
  sandbox together needs none, and the reservations still cover jobs that just started.
- **Docker networks: a larger pool and a wait, not a rerun** (slice A): Docker's default pools give
  31 networks per engine, which side-by-side test runs use up; /24 networks out of the same ranges
  give 4,096 without touching an address the sandbox did not already give Docker. The wait for 32
  free is the budget's part for an engine that was started with the old pools or leaks networks.
- **`YULIUS_CI_LANES` keeps its default** (slice A): it bounds checkouts per sandbox (each lane
  keeps its own job folders and caches, a disk cost), and the budget now decides the load, so a
  lower default would only make a small project's runs queue behind each other for nothing.

## Evidence

### Slice A: CI never takes more than the host has

- HBA-001, HBA-002: `yulius_domain::resources::{Capacity::ci_budget, admit}`; unit
  `resources::tests::ci_jobs_are_admitted_by_what_ci_really_holds` (64 CPUs give 56 for CI; four
  16-CPU jobs no longer fit; two jobs that reserved 32 but use 45 hold a 16-CPU job back with
  "CI holds 45 of the host's 56 CPUs for CI (32 reserved, 45 measured); this job needs 16"; memory
  too; a job larger than the budget alone). `App::ci_reserve` (`CiAdmission`, first come first
  served, a ticket left on cancellation), `App::ci_measured` (the `ci` and `engine` groups of the
  sandboxes running jobs, read under the admission's lock at each job's start, each dispatcher
  pass and each look of a waiting job, a rate over 5 to 30 s), and
  `Delegation::ci_use` (unit `cgroup::tests::ci_use_adds_the_ci_and_engine_groups`: `usage_usec`
  and `anon` of both groups, not the agents', none without the `cpu` split).
  `local_ci::a_job_waits_while_ci_uses_more_than_it_reserved` (real Git and jobs, a fake isolation
  measured at 2.5 CPUs): dev's new job fits beside the running pull request's 1 reserved CPU but
  waits with "waits for the host's CI budget: CI holds 3 of the host's 3 CPUs for CI (1 reserved,
  3 measured); this job needs 1", and starts once the use drops while the first job still runs;
  its reason is then cleared. Mutation: admitting by the reservations alone (`held = reserved`)
  fails it (the job never waits).
- HBA-003: `engine::ADDRESS_POOLS` in `dockerd_argv` (the argv test checks the five pools and
  that they add up to 4,096 /24 networks); `engine::networks` and `prune_networks` (unit
  `the_engines_networks_are_counted_and_pruned`: `host` and `none` not counted, a GET and a POST,
  a refusal an error, the capacity from `/info`); `SandboxManager::engine_networks`,
  `prune_engine_networks` (only a running engine, never through the sandbox's socket).
  `local_ci::a_job_waits_for_docker_networks`: 10 free of 4,096, the networks unused for 10
  minutes pruned first, the job waits with "waits for Docker networks: its sandbox's engine can
  make 10 more of 4096, and a job starts with at least 32 free", passes once there is room, and
  its reason is cleared.
- HBA-004: `cgroup::sandbox_weight(jobs, cpus)`;
  `cgroup::tests::ci_gets_about_the_cpus_its_jobs_hold`: 30 held CPUs get at least 30 of 64 with
  their sandbox's agents and four other sandboxes' agents building (28 with the flat 500), and the
  others keep a share. SPEC-362's weight tests stay green (four 1-CPU jobs still weigh 500).
- HBA-005: the job's reason while queued (`set_queued_ci_job_reason`), cleared at its start; the
  lanes' default is unchanged (ADR-0350, `docs/operations.md`).
- Checks run: `cargo clippy --workspace --all-targets -D warnings`, `cargo doc` of the three
  crates with `-D warnings`, the unit tests of `yulius-domain`, `yulius-sandbox`, `yulius-app`
  (1,334 passed), and `local_ci`, `disk`, `limits`, `isolation`, `sandbox_docker` (137 passed).
- Before (16 Nov, from the request and this project's `ci_status`): four quality runs at once,
  18.8 min each against about 10 alone, 3 runs failed for Docker address pools; this project's
  pull request #1354 waited 8 min 27 s for a slot. After: read a week later on the Pipeline tab's
  job spans (a job's `wait` stage and its run time, per project) and in `ci_status`'s reasons;
  the number of runs whose output says "could not find an available, non-overlapping IPv4
  address pool" (expected 0).
- **Review round 1** (Fable 5.2, head `2bed1757d`; its own CI run passed). Request changes, all
  own code. Answered at head `049fc02e8`:
  - Major: the admission test failed 3 of 24 (a rate of 3.0000002 rounded up to 4). Rates are
    rounded to a tenth before the ceiling, and the fake uses 2.5 CPUs. 24 of 24 passed under
    `--stress-count 24` (both admission tests, 300 s).
  - Major: two waiters reading the groups at once could apply an older reading after a newer
    one and reset the rate. Readings are now taken inside the admission's lock
    (`CiAdmission::measure` takes the reader). Unit
    `resources::tests::what_ci_uses_is_measured_over_a_recent_window`: no rate before 5 s, two
    sandboxes' rates add up, clock noise keeps 3, memory in MiB, a counter that goes back starts
    over, a sandbox no longer running is forgotten, a base older than 30 s is not settled.
  - Major: a run's default shares were of the capacity and no longer fitted the budget
    together. They are of the CI budget (`exec.rs`); domain
    `resources::tests::a_runs_default_shares_start_together` (1 to 8 jobs on 64 CPUs).
  - Major: the queue and the measurement had no tests of their own. Unit
    `resources::tests::the_queue_keeps_its_order`: a small job behind a waiting large one waits
    its turn (`Behind`), a large one leaving lets it in, an unsettled reading holds a job back
    only while CI runs. The `Weighing` fake records the CPUs, and
    `running_ci_jobs_weigh_on_their_sandbox_until_they_end` asserts they reach the isolation.
    Manager: `engine_networks` and `prune_engine_networks` are `None` without a holder, without
    an engine and with a stopped one. `scripts/dev mutants`: 135 listed, 98 caught, 0 missed,
    0 timed out, 16 unviable, 21 not reached (bound 30 min, took 1801 s). Of those not reached,
    two were undone by hand and caught: counting `host`/`null` networks instead of the others
    (`the_engines_networks_are_counted_and_pruned` fails) and a queued reason never written
    (`local_ci::a_job_waits_for_docker_networks` fails).
  - Minor: the rate window had no upper bound. The dispatcher reads the groups at every pass
    (at most 15 s apart), and a base older than 30 s is a new base: the next job waits one look
    ("waits for a reading of what CI uses now").
  - Minor: engines started with Docker's default pools never waited. The engine's `/info`
    (`DefaultAddressPools`, Docker's 31 when it names none) gives its capacity
    (`engine::pool_networks`, doctest; unit test with configured pools giving 512 and an old
    engine giving 31); a smaller engine needs a quarter free (`network_reserve`, 7 of 31).
  - Minor: the prune could remove a network an agent or a job had just made. It now removes only
    networks unused for 10 minutes (`until=10m`, asserted at the request), whatever runs.
  - Minor: a stopped engine logged a warning at each job: debug now.
  - Minor: the new address layout is in `docs/operations.md`, `changes.md` and ADR-0350 (the
    first network is 172.17.1.0/24; a project needing another range sets its own `subnet`).
  - Nit: the weight comment now says "at least its CPUs while `n` is below …"; ADR-0350 names
    the price for other sandboxes' agents while CI uses more than it reserved.
  - Nit (network room before the slot): kept, deliberately: a job held by one engine then holds
    no slot or budget the other sandboxes need. Said in the doc comment and the ADR.
- Security scan: `yulius security-scan` exited 2, not run (Vidoc refused the key, HTTP 401);
  CI's `security` job ends skipped. Task #478 issues a new key.
- **Review round 2** (Fable 5.2, head `7d0b6b2f2`). Request changes, all own code. Answered:
  - Major: the default-share fix had no test that failed without it.
    `local_ci::a_runs_default_shares_run_together` (4 CPUs, a budget of 3): the pull request's
    two jobs without `cpus` run at once, each told "[yulius] budget: 1 CPU, 2048 MiB". With
    the shares of the capacity (`exec.rs` reverted) it fails: the second job waits.
  - Minor: "not settled" reached only the caller that re-based. A re-based sample keeps
    `rebased` until a rate is computed, so every caller sees it (the unit test reads again at
    61 s: still not settled).
  - Minor: ADR-0350 and `operations.md` said the CI page shows a waiting job's reason; it is
    `ci_status` and the run's API. Corrected.
  - Minor: the weight test could not tell CPUs from jobs. `local_ci::a_job_weighs_by_the_cpus_it_holds`
    (a 2-CPU job: the isolation hears 2 per job).
  - Minor: the network test now asserts the reason is cleared once the job ran.
  - Minor: networks made under Docker's old pools count as one /24 each; stated in ADR-0350's
    Consequences, with what clears them.
  - Direction (jobs arriving within one window are checked against the same reading; `cpus = 30`
    twice no longer fits 56): both are now in ADR-0350's Consequences; quartio-v2 is told in
    slice E.
  - Nits: the clock and the running list are read under the lock; a prune logs at info only when
    it removed something; the memory reserve is at most half the capacity (a 2 GiB host keeps
    1 GiB for CI: doctest); the Evidence above describes this head's code.
