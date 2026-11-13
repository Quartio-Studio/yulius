---
yulius-task: a126e4ca-1a96-4142-884c-c5162cac1549
status: in-progress
---

# SPEC-486: a usage limit met while a dynamic workflow runs moves the task

Status: in progress
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
  run's window, regular files in real folders only, bounded beside the sub-agents' bound (the
  `MAX_WORKFLOW_RUNS` = 64 run folders changed last, `MAX_WORKFLOW_FILES` = 512 files). Their
  token usage is the session's (MET-004). `journal.jsonl` and other files there are not read.
- WFL-002 (a workflow agent's limit is the session's). A usage-limit refusal recorded in a
  workflow agent's transcript is reported as the session's `LimitReached`, with its reset,
  window and scope, unless the session's own thread did something (a prompt, agent work, an
  answer, an interrupt) recorded later than it: then it is history, as SPEC-072 LRS-001 says of
  a limit the session continued past. It is reported only once the session's transcript is read
  to its end, so a replay after a restart judges it against all the session did later; several
  refusals are one report (the latest), and nothing more is reported until the thread does
  something again, nor while the thread's own limit stands (that one was reported). A
  sub-agent's refusal outside a workflow stays unreported: the session's thread is woken and
  refused itself.
- WFL-003 (a workflow is background work). A Workflow launch in the session's transcript is
  `BackgroundStarted` (an agent, by its `taskId`) and names its run (`WorkflowLaunched`);
  its end is the task notification with a status for that `taskId`, as for an agent.
  `pendingWorkflowCount` of a turn record counts with `pendingBackgroundAgentCount`. So a task
  whose session waits for a workflow reads "Working · N background tasks still running", a
  proactive move waits for it (BGW-004), and Yulius knows a run is unfinished
  (`dynamic_workflow::Unfinished`, at most 8 runs). The unfinished runs are part of a replay's
  facts, so they are known after a restart. A notification Claude Code writes for the
  transcript only (`queueTranscriptOnly`: a resumed process naming a run the ended one left
  unfinished) ends the background work it names and starts no turn. A run id passes only as
  letters, digits, `_` and `-` (it is later named in a prompt).
- WFL-004 (a relaunch resumes the workflow). A launch that resumes a session while a workflow
  of it is unfinished opens with one prompt: the session continues in a new process, the
  workflow stopped with the previous one, each unfinished run (ids named) is to be resumed
  with the Workflow tool by its script path and run id (`resumeFromRunId`) and not started
  again, then the task continued. Past a limit (`RunStart::AfterLimit`: an automatic move, a
  person's move at a limit, the resume of a `limit` pause) that prompt replaces the silent
  continuation of SPEC-034, and the transcript is stepped back over the limit as before; a
  launch that has a prompt of its own (a held prompt's resume, a person's move of a working
  task) keeps it first. In goal mode the goal is typed after that turn, as after any opening
  turn. The terminal notice of the relaunch says the workflow is resumed. The relaunch forgets
  the unfinished runs (they ended with the old process; the resumed run is read from its new
  launch).
- WFL-005 (waiting). With no subscription that can run the task's model, the limit opens a
  `limit` pause as today: the task reads "Paused: … (resets ~HH:MM)" and resumes at the
  earliest reset by WFL-004. No Needs you unless nothing can run it (SPEC-464 UCL-010).
- WFL-006 (a prompt at a paused workflow). A prompt that reaches a session whose workflow met
  a limit and has not been relaunched or ended since gets, through the prompt hook's additional
  context, a note: the workflow (run ids named) is paused at a usage limit, it is not to be
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
  remaining agents and the run's end. A prompt hook's additional context is printed. Fixtures
  are synthetic, shaped as the probe's records.

## Plan

- [x] The cause is reproduced with Claude Code 2.1.323 against a local stand-in API and written here
- [x] The harness follows workflow agents' transcripts and reads a workflow's launch and pending count
- [x] The transcript watch reports a workflow agent's limit as the session's
- [x] The fake agent writes a workflow's records and a limit in its agent's transcript
- [x] A limit during a workflow moves the task and the relaunch resumes the workflow by its run id
- [x] A prompt typed at a workflow paused by a limit tells the agent to resume it
- [x] ADR-0326, the architecture, product and contract documents and the changes entry agree
- [ ] The gate and the security scan run on the final head and the independent review's findings are answered

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
