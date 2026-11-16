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
  recorded round approved (that head, or one followed only by clean merges of the integration
  branch) says so in the same record ("no recorded review round approved its head"); it is not
  refused for it. The runs that wrote it are those still going when the task's previous pull
  request merged and every later one.
- **REV-006** One read gives the numbers of the request: `GET /api/analytics/reviews?from=&to=`
  (sandbox members; the Dashboard's Pipeline tab shows it) answers, for the pull requests merged
  since then, the share of first rounds that asked for changes, the median rounds per pull
  request, the share of first rounds with no report, and the rounds per model.
- **REV-007** When round 1 or 2 starts and the session's subscription cannot run Fable 5.2 (its
  Fable window is at the threshold, or it is not a candidate for a Fable run), Yulius moves the
  session to a subscription that can run both the session's model and Fable 5.2, when one is a
  candidate: the Agent call is refused with why, the session is continued on that subscription,
  and its first prompt tells the agent to start the review again. No move is made while the
  session has background work a move would end (the refusal then names it: finish or stop it and
  start the review again). A prompt waiting in Claude Code's queue refuses the call the same way
  (answer it or end the turn). The move is announced only once the new session's start there
  was checked. When no subscription can run Fable 5.2, the new session could not start there, a
  move was refused moments ago, or the looks take longer than the hook may wait (3 s), the round
  runs `yulius-reviewer-opus`: the reviewer's prompt gets a line saying so, which its report
  repeats, and the round's record says `no_fable`. Yulius never waits for Fable or asks a person
  for it.
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
  `delivery-pipeline`, `om-fix`, `docs/engineering/git.md` and `development.md`, the workflows and
  the briefing (the definition of done names no local gate outside a playground, where CI may
  not run, so it stays): "While iterating,
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
  `GET /api/analytics/merge-queue?sandbox&from&to` (the same window and access as the reviews
  read; the Dashboard's Pipeline tab shows the counts beside the Review card), with the minutes
  from arming to merge.

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
  `*.test.*`, `*.spec.*`, `*.e2e.*`) and, in a Rust file, the lines of a `#[cfg(test)] mod name {`
  to its closing brace, read as Rust tokens (comments and strings skipped) (`#[cfg(test)] mod tests;` or a single test-only item is product code). A
  project sets its own in `yulius.toml`: `[review] product = [globs]` (only these count; tests and
  documents still do not) or `not_product = [globs]` (these never count). Lines are read with
  plumbing and `--text`, so the clone's configuration and attributes change nothing.
- **APR-003** The rule does not hold, and the head needs its review, when a file of the reviewed
  change is no longer part of the pull request's change, when a merge after the reviewed commit
  resolved a conflict or merged anything but the integration branch (the commit its origin
  names now, read from GitHub or a playground's own repository and then fetched, never a ref of
  the clone: without it nothing carries), when the reviewed commit is not an ancestor of the
  head, or when a product file's changed line has a NUL byte or is over 200 bytes (no line count
  says what it changed). Commits are read as their objects say: no replace ref, graft or
  commit-graph file of the clone counts.
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
- [ ] Fix 1d: a merge found on GitHub is judged against its merge commit's first parent, as `delivery_merge`'s is (REV-005)
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
  and no move; a queued prompt, the same. With no subscription for Fable, a start that would be
  refused there, a move refused moments ago or looks over 3 s: `yulius-reviewer-opus` runs, its
  prompt says why, the record says `no_fable`. Round 3: no move.
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
- **The move for a Fable reviewer refuses the Agent call and interrupts the turn (fix 1d).**
  Holding the `PreToolUse` hook until the session ended would run nothing meanwhile, but the hook
  client never holds `PreToolUse` (a `202` fails open there, and Claude Code's timeout for it is
  short), so every running session would need a new launch first. A refused call that the agent
  answers mid-turn is what a person's switch meets, so the move interrupts first as that one
  does. The first prompt on the new subscription says why and asks for the review again.
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
  reference disappeared). The cost: a pull request is visible before its approval; its body is
  the one Yulius writes for any pull request (the review rounds show in the delivery view and
  `delivery_status`), so nothing is added to it.
- **Fix 6 counts commit by commit.** A total diff would let a large change be undone by a second
  one and count as nothing; the reviewer saw neither. A merge that resolved a conflict is not
  counted in lines at all: it needs its review, as today.
- **Fix 6 after a request for changes: not built, and the person is not asked.** The request
  reads the decision as holding after an approval; of the re-reviews after a request for changes
  the table does not say how many blocked, so there is no evidence here for more.
