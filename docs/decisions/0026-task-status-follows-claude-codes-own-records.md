# 0026. A task's status follows Claude Code's own records

## Status

Accepted. Extends the finish rule of SPEC-001 TSK-014 and SPEC-016; specified in
[SPEC-032](../../.ai/specs/SPEC-032-recently-i-got-a-lot-of-wrong-status-mes.md).

## Context

A task's activity (Working, Your turn, Needs your action, Done) was set by events: Yulius's Stop
hook (idle), a typed prompt (working), a `Notification` hook (needs attention), and a probe of the
agent's Bash shells. Three screenshots (tasks #23, #24, #25) showed it wrong: Done while a
background review agent still worked, "Needs you" while a Bash command ran, and "Your turn" between
goal checks. Claude Code 2.1.292 does things these events miss: it sends informational
notifications (`agent_completed`, push, quota), records `pendingBackgroundAgentCount` when a turn
ends, runs the goal evaluator as a Stop hook after Yulius's own and sends the agent on with a meta
line that is not a prompt, and writes `stop_hook_summary` also when a Stop hook only blocks.

## Decision

Take the facts from Claude Code's own records instead of inferring them from the last event:

- The record that closes a turn (`turn_duration`) says how many background agents it waits for;
  that count joins the shell probe as the idle task's background work (the larger of the two).
- A notice makes a task need attention only for the types that ask a person (an allow-list); the
  agent's next assistant line takes a notice's flag back.
- A goal verdict of "not met" and assistant work newer than the last clean Stop make an idle task
  Working again; `stop_hook_summary` is no turn end. A replay applies the same rules.

## Alternatives

- Track `Agent` launches and `<task-notification>` ids ourselves: more state, wrong whenever a
  notification is absorbed mid-turn or a session restarts; Claude Code's count is exact.
- Keep the deny-list of notice types: every release adds informational types, each a false alarm.
- Hold the idle state until the goal verdict arrives (no flicker): a timeout and a second state
  machine for a gap of a second or two; filed as a follow-up instead.

## Consequences

- A goal that is never met reads Working while Claude Code keeps asking the model, as it is.
- Monitor tool streams are still not counted as background work (SPEC-016 limit).
- Between Yulius's Stop hook and the goal verdict the task still reads "Your turn" for a moment.
- The `background_shells` API field now means background work (shells and agents); the name stays.
