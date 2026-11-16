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
  larger than the budget runs alone.
- **HBA-003** Docker networks are part of the budget per engine. Each engine gets address
  pools of /24 networks within Docker's own default ranges (4,096 networks instead of 31). A job
  of a sandbox whose engine has fewer than 32 free networks waits, saying so; when none of its
  sandbox's CI jobs runs, unused networks are pruned first.
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

- [ ] Slice A: the host budget sampled from every sandbox's CI and engine groups
- [ ] Slice A: jobs admitted by the larger of reserved and measured use
- [ ] Slice A: Docker networks per engine in the budget, larger address pools
- [ ] Slice A: a sandbox's weight follows the CPUs its jobs hold
- [ ] Slice A: documents, ADR and review
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