- **How to read the numbers a week later** (each fix's number, on the instance):
  - Fix 1: `GET /api/analytics/reviews?from=<day>&to=<day>` (the Dashboard's Pipeline tab): share of
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
  - Fix 4 and fix 5: `GET /api/analytics/merge-queue?sandbox=<id>` (candidates formed, merged,
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
- **Review round 1** (head `d4bc49815`, Opus 5.5: Fable out until 14:00 UTC). Request changes.
  Answered (all own code):
  - Major: the authors skipped a run that started before the previous merge. Now a run counts
    unless it ended before that merge, and "the previous merge" is the latest one before this
    one (the new test caught it taking a later merge).
    `review_rounds::the_merge_record_names_this_pull_requests_rounds_and_authors` asserts the
    two records word for word.
  - Major: "no recorded review round approved" ignored the merged head. The merge paths that
    know it (the agent's or person's merge, a merge found on GitHub) pass it, and an approval
    counts for that head or one followed only by clean merges of the integration branch
    (`review_covers`). `review_rounds::an_approval_counts_for_the_head_it_approved`.
  - Major: the reviews read's scoping had no test:
    `analytics::the_reviews_read_counts_only_visible_sandboxes` (a member of one sandbox of
    two; the other by id is 404).
  - Minors: the delivery test asserts the exact record; rounds are matched to the merge by its
    time, not its number (a playground has none); a reviewer's transcript reaches its round in
    a live session (`usage_credits::a_reviewers_transcript_notes_its_model_and_cut_off_on_its_round`,
    a second reviewer without a cut-off), and while a restart catches up
    (`usage_credits::a_reviewers_transcript_written_while_yulius_was_down_is_noted_at_restart`
    fails with the replay's noting disabled); rounds by model count rounds, not reviewer runs;
    the read is capped at 20,000 rounds, newest merges first; the Pipeline shows its stages
    without the Review card when the read fails (`pipeline.e2e.ts`, 16 passed); the spec's
    query is `from`/`to`.
  - Nits: the doubled doc comment; heads cut by characters; a run without a recorded model
    counts as its task's model.
- **Review round 2** (head `2e63bb004`, on Fable 5.2: the reviewer was cut off at its first
  request by the Fable limit and resumed with a message after the 14:00 UTC reset, not started
  again). Request changes. Answered (all own code):
  - Major: the playground merge recorded without its head, so any approval counted. Every
    merge path now passes `MergedHead` (the head and, where known, the integration branch
    before the merge). `playground::a_playground_merge_records_whether_the_merged_head_was_approved`
    merges through the real path: an own commit after the approval gets the caveat, a clean
    merge of `dev` after it does not. Mutations caught: the playground passing no head; the
    clean-merge check forced to true; forced to false.
  - The clean-merge check ran after the merge, unsound for merge-commit merges (the merged
    head is then in the integration branch): it is now judged against the integration branch
    as it was before the merge (`review_covers_against` with the merge commit's first parent,
    or the playground's base); a merge found elsewhere has no base and counts only the
    approved head itself.
  - The analytics read keys a pull request by its merge (a playground merges with no number);
    the previous merge also comes from the earlier merge record (a merge with no round writes
    one too); the restart test also asserts the cut-off; the architecture text says the new
    rules; the API document names the read's 20,000-round bound; `same_commit` is
    `delivery_merge`'s own; a playground's record reads "the merge".
- **Review round 3** (delta `2e63bb004..e5a08d7ad`, Opus 5.5 by the policy for a third round).
  Request changes. It checked every round-2 answer and caught by mutation the playground
  passing no head or no base, the clean-merge check forced either way, and the restart replay
  skipping the cut-off. Its runs: 168 of 168 targeted, 1,092 of 1,092 in `yulius-app` and
  `yulius-domain`, lint and `cargo doc` clean. It found `<merge>^1` sound for squash,
  merge-commit and rebase merges: the commit is fetched before the check, and a failed fetch
  counts as "not approved". Answered (all own code):
  - Minor: no test held the GitHub path's base, because a squash gives the same answer before
    and after the merge. `delivery::a_merge_commit_merge_judges_the_head_against_the_branch_before_it`
    merges by merge commit through the person's merge. An approval followed by a clean merge
    of the integration branch keeps it; one followed by a merged side branch nobody reviewed
    gets the caveat. Mutations caught: base `None` (the clean case gets the caveat), and the
    base read after the merge, round 2's behaviour (the side branch counts as clean).
  - Nit: the comments and the architecture text said "the merge commit's first parent" and
    "before this merge". They now name what the base is on each path: `<merge>^1`, a rebase
    merge's previous rewritten copy, or a playground's base after its squash.
  - Nit, a trade-off: a merge found elsewhere has no base, so a reviewed head followed only by
    clean merges of the integration branch gets the caveat. Not a defect in this change, since
    the record only informs. It is fixed in fix 1d's pull request, which reads the merge
    commit from GitHub (Plan).


### Fix 1d: the move for a Fable reviewer (head `ca7b4219d` before the review)

- REV-007, the move: `review::move_for_fable` refuses the Agent call and moves the session
  (`switch_task_now`, reason `review_fable`, which interrupts the turn first) to
  `rotation::move_to_also_run`, the best other subscription that can run the session's model and
  Fable. Background work or a queued prompt refuses the call with that reason; with nowhere to
  go, or a move refused moments ago, the round runs `yulius-reviewer-opus` and its prompt gets a
  line saying so.
  - `rotation::tests::a_session_moves_to_a_subscription_that_also_runs_the_reviewer`.
  - `review_rounds::a_round_out_of_fable_is_refused_while_a_move_would_lose_work`: the refusal
    while background work runs, and while a prompt waits (no move, no round). Mutations caught:
    either check dropped. (Before round 1 this was `a_round_out_of_fable_moves_the_session_where_fable_runs`.)
  - `review_rounds::a_round_on_a_subscription_out_of_fable_runs_the_opus_reviewer`: the
    reviewer's prompt keeps its text and gets the line; nothing moves.
  - `review_rounds::real::a_session_out_of_fable_moves_and_is_asked_for_the_review_again`, with a
    real process: the fake agent's new `/agent` runs the Agent tool's `PreToolUse` through the
    real `yulius hook` client. The session moves from `a-first` (Fable used up) to `b-second`,
    the switch completes, and the resumed run gets the prompt asking for the review again.
  - Not tested: a real model's turn interrupted mid-answer. The fake agent is idle when the move
    ends it; the interrupt path is the one a person's switch uses.
- REV-005, a merge found on GitHub: `GhPull::merge_commit_sha` (from GitHub's `merge_commit_sha`
  of a merged pull request) gives `MergedHead::of_pull` the base `<merge>^1` on both reconcile
  paths. `delivery::a_merge_commit_merge_judges_the_head_against_the_branch_before_it` now also
  merges on GitHub and lets Yulius find it. Mutation caught: base `None` (the clean case gets
  the caveat).
- Round 4 of fix 1c's nit: the AC-GIT-2 doc comment is back on its own test.
- How to read it a week later: `GET /api/analytics/reviews` gives `no_fable_rounds` (rounds run
  on Opus because nowhere could run Fable); Task details lists the moves with the reason
  `review_fable`.
- Gate: `scripts/dev ci` exit 0 (4,496 tests passed, 17 skipped; lint, docs and repository
  checks). Security scan: disabled for this project in Yulius, not run.
- **Review round 1** (head `d931781d2`, Fable 5.2). Request changes. Answered (all own code):
  - Major: a move refused after its refusal had said "moves" left the agent told to wait for
    nothing (reproduced: no clone, so the switch was refused at its start check). The hook now
    runs `preflight_start` for the chosen subscription before it answers. A start it would
    refuse sends the round to the Opus reviewer, and nothing moves.
    `review_rounds::a_round_out_of_fable_whose_move_cannot_start_runs_the_opus_reviewer` (the
    reproduction) replaces the in-memory move test, which had accepted a refused switch. The real
    process test is the evidence of the move itself. What remains: the switch can still be
    refused between that check and its start (a quota reached in that second), as for every
    switch; the session then stays as it was.
  - Major: four guards without a test, each now with one, and each mutation caught:
    - round 3 does not move (`a_third_round_out_of_fable_does_not_move`);
    - the cooldown after a refused switch (`a_round_after_a_refused_move_runs_the_opus_reviewer`);
    - the queued prompt (`a_round_out_of_fable_is_refused_while_a_move_would_lose_work`);
    - `review_fable` interrupts first (`a_hand_over_interrupts_like_a_persons_switch`).
  - Minor: the Opus reviewer asked for by name gets the prompt line too
    (`the_opus_reviewer_asked_for_by_name_is_told_why`, mutation caught). The busy refusal and
    the queued-prompt refusal have their own sentences ("answer it or end your turn"). The
    second reconcile path, the sandbox's Git page, is in the merge-commit test; base `None`
    there is caught. The line no longer says "no subscription can run Fable" when the reason is
    a start refused or the cooldown: "cannot run Fable 5.2 now and cannot move to a
    subscription that can".
  - Nit: the looks are bounded by 3 s (the hook client waits 5 s), after which the round runs
    Opus. The spawned switch moves only the run the decision was made for, so a late second
    start cannot restart the session the first move made.
  - Met on the way: the real-process test read the switch's outcome once, which can still be
    "started" when the new run already printed its prompt (an 800 ms delay before the outcome
    is written fails the old assertion, as one parallel run did); it now waits for the outcome.
    `--stress-count 15` of the review tests and 6 runs of the three groups passed.
  - Gate after the answers: `scripts/dev ci` exit 0 (4,500 tests passed). A run before it
    failed once on `pool_supervision::a_reattached_run_at_a_limit_is_restarted_past_it`, the
    known load flake #1286 (untouched here; 10 of 10 alone).
- **Review round 2** (head `32523d6e4`, Fable 5.2). Request changes. It checked every
  round-1 answer and caught each guard's mutation. Answered (all own code):
  - Major, reproduced by the reviewer: the 3 s deadline dropped the start check mid-way. Over
    the disk quota, that check runs the disk upkeep, whose CI checkout claim had no guard: a
    cut-off removal left the sandbox's CI claimed until a restart. Two fixes:
    - The claim is now a value (`CiClaim`, `App::ci_claim_owned`) that goes with the removal to
      its blocking thread and ends there. A dropped upkeep keeps the claim until the removal
      ends, so no run starts in a half-removed checkout, and then frees it.
      `disk::an_upkeep_cut_off_mid_removal_frees_the_ci_lane` fails without it ("the sandbox's
      CI stayed claimed").
    - The start check runs in a task of its own; the deadline ends only the wait.
      `review_rounds::a_slow_move_decision_runs_the_opus_reviewer_within_its_bound` (an
      isolation that takes 5 s): the answer comes within the bound with the Opus reviewer, the
      check runs to its end, and nothing moves. Mutations caught: no deadline, and the check cut
      off.
  - Minor: the live-run check now has its test,
    `review_rounds::a_move_decided_for_an_ended_run_moves_nothing` (held at the new race point
    `ReviewMoveDecided` while the run ends). Mutation caught.
  - Nits: a timed-out decision is logged. REV-007 and AC-8 name every exit. The Evidence no
    longer names the replaced test.

### Fix 2: flaky tests (head before the review: see the review rounds)

- FLK-001: `tool_cli::tests::an_unreachable_yulius_exits_3`,
  `agent_call::tests::nothing_listening_is_unsent` and the three `mcp_bridge` outage tests
  bound a port, let it go and hoped nothing took it. They now hold a loopback socket that is
  bound and not listening (`held_port::HeldPort` in `yulius`, a copy in `yulius-git`'s tests;
  nix's `socket` and `net` features, which add `memoffset` to the lock). A connection to it is
  refused, and nothing else can listen on it while it is held: each test asserts that another
  bind of the port fails, which the old pattern did not hold. The outage tests then listen on
  the very same socket. Probe (AC-9): 40 stress iterations of the five tests while another
  process made 16,021,891 loopback binds and listens: 40 of 40 passed.
- FLK-002 (#1246). The first diagnosis here (a resync in the window) was wrong: a resync gives
  6 loads, never the 4 that CI reported. Review round 1's first reviewer (Fable 5.2, cut off by
  its usage limit, its notes kept) reproduced the exact failure with no resync. Its cause: the
  Team notepad's opening load, answered late on a loaded host, lands after the test's count
  starts, fails and retries 1 s later, which adds 2 loads of another notepad. A test defect: the
  test counted every notepad's loads.
  - Fix: the mock counts loads per notepad (`noteLoadsBy`). Both tests that count loads count
    the sandbox notepad's. The back-off test holds the Team notepad's opening load until its
    count starts (`hold=/api/notes`, the reviewer's probe).
  - Proof: with the old total count and the held load, "expected 4 to be less than or equal to
    3", CI's own message. With the sandbox notepad's count: 5 of 5 runs, and the whole file
    passes (36 tests).
  - A product change tried on the way (a resync leaving a reload that waited for its pause to
    that retry) was reverted after the Opus round. It also kept a newer revision from loading
    after a recovery for up to the 30 s back-off, and #1246 never needed it.
- FLK-003, not redone:
  - #1309 (#1171): the cd test keeps its probe (the deploy answer held until the page is back).
    It passed 5 of 5. With #1309's fix reverted (the page that is shown again does not read
    again), it fails ("toContainText failed").
  - #1313 (#1190): `an_exited_process_not_yet_reaped_is_gone` is the probe on a real unreaped
    child. With `a_stop_settles_a_run_no_server_holds`, 25 of 25 stress iterations passed beside
    16 busy loops on the 64-core host.
- Met and left: `pool_supervision::a_reattached_run_at_a_limit_is_restarted_past_it` (#1286,
  open) failed once in a full gate of fix 1d; it is outside the flakes this fix was measured
  on.
- **Review round 1** (Fable 5.2 on `fa1b9e2c1`, cut off by its usage limit before a report,
  which resets 21:10 UTC; then Opus 5.5 on `1454e14c2`, since Fable could not run). Request
  changes. Answered (all own code):
  - Major: the `notes.js` guard deferred every announcement while a load retry waited, so a
    notepad stayed stale for up to 30 s after the server recovered (the reviewer's probe on the
    head against the base). Reverted. `notes.js` is as on `dev`.
  - Minor: the bound of 2 had little slack on a starved host. It is back to 3 without the
    injected resync, which was not the cause.
  - Nits: `SOCK_CLOEXEC` is not offered on Apple targets, and the gate compiles for macOS, so
    the test-only socket stays without it. A bound socket that is not listening is refused on
    Linux. On macOS it may be dropped (a timeout instead), but these tests run on Linux only
    (`scripts/zig_macos.py` builds, it does not run).

### Fix 3: Yulius CI tests a head while its review runs

- CIR-001, CIR-002: `review::deliver_for_review` runs at every review start. The plan never holds
  `DeliveryAsk::Review` (`steps::tests::a_review_start_is_held_by_no_step`).
  `review_head_not_ready` says why a head cannot go out. Otherwise `try_auto_open_pr` pushes
  the head (and opens the pull request), then `ci_sync_sandbox` queues its run at once.
  - `delivery::a_review_start_delivers_the_head_and_queues_its_run`: with work committed, a valid
    spec and the review step open, the hook says "Yulius CI tests this head"; the head is
    pushed, pull request #1 opened and a run queued with no poll running. Uncommitted work: the
    agent hears "uncommitted changes" and nothing is pushed. A newer head is pushed, and the
    older head's run is cancelled.
  - Mutation caught: no immediate sync ("no Yulius CI run was queued").
- CIR-003: unchanged. `delivery_merge` still needs the exact head's passed run; the existing
  delivery and merge-train tests pass (274 of 274 in the touched groups).
- CIR-004: one rule, in `docs/engineering/workflow.md` step 4, `yulius-verify`, AGENTS.md,
  `om-check-and-commit`, `om-code-review` (the reviewer reads the head's run and runs the tests
  it doubts) and `delivery-pipeline`. The short form ("the whole gate is Yulius CI's") is in the
  Feature, Fix and Chore workflows (within their 1,444-byte budget) and the briefing (same
  length: "(commands, results)"). `yulius-rust-slice` and `yulius-runner` no longer say
  `scripts/dev ci`. `skills::tests::every_place_states_one_rule_for_local_checks` holds them and
  refuses the old wordings. ADR-0342. Generated skills by `apm install --force` (its `target`
  flips in `apm.lock.yaml` restored); repository check passed.
- How to read it a week later: the time from a pull request's opening to its first review
  round's report, against the time its first CI run took (`GET /api/analytics/reviews` with the
  Pipeline tab's CI runs); a run queued within seconds of a round's start.
- Fix 2's round-2 nit: the notes test's comment no longer ties #1246 to a resync.
- **Review round 1** (Opus 5.5, Fable out until 21:10 UTC; head `76286fe21`). Request
  changes. Answered (all own change):
  - Major: the rule was missing from `docs/engineering/git.md` ("`scripts/dev ci` before the
    review") and `development.md` ("the full gate after implementation stabilizes"). Both now
    state it, and the test holds them and refuses both wordings.
  - Major: `om-fix` still ran every gate between rounds. It states the rule now and is held by
    the test. The catalog README and `verification-before-completion` say the checks the change
    touches.
  - Minor: the hook no longer says the head is pushed: "Yulius delivers this head now when it
    can … `ci_status` shows its run". The briefing and `delivery_merge`'s answer name the
    review's start among the moments Yulius pushes.
  - Minor: the spec named an "in review" pull request body and the definition of done, and
    neither was changed. The spec now says what is true (Decisions, CIR-004). AC-10's
    invalid-spec case is in the test.
  - Minor, pre-existing and made likelier: two CI syncs of one sandbox could queue a head
    twice. One sync of a sandbox runs at a time now (`sync_locks`).
    `delivery::two_ci_syncs_of_a_sandbox_queue_a_head_once` holds the first sync after its plan
    (race point `CiSyncPlanned`). Before the lock: 2 runs at one head; with it, 1.
  - Minor: the readiness look in the hook is bounded (1.5 s). A late look says nothing, and the
    delivery checks again. Nit: `dlv_check_spec`'s doc comment is back on it.

### Fix 4: one CI cycle for an armed pull request whose base moved

- MQC-001: `form_train` takes an armed pull request behind its base whose own run passed **or is
  still queued or running**. `drive_train` merges a member only once its own run passed: a
  passed candidate waits for it, within the two-hour test limit, not GitHub's 15 minutes.
  - `merge_train::a_candidate_runs_beside_its_members_own_runs`: two armed pull requests behind
    the base with their own runs queued. Their candidate's run is queued at once beside them,
    nothing is merged into them, and both merge after 2 own runs and 1 candidate run.
  - `merge_train::a_passed_candidate_waits_for_its_members_own_runs`: the own run is held
    running and the candidate passes first. Three passes merge nothing ("merged before its own
    run passed" without the wait: mutation caught). Once the own run passed, it merges.
  - `a_moving_base_never_restarts_a_pre_check` now expects the candidate beside the pre-check;
    the pre-check is still never restarted or cancelled.
- MQC-002: `member_left` dissolves a candidate whose member's own run ended failed, errored or
  cancelled ("#N's own run … ended failed: it leaves the candidate"). The next pass forms it
  again from the others, and the member's armed merge ends in the one-by-one flow with its run
  named. A failed candidate waits for its members' own runs before it is bisected, so a member
  that explains the red result leaves instead of halving the candidate.
  `merge_train::a_member_whose_own_run_fails_leaves_the_candidate`: #1 merges, #2's armed merge
  ends with "failed", and the dissolve reads "#2's own run". Before the wait it was bisected
  instead.
- MQC-003: every merge-train test stays green (32 of 32), among them the red candidate bisected,
  three green in sequence with no run cancelled, and the restart.
- MQC-004: an `exclusive` job takes one turn per sandbox (SPEC-435 `job_turn`), so a
  candidate's run of it waits for the members' own runs of it. Code read; no test of its own.
- MQC-005: each dissolve's audit line ends with `(why: <kind>)` (`dissolved_why`, unit
  `the_why_of_each_dissolve`). `merge_queue::queue_numbers` counts per UTC day and the median
  minutes from a merge's last arming to it (unit tests). `AuditStore::audit_of_sandbox` reads a
  sandbox's entries and its tasks'. `GET /api/analytics/merge-queue`
  (`analytics::the_merge_queue_read_counts_candidates_from_the_audit_log`: 2 formed, 1 merged, 1
  dissolved because a member left, 180 min to merge, another sandbox's entry counted only for
  the admin, a hidden sandbox 404). The Pipeline tab's Merge queue card
  (`pipeline.e2e.ts`: the figures, and the tab without the card when the read fails).
- ADR-0285 amended ("Pre-check beside the candidate"); architecture and its diagram, api.md,
  changes.
- How to read it a week later: `GET /api/analytics/merge-queue?sandbox=<id>&from=&to=` for
  quartio-v2 and this repository, against 12-13 Nov: the median minutes from arming to merge
  (127 there for pull requests that needed a candidate), and candidates dissolved by why.
- **Review round 1** (Opus 5.5, Fable out until 21:10 UTC; head `a52293766`). Request
  changes. Answered (all own code):
  - Major: a slow or stuck own run held every member behind it, and the candidate formed again
    every two hours with that member in it. A passed candidate that outlasts its test limit
    while waiting for a member's own run now holds that member back (`held_back`, until armed
    again). The others form a candidate without it, and it goes one by one once its run passed.
    A failed candidate waits for own runs for at most the wait limit (15 min) from the start of
    its wait, then is bisected as before.
    `merge_train::a_stuck_own_run_holds_its_member_back_not_the_others` and
    `a_failed_candidate_waits_for_own_runs_within_the_wait_limit` (clock-driven, both caps).
  - Major: the 20 s pass fired while a candidate waited for a member's own run. The train records
    the wait (`own_wait`). `train_run_ended` is false within its bound, and `decidable_in` leaves
    a train's members to the train. Both tests assert `!decidable_in`. The own run's end starts
    a pass by itself, as before.
  - Major: person-armed merges (audited under the task id) and armings from before the range
    were missing from the minutes. The read takes the sandbox's task ids (mapped to
    `<slug>#<n>`), and armings from 30 days before the range. The analytics test has a person's
    arming, and an arming before the range: 3 merges, median 180 min.
  - Minors: `dissolved_why` names "moved on" and "the own run of #… ended" as a member leaving
    (unit test). A member whose own run is cancelled leaves
    (`a_member_whose_own_run_is_cancelled_leaves_the_candidate`). The audit read uses the target
    index (a `<slug>#` range, not `substr`) and keeps the newest entries within its limit.
    `yulius wait` says when the candidate waits for a member's own run (the stuck test reads it).
    The spec's route is corrected.
  - Nits: the doc comments are back on their functions, and ADR-0285's three-in-sequence
    sentence is back in its consequence.

### Fix 5: a CI job can say which files it tests

- Fix 4's round-2 minors first:
  - `merge_queue::queue_numbers` takes the range's start: a merge before it uses up its arming
    without being counted (domain test).
  - `analytics::the_audit_read_of_a_sandbox_keeps_the_newest_entries` holds the newest-first
    limit (oldest-first fails it), `<slug>2#n` left out and task ids read.
- CJP-001: `paths` on `[[ci.jobs]]` (`ci::parse_paths`: 1-50 scope-claim globs, normalized,
  each once), rendered back by `render_yulius_toml`.
  `config::tests::ci_jobs_name_the_files_they_test`: matrix jobs inherit them, a round trip,
  and an empty list, `..`, an absolute path, `a/**b` and a repeat are each a configuration error
  naming the job.
- CJP-002, CJP-003: `ci::paths_skip` (unit `a_job_runs_only_for_a_change_to_its_paths`: skipped
  with "no file this job tests changed (paths: …)"; a touched path, `yulius.toml` and an unknown
  change run it; `*` stays in its folder). `GitPort::changed_paths` (`base...head`, NUL-separated;
  `None` past 20,000 files or a non-UTF-8 path; an adapter without it says `None`).
  `ci_queue_placed` decides at queueing: a job is created `skipped` with its reason, output and
  skip code. The executor starts only the queued jobs. A run whose every job is skipped passes
  at queueing and takes no lane.
  `local_ci::a_job_with_paths_runs_only_for_a_change_to_them`, real Git and real jobs:
  - the pull request touching `feature.txt` passes with `web` skipped, its reason in its output
    and nothing run, and its status reads "1 skipped";
  - the integration branch's own run runs `web`;
  - a change to `crates/a/src/lib.rs` runs it.
- CJP-004: the jobs in force are `ci_gate_for_head`'s, as before; `paths` only decide run or
  skipped for each. A rerun goes through `ci_queue_placed` again, and a copied verdict keeps a
  skipped job skipped (`copied_job_verdict`, SPEC-162). A merge candidate is decided by the
  same rule on its commit (built on the integration branch, so its members' changes); no test
  of a candidate of its own.
- `ci_status` shows a skipped job's output, which now holds the reason (SPEC-162's view).
- ADR-0343, architecture, api.md, the `yulius.toml` reference in `config.rs`, changes.
- How to read it a week later: in quartio-v2 once it sets `paths` on `websites`, that job's
  runs against skips per day (the Pipeline tab's job stages), and the median minutes to merge
  of pull requests that do not touch it.
- **Review round 1** (Opus 5.5: the Fable 5.2 reviewer was cut off at its start by Fable's
  weekly limit, which resets 17 Nov 04:00 UTC; head `b0234dc7a`). Request changes. Answered
  (all own code):
  - Major: tree reuse carried a paths skip into the integration branch's own run, because a
    squash-merged pull request gives dev the tree its run tested. `ci_reuse_tree` no longer
    reuses into a branch's run a source that skipped a `paths` job.
    `local_ci::the_integration_branch_runs_a_paths_job_a_merged_tree_skipped` (reproduced first:
    `web` copied as skipped).
  - Major: the executor sent pre-skipped jobs through `ci_job`, where they waited for an
    `exclusive` turn and a slot and took a budget share. It now starts only the queued jobs, and
    a run of skipped jobs passes at queueing.
    `local_ci::a_skipped_exclusive_job_does_not_wait_for_its_turn` (3 lanes, a CPU each: the pull
    request's run ends while dev's `web` holds its turn). Without the fix it ran until dev's
    ended. The run of skipped jobs passing at queueing is
    `local_ci::a_run_of_skipped_jobs_passes_when_it_is_queued` (added in round 2).
  - Major: `--name-only` listed a rename by its new path only, so a file moved out of `paths`
    did not count. `--no-renames` lists both. Tested at the adapter:
    `git_ops::changed_paths_list_renames_deletions_and_stop_at_the_limit`, with the rename, a
    deletion, the base's own later commit left out, and `None` past the limit.
  - Minors:
    - A requeue (restart, disk quota) keeps a paths skip, while a job's own skip runs again
      (`local_ci::a_requeued_run_keeps_its_paths_skip`).
    - A skipped job gets its start and end after insertion, so its `job` span is recorded with
      outcome `skipped` (asserted in the paths test).
    - A path without a glob is the folder of that name too, and a changed path no glob can
      match runs the job (unit).
  - Nits: the skip code is 78 as for any skipped job. `paths` before `name` names the job
    (config test). The train note replacing the reason stays: a copied verdict says where it
    comes from.
- **Review round 2** (Opus 5.5, Fable out until 17 Nov 04:00 UTC; head `f9a060195`). Request
  changes. It confirmed each round-1 answer by mutation. Answered (all own code):
  - Major: the run of skipped jobs passing at queueing had no test.
    `local_ci::a_run_of_skipped_jobs_passes_when_it_is_queued`, with no dispatch: passed with
    its reason, both jobs skipped, the status success "2 skipped", the gate's answer Passed.
    Turning it off fails the test.
  - Minor: such a run never reached the executor, which starts the merge pass at a run's end.
    It records its sandbox (`passed_when_queued`), and the dispatcher, woken at once, starts the
    pass (tested in round 3).
  - Nits: tree reuse into a branch's run goes on past a same-tree source that skipped a `paths`
    job, to an earlier one that ran it. A run passing at queueing records the current time.
- **Review round 3** (Opus 5.5; head `375dd4dce`). Request changes: two answers of round 2 had no
  test. Answered with tests, each caught by its mutation:
  - `merge_train::a_merge_armed_on_a_run_of_skipped_jobs_happens_at_once`: dev's own run ends
    first, then the armed pull request's run passes at queueing. The dispatcher alone merges it.
    Without the drain's merge pass it waits ("the armed merge waited for another pass").
  - `local_ci::a_branch_reuses_a_same_tree_run_that_ran_its_paths_job`: an older passed run of
    another commit with the same tree ran `web`; the integration branch's run reuses it, not the
    newer pull request's run that skipped `web`. With `return` instead of `continue` it runs
    again.

### Fix 6: an approval survives a small follow-up

- APR-001, APR-003: `Git::follow_up_since` walks the first parents from the head to the reviewed
  commit (at most 64 commits). A merge passes only when its second parent is on the integration
  branch and its tree is what `merge-tree` gives; any other merge, or a reviewed commit off that
  way, is `None` (refused). Every other commit is counted by `Git::commit_lines` (`git show -U0
  --no-renames --first-parent`; unit `unified_zero_hunks_give_line_numbers`).
  `App::follow_up_since_review` then refuses a file of the reviewed change that left the pull
  request's change (`changed_paths` of the reviewed commit and the head).
- APR-002: `follow_up::ProductRule` (unit `product_code_is_every_file_but_tests_and_documents`)
  and `FileLines::product_lines` (unit `a_follow_up_counts_product_lines_commit_by_commit`: a
  Rust file's lines from its first `#[cfg(test)]` on do not count; the bound is 20, 21 is over).
  `[review] product` / `not_product` in `yulius.toml` (`parse_yulius_toml_review`; unit
  `the_review_table_sets_what_counts_as_product_code`: unknown keys, an empty list, `/etc`, `..`
  and `[[review]]` are errors, in the whole contract too). The integration branch's table is
  read, so a pull request does not loosen its own count; one that does not parse refuses.
- APR-004: the summary and the armed merge's audit entry end with "[reviewed at X; since then N
  product lines in M files: a 12, b 3]"; a refusal says the count and the files, or the reason.
- AC-17: `delivery::an_approval_carries_across_a_small_follow_up`, real Git through the agent's
  MCP `delivery_merge`:
  - 20 product lines plus 150 test and 50 document lines: armed, "since then 20 product lines in
    1 file: src/extra.rs 20" recorded;
  - 21 lines and two commits of 12 (24): refused with the count;
  - 40 lines in a Rust test module plus 3 lines: accepted, 3 counted;
  - `src/feature.rs` of the reviewed change removed: refused naming it;
  - a clean merge of `dev` plus 5 lines: accepted;
  - a conflicted merge, a clean merge that changed another file too, and a merge of a side
    branch: refused;
  - dev's `[review] not_product = ["src/gen/"]`: 25 lines there do not count; a `[review]`
    that does not parse refuses, naming it.
- Mutation checks (each restored after): dropping the left-the-change check, the test-module
  line, the tree check, the second parent's ancestor check, the bound, the configured rule (the
  default instead) or the parse refusal fails the test, each one. The tree and ancestor checks
  survived the first version of the test (a conflicted merge is refused by `merge-tree` alone);
  the clean merge that changed more and the side-branch merge were added for them.
- APR-005: the rule's sentence in `delivery-pipeline`, `om-code-review`, `git-hygiene`,
  `yulius-review`, `yulius-git`, the workflow, git.md and AGENTS.md, and the bound in
  `delivery_merge`'s description; `skills::every_place_states_the_follow_up_bound` holds them
  (dropping it from AGENTS.md fails it). The briefing has no byte to spare and says nothing new.
- ADR-0345 (amends ADR-0066 point 3), api.md's `[review]`, the `yulius.toml` reference in
  `config.rs`, changes.
- `delivery::a_review_carries_over_clean_merges_of_the_integration_branch` (SPEC-083) refused a
  one-line commit after the review; under APR-001 that line keeps the approval, so its new work
  is now 22 lines (21 added, 1 removed), still refused.
- Local runs: `cargo test -p yulius --test main` 1705 passed, 1 failed:
  `pool_supervision::a_reattached_run_at_a_limit_is_restarted_past_it`, the known load flake
  #1286 (passes alone, 3 of 3). `cargo test -p yulius-git`: one `git_ops` test at a time fails
  `ETXTBSY` on a fake git script under load (a different test in different runs; filed as #1332,
  present on dev, not touched here). Domain 529, app 576, clippy and fmt clean.
- How to read it a week later: `SELECT at, detail FROM audit_log WHERE action IN
  ('delivery.agent.merge', 'delivery.agent.merge_armed') AND detail LIKE '%since then%product
  line%'` lists the merges that used the bound, with their counts; the analytics read's rounds
  per pull request (fix 1's) says whether rounds fell.
- Review round 1 (Opus 5.5 while Fable's weekly limit held, head `de8377238`): request changes,
  1 blocker and 5 majors in own code, each answered test-first. All seven mutations below fail a
  test, and each is restored after.
  1. Blocker: `git show` follows the clone's settings (`color.diff=always` → 0 lines; `-diff`
     in `info/attributes` or a committed `.gitattributes` → "Binary files differ").
     - The fix: `commit_lines` reads `git diff-tree -r -p -U0 --text --no-color --src-prefix=a/
       --dst-prefix=b/` with `GIT_ATTR_NOSYSTEM=1`, `core.quotePath=false` and an empty
       `diff.orderFile`. `diff-tree --numstat -z` gives the exact paths, and its counts must
       agree with the patch. `--numstat` gives `-` for a `-diff` file; the patch, with
       `--text`, does not, and decides.
     - A changed line with a NUL byte or over 1,000 bytes makes its file opaque, and product
       code that is opaque refuses.
     - Tests: `git_ops::a_commits_lines_ignore_the_clones_settings` (real Git: colour, no
       prefix, `-diff` everywhere, a binary file, a path with a space and `ü.rs`), and the
       delivery cases "the clone's settings do not hide lines" and the NUL byte. Dropping
       `--text`, or the opaque refusal, fails them.
  2. Major: the stateless parser.
     - The fix: `parse_unified_zero` starts a file only at `diff --git` and gives a hunk
       exactly the lines its header counts; `None` (refused) when a hunk ends short.
     - Test: unit `a_patch_is_read_by_its_hunk_counts` (a `++ b/docs/x.md` content line, `\ No
       newline` markers, a new file, a mode change, a short hunk, NUL and long lines).
  3. Major: the first `#[cfg(test)]` ended the count.
     - The fix: `follow_up::test_module_spans` excludes only a `#[cfg(test)] mod name {` to the
       first `}` at its indentation. A module never closed, `mod tests;` and a single item are
       counted. APR-002 and the ADR are amended.
     - Tests: unit `only_test_modules_are_test_spans`; the delivery case
       `#[cfg(test)]\nmod tests;` + 30 lines is refused (32). Marking every line a test fails it.
  4. Major: the answer lacked the count, and the test read it from the audit.
     - The fix: an accepted answer has `review: "reviewed at X; since then …"`.
     - Test: `deliver_after_follow_up` returns the answer and the audit entries apart, and each
       accepted case asserts both. Dropping the field fails it.
  5. Major: the merge record (`delivery.review.models`) used only the clean-merge rule.
     - The fix: it also accepts a follow-up within the bound, judged against the merge's base.
     - Test: `a_merge_commit_merge_judges_the_head_against_the_branch_before_it` adds a small
       follow-up, merged three ways, which is not flagged; the side branch still is. Dropping
       the call fails it.
  6. Major: `origin/<branch>` resolves a tag or a local branch of that name first.
     - The fix: the integration branch is `refs/remotes/origin/<branch>` here and in
       `review_covers_against` (on dev before this change; fixed here because the follow-up
       relies on it). A failed refresh now refuses in both, where it only warned.
     - Tests: the side-branch merge with a tag and with a local branch `origin/dev`; and the
       origin moved away, which refuses with "could not be refreshed". The short name fails the
       first, keeping the warning fails the second.
  7. Minor: a Git error is logged and said ("Git could not walk the commits …"), not read as a
     conflict.
  8. Minor: the playground's merged and armed audit entries end with the count too.
  9. Minor: `product` globs no longer make tests or documents count (unit
     `product_code_is_every_file_but_tests_and_documents`).
  10. Minor (design), answered in words: shipped instruction files (`SKILL.md`, `AGENTS.md`)
      are documents by the repository's own rule (`is_documentation_path`, the one the spec
      check uses). The person decided that documents do not count. A project that wants them
      counted can drop them with `not_product`, which only removes paths, or narrow `product`.
      The bound is per follow-up after an approval, and every such merge's record names the
      files. Not changed.
  11. Nits:
      - `only_clean_merges_since` is now the follow-up walk with nothing counted.
      - The left-the-change check uses a set.
      - The second refresh in `delivery_merge` (once for the clean-merge rule, once for the
        count) stays: it is a local fetch, and only when the head moved past the review.
- Review round 2 (Opus 5.5, head `c6829dfec`): request changes, 1 blocker and 2 majors in own
  code. Round 1's findings 1, 2 and 4 to 11 are closed (the reviewer's probes against the real
  `commit_lines`); 3 was closed only in part. Answered test-first; all six mutations below fail
  a test.
  - A. Blocker: `git replace` (and grafts, or a forged commit-graph) made a commit look
    different to every read.
    - The fix: every Git command of Yulius now runs with `GIT_NO_REPLACE_OBJECTS=1`,
      `GIT_GRAFT_FILE=/dev/null` and `core.commitGraph=false` (`runner.rs`, pinned by the
      runner's test). This also covers the clean-merge walk, which was exposed on dev before.
    - Test: `git_ops::a_follow_up_is_read_from_the_commits_themselves` (a 200-line commit
      replaced by a 1-line one, plus a graft that skips it). Dropping either variable fails it.
    - The commit-graph setting has no test of its own: a forged graph file is not something the
      test can write.
  - B. Major: test-module spans were read as lines of text.
    - The fix: `test_module_spans` reads Rust tokens. `code_only` blanks line, nested block
      and doc comments, strings, raw strings and character literals (lifetimes stay code); a
      `#[cfg(test)]` whose next item, after further attributes and a visibility, is `mod name {`
      spans to its matching brace. Anything that does not lex gives no span.
    - Tests: unit `test_spans_are_read_from_rust_tokens` (a fake module in a block comment, in
      a string, in a raw string; braces in a char, a string, a comment and `'\u{7D}'`; an
      unformatted close; nested comments; unclosed ones; spaced attributes). Unit
      `real_files_have_their_test_module_and_nothing_else`: four of this crate's own files,
      whose tests hold these tricks, each have exactly their test module. The delivery case of
      a comment-borne module is refused (34). Lexing nothing, or never closing, fails them.
  - C. Major: the integration branch was read back from the clone after the fetch.
    - The fix: `App::integration_head` reads the commit from GitHub's branch (a playground: its
      own repository on the server) before the fetch, and every check takes that commit. Here
      and in `review_covers_against` (whose ref reading was on dev before).
    - Tests: in the delivery fixture, a ref moved right after every fetch
      (`forged_after_fetch`), with a merged side branch, is refused; a failed fetch and an
      origin that cannot be read refuse, each with its reason. Reading the ref again fails the
      first.
  - D. Minor: a type change (a link become a file) is one record and two patch sections; they
    are paired now (in the `git_ops` test above). Dropping the pairing fails it.
  - E. Minor, answered in words: when a fetch fails, the armed merge is dropped (fail closed).
    The message the agent gets says the branch moved and asks for a review of the new head. It
    makes no claim about clean merges, so it is not wrong; the agent re-arms once the fetch
    works.
  - F. Nit, taken: an opaque line is now one over 200 bytes (formatters wrap at 80-120), not
    1,000.
  - `git_ops` wrappers that find the subcommand by its place (`${31}`) now look two places
    later, for the new `-c` pair.
