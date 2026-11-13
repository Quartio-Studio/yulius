---
yulius-task: 1be0c20e-fe89-4ec0-a1ff-5c054ebc7ae7
status: in-progress
---

# SPEC-487: the Superagent does what its person may do

Status: in-progress
Date: 2026-11-13
Owner: Yulius task #469
Scope authorization: the request of task #469 (by MrFishPL, 2026-11-13): "the Superagent should
be able to do everything its person intended. That is what the Superagent is for. Today it often
cannot: it hands the job back as steps to click, or does it through a slow workaround. Fix that
in Yulius. Also decide, for each ability below, whether a normal task's agent should have it
too." Amends SPEC-001 TSK-012 and TSK-016, SPEC-037 (MGT), SPEC-017 (RUL) and SPEC-085 STT-002;
slice 1's decision is
[ADR-0328](../../docs/decisions/0328-a-task-is-not-done-while-yulius-has-pull-request-news-for-it.md);
the rule for the Superagent gets its record with slice 2.

## Goal

Whatever a person may do in Yulius, their Superagent does for them when they ask: with their
rights, checked at the call, and audited. It hands a job back only for a reason this document
names. A task chained after another starts when that task has really finished.

## Context

- **The evidence.** A read of all 20 Superagent conversations on the production instance
  (14 October to 13 November 2026, three people, about 1,500 messages) found 322 situations in
  which the Superagent could not do what was meant: twelve walls, listed in the task's request
  and under [Slices](#slices) below. The Superagent's tools (`COCKPIT_TOOLS`,
  `crates/yulius-app/src/mcp.rs`) were 39: no CI, delivery, CD, restart, pause, subscription or
  instance tool.
- **What exists to build on.** A person's own agent on their computer already drives Yulius
  through the remote MCP (SPEC-247, `crates/yulius-app/src/remote.rs`): `task_restart`,
  `task_pause`, `task_unpause`, `task_start_now`, `task_delivery`, `ci_runs`, `ci_run`,
  `ci_rerun`, `ci_cancel`, `cd_status`, `cd_deploy` and others, each one the function the web
  UI's route calls, with the person's `AuthContext` and an `Action` checked before it runs. The
  Superagent in a sandbox acts for the same person (`ProjectCaller`, `App::manage_context`) and
  is served none of them. Every function a person's route calls checks the person's rights
  itself (`App::require`, `App::load_task`: a task is changed by its owner only).
