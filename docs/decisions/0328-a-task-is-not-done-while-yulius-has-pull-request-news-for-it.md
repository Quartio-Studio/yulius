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

- **Pull request news that is due means the task goes on by itself.** One function says so
  (`App::pr_news_due`): a merge is armed or being decided; an armed merge ended without
  merging and the watch has not said how; or the pull request is merged and the watch, which
  saw it open, has not said so.
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
  tool writes the rule that names every task of the project that has not ended (prepared
  tasks, the task itself and tasks waiting on it left out). A rule names at most 64 tasks
  (16 before); the web editor keeps building 16.

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
  first), the task keeps waiting and so does its chain; the list says it waits.
- What the watch told is kept in memory beside its file, since the task list asks for every
  idle task with a merged pull request.
- A rule written by `after_all_tasks` needs a person once one of its tasks ends other than
  done, as every chain does.
