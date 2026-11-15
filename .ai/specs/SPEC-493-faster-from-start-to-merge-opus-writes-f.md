---
yulius-task: 839be374-cbd1-4e24-82d1-1c18faed4c18
status: in-progress
---

# SPEC-493: faster from a task's start to its merge

Status: in-progress
Date: 2026-11-15
Owner: Yulius task #472
Scope authorization: the person's decisions of 2026-11-13 and 2026-11-14, as given in the request of
task #472 (by MrFishPL): the model policy and ten fixes, each its own pull request. **Fast mode is
out of scope**: Claude Code's fast mode is not turned on and nothing is built for it.

## Goal

A task gets from its start to its merge sooner, without a weaker review where the review finds
defects. Opus 5.5 writes, Fable 5.2 reviews the first two rounds, Yulius CI tests a head while its
review runs, an armed pull request whose base moved waits one CI run, a job tests only what it is
for, a small follow-up keeps its approval, a review survives a usage limit, mutation testing runs
once with one command, and two tasks learn at once that they edit the same files. Each fix names
the number a person reads on the instance a week later.

## Context

### What was measured (production, pull requests merged 12 and 13 Nov 2026)

From the request; the person has the full tables. 42 pull requests here, 60 in quartio-v2.

- Start to merge, median: 2.1 h here, 3.2 h in quartio-v2. The review loop is 64 % and 52 % of the
  hours; after the last approval until the merge 11 % and 29 %.
- A Fable 5.2 reviewer found 8 of 8 blocking defects in a blind replay of four real changes; an
  Opus 5.5 reviewer 6 of 8, and approved two changes that needed fixes.
- Fable's weekly allowance was used up on 21 of 24 subscriptions within about three days. Authors
  made about 57 % of Fable's output tokens and 70 % of its cached reading; reviewers 37–38 % and
  about a quarter.
- Opus authors answered in about 6 s against 14 s and used about half the model time per pull
  request. Whether Opus writes worse code was not measured.
- Where more than 100 lines of product code changed since the reviewed commit, 20 of 63 re-reviews
  asked for changes, two thirds of them for a real defect.

### What reached `dev` since 2026-11-13 (read first, as asked)

52 commits (`git log --since=2026-11-13 origin/dev`, head `716458bab`): the audit (#465, SPEC-483,
slices 11 to 28), the Superagent work (#469, SPEC-487), #470 (SPEC-484/485/486), #473/#475
(SPEC-490) and #474 (SPEC-491). What bears on this task:

- **Of the ten fixes, half of fix 2 is done.** `task_done::a_stop_settles_a_run_no_server_holds`
  was fixed by #1313 (2026-11-15, issue #1190 closed: the test read `kill(pid, 0)`, which a
  killed host not yet reaped still answers; it now reads the process's state as the product
  does). `cd.e2e.ts` "a refusal after its view was left is a toast…" was fixed by #1309
  (2026-11-15, issue #1171 closed: the CD page read its runs before the server answered a deploy
  started there, then waited for its 30 s poll; a deploy answered after its page was left now
  makes a mounted CD page read again). Still open: the notes test (issue #1246, seen again in a
  merge candidate's run of #1316) and `an_unreachable_yulius_exits_3` (no issue; noted in
  SPEC-478).
- **Nothing of the other fixes is done.** `DEFAULT_CLAUDE_MODEL` is still `claude-fable-5-2`; no
  text names a reviewer's model or effort; no hook sees the Agent tool; nothing records a review;
  no mutation tool is pinned; a scope overlap is told to the claiming agent only. Both defects of
  fix 10 still hold by the code (the queue's left-over mark is now `agent_cli::Queue::turn_closed`,
  run only for a `turn_duration` record; `viewer_input_allowed` is at `instance.rs:2356` and asks
  only whether the policy is in force).
