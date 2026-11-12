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
"the request was rate limited" were followed by the session's own work with nobody typing: each
time Yulius had restarted the session on another subscription or at the reset and typed its goal
again (tasks #446, #450, #452), which is no person's prompt. Claude Code wakes a session whose
goal is paused too, with a `<task-notification>` when background work ends. The request stayed
open, and an open request alone puts a task under Needs you, so a working task showed "Send the
task a message…" for as long as it ran (task #154 of another project: 33 minutes into its turn).
While a request is open Yulius also types nothing into the task, so that session heard nothing of
its pull request.

A prompt is not proof that the session recovered. The other four pauses were followed by
another refusal: in task #438's session, which was paused six times that day, after a goal
check-in that Claude Code closed without a new notice and after messages a person typed; in
task #450's, after a restart by Yulius. Nor is a restart proof: a pause that follows a refused
try finds the request still open, so the 13 pauses made nine requests, and counted from the
first pause of each the model's first output came 10 to 76 minutes later, in five of the nine
only after two to four restarts.

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
- A restart of Yulius forgets when the pause flagged the task. A run that starts while the
  pause's request is open takes that time back from the task's state, when the task reads Needs
  you since the request was asked (the request is recorded first, the flag raised next).
- The time of a flag that work answers (a notice's, the pause's) stands only while the task
  reads that flag: Needs you, since no later than that time (`standing_notice_ms`). A status
  change replaces a flag without a word (a pause for capacity, a held turn that ended), and a
  flag raised afterwards is another matter's: the agent's work does not lift it, and the stale
  time is dropped.
- The replay after a restart of Yulius folds when the agent first worked after the last pause
  (`TranscriptFacts::worked_since_goal_pause`). With such work the request ends, the pause is no
  longer the transcript's last word, and the flag is answered by that work as it is live: the
  task follows its session (Working while the turn is open, settled once it closed).
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
- A session that worked and was paused again while Yulius was down keeps the request of the
  first pause, with its words and its age, where the live watcher ends it and asks anew. The task
  reads Needs you either way; asking anew there needs the time of each pause, which the
  transcript's fold does not keep (as before this change: SPEC-333 asks once).
- A woken turn that produces output and then dies with no new notice from Claude Code ends the
  request and settles as "Your turn" (ADR-0212) with no request text. The task stays under Needs
  you while its goal is unfinished, without the cause. Claude Code usually writes a notice when
  an error ends a goal turn, but not always: the refused check-in of task #438's session closed
  with none. Whether a turn that produced output can end that way is not known.
- After a restart of Yulius, while the pause's request is still open, a flag Yulius raised
  itself after the pause (a goal the restarted session never took, say) cannot be told from the
  pause's by its time: the agent's work lifts it too. Such a session that works has gone on.
