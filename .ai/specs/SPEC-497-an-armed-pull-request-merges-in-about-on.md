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
- **HBA-002** (amended 16 Nov 2026, [ADR-0353](../../docs/decisions/0353-ci-jobs-start-by-what-ci-uses-within-a-bounded-worst-case.md))
  A job starts when, with no CI job running, at once; otherwise when all of these hold (CPU
  pressure is not one of them since 17 Nov 2026: a job pinned to a few CPUs shows it while the
  host idles, ADR-0353's amendment):
  - **Bounded worst case.** The running jobs' budgets with its own are at most three times the CI
    budget's CPUs.
  - **Room.** The sum of these fits in the CI budget, in CPUs and memory: what every sandbox's
    `ci` and `engine` groups were measured to use over the last window, what jobs started in the
    last minute may still bring, and its own start. A starting job counts as its budget, at most
    a quarter of the CI budget. Where nothing is measured, the reservations decide.
  - **A free slot.** The job takes it when it is admitted.

  Waiting jobs are ordered: merge trains' and playground merges' jobs first, then pull requests',
  then branches', and in each the longest waiting. A job that cannot start lets the ones behind it
  that can; one that waited 10 minutes holds them back until it starts. A job's default share (no
  `cpus`) is a share of the CI budget, so a run's jobs start together on an idle host.
