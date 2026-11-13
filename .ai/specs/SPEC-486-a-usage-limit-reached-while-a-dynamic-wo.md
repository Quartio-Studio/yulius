---
yulius-task: a126e4ca-1a96-4142-884c-c5162cac1549
status: done
---

# SPEC-486: a usage limit met while a dynamic workflow runs moves the task

Status: done
Date: 2026-11-13
Owner: Yulius task #468
Scope authorization: the request of task #468 (by MrFishPL, 2026-11-13): "in ultra-code it didn't
change subscription, fix", with the later note "when I typed something, the superagents got lost".
Extends SPEC-034 (the resume past a usage limit, [ADR-0027](../../docs/decisions/0027-step-back-past-a-usage-limit.md)),
SPEC-072 ([ADR-0058](../../docs/decisions/0058-the-clis-own-continuation-ends-a-limit-pause.md)),
SPEC-079 (background work) and SPEC-087 MET-004 (sub-agents' tokens); decision in
[ADR-0326](../../docs/decisions/0326-a-workflow-agents-usage-limit-is-the-sessions.md).

## Goal

A usage limit met while a session runs a dynamic workflow (Claude Code's Workflow tool,
"ultracode") is handled as any other limit: the task goes on by itself on a subscription that can
run its model, with the workflow resumed from its run id, and waits (shown as paused, with the
reset) only when no subscription can.

## Context

- **What the person saw** (task #182 of another sandbox, 2026-11-13): the terminal read "Usage
  limit reached · continuing automatically at 6:10pm", "Waiting for 1 dynamic workflow to finish"
  and the workflow's row "paused · usage limit resets at 6:10pm". Other subscriptions were free,
  yet the task stayed. A minute later, after a typed "continue", the workflow's row was gone.
- **The cause, reproduced with the installed Claude Code 2.1.323** run in a terminal against a
  local stand-in for the model API that refuses requests the way a subscription at its limit
  does (HTTP 429 with `anthropic-ratelimit-unified-status: rejected`; scripts and outputs in
  `$HOME/.cache/yulius-task-468/probe`, everything synthetic):
  1. A workflow's agents write their conversations to
     `<session>/subagents/workflows/<run id>/agent-<id>.jsonl`. Yulius follows only
     `<session>/subagents/*.jsonl` (`SubagentTails`), and reads those for token usage alone.
  2. When the limit refuses a workflow's agent, its record (`isApiErrorMessage`,
     `apiError: "usage_limit_reached"`, the reset in the text) is written to that agent's file
     only. Claude Code pauses the workflow (the row "paused · usage limit resets at …") and
     **does not wake the session's own thread**: its transcript ends with an ordinary finished
     turn (`turn_duration` with `pendingWorkflowCount: 1`), with or without a goal (a goal's
     check is not made while a workflow is unfinished). For Yulius nothing happened: no limit,
     no move, and the task reads as a session that ended its turn.
  3. The launch itself is not read either: its tool result is
     `{status: "async_launched", taskType: "local_workflow", taskId, runId, scriptPath}`, and
     `background_started` asks for an `agentId`. `pendingWorkflowCount` is not read. So a
     session that waits for a workflow has, for Yulius, no background work.
  4. When the session's own thread is refused too (it asked the model while the limit was in
     effect: the first screenshot's "Usage limit reached · continuing automatically"), the
     limit record is in the session's transcript and Yulius moves the task as at any limit:
     the process ends, and with it the workflow. `claude --resume` then writes, for the
     transcript only, "Background workflow … (run wf_…) didn't finish before the previous
     session ended … relaunch with Workflow({scriptPath, resumeFromRunId})", and starts
     nothing: the resumed turn may or may not pick the workflow up.
- **What Claude Code offers.** A run's journal (`journal.jsonl` beside its agents' files) keeps
  every finished agent's result. `Workflow({scriptPath, resumeFromRunId})` in the same session,
  also after the process was ended and resumed with `--resume` (as a move does), returns those
  results at once and asks the model only for the unfinished agents: in the probe the finished
  agent was not asked again, the refused one and the next were. A prompt given with `--resume`
  starts that turn in both cases (the session's turn finished, or cut by the limit).
- **A typed prompt.** In 2.1.323 a prompt typed while the workflow is paused does not end the
  workflow: its row stays and "Waiting for 1 dynamic workflow to finish" returns after the turn
  (probe, with the session's own request answered and with it refused "again"). What removed the
  row in the person's session could not be reproduced with a scripted model; the likeliest cause
  is the session's agent acting on "continue". Whatever it was, the run's journal stays on disk
  and the run can be resumed.
- **Other background work.** A background agent (the Agent tool) refused by the limit ends and
  Claude Code notifies the session's thread at once; its request is then refused in the
  session's own transcript, which Yulius reads (probe). Background shells make no model
  request. The cause is the Workflow tool's alone.

## Requirements

- WFL-001 (workflow agents' transcripts are followed). `SubagentTails` follows
  `<session>/subagents/workflows/<run>/agent-*.jsonl` as it follows the sub-agents' files: the
  run's window, regular files in real folders only (a link in the place of the `workflows`
  folder, of a run's folder or of a file is not followed), bounded beside the sub-agents'
  bound: of the first `MAX_WORKFLOW_LISTED` = 1024 entries listed, the `MAX_WORKFLOW_RUNS` = 64
  run folders changed last; their files changed since the run under watch began (an older file
  holds none of its lines), the newest runs' first, up to `MAX_WORKFLOW_FILES` = 512. A file
  that cannot be read does not cost the lines of the others. Their token usage is the
  session's (MET-004). `journal.jsonl` and other files there are not read.
- WFL-002 (a workflow agent's limit is the session's). A usage-limit refusal recorded in a
  workflow agent's transcript is reported as `WorkflowLimitReached`, with its reset, window and
  scope, once the session's transcript is read to its end (so a replay after a restart judges
  it against all the session did later), and only while it is news:
  - not when a prompt recorded later than the refusal was worked on by the session's thread
    (the model answered after it: history, as SPEC-072 LRS-001 says of a limit the session
    continued past); an answer that was under way when the agent was refused belongs to an
    earlier prompt and changes nothing;
  - not while the thread's own limit stands (that one was reported);
  - not when its reset has passed (Claude Code continues the workflow by itself then);
  - several refusals are one report (the latest), and nothing more is reported until a prompt
    reaches the thread again.

  What it binds: a limit of the subscription, or of a named model family, is what its record
  says. A limit of "the model that runs" (a model's usage credits: `LimitScope::Model(None)`)
  is of that agent's model, which the agent's `agent-<id>.meta.json` names when the workflow's
  script gave it one (`model`, an id or an alias) and leaves out when it runs the session's;
  when that cannot be told the limit is not reported. A task's session is at the limit when it
  binds the model the run was launched with: then it is handled as the session's own limit
  (its turn is over, the pool decides). A limit of another model family is kept in the
  subscription's reading and changes nothing for the task. The Superagent does not act on a
  workflow agent's limit (see Decisions). A sub-agent's refusal outside a workflow stays
  unreported: the session's thread is woken and refused itself.
- WFL-003 (a workflow is background work). A Workflow launch in the session's transcript is
  `BackgroundStarted` (an agent, by its `taskId`) and names its run (`WorkflowLaunched`);
  its end is the task notification with a status for that `taskId`, as for an agent.
  `pendingWorkflowCount` of a turn record counts with `pendingBackgroundAgentCount`. So a task
  whose session waits for a workflow reads "Working · N background tasks still running", a
  proactive move waits for it (BGW-004), and Yulius knows a run is unfinished
  (`dynamic_workflow::Unfinished`, at most 8 runs). A run stopped in the terminal leaves no
  notification: it is over once a turn record counts less background work than the runs
  known, the oldest first (while a background agent is also pending, the count cannot tell and
  the run stays known until the next record that can). The unfinished runs are part of a
  replay's facts, so they are known after a restart. A notification Claude Code writes for the
  transcript only (`queueTranscriptOnly`: a resumed process naming a run the ended one left
  unfinished) ends the background work it names and its hand-over from Claude Code's queue
  (`QueueChange::Written`), and starts no turn. A run id passes only as letters, digits, `_`
  and `-` (it is later named in a prompt).
- WFL-004 (a relaunch resumes the workflow). A launch that resumes a session while a workflow
  of it is unfinished opens with one prompt: the session continues in a new process, the
  workflow stopped with the previous one, each unfinished run (ids named) is to be resumed
  with the Workflow tool by its script path and run id (`resumeFromRunId`) and not started
  again, then the task continued. Past a limit (`RunStart::AfterLimit`: an automatic move, a
  person's move at a limit, the resume of a `limit` pause) that prompt replaces the silent
  continuation of SPEC-034, and the transcript is stepped back over the limit as before; a
  launch that has a prompt of its own (a held prompt's resume, a person's move of a working
  task) keeps it first. In goal mode the goal is typed after that turn, as after any opening
  turn, and an idle task is not given its idle activity back by such a launch. The terminal
  notice of the relaunch says the workflow is resumed. The runs stay known until the new
  process's transcript is read (its own notice ends the old launches, the resumed run is read
  from its new launch), so a launch that fails, or runs again for another subscription, still
  tells its session; a fresh session knows none (a run is resumed in its own session only).
- WFL-005 (waiting). With no subscription that can run the task's model, the limit opens a
  `limit` pause as today: the task reads "Paused: … (resets ~HH:MM)" and resumes at the
  earliest reset by WFL-004. No Needs you unless nothing can run it (SPEC-464 UCL-010).
- WFL-006 (a prompt at a paused workflow). A prompt a person or Yulius typed that reaches a
  session whose workflow met a limit gets, through the prompt hook's additional context, a
  note, while the workflow is paused: until it is relaunched or ends, the thread's model
  answers again, or the limit's reset passes. Claude Code's own wake-ups get nothing added.
  The note: the workflow (run ids named) is paused at a usage limit, it is not to be
  started again, and it can be resumed by its run id. With no subscription free the hook holds
  a prompt as before (ROT-010), so this is the case of a session that stays at its limit with
  another subscription free: a refused move, or the moments before a move. Yulius itself types
  nothing into a session in a `limit` pause (as before), and nothing here types "continue".
- WFL-007 (the fake agent). `tools/fake-agent` writes what Claude Code writes
  (`workflow.rs`): `/workflow` launches a run (the launch records, a turn record with
  `pendingWorkflowCount`, an agent file with a finished agent); `--workflow-limit` makes the
  run's second agent write the limit record to its file only; with `--limit-after` reached the
  session's own turn is cut too; a resumed process names the unfinished runs for the
  transcript only, and a prompt naming a run and `resumeFromRunId` writes the relaunch, the
  remaining agents and the run's end. Each agent has its meta file, and
  `--workflow-limit-model MODEL` makes the refusal one of the usage credits of the model the
  agent runs (MODEL, named in the meta file, or the session's with `session`). A prompt hook's
  additional context is printed. Fixtures are synthetic, shaped as the probe's records.

## Plan

- [x] The cause is reproduced with Claude Code 2.1.323 against a local stand-in API and written here
- [x] The harness follows workflow agents' transcripts and reads a workflow's launch and pending count
- [x] The transcript watch reports a workflow agent's limit as the session's
- [x] The fake agent writes a workflow's records and a limit in its agent's transcript
- [x] A limit during a workflow moves the task and the relaunch resumes the workflow by its run id
- [x] A prompt typed at a workflow paused by a limit tells the agent to resume it
- [x] ADR-0326, the architecture, product and contract documents and the changes entry agree
- [x] The gate and the security scan are run and the head is given to an independent review
- [x] The review's first round is answered: the Superagent, another model's limit, stale runs and eight smaller findings

## Acceptance criteria

- With two subscriptions and the fake agent's `--workflow-limit`, the task moves to the second
  subscription with no person and no pause; the new run's launch carries the workflow prompt
  with the run id, and its transcript shows the run relaunched with `resumeFromRunId`.
- With one subscription, the same limit opens a `limit` pause with the limit's reset
  (`waiting_capacity`, `resumes_at`), and the task resumes at the reset with the same prompt.
- Without the change both tests fail: the task stays running on the first subscription.
- With the session's own turn cut by the same limit, the task moves once, and the relaunch
  opens with the workflow prompt on a transcript stepped back over the limit.
- A move a person asks for while a workflow runs (no limit) relaunches the session with the
  workflow prompt; nothing moves or pauses before that.
- A sub-agent's limit outside a workflow is not reported; a workflow agent's limit the session
  worked past is not reported (unit tests of the watch).
- With a move refused, a typed prompt gets the hook's note naming the run.
- A workflow agent on another model refused for that model's usage credits: the task stays,
  the subscription's reading keeps that model's limit and not the session's model's. The same
  refusal of an agent on the session's model moves the task.
- A relaunch that fails after the limit stops the task; the launch that continues it still
  carries the workflow prompt.
- After a Yulius restart with a workflow paused at a limit, the run is known again, the task
  still waits, and its resume carries the workflow prompt.
- The Superagent's own limit moves it; a workflow agent's limit does not (unit test).
- The real Claude Code's records of the probe runs read as the fixtures say (a replay through
  the watch).

## Test plan

- Unit (yulius-harness): the tails find `workflows/<run>/agent-*.jsonl`, skip `journal.jsonl`,
  links and files past the bounds; the launch record gives `BackgroundStarted` and
  `WorkflowLaunched`; `pendingWorkflowCount` adds to the pending agents.
- Unit (wiring): a workflow agent's limit is reported after the session's lines, once; held
  while the session's transcript is not read to its end; dropped when the session worked after
  it; a plain sub-agent's limit is not reported.
- Unit (app): the facts' unfinished runs (launch, end, relaunch); the prompt's text.
- Integration (`crates/yulius/tests`, fake agent): the acceptance criteria above.
- The real Claude Code against the stand-in API (not in CI: it needs the installed CLI): the
  probe runs named in Context.

## Decisions

- **The limit is read from the workflow agent's transcript**, not from the terminal (rejected in
  ADR-0055) and not from Claude Code's notices (none is written for a paused workflow).
- **Move at once, do not wait for the workflow.** The workflow cannot go on at the limit; its
  finished agents' results are in the run's journal and come back on the resume. Work of an
  agent in flight when the process ends is asked again (its file changes stay in the worktree).
  The session's own thread may be in a turn of its own when a workflow's agent is refused:
  that turn ends with the process too, as its next request would be refused; the relaunch's
  prompt tells the agent to continue the task after resuming the workflow.
- **The Superagent is not moved on a workflow agent's limit.** Its relaunch types nothing
  (SPEC-074), so it could not have the workflow resumed, and a move would end the workflow
  with nothing left to continue. Its paused workflow stays in its process and Claude Code
  continues it at the reset, as before this change; a limit its own thread meets moves it as
  before. That is why the watch reports a workflow agent's limit as an observation of its own.
- **A model's limit binds a session of that model only.** A workflow's script may give an
  agent another model. Booking that model's limit against the session's model would fill a
  window that is not used up and move a task off a subscription that can run it, again on the
  next one. Such a workflow stays paused where it is, as Claude Code leaves it: moving the
  task for another model's sake is not decided here.
- **What is left when the state is lost.** The unfinished runs live in the server's memory,
  rebuilt from the transcripts of a run that is watched. A task that waits without a process
  across a Yulius restart is resumed without the prompt; Claude Code's own notice in the
  transcript still names the run. The check that a session's `workflows` folder is no link
  and its listing are two steps, as for the `subagents` folder above it: what is read there is
  parsed as a transcript and nothing else.
- **One prompt at the relaunch, an exception to "nothing typed" (ADR-0027).** Claude Code does
  not resume an orphaned workflow by itself and, when the session's turn had ended, has no cut
  turn to continue: without a prompt the moved session would sit idle. The prompt replaces
  `CLAUDE_CODE_RESUME_INTERRUPTED_TURN` for that launch.
- **A sub-agent's limit outside a workflow is not the session's signal.** Reporting it too
  would act twice on every such limit; the session's own record follows within the same turn.
- **Yulius relaunches at the reset even when Claude Code would continue the paused workflow by
  itself.** That continuation is written only to the agents' files; reading it as "the session
  went on" would also take an agent that finishes after the session's own refusal for a
  continuation. The relaunch costs the agents in flight, never the finished ones.

## Evidence

- **The real Claude Code, reproduced.** Claude Code 2.1.323 run in a terminal against a local
  stand-in for the model API (`$HOME/.cache/yulius-task-468/probe`: `stub_api.py`, `drive.py`,
  the runs' outputs; a subscriber's sign-in imitated with made-up tokens, nothing left the
  host). Runs and what they showed:
  - `limit` (the workflow's second agent refused): the record `apiError: "usage_limit_reached"`
    is in `subagents/workflows/<run>/agent-<id>.jsonl` only; the session's transcript ends with
    `turn_duration` (`pendingWorkflowCount: 1`); the screen reads "Waiting for 1 dynamic
    workflow to finish" and "paused · usage limit resets at 7:27pm". A typed "continue" was
    answered and the workflow's row stayed.
  - `goal-run`, `goal-limit` (a goal set): the goal's check is not asked while the workflow is
    unfinished (first check after the workflow's end); with the agents refused, nothing is
    written to the session's transcript for 85 s, until a prompt is typed.
  - `limit-both` (the session's next request refused too): the session's transcript has the
    limit record and "Usage limit reached · continuing automatically at …", as in the first
    screenshot; a typed "continue" gives "Usage limit reached again …", and the row stays.
  - `limit-kill-resume`, `a-resume-prompt`, `both-resume-prompt` (the process ended and the
    session resumed, as a move does): the resumed process writes the transcript-only notice
    ("Background workflow … (run wf_…) didn't finish before the previous session ended") and
    asks the model nothing; `Workflow({scriptPath, resumeFromRunId})` then asks the model for
    the refused agent and the next one only (the stand-in's log: `alpha` once, before the
    limit), also when the resume carries the prompt on the command line.
  - `bg-agent` (a background agent refused): its notification wakes the session's thread at
    once and the thread asks the model, so the limit reaches the session's own transcript.
- **The real records through the watch** (a throwaway test, not kept; output in
  `target/evidence/real-replay.log`). The `limit` and `goal-limit` runs cut before the typed
  "continue": `WorkflowLaunched`, `BackgroundStarted`, `TurnEnded background=Some(1)`, then
  `LimitReached` with the reset (`1794598079000`), window 300 and the refusal's time; the
  facts say `limit_pending=true`, the run unfinished and paused. The whole `limit` run (the
  thread answered after the limit): no limit reported. `limit-both`: the session's own two
  limits, no third one. `limit-kill-resume`: the transcript-only notice ends the old launch
  and starts no turn. `bg-agent`: as before the change.
- **Tests that fail without the fix** (`target/evidence/mutants.sh`, log
  `target/evidence/mutants.log`, at `ccb0f3ad1`): nine mutants, each removing one part, against
  the nine integration tests (`workflow_limit::`, eight, and the hook test). Unmutated: all
  nine pass (1.5 to 2.6 s each).
  - 1 and 2, the workflow agents' files not followed and their limit not reported (the bug as
    it was): seven fail, among them
    `a_limit_met_by_a_workflow_s_agent_moves_the_task_and_resumes_the_workflow` after 90 s
    waiting for the move and
    `with_no_subscription_free_the_task_waits_and_then_resumes_the_workflow` after 30 s waiting
    for the pause; the two that pass need no agent's limit (the session's own turn cut, a
    person's move).
  - 3, the launch not naming its run: eight fail. 4, the relaunch past a limit saying
    nothing: five fail. 5, the prompt hook adding nothing: the hook test fails.
  - 6, another model's limit taken for the session's, and 7, the agent's own model not looked
    up: `a_limit_of_another_model_met_by_a_workflow_s_agent_leaves_the_task` fails.
  - 8, the runs forgotten when the launch is decided:
    `a_relaunch_that_fails_still_knows_the_workflow_at_the_next_one` fails. 9, a replay's
    unfinished runs not kept: `a_workflow_paused_at_a_limit_is_known_after_a_restart` fails.
- **Unit tests.** `yulius-harness`: `subagents::tests::a_workflow_s_agents_are_followed_and_marked`,
  `…_are_read_over_the_run_s_window`, `a_workflow_s_links_are_not_followed` (a linked file, a
  linked run folder, a linked `workflows` folder), `the_workflow_bounds_hold`,
  `the_newest_runs_and_this_run_s_files_come_first`,
  `a_file_that_cannot_be_read_does_not_cost_the_others_lines`;
  `claude::tests::a_workflow_s_launch_is_background_work_and_names_its_run`,
  `a_turn_end_counts_the_pending_workflows`, `a_transcript_only_notification_starts_no_turn`,
  `a_workflow_agent_s_refusal_reads_as_a_limit_with_its_reset`. `yulius-domain`:
  `agent_cli::tests::an_entry_written_for_the_transcript_only_ends_its_hand_over`. `yulius`
  (wiring): `a_workflow_agent_s_limit_is_reported_as_the_session_s_once`,
  `a_workflow_agent_s_limit_is_reported_only_while_it_is_news`,
  `a_model_s_limit_met_by_a_workflow_agent_is_of_that_agent_s_model`. `yulius-app`:
  `dynamic_workflow::tests` (five), `cockpit::tests::a_workflow_agent_s_limit_is_not_the_cockpit_s_to_act_on`.
  `fake-agent`: `workflow::tests` (two), `cli::a_workflow_is_launched_paused_by_a_limit_and_resumed`
  and `cli::a_workflow_agent_s_model_refusal_names_its_model_in_the_meta_file`.
- **A workflow agent's own model, in the real Claude Code** (probe runs `agent-models`,
  `agent-models-limit`): `agent(prompt, {model: 'haiku'})` and `{model: 'claude-opus-4-5'}`
  ask the API for `claude-haiku-5-5` and `claude-opus-4-5`, and the agent's
  `agent-<id>.meta.json` then has `"model": "haiku"` and `"model": "claude-opus-4-5"`; an
  agent given none has no `model` key and asks for the session's model.
- **The gate.** `scripts/dev ci` at `b75a01cbd` (before merging `dev` and before the review):
  exit 0, 4225 tests passed, 16 skipped. After merging `dev` (#1253) and the answers to the
  review's first round, at `db8ff8d6a`: exit 0, 4239 tests run, 4239 passed, 16 skipped
  (`target/evidence/gate.log`); `scripts/dev lint` exit 0.
- **Security scan.** `yulius security-scan`: "Security scan disabled: Vidoc is turned off in
  Yulius". Disabled, not passed: nothing was scanned.
- **Not tested here.**
  - Whether a real model follows the resume prompt and the hook's note: that needs a paid
    session at a real limit. The stand-in model was scripted, and the fake agent obeys.
  - The second screenshot's lost row: in 2.1.323 with a scripted model a typed prompt leaves
    the paused workflow in place, so what removed it in the person's session is not shown;
    the run's journal stays on disk either way, and the session is told to resume by run id.
  - A session at a real limit on the instance: nothing here could be run on a subscription.
  - The web UI is unchanged, so the end-to-end suite was not run.
  - A run stopped in the terminal (which, by Claude Code's own notice, leaves no record) was
    not produced with the real CLI; the rule for it is tested on the turn records' counts.
- **Independent review, first round** (a fresh session given the diff range, the spec and the
  review skills; head `9f02878be`): request changes, three majors and eight smaller findings,
  all in this change's code. Each is answered in the code at `ccb0f3ad1`:
  1. Major, reproduced by the reviewer: the Superagent was moved on a workflow agent's limit
     and its workflow lost. Answer: the limit is an observation of its own
     (`WorkflowLimitReached`) that the Cockpit does not act on (Decisions; `own_limit` and its
     test).
  2. Major: a model's limit met by an agent on another model was booked against the session's
     model. Answer: the agent's model is read from its meta file (confirmed with the real CLI),
     and only a limit that binds the run's model is the session's; mutants 6 and 7.
  3. Major: the unfinished runs went stale (a run stopped in the terminal; a fresh session).
     Answer: a turn record that counts less ends the oldest runs; a fresh launch drops them.
  4. The runs were taken before the launch could fail. Answer: they stay known until the new
     process's transcript is read; mutant 8.
  5. Over the bound the alphabetically first files were followed. Answer: the newest runs'
     first, only files changed since the run under watch began, 1024 entries listed.
  6. The hook's note outlived the limit. Answer: it ends with the thread's own work or the
     reset, and goes to typed prompts only.
  7. A transcript-only notice left the queue reading "handed on". Answer:
     `QueueChange::Written`; the fake agent writes the queue records.
  8. When the limit is acted on: (a) the thread's own turn in flight is ended too, now said
     in Decisions; (b) a limit whose reset has passed is not news; (c) an answer under way at
     the refusal no longer makes it history (a later prompt worked on does).
  9. Missing tests: the restart, the failed launch and the Superagent are tested now.
  10. Nits: an idle task is not given its idle activity back by a launch that opens with the
      workflow prompt. The terminal notice can still say "resumed" for a relaunch that then
      finds no recorded session to resume (it starts afresh): left, a wording case of a rare
      path.
  11. Nits: a file that cannot be read no longer costs the others' lines. The two-step link
      check and the larger worst case of one poll (768 files instead of 256, each read with
      the same per-poll bound) are stated in Decisions and left.
