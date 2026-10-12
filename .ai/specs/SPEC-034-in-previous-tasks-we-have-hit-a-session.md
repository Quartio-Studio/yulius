---
yulius-task: ecb18dd7-c9ca-4908-afbd-1ad38ff8999b
status: in-progress
---

# SPEC-034: a usage limit steps the conversation back instead of typing "Continue."

Status: in-progress
Date: 2026-10-12
Scope authorization: user request (task #32, F 2026-10-12): "in previous tasks we have hit a
session limit and the agent just typed continue. It shouldnt have happened: it should silently
change the account. Even if it was resumed, we'd expect just the whole conversation to step back a
few messages instead of typing continue (patching the transcript directly)", with a screenshot of
task #25's terminal: two "You've hit your session limit · resets 7:40pm" turns, Claude Code's
"Usage limit reached · continuing automatically at 7:40pm" and "Goal paused" lines, then
`❯ Continue.`.

## Goal

When a usage limit stops a task's turn, the task goes on, on another subscription or on its own once
the window resets, as if the limit had never happened: the session's transcript steps back over
the failed part (the limit errors and Claude Code's limit notices), and Claude Code redoes the turn
from there by itself. Nothing is typed, and "Continue." never appears in the conversation.

## Context

**Symptom.** Task #25 (session `a6cd287b`, 2026-10-12 15:39–15:40 UTC): the prompt "Yulius: you
have new messages…" and then a background-task notification each got Claude Code's usage-limit
record (`apiError: "usage_limit_reached"`). Three seconds after the second, Yulius moved the session
to another subscription, and the resumed session showed `❯ Continue.` as a user message. The same
happened in three other sessions that day (`6cce9222`, `1e8efad8`, `6a1a2b5e`). In all three the limit
hit mid-turn, right after a tool result.

**Root cause.** This was deliberate: F 2026-10-10 made `CONTINUE_TEXT = "Continue."`
(`capacity.rs`) "the only text the harness ever sends on its own after a pause", because a turn the
CLI ended with a usage limit "needs a user message to go on". Every limit resume used it:
`App::rotate` (reactive switch: `switch_task_now(…, Some("Continue."))`), `resume_paused` (a `limit`
pause moved to another subscription) and `release_in_place` (a `limit` pause whose own subscription
recovered typed `Continue.` + Enter into the live TUI). The premise is wrong. Claude Code 2.1.292
resumes an interrupted turn natively: started with `CLAUDE_CODE_RESUME_INTERRUPTED_TURN=1`, a
`--resume` whose transcript ends in an unanswered user prompt resubmits that prompt (shown once, as
the person's own message), and one that ends in a tool result or a task notification continues with
a hidden meta message. It skips API-error rows to find that tail, but only while the error is
younger than its staleness bound. The limit notices stay on screen either way.

**Why tests missed it.** AC-ROT-10/11 asserted the "Continue." prompt as the intended behaviour
(`pool_supervision.rs`), and the fake agent writes its limit record after a completed turn, so no
test resumed a turn the limit had actually cut.

**What else the cause could break.** `release_in_place` also typed into a session that Claude Code
may be continuing by itself ("continuing automatically at 7:40pm"), which can start two turns. Any
other path that resumes after a limit (a `limit` pause whose process died) resumed without
continuing the turn at all, leaving the agent idle.

**Experiment** (local, a fake Anthropic API on 127.0.0.1, so no paid request; `claude` 2.1.292 in
a PTY):

| Transcript (copy of a real one) | Flag | Result |
| --- | --- | --- |
| `6cce9222` cut mid-turn, limit rows removed | set | request ends with the tool result plus hidden "Continue from where you left off."; nothing shown |
| same | unset | no request: the session sits idle |
| `6cce9222` with its limit rows | set | resumes, but the screen shows "Usage limit…", "session limit", "Goal paused" again |
| `a6cd287b` before "Continue.", stepped back | set | one request with both lost messages (the Yulius prompt and the task notification) plus the hidden continuation; no "Continue", no limit lines on screen |

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| LIM-001 | Stepping back a Claude transcript after a usage limit removes, after the last successful assistant record, every usage-limit error record and every `system` record from the first limit record on; records that pointed at a removed record (`parentUuid`, `logicalParentUuid`) point at its nearest kept ancestor; everything else (prompts, tool results, attachments, queue operations, snapshots, unparsable lines) is kept byte for byte and in order. A transcript without such a tail is not rewritten. The file is replaced atomically and keeps its permissions. | `yulius-harness` unit tests `step_back_*` |
| LIM-002 | A native resume may ask to continue the interrupted turn: the transcript steps back (LIM-001), and Claude Code starts with `CLAUDE_CODE_RESUME_INTERRUPTED_TURN=1` and no prompt. A failed step back is logged and the resume goes on. | `yulius-harness/tests/launch.rs` |
| LIM-003 | Every resume after a usage limit uses it: the reactive switch, a `limit` pause resumed on any subscription (also the one it hit, instead of releasing it in place), and a `limit` pause whose process is gone. No path types or sends "Continue." (`CONTINUE_TEXT` is removed). In goal mode the goal is set again after the continued turn's Stop, as after a first message. | `supervision.rs` reactive switch; `pool_supervision.rs` AC-ROT-10 |

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Write the fix spec and the regression test that fails
- [ ] A transcript steps back over a usage limit and Claude Code continues the turn natively
- [ ] Every limit resume (switch, paused switch, recovery in place) uses the step back; no Continue. typed
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Docs, ADR, changes and every gate green

## Acceptance criteria

- A usage limit followed by a switch resumes the same session with no prompt and with the
  interrupted-turn flag; the transcript the new run reads has no usage-limit record.
- A `limit` pause resumed when a subscription recovers launches the same way; nothing is typed.
- The step back keeps the unanswered prompts and tool results, so Claude Code redoes exactly the lost
  turn (a prompt is shown once, as the person's own).

## Test plan

- Unit (`yulius-harness`): step back on the shapes seen in real transcripts: a prompt then the
  limit (task #25), two failed prompts in a row, a limit mid-turn after a tool result, a limit
  after a completed turn (the fake agent's shape), no limit, a limit followed by a successful
  answer (not the tail), relinking through removed records, unparsable and blank lines kept, file
  rewritten only when needed.
- Launch (`yulius-harness/tests/launch.rs`): a resume with `continue_interrupted` has no prompt in
  argv, has the env flag, and has stepped the transcript back; a plain resume has neither.
- Integration (real ptyhost, fake agent): the reactive switch test and AC-ROT-10 assert the launch
  mode (`continue_interrupted: true`, `prompt: None`), the flag in the agent's environment and the
  transcript without its limit record. Before the fix both see `prompt: Some("Continue.")`.

## Decisions

- **Patch the transcript, let Claude Code continue natively** (ADR-0027) instead of typing a
  prompt: it is what the person asked for, it uses the CLI's own interrupted-turn resume (hidden
  meta message, or the lost prompt resubmitted as itself), and the experiment shows the flag alone
  leaves the limit lines on screen and is subject to a staleness bound.
- **Remove only limit records and `system` records of the failed tail**, not the unanswered user
  messages: those are the work that was lost, and Claude Code resubmits or continues them. Stepping
  back further (to the turn's opening prompt) would drop the context of the tool calls whose effects
  are already on disk.
- **A recovered `limit` pause restarts by native resume instead of being released in place**: the
  live CLI cannot have its transcript patched, typing into it was the defect, and its own
  "continues automatically" is not always offered ("send a message after it resets").
- **Known limit**: `CLAUDE_CODE_RESUME_INTERRUPTED_TURN` is an undocumented Claude Code variable
  (2.1.292). If a later CLI drops it, the resumed session waits idle; the harness test pins the
  variable, and the CLI version is pinned with the transcript format (`claude.rs`).