- **HBA-003** Docker networks are part of the budget per engine. Each engine gets address
  pools of /24 networks within Docker's own default ranges (4,096 networks instead of 31), read
  back from the engine's `/info` (an engine started before keeps 31). A job of a sandbox whose
  engine has fewer than 32 free networks (a quarter of a smaller engine's) waits, saying so;
  networks no container used for 10 minutes are pruned first.
- **HBA-004** Agents' heavy local commands take what CI does not hold: a sandbox's weight among
  sandboxes grows with the CPUs its running jobs hold, so under contention its CI gets about its
  reservation; agent processes keep their own weight and nice level.
- **HBA-005** `YULIUS_CI_LANES` stays the bound of runs at once per sandbox, with its host-sized
  default; it no longer sizes the load, the budget does: neither the lanes nor a project's `cpus`
  take CI past three times the CI budget's CPUs (HBA-002 as amended), and measured use stops
  further starts. The job's wait says which part of the budget it waits for.

### Slice B: a failure outside the change is handled as such

- **OFC-001** A job that ended for an infrastructure reason (no free Docker address pool, the
  Docker socket's EOF or reset, killed for memory, its process host lost) is run again once by
  itself, its output saying why; a second infrastructure failure ends it error.
- **OFC-002** A test that fails in the same job in the runs of two different pull requests
  within 48 hours, at heads that share no commit outside the integration branch, while the
  integration branch did not fail it then, is flaky: Yulius
  quarantines it at once, files one issue in the project's repository, says so in the job's
  output and tells every later job of the project in its environment (`YULIUS_CI_QUARANTINE`, a
  nextest filter in `YULIUS_CI_QUARANTINE_NEXTEST`), so the project's gate leaves it out; closing
  the issue ends the quarantine. It is never rerun until green.
- **OFC-003** CI minutes are counted by outcome (passed, failed in the change, failed outside
  it, cancelled) per project and day, and read with the merge queue's numbers
  (`GET /api/analytics/merge-queue`, the Pipeline tab's Merge queue card), with the job minutes
  thrown away by infrastructure failures and the share outside the change.

### Slice C: the merge queue does not throw work away

- **MQW-001** A member's own run that errors or is cancelled no longer makes it leave or cancels
  the candidate's run, which still decides it; only a failed own run makes a member leave, and a
  candidate run whose tree holds that member decides no one and is cancelled (ADR-0357). A
  candidate formed again reuses its tree's passed run.
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
- [x] Slice A: review rounds answered
- [x] Slice A2: the operator's CI admission fix applied, its first review answered
- [x] Slice A2: the admission fix's last review round
- [x] Slice B: infrastructure failures rerun once by themselves
- [x] Slice B: CI minutes by outcome read and shown
- [x] Slice B: review round 1 answered: reruns tied to the failure, minutes kept per day
- [x] Slice A2: CI wait job-minutes per hour and host busy % measured and shown
- [x] Slice B: documents and ADR of the reruns and minutes
- [x] Slice B: the reruns and minutes' review rounds 1 to 3 answered
- [x] Slice B: the reruns and minutes' last review round
- [x] Slice A3: the operator's admission amendment applied
- [x] Slice A3: the amendment's review rounds
- [x] Slice B: flaky tests quarantined with an issue, the project told
- [x] Slice B: quarantine documents and ADR
- [x] Slice B: quarantine review round 1 answered
- [x] Slice B: quarantine review round 2 answered
- [x] Slice B: quarantine review round 3 answered
- [x] Slice C: a failed member leaves without cancelling the candidate
- [x] Slice C: a passed candidate merges while own runs go
- [x] Slice C: derived-file conflicts handled without a wasted cycle
- [x] Slice C: dissolve reasons shown; documents and ADR
- [x] Slice C: review round 1 answered
- [x] Slice C: review round 2 answered
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
- Slice C: an errored or cancelled own run does not cancel the candidate's run; a failed one
  makes its member leave; a passed candidate merges with an own run still going; dissolves are
  shown with their reasons.
- Slice D: a sharded job runs its shards side by side and passes only when every shard passed.
- Nothing merges without a passed Yulius CI run on the exact head or the exact merged result; a
  failed test is never rerun until green; ADR-0285's gains stay.

## Test plan

- Domain units for the admission rule, the infrastructure classifier, the flaky rule and the
  shard plan.
- `local_ci` (real Git, real jobs, fake isolation measurements) for admission, reruns and
  shards; `merge_train` for the queue changes; `analytics` for the reads.

## Decisions

- **What CI uses decides; declarations bound the worst case** (16 Nov 2026, amending the next
  decision; [ADR-0353](../../docs/decisions/0353-ci-jobs-start-by-what-ci-uses-within-a-bounded-worst-case.md)).
  Measured on production the same evening:
  - Reservations held 50–56 of 64 CPUs while the host was 25–30 % busy. Jobs waited a median
    10–11 min to start, against 5 min of work.
  - A one-job run (autoresearch `gate`, 12 s of work) asked for the whole host. Served first in
    line, it kept every job waiting 7–37 min after each push.
  - Pools at twice the host ran five jobs instead of two at ~68 % busy, but CI's processes waited
    for a CPU 10–25 % of 10-second windows at compile peaks.

  Hence measured use plus starting jobs for room, twice the budget for the worst case, the
  kernel's CPU pressure against contention, and jobs that fit passing one that does not, within
  10 minutes. Amended 17 Nov 2026 by the operator (slice A3): three times the budget, and no
  pressure rule (a job pinned to a few CPUs showed pressure while the host idled).
- **The budget counts measured use, not only declarations** (slice A): a job's `cpus` is a
  promise the job does not keep (its tools and containers use what is free), so admitting by
  declarations let four 16-CPU jobs take 18.8 min each. Measuring per job would need each
  container mapped to the job that started it; measuring the CI and engine groups of every
  sandbox together needs none, and the reservations still cover jobs that just started.
- **Docker networks: a larger pool and a wait, not a rerun** (slice A): Docker's default pools give
  31 networks per engine, which side-by-side test runs use up; /24 networks out of the same ranges
  give 4,096 without touching an address the sandbox did not already give Docker. The wait for 32
  free is the budget's part for an engine that was started with the old pools or leaks networks.
- **A flaky test is told to the project's gate, not turned into a pass by Yulius** (slice B,
  [ADR-0356](../../docs/decisions/0356-a-test-failing-on-two-pull-requests-is-quarantined-with-an-issue.md)):
  passing a failed job whose failing tests are all quarantined would need every failure read from
  its output, and a build error or a runner's own failure missed there would pass a run that gates
  a merge. The gate knows what it ran. The environment, not a file in the job's home, carries the
  list: the job could have linked such a file elsewhere. Two pull requests, not one pull request's
  repeated runs: only an unrelated change shows the failure is not the change's.
- **A merge candidate is decided by its own run** (slice C,
  [ADR-0357](../../docs/decisions/0357-a-merge-candidate-is-decided-by-its-own-run.md)): its run
  tested the exact merged result, so a passed one merges without the members' own runs, and an
  errored or cancelled own run is the candidate's to decide. A run that holds a failed member
  decides no one (its tree can never land) and is still cancelled: merging the members before it
  would leave the integration branch at an untested tree. Derived files: Yulius cannot run a
  project's formatter or generator where it builds candidates, so it saves the cycle it can,
  telling the member that breaks a combination once the others pass without it.
- **`YULIUS_CI_LANES` keeps its default** (slice A): it bounds checkouts per sandbox (each lane
  keeps its own job folders and caches, a disk cost), and the budget now decides the load, so a
  lower default would only make a small project's runs queue behind each other for nothing.

## Evidence

### Slice A: CI never takes more than the host has

- HBA-001, HBA-002: `yulius_domain::resources::{Capacity::ci_budget, admit}`; unit
  `resources::tests::ci_jobs_are_admitted_by_what_ci_really_holds` (renamed by ADR-0353 to
  `ci_jobs_are_admitted_by_what_ci_uses_within_a_bounded_worst_case`; 64 CPUs give 56 for CI; four
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

### Slice A2: the operator's CI admission fix (its own pull request, first)

- The instance's operator built production 1d52db59 (16 Nov 2026, 21:57 UTC): main with #1357,
  #1356 (slice A) and a fix to CI admission, because slice A's rule left the host 25-30 % busy
  while jobs waited a median 10-11 min to start. The same commit on top of dev is applied here as
  it came (`git am`, commit 94d4fc2fb, author the operator): HBA-002 amended, ADR-0353. It
  replaces this task's own `YULIUS_CI_OVERCOMMIT` carry-forward, no-convoy rule and default-share
  cap, which were never pushed (kept in this branch's history before b4447577f).
- To deliver it alone, b4447577f sets the branch to `dev`'s tree (slice B's reruns, minutes and
  CI-load work are held back); slice B comes back by reverting b4447577f once this is merged.
- Checks run on this head: `cargo fmt --check`; `cargo clippy -D warnings` on `yulius-domain`,
  `yulius-sandbox`, `yulius-app`, `yulius` (all targets); the resources, `ci::` and `cgroup`
  unit tests (116 passed); the whole `local_ci::` suite (79 passed).
- Its review (Fable 5.2, head e092c94bb). Request changes, all in the patch. Answered:
  - Major: the slot taken after admission had no test that failed without it.
    `local_ci::a_job_that_cannot_start_holds_no_slot` (two slots, a CI budget of 3: `a` with 2
    CPUs runs, `b` with 3 cannot start, `c` with 1 runs beside `a`). With the slot taken before
    admission (the two lines swapped back) it fails: `b` holds the slot and `c` never starts.
    The priority's own order is held by `jobs_that_fit_pass_one_that_does_not` and
    `CiPriority::of`'s doctest; `exec.rs` passes `CiPriority::of(&run.reference)` (read, no test
    of a merge train's job before a pull request's through the executor).
  - Minor: the decision index listed 0352 twice, 0353 before 0350. Fixed.
  - Minor: `architecture.md` described the old rule and wait text: the job waits section, its
    diagram and the pressure line say what the code does.
  - Minor: a job that ended in its first minute still counted as starting.
    `a_started_job_counts_until_the_readings_show_it` drops one of four starts and a fifth starts
    at once; without `started.retain` in `Reservation::drop` it fails.
  - Minor: pressure was untested from the port to the decision.
    `local_ci::a_job_waits_while_cis_processes_wait_for_cpus` (25.0 %: the job waits with "CI's
    jobs waited for a CPU 25.0 % of the last 10 seconds; a job starts at 10 % or less", then
    starts at 5 %); with `ci_measured` reading no pressure it fails. Unit
    `cgroup::tests::cpu_pressure_reads_some_avg10`: no `some` line, `nan`, negative, no `avg10`,
    over 100 %.
  - Minor: a counter that went back read as 0 CPUs. It is now a new base, not settled until a
    window passed (`what_ci_uses_is_measured_over_a_recent_window`).
  - Minor: a failed read kept the old pressure. It is forgotten
    (`ci_pressure_is_the_most_any_sandbox_waited`).
  - Minor: the 10-minute bound holds back the jobs of its priority and lower, not every job: the
    ADR and the constant's comment say so (the priority order is the decided design).
  - Direction (memory): the worst-case bound is on CPUs; ADR-0353's consequences and
    `operations.md` say that `memory_gb` counts at a job's start and measured memory after.
  - Nits: "a job starts at 10 % or less"; the start allowance's comment; a raised job limit wakes
    the waiting jobs at once (`ci_resize_slots`).
  - HBA-005 says what the bounded worst case allows. `scripts/dev mutants` on this head: 87
    listed, 78 caught, 0 missed, 0 timed out, 9 unviable, 0 not reached (1426 s).
- HBA-002 as amended, clause by clause: no contention (`Admission::Contended`, unit
  `ci_jobs_are_admitted_by_what_ci_uses_within_a_bounded_worst_case`,
  `a_started_job_counts_until_the_readings_show_it`, `local_ci::a_job_waits_while_cis_processes_wait_for_cpus`);
  the bounded worst case (the same domain unit); room beside measured use and recent starts
  (`a_started_job_counts_until_the_readings_show_it`,
  `local_ci::a_job_waits_while_ci_uses_more_than_it_reserved`); the slot after admission
  (`local_ci::a_job_that_cannot_start_holds_no_slot`); the order and passing
  (`jobs_that_fit_pass_one_that_does_not`).
- The last round (Opus 5.5, head 0199e93c2) approved: the answers hold (117 unit tests and 81
  `local_ci` tests on that head; the slot-order mutation caught). One pre-existing minor
  (`ci_use` sums the `ci` and `engine` groups and skips one it cannot read) is issue #1369.
  Merged as #1366 at 0199e93c2 on green Yulius CI; promotion to `main` is asked of a person.

### Slice A follow-up: round 3's minor and nit

- Superseded by ADR-0353 (the operator's fix, merged first): its own count of recent starts
  ("what jobs started in the last minute may still bring") replaces the one below, which was
  not merged.

- Jobs admitted after a reading add to it: `CiAdmission` keeps each running job's admission
  time, and CI holds the larger of the reservations and the measured use plus the reservations
  admitted after the reading the rate ends at. Unit
  `resources::tests::jobs_admitted_after_a_reading_count_on_top_of_it`: one job reserved 16 and
  measured at 40, two of five 8-CPU jobs start (56 of 56), and a later reading does not count
  them twice. Without the addition (the filter made empty) it fails: five start.
- The first network is 172.17.1.0/24 only after the sandbox's next start: ADR-0350,
  `operations.md` and `changes.md` say so.

### Slice B (first pull request): failures outside the change, CI minutes

- OFC-001: `yulius_domain::ci::infra::{infrastructure, Infra, JobEnd}` (doctests; units
  `the_infrastructures_own_words_at_the_end_count` and `a_failure_of_the_change_is_never_the_infrastructures`,
  see review round 2 below). `App::ci_job` runs the job again once in the same slot when
  `ci_infrastructure` says so (this run's own output, whether a failing test was kept, the
  sandbox's OOM count before and after), saying so in its output; passed then, its reason says
  why it ran twice; the infrastructure again, it ends error saying so. Migration 0064 keeps
  `infra`, `infra_ms` and `infra_end`; a requeue clears them. `local_ci` with real jobs:
  `an_infrastructure_failure_runs_the_job_again_once`,
  `a_job_runs_again_once_and_only_for_the_infrastructure`, `a_job_killed_for_memory_runs_again_once`,
  `a_kill_without_a_kill_for_memory_is_not_run_again`,
  `a_tests_failure_is_never_taken_for_the_infrastructure`, and those of review round 2.
- OFC-003: `merge_queue::Minutes` (unit `ci_minutes_are_counted_by_how_runs_ended`: by outcome,
  the rerun minutes apart, the share 4/21); the per-day tally `ci_minutes`, written in
  `transition_ci_run` with the run's new state in one transaction, read by `CiStore::ci_minutes`
  (store unit `a_runs_end_adds_to_its_days_minutes`: passed with 2 min thrown away, failed after
  a thrown-away first run, failed only for the infrastructure, cancelled, a run cancelled before it
  started adds nothing, the rows outlive the runs' prune, the range);
  `GET /api/analytics/merge-queue` gets `ci_minutes` and `ci_minutes_by_day`
  (`analytics::the_merge_queue_read_counts_ci_minutes_by_outcome`: 10 min each passed, failed
  outside and cancelled, 5 min run again, share 0.667, another sandbox's failed run only for the
  admin). The Pipeline tab's Merge queue card shows "CI minutes" and "Outside the change" against
  the 15 % target (`pipeline.e2e.ts`).
- ADR-0354; architecture (CI job states), `api.md`, `changes.md`.
- How to read it a week later: `GET /api/analytics/merge-queue?sandbox=<id>&from=&to=`,
  `ci_minutes.outside_share` (target at most 0.15; 16 Nov in quartio-v2: 114 cancelled and most
  of 526 failed of 1,228 minutes) and `ci_minutes.rerun`; the jobs whose reason starts "passed
  when run again".
- Review round 1 of these (Fable 5.2, head 98481f5bb). Request changes, all own code. Answered
  (the code came back on top of the operator's admission fix, ADR-0353, after it merged):
  - Major: a failing test was run again when the output named a Docker error the job went on
    from. The classifier reads only this run's own output: kept failing tests' blocks must all
    name the infrastructure; without blocks, the words must be in the last 20 lines with no
    test's failure after them. Unit cases (a survived Docker line, then a test's failure, in the
    output and in the blocks; two blocks that both name it) and
    `local_ci::a_tests_failure_is_never_taken_for_the_infrastructure` (first part: failed after
    one run).
  - Major: a test failure on the second run was reported as an infrastructure error (the first
    run's words were read again). Only what follows the rerun line counts; the same test's second
    part: failed, no reason, two runs.
  - Major: a run that failed in the change counted as failed outside it. `ci_jobs.infra_end`
    says whether the job's end itself was the infrastructure's; only then is a run
    `failed_outside` (store unit `a_runs_end_adds_to_its_days_minutes`: a job that failed its
    second run after a thrown-away first counts as failed).
  - Major: the minutes were read from runs, which are pruned to 50 per sandbox. A run's end now
    adds its minutes to a per-day tally (`ci_minutes`, written in `transition_ci_run`); the store
    unit reads the same rows after `prune_ci_runs(0, …)`, and the analytics test reads the tally.
  - Minor: a SIGKILL without an OOM kill: `local_ci::a_kill_without_a_kill_for_memory_is_not_run_again`
    (failed, one run). The second run's own time limit and the first run's dropped use are in
    ADR-0354. After a full address pool the second run waits for network room first.
  - Minor: a job admitted before another sandbox's reading (the follow-up of slice A) is
    superseded by ADR-0353's count of recent starts. Nits: "…when it was run again, after … in
    its first run" names both kinds when they differ; the Plan's ticks.

### Slice A2: CI wait job-minutes per hour and host busy %

- `yulius_domain::resources::{LoadMinute, LoadHour, busy_permille, load_by_hour}` (doctest;
  unit `the_hosts_minutes_add_up_by_hour`); `SystemProbe::cpu_ticks` (`/proc/stat`'s `cpu` line,
  idle and iowait not busy; unit `cpu_ticks_are_read_from_proc_stat`); `App::ci_record_load`
  once a minute from the dispatcher (busy since the last reading, CPUs reserved and measured,
  `ci_jobs_waiting`), kept 30 days in `ci_load` (migration 0064);
  `GET /api/analytics/ci-load` (`analytics::the_hosts_ci_load_is_kept_each_minute_and_read_by_hour`:
  two readings in one minute make one, 30 % busy from the ticks, a reversed range 422); the
  Pipeline tab's CI load card (`pipeline.e2e.ts`, 18 of 18).
- Before (the person's measurement from production, 16 Nov 2026): 648 job-minutes waited in hour
  18 and 904 in hour 19, the host 25-30 % busy while jobs held 50-56 CPUs. After: read
  `GET /api/analytics/ci-load?from=&to=` a week later (hours, `wait_job_minutes`,
  `busy_percent`, `held_cpus` against `measured_cpus`).
- Checks run on this head: `cargo clippy --workspace --all-targets -D warnings`; the unit tests
  of `yulius-domain`, `yulius-app`, `yulius-sandbox`, `yulius-store` (1,487 passed); `local_ci`,
  `analytics`, `limits`, `merge_train` (149 passed, then 150 with the CI load test);
  `pipeline.e2e.ts` (18 passed). Security scan: Vidoc refused the key (exit 2, not run).
- `scripts/dev mutants` on these lines: 226 listed, 119 caught, 0 missed, 0 timed out, 10
  unviable, 97 not reached (bound 40 min, 2402 s; not reached: 48 in the store's SQL, 26 in the
  classifier, 13 in the analytics route, 6 in the domain resources, 4 in the route table). Four
  of the classifier's by hand: the test failure after the words, every block naming the
  infrastructure, and a SIGKILL needing an OOM kill were caught; reading the whole output instead
  of its last lines was not, and `only_the_infrastructures_own_words_count` now has that case (a
  Docker error long before the job's own compile error), which catches it.
- Review round 2 of the reruns, minutes and load (Fable 5.2, head acbd80a6e). Request changes,
  all own code. Answered:
  - Major: a Docker error the job went on from, then a failure in Go's, pytest's or jest's words,
    was run again. The classifier now reads only the run's last 3 lines (Yulius's own left out)
    and refuses any line after the words that reads as a failure in any runner's words (`fail`,
    `panic`, `assert`, `not ok`, `timeout`, `timed out`, `error[`). Units with Go, pytest, jest,
    TAP, nextest `TIMEOUT`, a panic, an assertion and a compile error after each of Docker's
    words; `local_ci::a_failure_in_any_runners_words_is_never_run_again` (Go, pytest and jest
    shapes: failed, one run).
  - Major: real kept blocks (with their blank lines) were never classified, and a block could
    quote Docker's words. A job with any failing test kept is now the change's, whatever the
    block says: `local_ci::a_kept_failing_test_is_never_run_again` (nextest's block with its blank
    lines, its test failing on the address pool: failed, one run). A test failing on a full pool
    therefore costs a cycle; with 4,096 networks per engine (ADR-0350) that is rare.
  - Major: the wait for network room before the second run could not be cancelled or bounded.
    It races the cancellation and a 10-minute bound, flushes the job's output first and says
    what it waits for; cancelled, the job ends cancelled with no second start; past the bound it
    ends error for the infrastructure. `local_ci::the_wait_before_a_second_run_can_be_cancelled`.
  - Major: "only the second run's own output counts" had no test.
    `local_ci::the_second_run_is_judged_on_its_own_output` (the address pool, then a plain compile
    error: failed, no reason, two runs).
  - Minors: ADR-0350's bullet about jobs arriving in one window is dev's text again, pointing to
    ADR-0353; the Evidence above names what exists; `ci_jobs_waiting` has a store unit
    (`jobs_waiting_to_start_are_queued_jobs_of_running_runs`); a second start that cannot be
    made after a thrown-away run ends error for the infrastructure (`infra_end`); ADR-0354 says
    what the wait holds and for how long. The lost host's rerun is held at the classifier
    (`JobEnd::Lost`); the executor's one-line mapping from `Ended::Lost` has no test of its own.
  - Nits: the decision index's order; both runs' stages stay on the job's span (ADR-0354); the
    tally and the run's state in one transaction; `Infra::parse` removed; the busiest hour shows
    its day.
- Review round 3 of the reruns, minutes and load (Fable 5.2, head 8ce212c02). Request changes,
  all own code. Answered:
  - Major: `the_wait_before_a_second_run_can_be_cancelled` was flaky (the engine filled before the
    job's first look for room). It fills once the first run has started (its attempts file);
    the two wait tests passed 6 of 6 under `--stress-count 6`.
  - Major: a failure in Go's, pytest's or jest's words before Docker's words at the end was run
    again (the scanner keeps only nextest's and libtest's blocks). `FailureScanner` now also sees
    any line that reads as a failure in a runner's or a tool's words
    (`ci::infra::reads_as_failure`, doctest; unit `a_failure_in_any_runners_words_is_seen`), and
    such a run is the change's; the socket counts only in the Docker client's own words.
    `local_ci::a_failure_before_dockers_words_is_never_run_again` (Go's lines then the client's
    socket line last; pytest's then a container's log naming the socket: failed, one run).
  - Minors: a failing test now wins over a kill for memory and a lost host (unit); past the bound
    the reason says the engine had no room and no second run was made
    (`local_ci::a_second_run_that_finds_no_network_room_is_not_made`, the bound shortened to
    0.5 s through `CiSettings::rerun_network_wait`); a second start that cannot be made keeps its
    reason; failure lines without the old words (`1 error in 0.05s`, `ERRORS!`, mocha's `Error:
    expected`, a segmentation fault) refuse the rerun (unit), while `make`'s exit report does not.
    The second start that cannot be made and the executor's mapping of a lost host have no
    executor test (a fake session host failing only its second start was not built); both are
    one branch each, read.
  - Nits: a cancellation that came while the first run ended starts no second one; the
    Evidence's test names.
- Review round 4 (Opus 5.5, head e73c91224) approved. Its minors, fixed after the approval within
  the allowance: the socket counts only when the Docker client's words begin the line (after an
  `error:`), so `app-1  | dial unix …` is a container's log (unit cases); a second start that
  fails to spawn is the infrastructure's too, with its reason; mocha's `10 failing` is no passing
  summary (`" 0 failing"` as a whole word); a failure on an unfinished last line is seen (unit
  `a_failure_on_the_unfinished_last_line_is_seen`). Still read only, not tested through the
  executor: a second start that cannot be made, and the mapping of a lost host.

### Slice B (second pull request): flaky tests quarantined with an issue

- OFC-002: `yulius_domain::ci::flaky::{is_flaky, nextest_filter}` (doctests; units
  `a_test_failing_on_two_pull_requests_is_flaky`: one pull request's three runs, a failure older
  than 48 hours, the integration branch failing it and another branch's failure do not count;
  `the_quarantine_is_a_nextest_filter`: a name with `)` or `|` is left out). Migration 0065
  (`ci_test_failures`, `ci_quarantine`); store unit `failing_tests_and_the_quarantine_are_kept`
  (by job and time, forgotten past the keep, quarantined once, per sandbox, released).
  `App::ci_note_failing_tests` at a failed job's end, `ci_quarantine` (the issue through the
  GitHub port, audited `ci.test.quarantined`), `ci_quarantine_env` in every job's plan,
  `ci_release_quarantines` in the hourly prune (`GitHubPort::issue_open`; audited
  `ci.test.released`); unit `the_issue_names_the_test_its_pull_requests_and_how_it_ends`.
  `local_ci::a_test_failing_on_two_pull_requests_is_quarantined_with_an_issue` (real Git and
  jobs, a fake GitHub): the test fails in #1's run (nothing quarantined), then in #2's: one issue
  "Flaky test quarantined: app tests::flaky", #2's output says it is quarantined (issue #101);
  dev's next job gets `YULIUS_CI_QUARANTINE_NEXTEST=(binary_id(app) & test(=tests::flaky))`;
  the issue closed, the release leaves no quarantine.
- This repository's gate leaves the quarantine out: `cargo xtask test` adds
  `-E "not ($YULIUS_CI_QUARANTINE_NEXTEST)"` and prints what it left out.
- ADR-0356; architecture (CI job states), `api.md` (the job's variables), `operations.md`,
  `changes.md`.
- Review round 1 of the quarantine (Fable 5.2, head dd2e2cc75). Request changes, all own code.
  Answered, each test failing with its fix undone by hand (seven mutations, all caught):
  - Major: two jobs failing the test at once filed two issues. One quarantine is decided at a time
    (`CiState::quarantine`), the list read again under the lock; the second job names the issue.
    `local_ci::two_jobs_at_once_file_one_issue` (the fake GitHub holds the first filing).
  - Major: a release kept the test's failures, so one more failure quarantined it again.
    `release_test` forgets them (store unit; the main test's fourth pull request files nothing).
  - Major: a chained pull request and its predecessor counted as two changes. Each failure keeps
    its commit; two pull requests count only when Git's merge base of their heads is on the
    integration branch (domain unit; `local_ci::no_quarantine_when_the_integration_branch_or_a_shared_commit_fails_it`,
    which also covers the integration branch failing the test).
  - Major: a filter naming a binary the head lacks fails nextest (probed: "operator didn't match
    any binary IDs"). `YULIUS_CI_QUARANTINE_NEXTEST` names test paths only; `cargo xtask test`
    builds its own filter from the names and the binaries `cargo nextest list --list-type
    binaries-only` lists, with `binary_id(=…)` (unit
    `quarantined_tests_are_left_out_in_the_binaries_this_head_builds`).
  - Major: the cap of 10 counted kept names only. The omitted count is added
    (`failures::omitted_tests`, doctest; `local_ci::no_quarantine_without_its_issue_or_for_a_broken_job`:
    twelve 9 KiB failures, four kept).
  - Major: the app layer's other paths. The main test now covers the reason, each job running
    once, a third pull request filing no second issue, an open and an unreadable issue keeping
    the quarantine, and one failure after the release; the issue failing to be filed leaves
    nothing quarantined and the reason the job's own; `GitHubClient::issue_open` has a client
    test (open, closed, 404, 410).
  - Minors: the reason only when a test was quarantined; names checked by
    `flaky::quarantinable` (no backtick, `@`, control character or leading `/`; unit); `=` in
    the binary match. Nits: the release audited only when it was recorded, with the job; a
    sandbox gone or removed loses its quarantines; ADR-0356's recipe with `${…:+…}`;
    `job_key` is read by the release's audit.
- Review round 2 of the quarantine (Fable 5.2, head 49b46c83d). Request changes, own code.
  Answered:
  - Major: two chained heads that both merged a newer `dev` commit have two merge bases, and
    plain `git merge-base` named the `dev` one (probed by the reviewer with Git 2.53), so they
    read as unrelated; and the adapter turned every Git error into "no base", read as unrelated.
    `GitPort::merge_bases` (`merge-base --all`, an error stays an error; `yulius-git` test
    `merge_bases_lists_every_best_common_ancestor`: a criss-cross history's two bases, none for
    an orphan, an unknown revision an error); two heads are one change unless every base is on
    the integration branch. `no_quarantine_when_the_integration_branch_or_a_shared_commit_fails_it`
    now merges a newer `dev` commit into both chained branches (both fail, no issue) and asks
    about a head the clone lacks (no quarantine). Undone by hand, both cases fail.
  - Nits: names at most 200 bytes (the title within 256; unit at 200 and 201); the line for a
    test already quarantined says a job started before it, or a gate not reading the variable,
    still runs it; the lock is per sandbox (`CiState::quarantine_locks`); `cargo xtask test`
    prints its listing step and shows its compile output.
- Checks run after review round 2: `cargo fmt --check`; `cargo clippy --workspace
  --all-targets -D warnings`; rustdoc of `yulius-app`, `yulius-domain`, `yulius-git` with
  warnings denied; the unit tests of `yulius-domain`, `yulius-store`, `yulius-app`,
  `yulius-github`, `yulius-git`, `xtask` (1,430 passed); `local_ci::` and `analytics::` (105
  passed); `yulius-domain` doctests (24 passed).
- Review round 3 of the quarantine (Fable 5.2, head 2eb0fa696) approved: no blocker, major or
  minor. Its nit (the hourly release asks GitHub inside the CI poll loop, one request per
  quarantine) is taken off the loop in slice C's pull request, not after the approval.
- Leftovers of #1373's and #1377's reviews: an older client's `docker:` prefix before its socket
  words counts (unit case in `a_failure_of_the_change_is_never_the_infrastructures`, failing
  before the fix); the CI's CPU pressure is logged at `debug` (`cpu_pressure_permille`) where it
  is read; stale pressure wording in `operations.md`, ADR-0353's consequences, the spec's
  Decisions and two doc comments; ADR-0353's trailing blank line. Still read only, not tested
  through the executor: a second start that cannot be made, and the mapping of a lost host.
- Checks run on this head: `cargo fmt --check`; `cargo clippy --workspace --all-targets -D
  warnings`; the unit tests of `yulius-domain`, `yulius-store`, `yulius-app`, `yulius-github`
  (1,300 passed); `local_ci::` and `analytics::` (102 passed); `yulius-domain` doctests.
  `scripts/dev mutants`: 108 listed, 60 caught, 0 missed, 0 timed out, 48 unviable, 0 not
  reached (bound 40 min, 1613 s).
- Checks run after review round 1 (head after the fixes): `cargo fmt --check`; `cargo clippy
  --workspace --all-targets -D warnings`; rustdoc of `yulius-app` and `yulius-domain` with
  warnings denied; the unit tests of `yulius-domain`, `yulius-store`, `yulius-app`,
  `yulius-github`, `xtask` (1,314 passed); `local_ci::` and `analytics::` (105 passed);
  `yulius-domain` doctests (24 passed).

### Slice C: the merge queue does not throw work away

- MQW-001: `member_left` leaves a member only for a failed own run; an errored or cancelled one
  stays (`merge_train::a_member_whose_own_run_is_cancelled_or_errors_stays_in_the_candidate`:
  both own runs ended so, no dissolve, the candidate's run goes on, passes, both merge; undone, it
  fails). A candidate formed again takes its tree's passed run (`App::ci_reuse_for_candidate`,
  `ci_reuse_tree` for train references with no `paths`-skipping source;
  `merge_train::a_candidate_formed_again_reuses_its_trees_passed_run`: after a busy member's wait
  limit the same candidate forms again, passed at once with "not run again", one candidate run
  executed; undone, it fails). A failed member still leaves
  (`a_member_whose_own_run_fails_leaves_the_candidate`, unchanged).
- MQW-002: `drive_train` no longer waits for own runs
  (`merge_train::a_passed_candidate_merges_while_a_members_own_run_goes`: merged while its own run
  is held running, which never passed; `a_stuck_own_run_holds_nothing_back`: both merge, nothing
  dissolved, one candidate run; with the wait put back both fail).
- MQW-003: `SandboxTrains::suspect`, set by `bisect` when one member is left out, proven by a
  passed train of exactly the others on the same base (`Suspect::proven_by`, unit
  `a_suspect_is_proven_only_by_the_others_on_the_same_base`); `tell_suspect` ends its armed merge
  naming both runs. `a_red_candidate_is_bisected_and_the_breaking_pull_request_told` now expects
  `train/1+2+3 failed`, `train/1+2 passed` and no `train/3`;
  `a_member_left_out_is_blamed_only_when_the_others_pass_without_it` (the first member fails
  alone on the new base: it is told, the other merges). Undone (no suspect; the others' check
  dropped), the tests fail.
- MQW-004: `Dissolved::MemberFailed`; `QueueNumbers::dissolved_recent` (unit
  `each_dissolved_candidate_is_listed_with_its_reason`); `dissolved_recent` in
  `GET /api/analytics/merge-queue` (`analytics::the_merge_queue_read_counts_candidates_from_the_audit_log`);
  the card's list (`pipeline.e2e.ts`, 18 of 18).
- The quarantine review's round-3 nit: the hourly release runs off the CI poll (spawned).
- ADR-0357; architecture (merge queue, diagram), `api.md`, `product.md`, `changes.md`.
- Checks run on this head: `cargo fmt --check`; `cargo clippy --workspace --all-targets -D
  warnings`; rustdoc of `yulius-app`, `yulius-domain`, `yulius-contracts`; the unit tests of
  `yulius-domain`, `yulius-store`, `yulius-app`, `yulius-github`, `yulius-git`, `xtask` (1,432
  passed); `local_ci::`, `analytics::`, `merge_train::` (144 passed); `pipeline.e2e.ts` (18
  passed). `scripts/dev mutants`: 54 listed, 24 caught, 0 missed, 0 timed out, 30 unviable, 0 not
  reached (724 s). Hand mutations: see each requirement above.

- Review round 1 of slice C (Fable 5.2, head d10b67896). Request changes, all own code but one.
  Answered:
  - Blocker: the card's list padding (18px) was off the spacing scale, so the repository check
    failed and CI stopped in 4 s. 16px; `scripts/check_repository.py` passes.
  - Major: nothing held "a failed own run beats a passed candidate".
    `merge_train::a_failed_own_run_beats_a_passed_candidate` (#2's own run held, the candidate
    passes, then #2's run fails: #2 is not merged, `member_failed`, its armed merge ends, #1
    merges on a candidate formed again). The failure is now read at each pass's start and again
    before each member's merge (`own_run_failed`); undoing both reads fails the test; undoing
    only the second fails it 1 time in 40 (the reviewer's count: which read catches it depends
    on the background pass a candidate's end starts; no test times it, ADR-0357 says so).
  - Major: nothing held the reuse guard for candidates. `Project::Paths` (a job with `paths =
    ["never/**"]`) and `a_run_that_skipped_a_paths_job_is_no_source_for_a_candidate`: the
    candidate formed again gets a run of its own. Undone (`Branch` only), it fails.
  - Minor: the failed own run is read at the member's armed head now (a clean merge of the
    integration branch with the same arming moves it), not only the head it joined with.
  - Minor: the suspect keeps the others' heads, so another #2 head proves nothing (unit case).
  - Minor: the told message says what was seen and its own run's state, not "combine badly";
    the audit is written only when the merge is dropped.
  - Minor: the records: ADR-0357 names the exception to ADR-0285's base-merge rule and the
    fallback push without a verdict; ADR-0285's section is marked amended; MQW-001 and the
    acceptance line say what is cancelled and why.
  - Nits: stale texts; the dead `merging` arm in `train_run_ended`; "the own run of #N … ended
    failed" counts as `member_failed`; the product summary.
  - Pre-existing: a bisected candidate's second half reruns the red tree: #1384.
- Review round 2 of slice C (Fable 5.2, head eebe8f366) approved; its CI run passed (quality,
  e2e; security skipped, Vidoc 401). Its minor (a deterministic test of the read before each
  merge, with a pause point) and two nits (an assertion that races the background pass, one long
  ADR line) go into slice D's pull request, not after the approval.

### Slice A3: the operator's amendment to the admission (its own pull request, first)

- The instance's operator built production 915ebb6a (17 Nov 2026, 01:41 UTC): main 7a705569 plus a
  fix to the admission of #1366, measured on production that night. Applied here as it came
  (`git am`, commit 9a7e9421b, author the operator), on top of `dev` with #1373:
  - the `cpu.pressure` rule is gone (`Admission::Contended`, `CI_PRESSURE_LIMIT`): a project's CI
    that pins its workers to its own CPUs showed 15-20 % pressure in its `ci` group while the host
    was 45 % busy, and 117 jobs of every project queued; the reading stays, for diagnosis;
  - `CI_OVERSUBSCRIPTION` is 3, not 2: at twice the budget the worst case bound all night (108 of
    112 with the host 60-77 % busy).
  ADR-0353 has two amendments; HBA-002 and HBA-005, operations, architecture, changes and the
  decisions index say the same. `local_ci::cpu_pressure_holds_no_job_back` replaces
  `a_job_waits_while_cis_processes_wait_for_cpus`.
- The decisions index got the amended 0353 line twice from the union merge; it is kept once, in
  order.
- Checks run on this head: `cargo fmt --check`; `cargo clippy --workspace --all-targets -D
  warnings`; the unit tests of `yulius-domain`, `yulius-app`, `yulius-sandbox` (1,366 passed);
  `local_ci::` and `analytics::` (101 passed).
