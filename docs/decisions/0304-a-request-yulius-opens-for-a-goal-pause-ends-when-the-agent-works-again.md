# 0304. A request Yulius opens for a goal pause ends when the agent works again

## Status

Accepted. Specified in [SPEC-469](../../.ai/specs/SPEC-469-even-if-it-woke-up-it-still-has-needs-yo.md);
task #453, issue #1183. Refines
[ADR-0225](0225-claude-code-owns-the-goal-retry-after-an-api-error.md) (the request's exits) and
keeps [ADR-0111](0111-agents-ask-a-person-with-needs-you.md) for the requests agents make.

## Context

When Claude Code stops retrying a goal ("Goal paused after 3 automatic retries · …", "Goal paused
· the request was rate limited"), Yulius flags the task and opens a Needs you request: "Send the
task a message (for example "continue") to retry once the API works again", with the problem "the
task does not go on by itself" (ADR-0225). Like every request, it ended only when a person typed
into the task, sent it a message or continued it, or when the agent called `needs_you {resolved:
true}`, which an agent that did not open the request never does.

Sessions do go on by themselves after such a pause. On this host on 2026-11-11, 9 of 13 pauses
"the request was rate limited" were followed by the session's own work with nobody typing: Yulius
restarted the session on another subscription or at the reset (tasks #446, #450, #452), or Claude
Code woke it with a `<task-notification>` when background work ended. The request stayed open, and
an open request alone puts a task under Needs you, so a working task showed "Send the task a
message…" for as long as it ran (task #154 of another project: 33 minutes into its turn).

A prompt is not proof that the session recovered. In task #438's session a goal check-in woke the
paused session, the API refused again within a second, and Claude Code closed the turn without a
new notice.

## Decision

- A request Yulius opens for a state ends when that state does. For a goal pause the state is
  "the session does not go on by itself", and it ends at the agent's first work after the pause:
  an assistant record of the session that is the model's own (a tool call, text, its answer). An
  API error record, a line Claude Code writes itself (`<synthetic>`) and a sub-agent's records are
  not work. What started the work does not matter: Claude Code's wake-up, a prompt Yulius typed, a
  session Yulius restarted, or no prompt anyone sees.
- A prompt alone ends nothing.
- Only that request ends this way. It is told by its action text and ended in one store statement
  (`end_task_needs_you_asking`), so a request the agent made meanwhile is never the one ended. A
  request the agent made stays open "whatever the agent does meanwhile" (ADR-0111).
- The task remembers that it asked (`goal_pause_asked`, across runs), so the store is asked once
  per pause, not at every line. A run that starts while such a request is open (a restart of
  Yulius, a session Yulius restarts) watches for it again. A failed write is tried again at the
  agent's next work.
- The pause's flag (the activity `needs_attention`) is answered by work like a notice's
  (`notice_attention_ms`, STA-003): a task whose session goes on with no prompt seen reads Working.
  A task that was flagged before the pause keeps that flag.
- The replay after a restart of Yulius folds when the agent first worked after the last pause
  (`TranscriptFacts::worked_since_goal_pause`). With such work the request ends, the pause is no
  longer the transcript's last word, and a task that reads Needs you since before that work
  follows its session (Working while the turn is open, settled once it closed). A flag raised
  after the work began is left alone: something the session did later raised it.
- The end is audited as `task.needs_you.resolved` by `system`, and the request's wait is measured
  like an answered one.

## Alternatives

- End the request at any prompt that starts a turn (Claude Code's wake-up, Yulius's own): simpler,
  and wrong when the API refuses again. The task would read "Your turn" with no word on what to
  do, which is the state ADR-0225 removed.
- End it when Yulius restarts the session: only one of the ways a session wakes, and a restarted
  session can be refused too. Task #449 (SPEC-464) does end it at a move past a usage limit, where
  Yulius knows the new subscription can run the model; that stays a separate, earlier exit.
- Stop showing a request while the task's activity is Working (a rule in `AttentionFacts::group`
  and `taskInfo`): the request an agent makes must stay visible while it works on something else
  (ADR-0111), and the stale request would come back at the turn's end.
- Let the agent resolve it (`needs_you {resolved: true}`): the agent did not open it and does not
  know it exists.
- Ask the store at every assistant line instead of keeping a flag: a write per line for every
  task, for a request that is open in few of them.

## Consequences

- A task whose session works again after a goal pause leaves Needs you at its first assistant
  record, within one transcript poll (about a second).
- A session that is woken and refused again keeps the request, with Claude Code's words.
- Tasks that an older Yulius left working with the request open lose it at the first start of a
  Yulius with this change, when their transcript is read again.
- The match follows the request's action text. Rewording it leaves requests stored with the old
  wording to a person's message, as before.
