# 0027. A usage limit steps the transcript back; Claude Code continues the cut turn

## Status

Accepted. Supersedes the neutral "Continue." of F 2026-10-10 (SPEC-001 ROT-010, SPEC-002 ADR-17);
specified in [SPEC-034](../../.ai/specs/SPEC-034-in-previous-tasks-we-have-hit-a-session.md).

## Context

When an account's usage limit rejects a request, Claude Code ends the turn with an error record and
prints "You've hit your session limit", "Usage limit reached · continuing automatically at …" and
"Goal paused". Yulius moved the session to another subscription, or waited for the reset, and then
sent "Continue." as a user message, because the cut turn "needs a user message to go on". People
saw "Continue." typed into their tasks (task #32) and asked for the account to change silently and
the conversation to step back instead.

## Decision

Every resume after a usage limit is a native resume of the same session with no prompt
(`RunStart::AfterLimit` → `SessionMode::Resume { continue_interrupted: true }`):

1. Before the launch, the harness adapter steps the transcript back
   (`yulius_harness::step_back`). In the failed tail (after the last answered assistant record),
   it removes the usage-limit error records and Claude Code's `system` notices, and relinks
   `parentUuid`/`logicalParentUuid` to the nearest kept record. Unanswered prompts, task
   notifications and tool results stay.
2. Claude Code starts with `CLAUDE_CODE_RESUME_INTERRUPTED_TURN=1`. It resubmits an unanswered
   prompt as the person's own message, or continues after a tool result with a hidden "Continue
   from where you left off.".

This applies to the reactive switch, to a `limit` pause resumed on any subscription (including the
one it hit, which no longer releases in place), and to a `limit` pause whose process is gone.
`CONTINUE_TEXT` is removed: Yulius sends nothing on its own after a pause.

## Alternatives

- **Keep "Continue."**: it works, but it is what people asked to remove.
- **Set the flag without patching**: Claude Code skips API-error rows only while they are younger
  than its staleness bound, and the limit notices stay on screen (measured, SPEC-034).
- **Patch and resubmit the lost prompt as the launch prompt**: it covers only a limit between
  turns. Most real limits (3 of 4 on 2026-10-12) cut a turn after a tool result, where there is
  no prompt to resubmit.
- **Step back to the turn's opening prompt and resend it**: the agent would lose the context of
  tool calls whose effects are already on disk, possibly a whole goal-mode task.
- **Let the live CLI continue at the reset by itself**: it does not always offer to ("send a
  message after it resets"), and it cannot move to another subscription.

## Consequences

The resume depends on an undocumented Claude Code variable (pinned with the 2.1.292 transcript
format). If a later CLI drops it, the resumed session waits idle until a person or the goal check
moves it. The launch test pins the variable. The transcript is rewritten in place (atomically, same
permissions) while no process uses it. A recovered `limit` pause now costs a restart, with a cold
prompt cache, like a switch.
