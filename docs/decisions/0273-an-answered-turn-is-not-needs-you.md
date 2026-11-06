# 0273. An answered turn is not Needs you: only unfinished work or an explicit request is

## Status

Accepted. Amends [0068](0068-needs-you-means-a-person-has-to-act.md) and
[0111](0111-agents-ask-a-person-with-needs-you.md). Specified in
[SPEC-429](../../.ai/specs/SPEC-429-hey-why-yulius-still-mark-this-thread-as.md); task #411, #1090.

## Context

Every clean turn end without an achieved `/goal` read "Your turn" under Needs you, and a typed
prompt drops an achieved goal. So a conversation, such as "so all good here and you don't need
anything from me?" answered with "Yes, nothing is needed from you", put the task in Needs you and
notified the person. ADR-0068 and ADR-0111 meant Needs you for a person's move. The things that
do ask a person are already explicit: a permission or question prompt, `needs_you` (also for spent
goal retries), a browser request, a held prompt, failing checks and a blocked chain.

## Decision

1. An idle `waiting` agent (no background shell, no `yulius wait`) needs a person only while its
   work is visibly unfinished: its plan has steps not done or skipped, or Claude Code last recorded
   its goal as set, not met or failed (`goal_unfinished`). Otherwise the task reads "Answered" in
   the Done group. It is not counted and notifies nobody.
2. The activity stays `waiting`. "Answered" is a reading of it, not TSK-014's Done: chains do not
   start, the queue does not take the slot, and a prompt makes the task Working as before.
3. The server (`AttentionFacts::group`) and the web UI (`taskInfo`) apply the one rule. The task
   view carries `goal_unfinished`, and the plan counts after every finished turn.
4. The `needs_you` tool tells agents that a question asked only in plain text reaches nobody. The
   briefing is at its byte budget, and agents read the tool's description when they choose how to ask.

## Alternatives

- **Flag the turn when the last message ends with "?".** It misses "let me know which…" and flags
  rhetorical questions, and it needs the message text at the turn's end.
- **Have a model judge each turn end.** It adds cost and latency to every turn, and the agent can
  already say so itself.
- **Make the activity `done`.** It would start the task's chains and free its slot while the
  person may still be talking to it.
- **Keep "Your turn" and only stop notifying.** The sidebar would still count it under Needs you,
  which is what was reported.

## Consequences

- A conversation the agent answered reads "Answered" under Done until someone writes to it.
- An agent that stops mid-plan, or with a failed or open goal, still reads "Your turn".
- An agent with neither a plan nor a goal that asks in plain text is not flagged. The `needs_you`
  description covers it, and the transcript still shows the question.
