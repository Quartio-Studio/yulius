# 0326. A workflow agent's usage limit is the session's, and a relaunch resumes the workflow by its run id

## Status

Accepted. Specified in [SPEC-486](../../.ai/specs/SPEC-486-a-usage-limit-reached-while-a-dynamic-wo.md)
(task #468). Extends [0027](0027-step-back-past-a-usage-limit.md) (one launch past a limit now
opens with a prompt) and [0063](0063-background-work-and-goal-verdict-from-the-transcript.md) (a workflow
is background work); keeps [0055](0055-cockpit-moves-past-a-usage-limit.md)'s rule that the
terminal is not read for limits.

## Context

Claude Code's Workflow tool ("ultracode", dynamic workflows) runs a script that starts many
agents inside the session's process, beside the session's own thread. Run against a local
stand-in for the model API (Claude Code 2.1.323), it behaves like this:

- The run's agents write to `<session>/subagents/workflows/<run id>/agent-<id>.jsonl`. A usage
  limit that refuses one of them is recorded there only. Claude Code pauses the workflow (its
  row reads "paused · usage limit resets at …") and does not wake the session's thread: the
  session's transcript ends with a finished turn whose record counts the workflow
  (`pendingWorkflowCount`). A goal's check is not made while a workflow is unfinished.
- Yulius followed `<session>/subagents/*.jsonl` only, for token usage only, and read neither the
  launch (`toolUseResult` with `taskType: "local_workflow"`) nor `pendingWorkflowCount`. So a
  limit met during a workflow did not exist for it: the task stayed on its subscription until
  that subscription's reset while others were free (task #182 of another sandbox, 2026-11-13).
- A process that ends takes its workflows with it. A resumed process names each unfinished run
  in a notification written for the transcript only and starts nothing. The run's journal keeps
  every finished agent's result, and `Workflow({scriptPath, resumeFromRunId})` in the resumed
  session returns those at once and asks the model only for the rest.
- A background agent outside a workflow that is refused ends, and its notification wakes the
  session's thread, whose own request is then refused in the session's transcript. Only a
  workflow pauses silently.

## Decision

1. **The limit is read from the workflow agent's transcript.** The harness follows the agents'
   files of the workflow runs changed last, with bounds of their own (64 runs, 512 files), and
   the watch reports a usage limit found there as the session's `LimitReached`: once the
   session's own transcript is read to its end, once until the session's thread does something
   again, and not when the thread did something later than the refusal or its own limit stands.
   From there the limit is any other limit: the pool decides, the task moves or waits.
2. **A workflow is background work with a name.** The launch is `BackgroundStarted` by its task
   id, ended by that task's notification, and `WorkflowLaunched` names its run. A turn record's
   pending workflows count with its pending agents. The session's unfinished runs are kept per
   task and are part of a replay's facts.
3. **A relaunch tells the agent to resume the unfinished runs.** Any launch that resumes a
   session while runs of it are unfinished opens with one prompt (after the prompt it would
   have had, if any): the workflow stopped with the previous process, resume it with the
   Workflow tool by its script path and run id, do not start it again, then continue the task.
   Past a limit this replaces the silent continuation of 0027 for that launch; the transcript
   is stepped back as before. The run ids come from Claude Code's own records and pass only as
   letters, digits, `_` and `-`.
4. **A prompt at a paused workflow gets the same advice.** While a run is unfinished after a
   limit and nothing relaunched it, the prompt hook adds a note to a prompt that reaches the
   session: the workflow is paused, do not start it again, resume it by its run id.
5. **A notification written for the transcript only starts no turn.** `queueTranscriptOnly`
   records end the background work they name and are not a prompt.

## Alternatives

- **Read the terminal** ("paused · usage limit resets at"). Rejected in 0055 and again here:
  the screen is a rendering, per viewer size and CLI version, and the record exists.
- **Report every sub-agent's limit as the session's.** Outside a workflow the session's own
  record follows within the same turn, so every such limit would be acted on twice.
- **Wait for the workflow before moving.** At the limit it cannot go on; waiting is the bug.
  The finished agents' results survive the move in the journal; an agent in flight is asked
  again.
- **Keep the silent continuation and trust Claude Code's own notice.** When the session's turn
  had ended there is no turn to continue, so the moved session would sit idle; when it was cut,
  whether the agent picks the workflow up would be left to chance.
- **Treat a workflow agent's later line as "Claude Code continued by itself"** (0058), so the
  resume at the reset would leave a self-continued workflow alone. An agent that finishes after
  the session's own refusal would then end the pause while the session is still at its limit.
  Yulius relaunches at the reset; it costs the agents in flight, never the finished ones.
- **Resume the workflow from Yulius** (type the tool call, or a slash command). There is no
  command for it; the Workflow tool is the model's to call.

## Consequences

- A limit during a workflow moves the task within the watch's next poll, with no person. With
  no subscription free the task reads paused with the reset, as at any limit.
- Workflow agents' tokens now count in a run's metrics (they were not read before).
- A workflow keeps a proactive move waiting like a background agent, at most an hour per launch
  without its notification (0063's bound).
- One more place where Yulius's words reach the agent: the resume prompt and the hook's note.
  Whether a model follows them cannot be tested without a paid session; the fake agent does,
  and the real CLI was driven by a scripted stand-in.
- More than 512 agent files in a session's last 64 runs are not followed: a limit met only by
  a later agent would be missed until the session's thread asks the model.
