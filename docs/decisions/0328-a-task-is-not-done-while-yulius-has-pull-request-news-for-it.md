# 0328. A task is not Done while Yulius has pull request news for it

## Status

Accepted. Specified in [SPEC-487](../../.ai/specs/SPEC-487-the-superagent-can-do-everything-its-per.md)
(slice 1, CHN-001 to CHN-006); task #469. Amends
[ADR-0012](0012-task-intake-and-chains.md) and [ADR-0018](0018-task-start-rules.md) (when a
chained task starts) and SPEC-085 STT-002 (what "waits for Yulius" means).

## Context

A task is Done when its turn ended, its plan is complete and its pull request is merged, or
open with green checks (TSK-014). Chains, start rules, a supertask's wait and the task list ask
that one function. The rule was written when a person merged pull requests. Since tasks merge
their own (ADR-0141) and are told of their pull request by a watch instead of a `yulius wait`
shell they keep alive (SPEC-444, ADR-0287), a task passes through that state on its way:

- its agent arms the merge and ends its turn; once the checks pass, the pull request is open
  and green while the merge queue works;
- Yulius merges, and the watch types the news into the session within 20 seconds; until then
  the agent is idle with a merged pull request.

On 13 November 2026 task #465 delivered fourteen pull requests in a row. Tasks #466, #467 and
#468, created to run after it, started while it went from one slice to the next, and #469
started from its unmerged branch while pull request #1257 waited, armed and green, in the
queue. A person who reads "Done · PR #1257" for a minute and then "Working" again was shown
the same mistake. A rule over several tasks started early the same way, and the tools answered
such a rule as "runs after" its first task, so nobody could see what the task waited for.

## Decision

- **Pull request news that is due means the task goes on by itself.** One rule says so
  (`news_due`, asked through `App::pr_news_due`): a merge is armed or being decided; or, for a
  task the watch types into, news was typed and the turn it starts has not begun, the pull
  request is merged and the watch, which saw it open, has not said so, or an armed merge ended
  without merging and the watch has not said how. The watch's own conditions (`merge_untold`,
  `outcome_key`) decide both what it types and what is due.
- **Typed is not heard.** After the watch typed, the task still reads idle until Claude Code's
  prompt hook (or the transcript's turn) makes it Working. The news stays due until then,
  for at most two minutes (`NEWS_TURN_WAIT_MS`: a paste Claude Code swallowed must not hold
  a chain for ever).
- **An armed merge is known before it is restored.** A server that started again serves
  hooks and chain passes before it put the saved armed merges back; until it did, their file
  answers (`App::merge_is_armed`).
- **A paused task is told nothing, and reads as before.** The watch types nothing into a task
  a person paused or an update holds, so no news is due to it, except an armed merge, which
  Yulius makes whatever the agent does.
- **Such a task is not Done and not a person's turn.** `task_is_done` and
  `task_is_done_locally` answer no, and the task's `yulius_wait` fact is true, so the list
  reads "Waiting for checks or merge", as it does for an agent that waits in `yulius wait`.
  The news is typed, the agent's turn runs, and what that turn leaves decides: more work, or
  Done.
- **Final states stay as they were.** An achieved goal, a closed task, a pull request whose
  news was all told, and a task the watch tells nothing (a playground's, a pull request it
  never saw open) are Done by the old rule.
- **A start rule is answered as stored.** `tasks_list`, `task_get`, `task_create` and
  `task_update` answer `start_rule` (the whole rule), `runs_after` only for a task chained
  after one task, and `start_blocked` with the reason when the task will not start by itself.
- **"After all current tasks"** is `after_all_tasks` on `task_create` and `task_update`: the
  tool writes the rule that names every task of the project with work still coming. Left
  out: tasks that ended or are stopping; tasks the list shows under Done (done, or
  "Answered": an idle session with nothing open never becomes `done`, and would hold the rule
  for good); prepared tasks; and the task whose start changes, with the tasks waiting on it.
  A rule names at most 64 tasks (16 before) in at most 1400 characters (500 before); the web
  editor keeps building 16.

## Alternatives

- **Done only with an achieved goal.** A task whose goal evaluator never says "met" holds its
  chain for good while its pull request is merged and its agent idle; prompt-mode tasks have
  no goal at all.
- **"Run after" waits for the merge.** A prompt-mode task that opens a pull request for a
  person to merge is done; `#N merged` already says the other thing, and the tools now name
  it.
- **A delay after the merge before Done.** A guess at how long the watch takes; the watch
  knows what it told.
- **A new state for the task list.** The list already has the fact ("goes on by itself when
  Yulius has news"); a second one would be the same thing twice.
- **A new rule word for "all current tasks".** The set is fixed at the call; a rule that names
  the tasks reads back, shows in the editor and needs no second evaluation.

## Consequences

- A chained task starts later than before by the time the merge queue and the watch take
  (seconds to minutes), and from the integration branch, not from its predecessor's unmerged
  branch, when that predecessor merges its own pull request.
- While the watch cannot type (a person's draft sits in the prompt box, a person must act
  first), the task keeps waiting and so does its chain; the list says it waits. For the same
  time a merged task is not archived or deleted without stopping it first (the list does not
  show it Done yet): seconds, in the usual case.
- A task paused with a merged pull request it was not told of reads Done, as before this
  decision; when it is resumed it is told, and goes on if it has more to do.
- News typed in the instant before a restart of the server is not remembered as "typed":
  the task may read Done until its turn begins.
- A rule of 64 tasks reads each named task once per pass, and a reading may ask GitHub (an
  idle task with an open pull request and no armed merge); passes run at every turn's end in
  the sandbox and every minute.
- What the watch told is kept in memory beside its file, since the task list asks for every
  idle task with a merged pull request.
- A rule written by `after_all_tasks` needs a person once one of its tasks ends other than
  done, as every chain does.