- **One rule says which jobs test a commit** (#1254, `App::ci_gate_for_head`, SPEC-483 CGR-001):
  fix 5 extends that rule, it adds no second one.
- **A usage limit met by a dynamic workflow's agent is the session's** (#1255, SPEC-486,
  ADR-0326): the harness follows the agents' transcripts, the pool moves the task, and the new
  process is told to resume the workflow. A plain Agent-tool subagent's limit was left out there,
  because "the session's own record follows within the same turn". With fix 1 that no longer
  holds: an Opus session's Fable reviewer is refused while the session itself goes on. Fix 7
  builds on SPEC-486's parts.
- **A pull request delivers the finished part of a plan** (#1261, SPEC-484, ADR-0325) and **an
  armed merge lives with the run that armed it** (#1263): fix 3 opens the pull request earlier
  through the same `try_auto_open_pr`.
- **Done needs a pull request that would merge** (#1294, SPEC-491, ADR-0332): unchanged by an
  earlier pull request, since the plan's steps are still open then.
- **The Superagent's model**: SPEC-487 slice 6 lets it change a task's model and effort; the
  Superagent itself runs the default model, or what a person types with `/model`.

### How the parts work today (facts, with their code)

- **Default model.** `yulius_domain::harness::DEFAULT_CLAUDE_MODEL` (ADR-0289, ADR-0295: Fable 5.2
  at high). A task record keeps `None` for "Default"; every launch applies the constant
  (`plan_launch`, `plan_cockpit`). Derived from it: the first of `OFFERED_CLAUDE_MODELS` (and
  `models.js`), `capacity::model_family(None)` and `default_family()` (what "limited" and
  "available" mean on the Capacity panel, SPEC-464 UCL-005), `capacity::fallback_of(None)`
  (SPEC-482), `agent_cli::lacks_default` (a Claude Code build that does not know the default never
  runs in a sandbox, SPEC-461 CCM-003), `accounts.rs` (which model is asked about for a family),
  `task_create`'s descriptions.
- **The review.** Rules only: the briefing, the request's definition of done, the catalog skills
  `om-code-review`, `delivery-pipeline`, `om-check-and-commit`, the workflows, this repository's
  `yulius-review`, `yulius-verify`, `yulius-git`, `docs/engineering/workflow.md`, `git.md`,
  `AGENTS.md`, and the tools' descriptions. The reviewer is "a read-only subagent given only the
  diff range, the spec and skill `om-code-review`": any agent type, on the session's model.
  `delivery_merge` takes `reviewed_head` and a free-text summary from the author and keeps them
  in `audit_log.detail`; the head must be the reviewed commit or follow it by clean merges of the
  integration branch only (`only_clean_merges_since`, SPEC-083 REV-001).
- **Claude Code 2.1.325, measured with the real CLI against a local stand-in for the model API**
  (`$HOME/.cache/yulius-task-472/probe`; the request of every agent is logged):
  - An agent definition's `model: claude-fable-5-2` and `effort: high` are what the subagent's
    requests carry (`model`, `output_config.effort`), whatever the session runs.
  - A definition given at the launch (`--agents <json>`) outranks a file of that name in the
    agent's home (`~/.claude/agents`), which the agent can write. A file of that name in the
    repository (`.claude/agents`) outranks both.
  - A `PreToolUse` hook matching `Agent` gets `tool_input.subagent_type` and can answer
    `updatedInput` with another `subagent_type` and prompt: that definition then runs. Its
    `additionalContext` reaches the session.
  - `SubagentStart` carries `agent_id` and `agent_type`; `SubagentStop` adds
    `agent_transcript_path` and `last_assistant_message`. A foreground and a background subagent
    fire both.
  - A usage limit that refuses a subagent fires **no** `SubagentStop`. The subagent's transcript
    gets an assistant record with `apiError: "usage_limit_reached"`; the session gets a
    notification "Agent terminated early due to an API error … (model sent to the API:
    claude-fable-5-2) … send the agent a message (SendMessage) to let it continue".
  - `SendMessage` to that agent resumes it on the same model with all it had read, also after
    the process was ended and started again with `--resume` (what a move to another
    subscription does). `SubagentStart` fires again with the same `agent_id`.
- **Yulius CI.** No webhook: a poll every 60 s tests the integration and production heads and the
  heads of open pull requests (`ci_sync_sandbox`). A branch pushed without a pull request gets no
  run. A commit is tested once whatever reference carries it, and a pull request's run is
  superseded by its own newer head (`yulius_domain::ci::plan`). Yulius pushes a branch and opens
  its pull request at a clean turn's end once every step is resolved, or at `delivery_merge`
  (`try_auto_open_pr`).
- **The merge queue** (ADR-0285, `merge_train.rs`): a pull request's own run always finishes
  first; green and current merges at once; green ones whose base moved go into one candidate
  (`form_train` takes only members whose own run **passed**). Candidates are counted only as
  `audit_log` actions `delivery.train.formed`, `.merged`, `.dissolved`.
- **`[[ci.jobs]]`** (`spec_format/config.rs`): `name, command, matrix, cpus, memory_gb, secrets,
  timeout_minutes, exclusive`. An unknown key rejects the whole `yulius.toml`, and the running
  instance parses the integration branch's file. A job ends skipped only by exit code 78
  (ADR-0133); a skipped job blocks no merge.
- **Usage limits.** The pool policy is `yulius_domain::rotation` (a candidate is below 98 % in
  every window binding the run's model family; `Tier::Tight` below 5 points of room). A turn's
  end moves a session only while no background work runs (`hook_stop`, `background_work_now`).
  The harness follows subagents' transcripts for tokens; a limit there is reported only for a
  workflow's agent (`WorkflowLimits`).
- **Mutation testing.** No tool is pinned (`.config/tools.toml`), no gate step runs one, and the
  only rule is the TDD skill's "mutation check" by thought.
- **Scope claims.** `scope_claim` always stores the claim and answers the overlaps to the
  claiming agent ("nobody else is told", SPEC-012 MSG-002). The other task sees claims only in
  the briefing of its session's start. The web UI shows no claims.

### Scope

In scope: this repository, as ten fixes in separate pull requests, in the request's order. Fix 1
is four pull requests (the default; the reviewer and its rounds; what is recorded; the move before
a review), because one would exceed the size a review reads well.

Not in scope: Claude Code's fast mode; quartio-v2's own part (its jobs' `exclusive` setting, its
gate script, its merge settings: a companion task there); stopping to block on test strength
alone (the person did not take that choice); the rule of fix 6 after a request for changes (the
person's decision is read as holding after an approval).

## Requirements

### Fix 1: the model policy

- **MDL-001** A task that names no model, a Cockpit and both Superagents are launched with
  `--model claude-opus-5-5 --effort high`. `DEFAULT_CLAUDE_MODEL` is the one place; what is
  derived from it (the composer's first model and its "Default" label, the Capacity panel's
  "available", `task_create`'s descriptions, the Claude Code release check) follows it. Nothing
  stored changes: a running default task takes Opus 5.5 at its next launch.
- **MDL-002** A task set to a Fable model still runs it, and while no subscription can run it the
  task falls back to Opus 5.5 as before (SPEC-482). A default task and the Superagents have no
  fallback: they run Opus 5.5, bound by the weekly Opus window. A session its person switched to
  Fable inside it (`/model`, a task's terminal or a Superagent) runs Fable while the subscription
  has credits for it. A usage limit is the limit of the model the refused request asked for
  (Claude Code 2.1.325 records it before each request): refused for Fable's credits, such a
  session fills Fable's window on the subscription, not the window of the model it was launched
  with, and goes on at once with the model Yulius runs it with (its own, or the fallback of a task
  set to Fable while no subscription runs Fable), wherever the pool places it, the same
  subscription included, continuing the cut turn and saying with which model. With a Claude
  Code that writes no such record the limit stays the launched model's.
- **MDL-003** A Claude Code release that lacks Opus 5.5 (the default) never runs in a sandbox, as
  before; one that lacks Fable 5.2 is installed and the gap reported. Naming Fable 5.2 there as
  "the independent review's model" comes with fix 1b, when reviews run it.
- **REV-001** Every task session is launched knowing two reviewer agents, defined by Yulius at
  the launch (`--agents`), out of the agent's reach: `yulius-reviewer` (`claude-fable-5-2`, effort
  `high`) and `yulius-reviewer-opus` (`claude-opus-5-5`, effort `high`), both by full model id,
  never an alias. Their prompt is the reviewer's contract: read-only, the diff range and the spec
  it was given, skill `om-code-review`, and a last line `Verdict: approve` or
  `Verdict: request changes`.
- **REV-002** The independent code review is started with the Agent tool and
  `subagent_type: yulius-reviewer`. Yulius sees that start (a `PreToolUse` hook on the Agent
  tool) and decides the round's model: rounds 1 and 2 of a pull request run `yulius-reviewer`;
  from round 3 on Yulius starts `yulius-reviewer-opus` instead (`updatedInput`). A round is
  counted per pull request of the task: the rounds since the task's last merge, a round being one
  that ended with a report; a reviewer started again for a round that was cut off keeps that
  round's number and model. A spec review, and any other subagent, runs on the session's model:
  Yulius does not touch it.
- **REV-003** When Yulius cannot be asked (the hook fails or times out), the reviewer starts as
  asked: Fable 5.2. The start is never refused for that.
- **REV-004** Each round is a record (`review_rounds`): the task, the round's number, the head
  commit at its start, the model asked for and why (`round`; `kept`, a reviewer started again for
  a round with no report; `no_fable`), the subagent's id, the
  model that answered (read from the reviewer's own transcript, not from what was asked), when it
  started and ended, its verdict (`approve`, `request_changes`, or none when the report has no
  verdict line) and whether a usage limit cut it off. A resumed reviewer continues its record.
- **REV-005** A merge writes the models into its record: `audit_log` action
  `delivery.review.models` at the merge, with the model of each run that wrote the pull request
  (`attempts.model`) and each round's number, model, verdict and head; the rounds get the pull
  request's number and the merge time. `delivery_status` and the task's delivery view
  (`GET /api/tasks/{id}/delivery`) list the current pull request's rounds. A merge whose head no
  recorded round approved says so in the same record ("no recorded review round approved …"); it
  is not refused for it.
- **REV-006** One read gives the numbers of the request: `GET /api/analytics/reviews?since=`
  (sandbox members; the Dashboard's Pipeline tab shows it) answers, for the pull requests merged
  since then, the share of first rounds that asked for changes, the median rounds per pull
  request, the share of first rounds with no report, and the rounds per model.
- **REV-007** When round 1 or 2 starts and the session's subscription cannot run Fable 5.2 (its
  Fable window is at the threshold, or it is not a candidate for a Fable run), Yulius moves the
  session to a subscription that can run both the session's model and Fable 5.2, when one is a
  candidate: the Agent call is refused with why, the session is continued on that subscription,
  and its first prompt tells the agent to start the review again. No move is made while the
  session has background work a move would end (the refusal then names it: finish or stop it and
  start the review again). When no subscription can run Fable 5.2, the round runs
  `yulius-reviewer-opus`: the reviewer's prompt gets a line saying so, which its report repeats,
  and the round's record says `no_fable`. Yulius never waits for Fable or asks a person for it.
- **REV-008** The rules name the reviewer the same way wherever the review is described (the
  briefing, the definition of done, `om-code-review`, `delivery-pipeline`, the Feature and Fix
  workflows, `yulius-review`, `yulius-git`, `docs/engineering/workflow.md`, `git.md`,
  `AGENTS.md`, `delivery_merge`'s description): "start the independent code review with the
  `yulius-reviewer` agent; Yulius picks its model (Fable 5.2 for a pull request's first two
  rounds, then Opus 5.5)". A test holds the places to the sentence.

### Fix 2: flaky tests

- **FLK-001** `tool_cli::tests::an_unreachable_yulius_exits_3` asks an address nothing can come
  to listen on while the test runs (no port let go and hoped free). The same pattern in
  `agent_call::tests::nothing_listening_is_unsent` and `mcp_bridge`'s test is fixed with it.
- **FLK-002** The notes e2e test "a failed reload after a teammate's edit backs off" (#1246) is
  reproduced by a probe at the racing step, fixed at the root (a product defect in the product, a
  test defect in the test), and proven by the probe passing where it failed. It is not retried,
  skipped or given a longer timeout as its fix.
- **FLK-003** The two tests fixed on `dev` since the measurement (#1313, #1309) are not redone:
  each is run under the probe that made it fail before its fix, and what that shows is recorded.

### Fix 3: Yulius CI tests a head while its review runs

- **CIR-001** When a task starts its independent code review (REV-002) and its work is committed,
  Yulius pushes the head and opens the task's pull request if it has none, or pushes the new head
  to it, without waiting for the plan's steps (the review step is still open). The spec must be
  valid and the worktree clean, as for any delivery; when it is not, the reviewer still starts
  and the agent is told why no run was started. Yulius CI then tests that head as it tests every
  open pull request's; a newer head supersedes the older run; `delivery_merge` after the approval
  finds that head's checks done or running and merges or arms as today.
- **CIR-002** The run for a head the review asked for is queued at that moment, not at the next
  60-second poll.
- **CIR-003** Nothing merges without a passed Yulius CI run on the exact head, and a local run is
  never reported as that run (unchanged; tests hold it).
- **CIR-004** One rule for local checks, stated the same way in `docs/engineering/workflow.md`
  step 4, `yulius-verify`, `AGENTS.md`, `om-check-and-commit`, `om-code-review`,
  `delivery-pipeline`, the workflows, the briefing and the definition of done: "While iterating,
  run the checks your change touches. The whole gate is Yulius CI's job: it tests the head while
  the reviewer reads. Do not run the whole gate locally before a review or between rounds, except
  to reproduce a CI failure or when CI cannot run (a playground without CI, an outage)." The
  reviewer reads the CI run of the head it reviews and runs the tests it doubts; it does not run
  the whole gate either. A test holds the places to the rule.

### Fix 4: one CI cycle for an armed pull request whose base moved

- **MQC-001** An armed pull request that is behind its base joins a merge candidate while its own
  run is still queued or running; the candidate's run goes beside the members' own runs. Members
  merge when the candidate passed **and** their own run passed.
- **MQC-002** A member whose own run fails, errors or is cancelled leaves the candidate: the
  candidate is formed again from the others (as a member leaving does today), and that member's
  armed merge ends as a failed own run ends it today. A red member costs the others no more than
  today: at most the candidate run it was in.
- **MQC-003** What ADR-0285 won stays: a moved base restarts no pull request's own run; Yulius
  merges the base into no pull request before its own run passed; green and current merges at
  once; a red candidate is bisected.
- **MQC-004** It holds for projects with `exclusive` jobs too: there a candidate's run of such a
  job waits for the members' own runs of it (one at a time), so the gain is what does not wait.
- **MQC-005** Candidates formed, merged and dissolved (with why: a member left, bisected, base
  moved, errored, too long) are counted per day and read at
  `GET /api/sandboxes/{sid}/analytics/merge-queue` (the CI/CD page shows the counts), with the
  minutes from arming to merge.

### Fix 5: a CI job can say which files it tests

- **CJP-001** `[[ci.jobs]]` takes `paths`: 1 to 50 path globs (the scope claims' glob form) of the
  files the job tests. A malformed glob is a configuration error with the job's name.
- **CJP-002** For a pull request's head and a merge candidate, a job with `paths` runs only when
  the change (the head against its merge base with the integration branch; for a candidate, its
  members' changes) touches a file matching one of them. Otherwise the job ends **skipped** with
  the reason "no file this job tests changed (paths: …)" in its output, without starting its
  command or taking a lane. It blocks no merge; the run page and `ci_status` show it skipped with
  that reason (ADR-0133).
- **CJP-003** A job with `paths` runs regardless: on the integration and production branches' own
  runs (so every merged state is tested by every job, and a promotion's head too), when the
  change touches `yulius.toml` (the job's own definition) and when the changed files cannot be
  worked out (no merge base, a failed diff): never skipped on doubt.
- **CJP-004** It is part of the one rule that says which jobs test a commit
  (`ci_gate_for_head`): the jobs in force are decided there, then each job's `paths` decide run
  or skipped; a rerun and a copied verdict keep the decision.

### Fix 6: an approval survives a small follow-up

- **APR-001** `delivery_merge` accepts a head that follows the reviewed commit when, beside clean
  merges of the integration branch, the commits after it change at most 20 lines of product code
  in total (added plus removed, counted commit by commit, so a line changed twice counts twice).
  Tests and documents do not count.
- **APR-002** What counts: every file except documentation (`is_documentation_path`), test files
  (a `tests/`, `test/`, `e2e/`, `__tests__/` or `fixtures/` folder; `*_test.*`, `test_*.*`,
  `*.test.*`, `*.spec.*`, `*.e2e.*`) and, in a Rust file, the lines from its first `#[cfg(test)]`
  on. A project sets its own in `yulius.toml`: `[review] product = [globs]` (only these count) or
  `not_product = [globs]` (these never count).
- **APR-003** The rule does not hold, and the head needs its review, when a file of the reviewed
  change is no longer part of the pull request's change, when a merge after the reviewed commit
  resolved a conflict, or when the reviewed commit is not an ancestor of the head.
- **APR-004** The merge record says what was counted ("reviewed at X; since then N product lines
  in M files: …"), and so does the tool's answer; a refusal names the count and the files.
- **APR-005** The review rules say, the same way everywhere: after an approval, fix the remaining
  nits within 20 product lines and merge without another round; more than that, or a file leaving
  the change, needs a new round. ADR-0141 stays: findings are fixed before the merge.

### Fix 7: a review survives a usage limit

- **RLM-001** A usage limit that refuses a plain subagent is the session's news as a workflow
  agent's is (SPEC-486): the harness reports it from the subagent's transcript, with the model it
  binds. A limit of the session's own model family moves or pauses the session as today.
- **RLM-002** A limit that cut a reviewer off (REV-004: `cut off`) moves the session to a
  subscription that can run the reviewer's model, at once when the session's thread is idle and
  else at its next turn's end, also when the limit binds another model family than the
  session's. The process that continues the session is told to resume that reviewer by its id
  with `SendMessage`, and not to start a new one. With no such subscription, the agent is told
  that the round can run on Opus 5.5 (REV-007).
- **RLM-003** A review round does not start on a subscription about to run out: one with less
  than 5 points of room (`Tier::Tight`) in a window binding the reviewer's model, while another
  subscription has that room. The session moves first, as in REV-007.
- **RLM-004** The rules say: resume a cut-off reviewer, never start a new one for the same round;
  the reviewer runs at high effort, never max.

### Fix 8: mutation testing once per pull request

- **MUT-001** `scripts/dev mutants` runs one bounded mutation campaign on the lines the branch
  changed against the integration branch (a pinned `cargo-mutants`, its diff filter, a bound on
  the whole run and per mutant), and prints the caught, missed and timed-out mutants with a
  summary line for the spec's Evidence. Its bound and cost are measured on real changes of this
  repository and stated.
- **MUT-002** The rules say, in words that hold for any project (`om-code-review`,
  `delivery-pipeline`, `test-driven-development`, the workflows) and for this one (`yulius-verify`,
  `yulius-review`, `docs/engineering/development.md`): the author runs the project's mutation
  command once, on the head it asks the review for, and puts the result in the Evidence; without
  such a command it undoes the few lines its tests are meant to hold; the reviewer reads the
  result and may check single mutants it doubts, and runs no campaign; nobody writes a mutation
  script by hand. A test that cannot fail is still fixed in the same pull request (ADR-0141).

### Fix 9: two tasks do not edit the same files unknowingly

- **SCO-001** A claim that overlaps a live task's claim is told to both at that moment: the
  claiming agent's answer names the holding task, what it is changing (its title, its claim's
  note, its step in progress) and where its pull request stands (none, open with its checks,
  armed); the holding task gets one typed message naming the claiming task and the paths (not
  repeated for the same pair and paths).
- **SCO-002** The overlap is shown to people: on both tasks' pages and as a mark in the task
  list, until one of the claims is released or its task is no longer live.
- **SCO-003** The rules say what to do then (the briefing's parallel-tasks section, `scope_claim`'s
  description, `delivery-pipeline`): wait for that pull request and merge the integration branch
  before editing those files, or agree by a note on the claim who changes what.
- **SCO-004** Where the overlap is visible before a task starts (tasks created with the paths
  they will touch: `task_create`'s new `paths`, by the Superagent or a task), Yulius answers the
  overlap with the live or queued task that holds those paths and offers to run the new task
  after it (`after`).

### Fix 10: two defects found on the way

- **DEF-001** A queue entry Claude Code never takes out holds a session on its subscription for
  one turn's end only, also in a goal run whose Stop is blocked (SPEC-479's promise).
- **DEF-002** A running sandbox browser is unlocked for viewers' input only when no native file
  chooser opened before the policy appeared can still be on the display (SPEC-483 AC-36).

Both are checked on current `dev` first and fixed test-first if they still hold.

## Plan

- [x] Fix 1a: tasks and the Superagents run Opus 5.5 by default; a Fable choice is kept and still falls back (MDL-001 to MDL-003)
- [x] Fix 1a's documents: the decision amending ADR-0289, ADR-0295 and ADR-0312; SPEC-023, SPEC-464 and SPEC-482 notes; product, API, changes
- [x] Fix 1a's independent review answered test-first
- [x] Fix 1b: every task session is launched with the two reviewer agents by full model id (REV-001)
- [x] Fix 1b: Yulius sees the reviewer start, counts the pull request's rounds and gives rounds 3 and later the Opus reviewer; each round is a record (REV-002 to REV-004, its start)
- [x] Fix 1b: the rules name the reviewer agent the same way everywhere, held by a test (REV-008)
- [x] Fix 1b's independent review answered test-first
- [x] Fix 1c: a round's record gets the model that answered, the verdict and a cut-off from the reviewer's transcript (REV-004)
- [x] Fix 1c: the merge record, the delivery view and one analytics read name who wrote and who reviewed (REV-005, REV-006)
- [ ] Fix 1c's independent review answered test-first
- [ ] Fix 1d: a first or second round moves the session to a subscription that can run Fable 5.2, or runs on Opus 5.5 and says so (REV-007)
- [ ] Fix 1d's independent review answered test-first
- [ ] Fix 2: the unreachable-server tests ask a port nothing can take (FLK-001)
- [ ] Fix 2: the notes reload e2e test reproduced and fixed; the two tests fixed on dev since are checked under their probes (FLK-002, FLK-003)
- [ ] Fix 2's independent review answered test-first
- [ ] Fix 3: starting the review pushes the head and opens the pull request, and its CI run is queued at once (CIR-001 to CIR-003)
- [ ] Fix 3: one rule for local checks everywhere, held by a test; the decision amending the whole-gate-before-handoff step (CIR-004)
- [ ] Fix 3's independent review answered test-first
- [ ] Fix 4: a merge candidate forms beside its members' own runs, and a member whose own run fails leaves it (MQC-001 to MQC-004)
- [ ] Fix 4: candidates formed, merged and dissolved are counted and shown; ADR-0285 amended (MQC-005)
- [ ] Fix 4's independent review answered test-first
- [ ] Fix 5: a CI job names the files it tests, and a change that touches none of them leaves it skipped with the reason (CJP-001 to CJP-004)
- [ ] Fix 5's documents and the decision: when such a job runs regardless
- [ ] Fix 5's independent review answered test-first
- [ ] Fix 6: delivery_merge keeps an approval across at most 20 product lines and records what it counted (APR-001 to APR-004)
- [ ] Fix 6: the review rules state the bound; a project sets what counts as product code; the decision amending the reviewed-head rule (APR-002, APR-005)
- [ ] Fix 6's independent review answered test-first
- [ ] Fix 7: a limit that cuts a subagent off is the session's news, and a cut-off reviewer is resumed after the move (RLM-001, RLM-002)
- [ ] Fix 7: a review does not start on a subscription about to run out; the rules say resume, never restart, high effort (RLM-003, RLM-004)
- [ ] Fix 7's independent review answered test-first
- [ ] Fix 8: scripts/dev mutants, one bounded mutation run on the changed lines, measured (MUT-001)
- [ ] Fix 8: the rules: once per pull request, the result in the Evidence, no campaign by the reviewer (MUT-002)
- [ ] Fix 8's independent review answered test-first
- [ ] Fix 9: an overlapping claim is told to both tasks, with the holder's change and pull request (SCO-001, SCO-003)
- [ ] Fix 9: the overlap shows on both tasks and in the list (SCO-002)
- [ ] Fix 9: a task created with paths another task holds is offered to run after it (SCO-004)
- [ ] Fix 9's independent review answered test-first
- [ ] Fix 10a: a queue entry never taken out holds a goal run's move for one turn's end (DEF-001)
- [ ] Fix 10a's independent review answered test-first
- [ ] Fix 10b: viewers' input waits until no earlier native file chooser can be on the display (DEF-002)
- [ ] Fix 10b's independent review answered test-first

## Acceptance criteria

- **AC-1** (MDL-001) A new task, a resumed task, a task moved to another subscription, a Cockpit
  and both Superagents, with no model named, start `claude --model claude-opus-5-5 --effort high`.
- **AC-2** (MDL-002) A task set to `claude-fable-5-2` starts with it; with every subscription's
  Fable window full and room for Opus it starts with `claude-opus-5-5` and says so (SPEC-482's
  tests, now with a named Fable task). A default task with every Opus window full waits; it does
  not fall back.
- **AC-3** (MDL-003) A release without `claude-opus-5-5` is refused for sandboxes; one without
  `claude-fable-5-2` is installed and reported.
- **AC-3a** (MDL-002) A session launched with Opus 5.5 whose transcript holds a request for
  `claude-fable-5-2` and then a refusal for usage credits: the subscription's `weekly_fable` is
  full, `weekly_opus` untouched, its status `ok`; the session starts again on the same
  subscription with `--model claude-opus-5-5`, continuing the turn, and its terminal says "Fable
  is used up on this subscription: this session goes on with Opus 5.5." (a task and a
  Superagent; also a task set to Fable that runs on its fallback). Without the request record:
  as before, the launched model's.
- **AC-4** (REV-001) The launch arguments of a task hold `--agents` with both definitions, their
  full model ids and `high`; a Cockpit's do not. The real CLI's request for a `yulius-reviewer`
  subagent names `claude-fable-5-2` and effort `high` under an Opus session (probe).
- **AC-5** (REV-002, REV-003) The first and second `yulius-reviewer` start of a task are let
  through; the third is answered with `updatedInput` naming `yulius-reviewer-opus`; a round that
  ended with no report does not advance the count; after the task's merge the count starts again;
  an Agent call of another type gets no answer; with the store failing the start passes.
- **AC-6** (REV-004) After a reviewer whose transcript's answers name `claude-fable-5-2` ends with
  "Verdict: request changes", the round's record holds that model and verdict; a report without a
  verdict line gives none; a transcript that names another model than asked records the one that
  answered; a limit record in the transcript marks the round cut off, and the same agent's later
  end completes the same record.
- **AC-7** (REV-005, REV-006) A merge writes one `delivery.review.models` entry with the authors'
  models and the rounds; `delivery_status` lists the rounds; a merge with no approving round says
  so and still merges; the analytics read gives the four numbers for a fixture of merged pull
  requests and none for a sandbox the caller is not a member of.
- **AC-8** (REV-007) Round 1 on a subscription whose Fable window is full, with another that can
  run both: the Agent call is refused with the reason, the session moves there and its first
  prompt says to start the review again. With background work running: refused with that reason
  and no move. With no subscription for Fable: `yulius-reviewer-opus` runs, its prompt says why,
  the record says `no_fable`. Round 3: no move.
- **AC-9** (FLK) The notes test passes with the probe that made it fail (a delay or a held step
  at the racing point); the unreachable-server tests pass while another process binds free ports
  in a loop; the two tests fixed by #1313 and #1309 pass under their old probes.
- **AC-10** (CIR-001, CIR-002) A task with committed work, a valid spec and an open review step
  starts `yulius-reviewer`: its branch is pushed, the pull request is opened and a run for that
  head is queued before any poll. A second start at a newer head supersedes the first run. With
  uncommitted changes or an invalid spec: the reviewer starts, nothing is pushed, the agent is
  told why. `delivery_merge` at the approved head with the run passed merges; with it running,
  arms.
- **AC-11** (CIR-003) `delivery_merge` at a head with no Yulius CI run, or a run at another
  head, does not merge (existing tests stay green).
- **AC-12** (CIR-004, REV-008, APR-005, RLM-004, MUT-002) A unit test reads every place a rule is
  stated and fails when one lacks the sentence or keeps the old one ("`scripts/dev ci` before
  handoff", "Run every gate … before anything leaves your machine", a reviewer "runs the full
  gate").
- **AC-13** (MQC-001, MQC-002) Two armed pull requests behind the base, their own runs still
  running: a candidate forms at once and its run is queued beside theirs; both own runs and the
  candidate pass: both merge after one cycle. One member's own run fails: it leaves, its armed
  merge ends with the run named, the candidate forms again from the other, which merges.
- **AC-14** (MQC-003) A base move restarts no own run; no base merge is pushed to a pull request
  whose own run has not passed; a red candidate of two is bisected (existing tests stay green).
- **AC-15** (MQC-005) The merge-queue read counts candidates formed, merged and dissolved by
  reason from the audit entries of a fixture.
- **AC-16** (CJP) A job with `paths = ["crates/a/**"]`: a pull request touching only `docs/x.md`
  gets it skipped with the reason and merges; one touching `crates/a/src/lib.rs` runs it; one
  touching `yulius.toml` runs it; a run on the integration branch runs it; a candidate of a
  touching and a non-touching member runs it; an unknown merge base runs it; a bad glob is a
  configuration error naming the job; `ci_status` and the run page show "skipped: no file this
  job tests changed".
- **AC-17** (APR) Reviewed commit, then a commit changing 20 product lines and 200 test and
  document lines: `delivery_merge` accepts and records the count. 21 product lines: refused with
  the count. Two commits of 12: refused (24). A file of the reviewed change reverted out of the
  change: refused. A conflicted merge after the review: refused. A clean merge of the integration
  branch plus 5 lines: accepted. `[review] not_product` moves a path out of the count.
- **AC-18** (RLM-001, RLM-002) A plain subagent's transcript gets a usage-limit record: the limit
  is recorded on the subscription for that model's family. For a cut-off reviewer of another
  family than the session's, the session moves at its turn's end and the new process's first
  prompt names the reviewer's id and `SendMessage`; with the real CLI the resumed reviewer
  answers on the same model (probe).
- **AC-19** (RLM-003) A reviewer start on a subscription with 3 points of room for its model while
  another has 40: refused, moved, told to start again; with no roomier one: it starts.
- **AC-20** (MUT-001) `scripts/dev mutants` on a branch whose change has a line no test holds
  lists that mutant as missed and exits non-zero; with every changed line held it exits zero; on
  a branch that changes no Rust code it says so and exits zero; the bound ends a run with the
  mutants not reached named.
- **AC-21** (SCO-001, SCO-002) Task B claims `crates/a/**` while live task A holds
  `crates/a/src/x.rs`: B's answer names A, its title, note, step and pull request state; A's
  session gets one message; both tasks' API views and the list carry the overlap; a second claim
  of the same paths types nothing again; after A's release the mark is gone.
- **AC-22** (SCO-004) `task_create` with `paths` overlapping a live task's claim answers the
  overlap and the offer; with `after` set to that task it is created queued behind it.
- **AC-23** (DEF-001) A goal run with a queue entry never taken out: the first Stop stays for it,
  the next Stop moves the session although no `turn_duration` was written between them.
- **AC-24** (DEF-002) A browser started before the policy, with a file chooser possibly open, is
  not unlocked when the policy appears until it was restarted or the chooser is known closed.

## Test plan

- Unit tests in `yulius-domain` (the default and the fallback, the rotation helpers for a
  reviewer's family, round counting, product-line counting and classification, job `paths`
  matching, config parsing) and `yulius-harness` (launch arguments, hook payloads, a subagent's
  limit observation).
- Integration tests in the `yulius` crate's one binary with the fake agent and the fakes of Git,
  GitHub and CI: the review gate, round records, the merge record, the analytics reads, the
  earlier pull request and its run, the candidate beside own runs, skipped jobs, the approval
  bound, the move before a review and after a cut-off, scope overlaps.
- e2e (`e2e/`) for what a page shows: the rounds on the task's delivery, a skipped job's reason on
  the run page, the merge-queue counts, the overlap mark.
- The real CLI against the local stand-in API (`$HOME/.cache/yulius-task-472/probe`) for what the
  fakes cannot prove: which model and effort a reviewer's requests carry, the hook payloads, the
  cut-off and the resume. Its outputs are quoted in the Evidence.
- Each flaky test: the probe that reproduces it, before and after.
- Not testable here, and said in the Evidence: the effect on the instance's numbers (they are
  read a week later, see Decisions), a live subscription's Fable allowance, and quartio-v2's
  jobs.

## Decisions

- **The order is the request's.** Fix 2 first would spare this task's own later pull requests a
  flaky run (6 of 60 runs), but the default model is what the person decided first and it is the
  smallest change; no reason weighs more than the stated order.
- **Fix 1 is four pull requests.** The default (1a) changes what every task runs and is a
  constant; the reviewer agents and the round policy (1b), the records (1c) and the move (1d)
  each stand alone and are each near the size a review reads well.
- **The reviewer is an agent Yulius defines at the launch, not a rule about the Agent tool's
  `model`.** Measured: `--agents` definitions carry a full model id and an effort to the
  subagent's requests, and outrank the agent's home. The Agent tool's `model` takes aliases only,
  and an alias moved before (an older Claude Code read `fable` as Fable 5.1). Alternatives: a file
  in `~/.claude/agents` (the agent can rewrite it; refused); a rule "pass model fable" (an alias,
  and nothing sees whether it was followed; refused).
- **The name is `yulius-reviewer`**, not `code-reviewer`: a repository's own
  `.claude/agents/<name>.md` outranks a launch definition (measured), so the name must not meet a
  project's own. A repository that defines `yulius-reviewer` itself overrides the model on
  purpose, and the round's record shows the model that answered.
- **Yulius decides the round, the agent always asks for `yulius-reviewer`.** The agent cannot
  miscount rounds, and a rule that says "from round 3 use another agent" is one more thing to
  get wrong. When the hook cannot be asked the reviewer runs Fable: the stronger reviewer is the
  safe default, the cost is credits.
- **A round is counted when it ended with a report.** A reviewer cut off by a limit and started
  again (against the rule) must not push round 2 to Opus; and the request says a resumed round
  keeps its model.
- **No refusal of `delivery_merge` for a missing round record.** A session started before the
  update has no reviewer agents, and a verdict line can be missing from a real approval. The
  record says "no recorded review round approved", and the analytics read counts such merges, so
  the person sees whether the rule is followed. A refusal can be added once that share is known.
- **Fix 3 opens the pull request at the review's start**, instead of testing a head that has no
  pull request. Everything downstream already works from an open pull request: the poll, the
  supersede rule, the commit status, `delivery_merge` finding the checks, the pull request's
  events typed into the session (a conflict with the base is then seen during the review, which
  fix 9 wants too). A run without a pull request would need a second reference kind whose run
  must be handed over when the pull request opens (the planner cancels an active run whose
  reference disappeared). The cost: a pull request is visible before its approval. It says
  "in review" in its body until the merge is asked.
- **Fix 6 counts commit by commit.** A total diff would let a large change be undone by a second
  one and count as nothing; the reviewer saw neither. A merge that resolved a conflict is not
  counted in lines at all: it needs its review, as today.
- **Fix 6 after a request for changes: not built, and the person is not asked.** The request
  reads the decision as holding after an approval; of the re-reviews after a request for changes
  the table does not say how many blocked, so there is no evidence here for more.
- **How to read the numbers a week later** (each fix's number, on the instance):
  - Fix 1: `GET /api/analytics/reviews?since=<date>` (the Dashboard's Pipeline tab): share of
    first rounds that asked for changes (baseline 47 %, Fable reviewing Fable's work), median
    rounds per pull request (baseline 3), per model. With the reviewer held at Fable for round 1,
    a higher share than 47 % means Opus's first versions are worse than Fable's were; the same
    share with a shorter start-to-merge (the Pipeline tab's task spans; baseline 2.1 h here,
    3.2 h in quartio-v2) means the policy pays. Raw rows: `SELECT round, model, verdict, cut_off
    FROM review_rounds WHERE started_at >= …`.
  - How long the Fable credits last: reviewers made 37–38 % of Fable's output tokens and about a
    quarter of its cached reading when Fable did everything, and rounds 3 and later (a median of
    3 rounds per pull request) leave Fable too. So reviews use at most about a third of what was
    used; an allowance that lasted about three days of a week lasts the week when nothing else
    runs Fable (3 days / 0.35 ≈ 8.5 days). The read: the Capacity panel's Fable windows
    (`usage.weekly_fable` in `GET /api/capacity`) on day 5 of a week, and the share of rounds
    recorded `no_fable` in the analytics read (above zero means the credits ran out).
  - Fix 2: failed Yulius CI runs whose commit passed in another run: `SELECT sha, COUNT(*),
    SUM(state='failed') FROM ci_runs WHERE created_at >= … GROUP BY sha HAVING SUM(state='failed')
    > 0 AND SUM(state='passed') > 0` (baseline 5 of 60 runs in two days).
  - Fix 3: minutes from the last approval to the merge, and local whole-gate runs per pull
    request: the Pipeline tab's `wait/merge_armed` spans (baseline: Yulius CI started at the
    arming and took 9 min here, 31 min in quartio-v2 with nothing else happening).
  - Fix 4 and fix 5: `GET /api/sandboxes/{sid}/analytics/merge-queue` (candidates formed, merged,
    dissolved; minutes from arming to merge; baseline quartio-v2: 127 min with a candidate, 17
    without) and the CI page's job list (a job's runs and skips per day).
  - Fix 6 and fix 7: the analytics read's rounds per pull request and its share of first rounds
    with no report (baseline 14 of 86 at high effort).
  - Fix 8: `grep -c 'mutants' ` over the week's sessions is not a page; the read is the share of
    blocking rounds whose findings are test strength only, which a person samples from the round
    records' reports. The command's own time is in each spec's Evidence.
  - Fix 9: the share of pull requests that hit a conflict: `audit_log` actions
    `delivery.conflict` per merged pull request (baseline 29 of 60 in quartio-v2, 7 of 42 here).

## Evidence

### Fix 1a: the default model (head `1ab29446c` before the review)

- **Test first.** With `DEFAULT_CLAUDE_MODEL` put back to `claude-fable-5-2` on the committed
  tests, `cargo nextest run -p yulius-domain -p yulius-harness`: 10 of 692 fail
  (`harness::tests::the_default_model_is_opus_5_5_and_the_review_model_fable_5_2`,
  `a_fable_model_falls_back_to_opus_5_5`, two release-gap tests, and six launch tests on
  `--model claude-opus-5-5 --effort high`). With the review model's label taken out of
  `gap_text`, `a_build_without_the_default_model_is_refused_and_another_gap_is_named` fails. With
  the capacity view's `fallback_model` tied to the default model again,
  `subscription_pool::the_capacity_view_counts_for_the_default_model` and
  `model_fallback::a_task_starts_on_the_fallback_and_its_next_launch_runs_fable_again` fail.
- **What the flip broke, and why each test changed.** The first run of the whole suite after the
  constant changed: 64 of 4456 failed. All took a default task for a Fable task: launch
  arguments (now Opus), the Superagent's fallback (it runs the default model, which has none:
  `a_superagent_refused_its_model_on_the_only_subscription_stays`,
  `a_global_superagent_starts_on_opus_where_fable_is_used_up`,
  `a_spent_opus_cap_keeps_the_cockpit_off_a_subscription`), and the fallback, usage-credit and
  workflow-limit tests, whose task is now set to Fable (`new_fable_task`,
  `fable_running_on_first`) and whose "limited" assertions follow the rule that a subscription
  is limited only while the default model cannot run on it. No product rule of SPEC-464 or
  SPEC-482 changed.
- `cargo nextest run --workspace --all-features --no-fail-fast`: 4456 passed, 17 skipped.
- `scripts/dev check`: exit 0. `scripts/dev lint`: exit 0.
- `cd e2e && npx e2e run`: 84 files passed, 1 skipped; 1050 tests passed, 1 skipped. The first
  run had 10 failures in `subscriptions.e2e.ts` and `model-fallback.e2e.ts`, all on the mock's
  subscription that was "limited" by Fable's allowance alone; it is now out of the default
  model's weekly window too, and the page-unit tests state the new rule (out of Fable alone:
  not limited, its line still shown).
- AC-1: `yulius-harness` launch tests (new, resumed, Cockpit, Superagent arguments) and
  `cockpit::`, `global_superagent::` against the fake `claude`. AC-2: `model_fallback::` (a task
    set to Fable, 7 tests), `usage_credits::` (19),
  `capacity::tests::a_runs_family_is_its_launch_models`
    (`fallback_of(None)` is none). AC-3:
  `agent_cli::tests::a_build_without_the_default_model_is_refused_and_another_gap_is_named`,
  `claude_updates::` (a release without the default is refused).
- Not exercised here: a live Claude Code run on a subscription (the launch arguments are proven
  against a fake `claude`), and the instance, which gets the change with a promotion and update.
- `scripts/dev security`: "Security scan disabled" for this project in Yulius: disabled, not
  passed.
- The whole gate and the e2e suite also run on Yulius CI at the pull request's head.
- **Review round 1** (fresh session, head `1b749ce6f`): request changes. Answered:
  - Blocker (own code): `subscriptions.e2e.ts` announced task #12's model to an open view and
    then clicked: the menu could read the old model (failed 2 of 4 runs). The model is now set in
    the mock before the page reads the task (`setTaskModel`); the file passed 6 runs of 6.
  - Major (own code; the attribution rule pre-existing): a refusal for usage credits names no
    model and was booked to the launched model, so a session switched to Fable with `/model`
    would mark Opus as used up and stop default work. Measured with the real CLI 2.1.325: each
    request of the thread is preceded by an `api-request` record naming its model. Test first:
    `wiring::harness::tests::a_models_limit_is_of_the_model_the_thread_asked_for` (failed:
    `Model(None)` against `Model(Some(Fable))`), `usage_credits::a_session_switched_to_fable_goes_on_with_its_own_model_when_fable_is_refused`
    (failed), `cockpit::a_superagent_switched_to_fable_goes_on_with_opus_when_fable_is_refused`
    (failed on the notice: "No other subscription has capacity"); all pass with
    `claude::requested_model`, `limit_observation` and `supervisor::switched_model`. MDL-002
    and AC-3a state the rule; ADR-0340 records it.
  - Minors: the Global Superagent's launch line in `docs/api.md`; `CapacityView` docs; a task's
    details name a fallback only for a run launched with it (`fell_back_to`, unit test); the
    Global Superagent test asserts the record has no `model`; the inert Superagent fallback is
    kept and said so in ADR-0340 and SPEC-482 (deleting it would touch the launch port and every
    Cockpit request); the "independent review's model" label waits for fix 1b (MDL-003); the
    e2e README row; documents no longer say reviews run Fable yet; stale comments and the
    decision index lines.
- After round 1: `cargo nextest run --workspace --all-features`: 4463 passed, 17 skipped;
  `scripts/dev lint` exit 0; `check_repository.py` passed; `cargo doc` no warnings; e2e
  `claude-code`, `model-fallback`, `subscriptions`: 61 passed.
- **Review round 2** (fresh session, head `63bb46fdd`). It ran on Opus 5.5: the Fable reviewer
  was refused at its first request by this subscription's Fable limit ("You've hit your Fable
  limit · resets 2pm (UTC)"), the case fix 1d covers. Verdict: approve. Both round-1 findings
  were checked as fixed: the e2e file passed 3 runs of 3, a scan of 547 real transcripts found
  `api-request` records of sub-agents only in their own files, and two mutations
  (`limit_observation(limit, None)`; no `input.running = ran`) failed the new tests. Answered:
  - Minor (own code): the notice named the task's own model, not the model the relaunch runs
    (a task set to Fable on its fallback, switched to Fable and refused, read "goes on with
    Fable 5.2"). Test first:
    `usage_credits::a_fable_task_on_its_fallback_switched_to_fable_says_the_model_it_goes_on_with`
    failed with that notice; the notice now names the model of the relaunch ("Fable is used up
    on this subscription: this session goes on with Opus 5.5.").
  - Minor (made stale): ADR-0020 and ADR-0055 status lines point to ADR-0340.
  - Nits: a moved doc comment in `claude.rs`, `use super::*` first in `tasks.rs` tests, the
    `ui_contract.rs` example, MDL-002's wording on where the session goes on.
  - Pre-existing, outside fix 1a: a workflow agent of the default type "runs the session's
    model", still read as the launched model after `/model` (SPEC-486 code). Not changed here.
- After round 2: `cargo nextest run -p yulius-harness -p yulius-app`: 748 passed;
  `usage_credits::`, `cockpit::`, `model_fallback::`: 104 passed; `scripts/dev lint` exit 0.
- **Review round 3** (delta `63bb46fdd..5b399be20`, Opus 5.5 by the policy for a third round):
  approve, with two own-code minors of one cause. The notice was worked out from a separate
  read of capacity, so a branch went untested (a Fable task on Fable, switched to Opus and
  refused) and a usage write between the two reads could make it name another model than the
  launch runs. Answered: the placement keeps the refused family (`switched_limit`) and the
  relaunch says the notice after its preflight, with the model it runs. Test first:
  `usage_credits::a_fable_task_switched_to_opus_goes_on_with_fable_and_says_so` (passed
  before, so the reviewer's surviving mutant is now checked instead): with the notice naming
  the task's own model,
  `a_fable_task_on_its_fallback_switched_to_fable_says_the_model_it_goes_on_with` fails; with
  it naming the fallback whenever there is one, the new test fails. `usage_credits::`,
  `cockpit::`, `model_fallback::`: 105 passed. Long lines rewrapped.
- **Review round 4** (delta `5b399be20..3ec7234dc`, Opus 5.5): request changes. Own code: the
  notice was still said from the preflight's placement, and the launch places the run again
  once the old one has ended (reproduced by holding the switch at `SwitchDecided` and changing
  the usage); and the kept family could outlive a switch that returned early (read in the code,
  not reproduced). Answered: the launch says the notice where it sets `--model`, from the
  model it runs; a relaunch of the refused family again (its usage changed meanwhile) says only
  "Fable 5.2 can run again" (the existing line); every limit decision writes the kept family
  (none unless it moves the run) and a failed switch clears it. Test first:
  `usage_credits::the_notice_past_a_switched_limit_names_the_model_the_launch_runs` (holds the
  switch, gives Fable room again) failed with "Fable is used up …: goes on with Opus 5.5" while
  the relaunch ran Fable 5.2; it passes now. Mutations: the notice naming the task's own model
  fails the fallback test; no same-family filter fails the new test. `usage_credits::`,
  `cockpit::`, `model_fallback::`, `subscription_moves::`, `claude_code_moves::`: 176 passed;
  `scripts/dev lint` exit 0. Spec indentation nit fixed.
- **Review round 5** (delta `3ec7234dc..d53d9a7eb`, Opus 5.5): approve. It confirmed the race
  fix (the new test fails on round 4's code; 8 of 8 stress runs pass) and killed 3 of 5
  mutations; the two that survived were the guards of round 4's unreproduced finding (no write
  for a decision other than Switch; the clearing after a failed switch), and it read one stale
  case left (a paused switch, then a stop and a Continue). Answered by a simpler rule that
  leaves no such branch: every limit decision writes the family, and every launch takes it,
  saying it only when it is the launch past a limit. The two guards are gone, so a limit that
  pauses the task is now said when the session goes on past it. Test first:
  `usage_credits::a_switched_limit_that_waits_is_said_when_the_session_goes_on` failed (no
  notice at the resume); it passes now. 177 passed in `usage_credits::`, `cockpit::`,
  `model_fallback::`, `subscription_moves::`, `claude_code_moves::`; lint exit 0. Its nits
  (a launch that places again after its account was removed, a failed spawn after the notice)
  behave as the existing `model_change_notice` does, and the lines that follow correct them.

### Fix 1b: the reviewer agents and their rounds

- Fix 1a merged as pull request #1321 at `be80395aa`, on its own green Yulius CI run.
- Round 6's note on fix 1a (the switched-limit notice's `AfterLimit` filter had no test) is
  answered first here: `usage_credits::a_switched_limit_is_not_said_at_a_later_start` (a task
  paused at a switched limit, stopped and continued, says nothing of it) fails with the filter
  removed and passes with it.
- **REV-001.** Test first: `launch::a_task_session_is_launched_with_the_reviewer_agents` failed
  (no `--agents`); passes. Real CLI 2.1.325 against the stand-in API, with the launch's exact
  `--agents` JSON, an Opus session and `subagent_type: yulius-reviewer`: the subagent's request
  names `claude-fable-5-2` and `output_config.effort: high`, `resolvedModel` is
  `claude-fable-5-2` (probe `$HOME/.cache/yulius-task-472/probe`, scenario `review`).
- **REV-002 to REV-004.** `review_rounds::the_third_round_of_a_pull_request_runs_the_opus_reviewer`
  (rounds 1 and 2 as asked, round 3 `updatedInput` `yulius-reviewer-opus` with the rest of the
  input; a cut-off round keeps its number and reviewer; verdicts from the last line; a merge
  closes the rounds and the next pull request starts at round 1; another subagent untouched),
  `a_resumed_reviewer_and_other_subagents_change_no_round`,
  `a_round_on_a_subscription_out_of_fable_runs_the_opus_reviewer` (failed before the rule:
  Fable was asked on a subscription out of Fable), `review::tests::*` (round policy, verdict
  line), `hooks::tests::subagent_hooks_and_an_updated_tool_input` (the contract's JSON as the
  real CLI takes it), `guard_client::tests::a_subagents_start_is_sent_with_its_input`, and the
  launch snapshot with the `Agent|Task` matcher and the `SubagentStart`/`SubagentStop` hooks.
  Migration 0062 is pinned in the store's migration test.
- **REV-008.** `skills::tests::every_place_names_the_reviewer_agent_and_who_picks_its_model`
  failed on the first place before the edits; passes. The briefing's wording is the same length
  as before (139 bytes); the Fix workflow stays within its 1,444-byte budget by shorter wording
  elsewhere in it (no rule dropped: `review_findings_in_own_code_are_never_deferred` passes).
- `cargo nextest run --workspace --all-features`: 4476 passed, 17 skipped (before the last
  test above). `check_repository.py` passed; `apm install --force` regenerated the two
  repository skills' copies.
- Not exercised here: a whole task session on a subscription reviewing through Yulius (the hook
  path is proven through the internal API with fakes, the agent definitions with the real CLI
  and the stand-in API).
- **Review round 1** (fresh session, head `834d6afc0`; on Opus 5.5: this subscription was out
  of Fable until 14:00 UTC, the case the `no_fable` rule covers). Request changes. Answered:
  - F1, major (own code): the next round counted ended records, so a round reported by two of
    its reviewers (a cut-off one resumed, or two at once) skipped a number and moved round 2 onto
    Opus (reproduced: records 1, 1, 3). Test first:
    `review_rounds::a_round_with_two_reports_is_counted_once` failed (`yulius-reviewer-opus`);
    the next round now follows the last one that reported; passes.
  - F2, major (own code): fail-open on a store failure had no test:
    `review_rounds::a_round_yulius_cannot_record_starts_as_asked` (table dropped: 200, nothing
    changed or denied). It fails when the error branch denies.
  - F3, major (own code): the sandbox client's forwarding had no test:
    `hook_client::a_subagents_start_goes_to_yulius_and_its_answer_is_printed` (whole input sent,
    the answer printed, nothing printed with no server). It fails with the forwarding removed.
  - F4, minor (own code): `review_rounds::a_round_on_an_unknown_subscription_starts_as_asked`
    (account removed: Fable, reason `round`); fails when an unknown account fails closed.
  - Nits: `apm.lock.yaml` keeps `target: legacy` (a newer apm wrote `codex`; only the content
    hashes change); `kept` is listed in REV-004 and ADR-0341; the touched paragraphs are
    rewrapped. "No shared context" stays in the reviewer agent's contract: the Feature
    workflow is at its byte budget. The reviewer agents keep every tool: a review runs tests,
    and its read-only rule is its contract, as before.
- Round 2 (head `2628ad31f`, Opus 5.5 again: Fable still out): approve. Its two notes needed no
  change: the store-failure test's assertions also hold on the success path (it does catch
  failing closed), and `link_review_subagent` could miss a reviewer when two start at once on
  different reviewer agents (the round still counts through the other). Merged as pull request
  #1322 at `2628ad31f` on its own green Yulius CI run.

### Fix 1c: what each round ran on, the merge record and the numbers

- **REV-004.** Test first: `wiring::harness::tests::a_subagents_model_and_its_cut_off_are_read_from_its_transcript`
  (a subagent's answers name its model once per change, a refusal in its file says it was cut
  off, `<synthetic>` never counts, a workflow agent's lines say neither) did not compile before
  the observations existed; passes. `review_rounds::a_rounds_answering_model_and_cut_off_are_noted`
  (the model and the cut-off on the round; a later report keeps the cut-off; a subagent that is
  no round's changes nothing).
- **REV-005.** `delivery::agents_merge_only_through_gated_tools` now records a round before the
  agent's merge: the delivery view lists it, and the merge writes one `delivery.review.models`
  entry ("pull request #1: written by …; review rounds: 1 claude-fable-5-2 approve at <head>"),
  after which the view lists none. `review_rounds::the_merge_record_names_this_pull_requests_rounds`
  (a second pull request: its rounds only, "no report" for a cut-off one, and "no recorded
  review round approved it"). `mcp_text::tests::delivery_status_lists_the_review_rounds`.
- **REV-006.** `review::tests::the_review_numbers_of_merged_pull_requests` (shares, median,
  rounds per model, no-Fable rounds, a round reported after a cut-off);
  `review_rounds::the_reviews_read_counts_the_rounds_of_merged_pull_requests` (the API over a
  range, another range, 422 for a reversed range, 401 signed out); `pipeline.e2e.ts` (the
  Review card's four tiles from the mock): 15 passed.
- Decision: the person reads the round records three ways (the task's delivery view while the
  pull request is open, the merge record afterwards, the analytics read for the week); no
  separate page.