- **Chaining (wall 3), the cause.** A chained task starts when its predecessor is Done
  (`App::task_is_done`, `crates/yulius-app/src/delivery.rs`): its turn ended, its plan is
  complete, and its pull request is merged, or open with green checks. Since tasks merge their
  own pull requests (ADR-0141) and hear of them without a `yulius wait` shell (SPEC-444), that
  state is passed through on the way, not only reached at the end:
  1. *Armed.* The agent calls `delivery_merge` while the checks run and ends its turn. When the
     checks pass, the pull request is open and green and the agent is idle, for as long as the
     merge queue takes. On 13 November task #465's pull request #1257 went green at about
     19:56 UTC, armed, in the queue; this task (#469, "after #465") was started at 19:57:02 from
     #465's unmerged branch.
  2. *Merged, not told yet.* Yulius merges, and the pull request watch types the news into the
     session within 20 s. Until then the agent is idle with a merged pull request and a
     complete plan. Task #465 delivers one pull request per slice: after each merge it read
     Done for that window, and #466, #467 and #468 ("after #465") started while it went on to
     its next slice.
  A start rule's `#N done` asks the same function, so a rule over several tasks started early
  the same way. The task list showed such a task as "Done · PR #N" for the same time.
- **Chaining, what is shown.** `tasks_list` and `task_get` answer `runs_after: N`, the first
  task a rule names (`ChainRecord::after_number`, the rule's anchor), and never the rule. A
  rule naming three tasks read "after #12". The rule is stored whole (`task_chains.rule`).
- **"After all current tasks are done"** has no words: a rule takes at most 16 tasks
  (`MAX_ATOMS`) and the caller has to list them.

### Slices

One pull request each, in this order (the request: "chaining first, then task control, then CI
/ delivery, then subscriptions and instance, then the rest"). The Plan below holds the slice
under way; a slice is added to it when the one before is merged.

| Slice | Delivers | Closes |
| --- | --- | --- |
| 1 | A chained task starts when its predecessor really finished; the rule is shown as stored; "after all current tasks" | wall 3 |
| 2 | The rule (its decision record) and the person's project tools served to the Superagent: restart, pause, resume, start now, a task's delivery, CI read / run again / cancel, CD status and dev deploy; `COCKPIT_INSTRUCTIONS` and the tool descriptions rewritten | walls 2 and 4 (part), 12 |
| 3 | Task control that had no function: rename, edit of a prompt not started, order in the queue, close out, bulk stop / pause / message | wall 2 |
| 4 | Tasks' questions: list open ones, why a task needs attention, answer or dismiss for the person, the end of a session (`session_read` tail) | wall 7 |
| 5 | Merges: arm or re-arm a task's merge for the person, the merge queue of the sandbox, why a green pull request is not merging; CI order and hold | wall 4 |
| 6 | Subscriptions and model: which one a task is on, move it, change model or effort | wall 5 |
| 7 | The instance: version, health, update status, checks, Update now and Restart with the buttons' refusals; reconnection after a restart | wall 1 |
| 8 | Requests to the person that last: cross-sandbox requests stored, amendable, still there later; a pending decision of the Superagent's own; another sandbox's task list | walls 6, 9 |
| 9 | The Superagent's session lasts: standing instructions and checks survive a restart, a message typed during one is kept, a tool list that changes restarts nothing | wall 8 |
| 10 | Secrets (a value entered masked beside the chat, never seen by the agent) and visibility (cost, load, a crash's exit and last screen, a checkout that is current) | walls 10, 11 |
| 11 | A missing ability reported to the Yulius project in one call; the ability table in the product documentation, complete | wall 12 |

### The ability table (target)

The target, for all of it; it goes into `docs/product.md` complete with slice 11, and until
then each slice's part is described there in words. "Person's rights":
the function the web UI's route calls, with the person's rights checked at the call.

| Ability | Superagent | Task's agent | Why |
| --- | --- | --- | --- |
| Create, chain, schedule, stop, archive, delete a task; automations | yes (had) | yes (had) | SPEC-037: a follow-up runs as its own task |
| Read a start rule back as stored; "after all current tasks" | yes (slice 1) | yes (slice 1) | Both create tasks with `task_create` |
| Restart, pause, resume, start now | yes (slice 2) | no | A person speaks to their tasks; tasks do not steer each other (ADR-0178) |
| Rename, edit a prompt not started, reorder, close out, bulk actions | yes (slice 3) | no | As above |
| Message a task | yes (had) | no | ADR-0178 |
| List open questions, see why a task needs attention | yes (slice 4) | yes, read only (slice 4) | A task that sees another's question does not ask it again |
| Answer or dismiss a task's question for the person | yes (slice 4) | no | The question is to a person; their Superagent answers when they say so |
| The end of a session (`session_read` tail) | yes (slice 4) | yes (slice 4) | Sessions are docs for both (SPEC-001 SES) |
| CI: read runs and logs, run again | yes (slice 2) | yes (had) | A task needs its own checks |
| CI: cancel, order, hold | yes (slices 2, 5) | no | They change other tasks' runs |
| A task's delivery, the merge queue, why a pull request is not merging | yes (slices 2, 5) | its own (had) | A task delivers its own work |
| Arm or re-arm a task's merge | yes, for the person (slice 5) | its own (had) | The reviewed head is the task's to name; the person may merge what they own |
| Dev deploy, CD status | yes (slice 2) | yes, when its task asks (had) | SPEC-134 |
| Production deploy, promotion to `main` | behind one confirmation in the panel (slice 8) | no | ADR-0110: a person decides; the Superagent prepares and asks |
| A task's subscription: see, move; model and effort | yes (slice 6) | no | Tasks are subscription-agnostic (SPEC-001 ROT-007) |
| Instance: version, health, update status, checks | yes (slice 7) | no | Not a task's work |
| Instance: Update now, Restart | yes, a superadmin's (slice 7) | no | The buttons' rights and refusals |
| Another sandbox: list and read tasks | yes (slice 8) | no | SPEC-335: only the person's Superagent reaches their other sandboxes |
| Another sandbox: create, change, message, stop | behind one confirmation in the panel (had; lasting in slice 8) | no | ADR-0227 |
| A secret's value | never | never | SPEC-002 ADR-20: no agent sees a value |
| Enter a secret's value | the person, masked, beside the chat (slice 10) | no | The value goes from the person to the vault |
| Report a missing ability to the Yulius project | yes (slice 11) | yes (slice 11) | Both meet walls |

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| CHN-001 | A running task whose merge is armed or being decided is not Done, whatever its pull request's checks say: Yulius will merge it and tell its agent. A task chained after it, or waiting on a rule that names it `done`, keeps waiting, and the task list reads "Waiting for checks or merge", not Done and not a person's turn. A server that started again knows the armed merges before it put them back. | `merge_train::a_task_chained_after_an_armed_green_pull_request_waits_for_its_merge`, `merge_train::a_restarted_server_keeps_a_chained_task_waiting_for_an_armed_merge` |
| CHN-002 | A running task whose pull request is merged is Done only once the pull request watch, when it saw that pull request open, has told its agent of the merge and the turn that news starts has ended. While the news is typed and that turn has not begun, it still is not Done: typed into an idle session, until the task goes from idle to Working, for at most two minutes; typed while a turn runs (Claude Code queues it), as long as Claude Code's queue says it waits for its turn, and for two minutes while its queue record is not read yet. The same holds for an armed merge that ended without merging until the watch said how, and for a merge GitHub reports before the record says so. One decision reads the task's delivery once. A task the watch tells nothing is Done as before: a playground's, one whose pull request it never saw open, and one a person paused. An update's hold or a pause for capacity changes nothing: the news is typed right after. A supertask's wait and Home's counts follow the same rule. | `task_done::a_chained_task_waits_until_its_predecessor_heard_of_its_merge` (real hooks, the prompt hook held at its check), `task_done::news_typed_while_a_turn_runs_is_due_until_its_own_turn` (a held turn, Claude Code's queue, a held hand-over), `merge_train::a_task_chained_after_a_dropped_armed_merge_waits_for_its_news`, `supertask::a_supertask_waits_for_a_task_that_has_not_heard_of_its_merge`, `supertask::a_merge_during_an_updates_hold_is_still_news_due`, `pr_watch::tests::news_is_due_until_the_agent_heard_it`, `pr_watch::tests::typed_news_is_due_until_its_turn` |
| CHN-003 | An achieved goal is Done as before, and so are a closed task and a task with no pull request news left: CHN-001 and CHN-002 remove no Done state that was final. | the tests of TSK-012 and TSK-014 unchanged (`intake::chained_tasks_start_on_done_and_wait_for_a_person_otherwise`, `task_done::a_chain_starts_when_its_predecessor_achieves_its_goal`) |
| CHN-004 | `tasks_list`, `task_get`, `task_create` and `task_update` answer how a waiting task starts as it is stored: `start_rule` with the whole rule for a task waiting on a rule, `runs_after` only for a task chained after one task, and `start_blocked` with the reason when it needs a person. Their text shows the rule in full. A task that has started answers neither `start_rule` nor `start_blocked`. | `manage_tools::a_start_rule_reads_back_whole`, `manage_tools::task_update_after_all_tasks_with_nothing_left_starts_now`, `mcp_text::tests::a_waiting_tasks_rule_reads_whole` |
| CHN-005 | `task_create` and `task_update` take `after_all_tasks: true`, alone among the start choices (a blank text is no choice): the task waits for every task of the project with work still coming to be done, stored as the rule that names them, so it reads back like any rule. Left out: tasks that ended or are stopping, tasks the list shows under Done (done or "Answered"), prepared tasks, and the task whose start changes with the tasks waiting on it, directly or through others. A rule names at most 64 tasks in at most 1400 characters; more tasks are refused with what to do. With no such task it starts now and the answer says so. The caller's person is checked before anything is counted. | `manage_tools::after_all_tasks_names_every_task_that_has_not_ended`, `manage_tools::after_all_tasks_with_nothing_left_starts_now`, `manage_tools::task_update_after_all_tasks_with_nothing_left_starts_now`, `manage_tools::after_all_tasks_counts_nothing_for_a_person_without_access`, `rule::tests::the_limits_are_enforced` |
| CHN-006 | `task_create`, `task_update` and their descriptions say what `done` means (the task finished and nothing of its pull request is left to come), and name `#N merged` for work that needs the merge itself. | `mcp::tests::the_start_choices_say_what_done_waits_for` |
| SAR-001 | The rule: every function a person's route calls is served to their Superagent as a tool, run with the person's rights at the call and audited with the Superagent as actor, unless the ability table names why not. Slices 2 to 11 deliver it wall by wall. | slices 2 to 11 |

## Plan

- [x] A chained task waits while Yulius has pull request news for its predecessor (CHN-001 to CHN-003)
- [x] Tools read a start rule back as stored and take "after all current tasks" (CHN-004 to CHN-006)
- [x] Slice 1's documents: product, architecture, API, the decision and the changes log
- [x] Slice 1's independent review answered test-first

## Acceptance criteria

- AC-CHN-1: with a predecessor idle, its plan complete and its pull request green and armed,
  the chained task stays `chained` through a chain pass and the predecessor's summary has
  `yulius_wait: true`; after the merge it leaves `chained`.
- AC-CHN-2: with a predecessor idle and its pull request merged but not told, the chained task
  stays `chained` through a chain pass and the background tick's pass; it stays so after the
  watch typed the merge while that turn has not begun; once that turn ended, it starts.
- AC-CHN-3: `task_get` of a task waiting on `#1 done and #2 done and #3 done` answers that
  rule, in text and in the record, and no `runs_after`.
- AC-CHN-4: `task_create` with `after_all_tasks: true` among two live tasks, an ended, a
  stopping, an answered and a prepared one answers a `start_rule` naming exactly the two;
  with none it answers a task that starts now.
- Every gate of the repository passes on the pull request head.

## Test plan

- Integration, real repositories and the production Git adapter (`tests/merge_train.rs`): an
  armed, green pull request and a task chained after it; the merge by the queue.
- Integration, a live session of the fake agent with Claude Code's real hooks
  (`tests/task_done.rs`): a merged pull request, the watch's pass that types the news, the
  prompt hook held at its check (`race::Point::PromptTurnChecked`), the chain's start at that
  turn's end.
- Integration (`tests/merge_train.rs`, `tests/supertask.rs`): a server started again before
  the armed merges are back; an armed merge dropped on a green head; a supertask and Home's
  counts with a merge not told.
- Integration over the MCP endpoint (`tests/manage_tools.rs`): the rule read back, `after_all_tasks`
  with live, ended and waiting tasks, the 64-task bound.
- Unit: the rule's bounds (`yulius-domain`), the tools' texts (`mcp_text`), the descriptions.
- Unit: the news rule's every branch and its bound (`pr_watch::tests::news_is_due_until_the_agent_heard_it`,
  `typed_news_is_due_until_its_turn`).
- Mutation: ten mutants of the rule and its uses, each failed by a named test
  (`target/evidence/mutants.py`).
- Load: the two session tests 96 times each beside 96 busy loops (`target/evidence/r2-stress.log`).
- Not exercised by a test: the watch telling a merge to a run reattached after a restart, and
  a merge GitHub reports before the delivery record says so.

## Decisions

- **"Done" stays one rule; what changes is what it waits for.** The chain, the start rules, the
  supertask's wait and the task list ask one function. Requiring a met goal for Done was
  rejected: a task whose goal evaluator never says "met" (seen: the open-issues runs) would
  hold its chain for good while its pull request is merged and its agent idle. Requiring a
  merge for "Run after" was rejected too: a prompt-mode task that opens a pull request for a
  person to merge is done, and `#N merged` already says the other thing.
- **The wait is the fact the list already has** (`yulius_wait`: "goes on by itself when Yulius
  has news"). Since SPEC-444 a task hears of its pull request without a `yulius wait` shell,
  so the fact now also holds when the news is due without one. No new state for the web UI.
- **What the watch told is read from memory**, kept beside its file: the task list asks for
  every idle task with a merged pull request.
- **`after_all_tasks` is stored as the rule it means**, so it reads back, shows in the editor
  and is evaluated like any rule. A task that ends other than done makes it need a person, as
  for every chain: "all done" is not "all gone".
- **What `after_all_tasks` waits for is what the list shows as not finished** (review round 1,
  M4). Naming every task that has not ended would name an "Answered" session, which never
  becomes `done` by itself: the rule would never be true, and closing that session would make
  it false. So the tool asks the list's own rule (`AttentionFacts::group`) and leaves Done
  and ended tasks out.
- **Only a person's pause mutes news** (review rounds 1 and 2). Nothing is typed into a task
  a person paused, and that pause may never end: waiting for it would hold a chain and keep a
  merged task from being archived, so such a task reads as before this change (an armed merge
  excepted). An update's hold ends by itself and the watch types right after, so a merge that
  lands during an update is still news due.
- **News typed while a turn runs is followed through Claude Code's queue** (review round 2),
  which Yulius already reads for moves of a session (SPEC-471, SPEC-479), not by a second
  clock: the running turn may take any time. Time decides only until the queue's record of it
  was read.
- **The person's decisions (asked 2026-11-13, answered "ok").** For the later slices:
  production deploy, rollback and promotion to `main` stay behind one confirmation the person
  gives in the Superagent's panel (the typed project slug; stored, amendable, still there when
  they return); each cross-sandbox action keeps its approval, made lasting; Update now and
  Restart run directly for a superadmin's Superagent, with the buttons' refusals; no agent
  ever sees a secret's value, which the person types masked beside the chat.
- **The rule is stored in its canonical words** (`#12 finished && #13` reads back as
  `#12 done and #13 done`): one form for the editor, the tools and the evaluation.
