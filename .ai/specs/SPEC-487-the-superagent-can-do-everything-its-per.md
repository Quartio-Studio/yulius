---
yulius-task: 1be0c20e-fe89-4ec0-a1ff-5c054ebc7ae7
status: done
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
[ADR-0328](../../docs/decisions/0328-a-task-is-not-done-while-yulius-has-pull-request-news-for-it.md),
the rule for the Superagent is
[ADR-0330](../../docs/decisions/0330-the-superagent-does-what-its-person-may-do.md).

SPEC-612 supersedes CHN-002 for finished deliveries. Their merge receipt releases chained work without a terminal message. Running tasks with declared work left keep their PR notifications.

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
/ delivery, then subscriptions and instance, then the rest"). The Plan below holds every
slice (ADR-0325): a slice's step is done when its change is reviewed and approved, and its
pull request delivers that part.

| Slice | Delivers | Closes |
| --- | --- | --- |
| 1 | A chained task starts when its predecessor really finished; the rule is shown as stored; "after all current tasks" | wall 3 |
| 2 | The rule (its decision record) and the person's project tools served to the Superagent: restart, pause, resume, start now, a task's delivery, CI read / run again / cancel, CD status and dev deploy; `COCKPIT_INSTRUCTIONS` and the tool descriptions rewritten | walls 2 and 4 (part), 12 |
| 3 | Task control that had no function: rename, edit of a prompt not started, order in the queue, close out, bulk stop / pause / message | wall 2 |
| 4 | Tasks' questions: list open ones, why a task needs attention, answer or dismiss for the person, the end of a session (`session_read` tail) | wall 7 |
| 5 | Merges: arm or re-arm a task's merge for the person, the merge queue of the sandbox, why a green pull request is not merging; CI order and hold | wall 4 |
| 6 | Subscriptions and model: which one a task is on, move it, change model or effort | wall 5 |
| 7 | The instance: version, health, update status, checks, Update now and Restart with the buttons' refusals; reconnection after a restart | wall 1 |
| 8a | Requests to the person that last: cross-sandbox requests stored, amendable, still there later; a pending decision of the Superagent's own; another sandbox's task list | walls 6, 9 |
| 8b | Production deploy and rollback, promotion to `main` and a project's onboarding behind one confirmation in the panel with the typed project slug (the person's decision of 2026-11-13) | wall 4 (production) |
| 9a | Nothing typed to the Superagent is lost in a restart: keys typed while it restarts reach the new session, and an update's restart waits for text left in its prompt box | wall 8 |
| 9b | Its standing jobs have a durable home: scheduled checks held by Yulius, kept across restarts, typed into its session when it is idle | wall 8 |
| 10a | Secrets: a value the person types masked on the Superagent's panel goes to the vault; the agent sees its name only | wall 10 |
| 10b | Visibility: a task's cost and load, a crash's exit and last screen, a checkout that is current | wall 11 |
| 11 | A missing ability reported to the Yulius project in one call; the ability table in the product documentation, complete | wall 12 |

### The ability table

What the Superagent and a task's agent may do, and why; the same table is in
`docs/product.md`. "Yes" is with the person's rights: the function the web UI's route calls,
with those rights checked at the call.

| Ability | Superagent | Task's agent | Why |
| --- | --- | --- | --- |
| Create, chain, schedule, stop, archive, delete a task; automations | yes (had) | yes (had) | SPEC-037: a follow-up runs as its own task |
| Read a start rule back as stored; "after all current tasks" | yes (slice 1) | yes (slice 1) | Both create tasks with `task_create` |
| Restart, pause, resume, start now | yes (slice 2) | no | A person speaks to their tasks; tasks do not steer each other (ADR-0178) |
| Start a project's onboarding (also by continuing its onboarding task before it started) | behind the person's click in the panel (slice 8b) | no | SPEC-051 OBD-002, ADR-0038: a person decides that a repository is adapted |
| Project variables: read | yes (slice 2) | in its environment (had) | They are not secrets |
| Project variables: set, delete | yes (`variables_set`, `variable_delete`, slice 11; Configure) | no | Plain values, never a secret's; as a person's own agent through the remote MCP |
| Rename, edit a prompt not started, reorder, close out, bulk actions | yes (slice 3) | no | As above |
| Send a message past text the person left unsent in a task's prompt box | yes, for the task's owner (slice 3) | no | SPEC-480: the text is the person's; they decide that it goes out with the message |
| Discard text a person left unsent in a task's prompt box | no | no | Yulius sees keys, not the box, and the keys that clear it (Esc, Ctrl+C) also interrupt a turn or end the session; the owner's message goes past it instead |
| Message a task | yes (had) | no | ADR-0178 |
| List open questions, see why a task needs attention | yes (slice 4) | yes, read only, without a held prompt's words (slice 4) | A task that sees another's question does not ask it again; a prompt Yulius held reached no agent |
| Answer or dismiss a task's question for the person; decide a held prompt (send, dismiss, a new task now or after) | yes (slice 4) | no | The question is to a person; their Superagent answers when they say so |
| Decide a held prompt by skipping steps or reverting the task | no: the person, on the task's page | no | Reverting discards the task's work and asks for the person's own confirmation there (TSK-013) |
| Answer a question Claude Code asks in a task's terminal (a permission menu, a choice) | no: it reads the menu (`task_screen`) and tells its person | no | Typing into a task's terminal is its owner's control (SPEC-490); keys typed from a screen read a moment before could take another choice |
| The end of a session (`session_read` tail) | yes (slice 4) | yes (slice 4) | Sessions are docs for both (SPEC-001 SES) |
| CI: read runs and logs, run again | yes (slice 2) | yes (had) | A task needs its own checks |
| CI: cancel, order | yes (slices 2, 5) | no | They change other tasks' runs |
| CI: hold the queue | no: no page holds it either; cancel a run and run it again later | no | A held queue would hold every task's checks and merges; asked for in no case where cancel and run again did not do |
| A task's delivery, the merge queue, why a pull request is not merging | yes (slices 2, 5) | its own (had) | A task delivers its own work |
| Arm or re-arm a task's merge; merge a task's pull request | yes, for the task's owner (slice 5) | its own, after an independent review (had) | The person decides what of theirs merges, as the page's Merge does; an agent delivers its own reviewed work |
| Dev deploy, CD status | yes (slice 2) | yes, when its task asks (had) | SPEC-134 |
| Production deploy and rollback, promotion to `main` | behind the person's typed project slug in the panel (slice 8b); a dev-stage rollback at once | no (a dev-stage deploy and rollback, had) | ADR-0110: a person decides; the Superagent prepares and asks |
| A task's subscription: see, move; model and effort | yes (slice 6) | sees them in the task list (slice 6) | Tasks are subscription-agnostic (SPEC-001 ROT-007); a person moves their task |
| The pool's settings: priorities, excluding a subscription | no: no such setting exists ("the pool has no priorities or switches") | no | An administrator adds, signs in or removes a subscription on its panel |
| Instance: version, health, update status, checks | yes (slice 7) | no | Not a task's work |
| Instance: Update now, Restart, Cancel, Check | yes, a superadmin's (slice 7; not the Global Superagent) | no | The buttons' rights and refusals |
| A task's screen, live or its last run's, and how that run ended (exit code, reason) | yes (`task_screen`, slice 10b) | no | The page's terminal shows it to anyone who sees the task; a task's agent reads its own transcript, not another's screen |
| A task's tokens, the period's by model | yes (`task_usage`, slice 10b; cost as tokens: no price per task is kept) | no | The Dashboard's figures |
| A task's or the host's load | the host's (`instance`); a task's: no | no | Yulius measures the host and the sessions per sandbox, not each task's processes |
| How far the repository's checkout is from its integration branch | yes (`checkout`, slice 10b) | it has its own branch and `git` | A stale checkout gave a wrong "no" |
| Instance: logs, `yulius doctor` | no: no page shows them; `instance` shows health (load, memory, disk, sessions) | no | The logs and the doctor are the host's (`journalctl`, the CLI), outside what Yulius serves |
| Another sandbox: list and read tasks | yes (slice 8a) | no | SPEC-335: only the person's Superagent reaches their other sandboxes |
| Another sandbox: create, change, message, stop | behind one confirmation in the panel (had; lasting, amended, withdrawn in slice 8a) | no | ADR-0227, ADR-0333 |
| Leave a decision only the person makes, answered Yes or No on its panel (`person_ask`) | yes (slice 8a) | asks with `needs_you` (had) | A question in a report is not read; the panel keeps it until answered |
| A secret's value | never | never | SPEC-002 ADR-20: no agent sees a value |
| Enter a secret's value | the person, masked, on the Superagent's panel (`secret_request_value`, slice 10a) | no (it asks a person: `secret_request_copy`, `needs_you`) | The value goes from the person to the vault; ADR-0336 |
| Report a missing ability to the Yulius project | yes (`ability_report`, slice 11) | yes (slice 11) | Both meet walls; ADR-0337 |
| Standing jobs (monitoring, a disk guard) | yes: scheduled checks Yulius holds (`superagent_check_set`, slice 9b) | its own automation or task | Its session's timers end with every restart; ADR-0335 |
| Text typed to it while it restarts, or left in its prompt box | kept (slice 9a) | (tasks: SPEC-468, SPEC-480) | A person's message was lost to a restart |

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| CHN-001 | A running task whose merge is armed or being decided is not Done, whatever its pull request's checks say: Yulius will merge it and tell its agent. A task chained after it, or waiting on a rule that names it `done`, keeps waiting, and the task list reads "Waiting for checks or merge", not Done and not a person's turn. A server that started again knows the armed merges before it put them back. | `merge_train::a_task_chained_after_an_armed_green_pull_request_waits_for_its_merge`, `merge_train::a_restarted_server_keeps_a_chained_task_waiting_for_an_armed_merge` |
| CHN-002 | A running task whose pull request is merged is Done only once the pull request watch, when it saw that pull request open, has told its agent of the merge and the turn that news starts has ended. While the news is typed and that turn has not begun, it still is not Done: typed into an idle session, until the task goes from idle to Working, for at most two minutes; typed while a turn runs (Claude Code queues it), as long as Claude Code's queue says it waits for its turn, whichever entry gets its turn first, for at most two minutes of an idle session, and for two minutes from the typing while the queue has not shown it yet. The same holds for an armed merge that ended without merging until the watch said how, and for a merge GitHub reports before the record says so. One decision reads the task's delivery once. A task the watch tells nothing is Done as before: a playground's, one whose pull request it never saw open, and one a person paused. An update's hold or a pause for capacity changes nothing: the news is typed right after. A supertask's wait and Home's counts follow the same rule. | `task_done::a_chained_task_waits_until_its_predecessor_heard_of_its_merge` (real hooks, the prompt hook held at its check), `task_done::news_typed_while_a_turn_runs_is_due_until_its_own_turn` (a held turn, Claude Code's queue, a held hand-over), `task_done::queued_news_stays_due_while_another_entry_gets_its_turn_first`, `merge_train::a_merge_github_reports_first_releases_the_finished_tasks_successor`, `merge_train::a_task_chained_after_a_dropped_armed_merge_waits_for_its_news`, `supertask::a_supertask_waits_for_a_task_that_has_not_heard_of_its_merge`, `supertask::a_merge_during_an_updates_hold_is_still_news_due`, `pr_watch::tests::news_is_due_until_the_agent_heard_it`, `pr_watch::tests::typed_news_is_due_until_its_turn` |
| CHN-003 | An achieved goal is Done as before, and so are a closed task and a task with no pull request news left: CHN-001 and CHN-002 remove no Done state that was final. (Amended by SPEC-491, ADR-0332: by its plan and pull request a task is Done only when the pull request would merge as it is and the task's spec does not say that work is left; an achieved goal stays Done.) | the tests of TSK-012 and TSK-014 unchanged (`intake::chained_tasks_start_on_done_and_wait_for_a_person_otherwise`, `task_done::a_chain_starts_when_its_predecessor_achieves_its_goal`) |
| CHN-004 | `tasks_list`, `task_get`, `task_create` and `task_update` answer how a waiting task starts as it is stored: `start_rule` with the whole rule for a task waiting on a rule, `runs_after` only for a task chained after one task, and `start_blocked` with the reason when it needs a person. Their text shows the rule in full. A task that has started answers neither `start_rule` nor `start_blocked`. | `manage_tools::a_start_rule_reads_back_whole`, `manage_tools::task_update_after_all_tasks_with_nothing_left_starts_now`, `mcp_text::tests::a_waiting_tasks_rule_reads_whole` |
| CHN-005 | `task_create` and `task_update` take `after_all_tasks: true`, alone among the start choices (a blank text is no choice): the task waits for every task of the project with work still coming to be done, stored as the rule that names them, so it reads back like any rule. Left out: tasks that ended or are stopping, tasks the list shows under Done (done or "Answered"), prepared tasks, and the task whose start changes with the tasks waiting on it, directly or through others. A rule names at most 64 tasks in at most 1400 characters; more tasks are refused with what to do. With no such task it starts now and the answer says so. The caller's person is checked before anything is counted. | `manage_tools::after_all_tasks_names_every_task_that_has_not_ended`, `manage_tools::after_all_tasks_with_nothing_left_starts_now`, `manage_tools::task_update_after_all_tasks_with_nothing_left_starts_now`, `manage_tools::after_all_tasks_counts_nothing_for_a_person_without_access`, `rule::tests::the_limits_are_enforced` |
| CHN-006 | `task_create`, `task_update` and their descriptions say what `done` means (the task finished and nothing of its pull request is left to come), and name `#N merged` for work that needs the merge itself. | `mcp::tests::the_start_choices_say_what_done_waits_for` |
| SAR-001 | The rule: whatever a person may do in Yulius, their Superagent does for them when they ask, by a tool that runs the function the web UI's route calls, unless the ability table names why not. The person's project tools are one list (`person_tools::PERSON_TOOLS`), served by the same code to the Superagent in the sandbox, to the person's own agent through the remote MCP (with `project`) and to their Global Superagent (without what ADR-0271 withholds). With slice 2: `task_restart` (Continue), `task_pause`, `task_unpause`, `task_start_now`, `task_delivery`, `ci_runs`, `ci_run`, `ci_rerun`, `ci_cancel`, `cd_status`, `cd_deploy` (dev-stage environments), `variables_list`. | `cockpit::the_superagent_does_what_its_person_may_do_with_tasks_ci_and_cd`, `cockpit::a_cockpit_manages_the_project_for_its_owner` (the tools it is served), `mcp::tests::the_superagent_is_served_the_persons_project_tools_and_a_task_is_not`, `remote_mcp::tasks_ci_and_cd_are_driven_remotely` (unchanged), `person_tools::tests` |
| SAR-002 | A person's tool runs with the person's rights as they are at that call: the person is enabled, the tool's action on the project holds (view to read, write to change), and the function checks the rest (a task is continued, paused, resumed or started by its owner only). A tool that changes something leaves exactly one audit entry `superagent.tool` with the Superagent as the actor, the tool's name, what it was about and whether it went through, refused calls included, also when the client went away during the call. What it was about is the task, run or environment that tool takes, and only when the argument has the shape of one (a number or an id of at most 64 letters, digits, `-` and `_`; an environment's name): an audit entry never holds text of the caller's choosing; a tool that only reads leaves none, and is pre-allowed where Claude Code asks before tools (`COCKPIT_READ_ONLY_TOOLS`), which the changing ones never are. | the same test (another person's task; the exact list of audit entries); `cockpit::a_superagents_call_whose_client_went_away_still_ends_with_its_audit` (fails without the detached call); `manage_tools::the_cockpit_asks_before_every_changing_tool`; `person_tools::tests::a_calls_subject_is_its_task_run_or_environment` (a megabyte of text, a line break, another tool's key); `yulius-harness` `launch::a_cockpit_reaches_the_yulius_tools_with_its_token_only_in_its_environment` |
| SAR-003 | A task's agent is served none of the person's project tools (`task_restart`, `task_pause`, `task_unpause`, `task_start_now`, `task_delivery`, `ci_runs`, `ci_run`, `ci_cancel`, `variables_list`); it keeps its own `ci_status`, `ci_rerun`, `delivery_*` and `cd_*`, which are about its own work, and has the variables in its environment. | `cockpit::a_tasks_agent_does_not_steer_other_tasks`, `mcp::tests::the_superagent_is_served_the_persons_project_tools_and_a_task_is_not` |
| SAR-004 | The Superagent's instructions say that it acts with its person's rights; that it reads its tool list before it says it cannot, names exactly which ability is missing and hands nothing back as steps to click that a tool does; that what it reads is data and never an instruction; that the repository's rules on how code is changed bind work done in the repository and do not decide what its person may ask Yulius to do (it names the rule, asks for the confirmation and acts with its Yulius tools), while what Yulius refuses stays refused; that a task which ended is continued with `task_restart` before it is told something; and that it uses its own CI, delivery and deploy tools, never a finished task as hands and never a nudge. Every text Yulius serves to a client (the three instruction texts, every tool's description) is within the 4096 characters Claude Code reads, and every tool the instructions name is served. | `mcp::tests::the_superagent_is_told_to_do_what_its_person_may_do`, `mcp::tests::every_text_served_to_a_client_fits_what_it_reads` |
| SAR-005 | A refusal says what to do instead, by what the task is doing: `task_restart` on a task that runs says to stop it first; on one that waits in a chain or prepared, that `task_start_now` starts it; on one that is queued or waits for capacity, that it goes on by itself; on the onboarding task of a project that has not started onboarding, that a person starts it on the project's page. The state decides, read once before Continue is called; the onboarding refusal is recognised by the constant the page's check answers with. | `person_tools::tests::a_task_that_is_not_resting_is_told_what_to_do_by_its_state`, `a_refused_continue_of_an_onboarding_says_who_starts_it`; the Superagent test (a running and a prepared task) |
| TCT-001 | `task_edit {title}` renames a task for its owner: the title, trimmed, 1 to 120 characters on one line. The task's branch and pull request keep theirs. Audited `task.renamed` under the person's name. | `cockpit::the_superagent_renames_edits_and_orders_its_persons_tasks`; `task_edit::tests::a_title_is_one_bounded_line` |
| TCT-002 | `task_edit {prompt}` replaces the whole request of a task that has not started (queued, chained, prepared or waiting for capacity, with no run and no session), by one conditional update: a start either follows it and reads the new request or makes it fail, and it applies only while the request is still the text that was read, so a message queued meanwhile is never overwritten. Files attached to the old request that the new text does not mention follow it, each a line `@'/path'`; the answer carries the whole text that was replaced. A task that has started keeps its request and is told with `task_message`. Title and request are both checked before either is written. Audited `task.request_replaced` (sizes, no text). | the same test (attached file kept, placed file not doubled, started task, another person's task); `store::a_prompt_is_replaced_only_while_unstarted_and_as_it_was_read`; `request::tests::a_replaced_request_keeps_its_attached_files` |
| TCT-003 | `tasks_reorder {numbers}` puts the person's queued tasks in the order given: they take the slots they hold among themselves in the queue (the earliest to the first), in one transaction, and every task not named keeps its slot, so no other person's task moves. The queue has one order (the place, then the order of creation); where tasks shared a millisecond, the places from the first named slot on are written a millisecond apart so the order asked for is the order read. Each task's stored start keeps naming its place. Refused, with nothing changed, when one of them is not queued, is another person's, is no task, or is named twice, and for fewer than two. | the same test; `store::queued_tasks_swap_the_places_they_hold`, `queued_tasks_that_share_a_millisecond_take_the_order_asked_for`; `task_edit::tests::a_call_names_a_bounded_list_of_tasks_each_once` |
| TCT-004 | `task_close` closes out a running task whose agent is idle, for its owner: the session ends gracefully and the task is `closed`, as Yulius closes a finished task, whatever its plan or pull request say; `task_restart` continues it. Background shells it left end with the session. Refused: a task in a turn (a turn in progress counts whatever its activity reads; its first turn, before any activity is known, too) with "wait or stop it"; a task Yulius still has pull request news for (CHN-001, CHN-002: it goes on by itself and its chain waits for that); one that has not started; one that waits with a session of its own (queued again, held for capacity); one that has ended. Audited `task.closed_by_person`. | `task_message::the_owner_closes_out_an_idle_task` (a live session); `task_edit::tests::only_an_idle_running_task_is_closed_out` (every case of the rule); `cockpit::the_superagent_acts_on_several_of_its_persons_tasks_in_one_call` (the refusals through the tool, a held task with a live run among them) |
| TCT-005 | `tasks_bulk {do, numbers \| all}` does one action (stop, pause, unpause, message) to several tasks in one call. Each task gets what the single tool does, with its checks and its audit; one task's refusal does not stop the others, and the answer names the tasks done and the tasks refused with the reason. `all` is every task of the person's own that the action applies to (stop: not ended; pause: running and not paused; unpause: paused by a person; message: running or not started); other people's tasks are left alone and counted. A stop takes the waiting tasks before the running ones, so no session it frees starts a task it is about to stop. An unpause of a task nobody paused is refused, not listed as done. At most 100 tasks a call; what `all` did not reach is said. The call runs to its last task and its audit when the client goes away, from the Superagent and through the remote MCP. | the same test; `task_edit::tests::all_takes_the_tasks_an_action_applies_to`, `a_bulk_stop_takes_the_waiting_tasks_first`; `remote_mcp::a_persons_agent_edits_orders_and_stops_tasks_remotely`, `a_bulk_call_whose_client_went_away_still_reaches_every_task`; `cockpit::a_bulk_call_reaches_a_hundred_tasks_and_says_what_is_left` |
| TCT-006 | `task_message {with_draft: true}`: the task's owner, and only they, has a message sent past text they left unsent in the task's prompt box (SPEC-480 held every message until they went to the terminal). The message is typed behind that text, after a blank line, and both are submitted as one prompt; that submit ends the draft as a prompt sent from the box does, also a draft known only from its mark after a restart of the server; never within a minute of their last key, and not while a prompt Yulius held for a decision (TSK-013) is back in the box. The refusal for a draft names the argument. | `task_message::the_owner_sends_a_message_past_their_own_draft` (a live session: at the keyboard, without the argument, another owner, a held prompt, then sent as one prompt and the next plain message typed), `a_message_sent_past_a_draft_from_before_a_restart_ends_that_draft` |
| TCT-007 | The four tools are the person's project tools (SAR-001, SAR-002): served to the Superagent and, with `project`, through the remote MCP, to no task's agent; each call from the Superagent leaves one `superagent.tool` entry naming the task, or the tasks (numbers, or `all`) and the action, never a title, a request or a message. None is pre-allowed. | the two Superagent tests (audit), `cockpit::a_tasks_agent_does_not_steer_other_tasks`, `manage_tools::the_cockpit_asks_before_every_changing_tool`, `remote::tests::every_persons_project_tool_is_served_by_both_callers_with_one_action` |
| QST-001 | `tasks_list` and `task_get` say, for each task, where the task list shows it (`shows`: needs_you, working, queued, done, ended, by the list's own rule) and, only when there is one, why it waits for a person or is held, as its page does: `needs_you` (what its agent asked), `attention` (the reason of its flag: Claude Code asks in its terminal, or Yulius's reason and what to do), `held_prompt` (a typed prompt Yulius held), `pause` (who paused it). The text a model reads says the same, each text on one line without what hides or reverses text, so typed or agent-written words cannot forge another task's line. A task's agent reads it too, without a held prompt's words: what a person typed and Yulius held reached no agent and no transcript. | `cockpit::the_superagent_sees_why_tasks_need_its_person_and_answers_for_them` (the record, the text, a working and a paused task, a task's agent) |
| QST-002 | `tasks_list {scope: "needs_you"}` lists the tasks the list shows under Needs you, other people's included (they are read by everyone who sees the project), and no other. | the same test (four of five tasks; after a dismissal and a decision, two) |
| QST-003 | `session_read {tail: true}` returns a session's end: the last `limit` entries whose shown text fits `max_chars` (a tool's long result counts as the few lines shown of it; a note that shows nothing neither takes the place of the last entry that does nor counts toward `limit`), leaving entries out at the front and never at the end, redacted as every read; it says where the text begins (`from`), whether lines before it were left out (`earlier`) and where a later read goes on (`next_from`). `tail` with `from` is refused. | `sessions::agents_search_and_read_only_their_own_sandbox_sessions` (three of five entries, the whole session, a bound of 1000 characters, a file read before the tail still redacted, both arguments) |
| QST-004 | The person's answer to a task's question is passed on with `task_message`: a message that is sent ends the agent's request. The tool's description and the instructions say so. | `task_message::a_remote_message_names_the_persons_own_agent` (unchanged: the request ends with the message); `mcp::tests::the_superagent_is_told_to_do_what_its_person_may_do` |
| QST-005 | Two more of the person's project tools (SAR-001, SAR-002; never a task's agent's): `task_question_dismiss` ends the request of a task that has not ended (its agent's, or one Yulius opened for it; an ended task waits for nobody and shows none) for the task's owner without a word to the agent, only while it still asks what was read, and answers what was asked (audited `task.needs_you.dismissed`); with nothing asked it is refused, naming the flag's reason when the task is flagged. `task_held_prompt {action}` decides the task's held prompt with the function its card calls: send, dismiss, parallel or after; skipping steps and reverting the task stay on the page. The `superagent.tool` entry names the task and the decision, never the request's or the prompt's words. | the same Superagent test (another person's request, a second dismissal, a flag, the four actions and the two refused, the hold's state, the audit); `person_tools::tests::a_calls_subject_is_its_task_run_or_environment` |
| MRG-001 | `task_delivery` adds `merge`: whether a merge of the task is armed, the head that is to merge and since when, whether Yulius decides it now, why it has not merged yet (its merge train's sentence, or what its last check found: a branch behind its base, an agent still working, checks still running) and how the last armed merge ended. | `merge_train::a_person_merges_their_tasks_pull_request_when_its_checks_pass` (before, while armed, after the merge); `cockpit::the_superagent_reads_the_merge_queue_and_orders_ci` (through the tool) |
| MRG-002 | `merge_queue` lists the project's armed merges in the order a merge train takes them (when they were armed), each with its task, owner, pull request, head and why it has not merged, and the merge train under test (its run, its members in merge order, whether it passed and is merging, those left out for conflicts). Read with view access. | the same two tests |
| MRG-003 | `task_merge` merges a task's pull request for its owner as the page's Merge does when the checks at its head passed and the branch is up to date; otherwise it arms the merge of the branch's head for them, which Yulius merges once those checks pass (bringing the branch up to date first when it can), keeps in its place when asked again and drops at a new commit. The arm remembers that the owner made it: the merge it makes is audited under their name (`delivery.person.merge`, "armed by its owner, no independent review asked for"), never as an agent's reviewed merge, and the task's delivery says who armed it. Refused, with the reason, when the checks failed, without an open pull request, for a pull request into another branch, for another person's task, for a task whose run ended (an armed merge lives with the run: it merges at once once its checks passed), for a head the pull request does not have yet (not pushed), and for what the branch policy refuses. A playground's task goes to its Merge, which squashes on green. | the merge-train test (armed, kept in place, merged by the pass, a failed head refused, the audit); the Superagent test (another person's task, no pull request) |
| MRG-004 | `ci_prioritize` puts a queued CI run before its project's other queued runs, and the project's next free lane starts it; other projects' queues are not touched (each project has its own lanes). It never goes before another run of its own commit, since the latest run of a commit is read by place. Refused for a run that is not queued, is first already (or as far forward as it goes) or belongs to another project. Audited. | the Superagent test (three runs of the project, one of another, a running run, an unknown one, the audit) |
| MRG-005 | The three are the person's project tools (SAR-001, SAR-002): served to the Superagent and, with `project`, through the remote MCP, to no task's agent; `merge_queue` is pre-allowed, the two that change something are not. The Global Superagent is not served `task_merge` (as `change_merge`, ADR-0271: text read in one sandbox must not merge code in another). | the Superagent test ("unknown tool" for a task's agent); `manage_tools::the_cockpit_asks_before_every_changing_tool`; `yulius-harness` `launch::a_cockpit_reaches_the_yulius_tools_with_its_token_only_in_its_environment`; `person_tools::tests` |
| SUB-001 | `subscriptions` shows the subscriptions as the Subscriptions panel shows them to the caller (names, states, usage, capacity, burn rate, paused tasks; ids and live counts for an instance administrator). | `cockpit::the_superagent_reads_subscriptions_and_sets_a_tasks_model` |
| SUB-002 | `tasks_list` and `task_get` name each task's subscription (its live run's, none without one or for a removed one), model and effort (none: the instance's defaults), in the record and the text. | the same test |
| SUB-003 | `task_switch` moves a task for its owner to the subscription named by its label (any case) or id, or to the pool's best other one: the page's Switch. A name that is no subscription is refused with the names there are; one shared by two subscriptions is refused with how to name one. | the same test (an unknown name, a shared name, another person's task) |
| SUB-004 | `task_model` sets a task's model, effort or both for its owner (`default` goes back to the default); stored with the task, taken at its session's next start, and said so (not started: when it starts; running: at its next start, `task_switch` restarts it now; ended: when continued). A value of a shape a launch does not take is refused. Audited. | the same test; `task_setup::tests::a_model_and_an_effort_are_what_a_launch_takes` |
| SUB-005 | The three are the person's project tools (SAR-001, SAR-002), to no task's agent; `subscriptions` is pre-allowed. | the same test; `manage_tools::the_cockpit_asks_before_every_changing_tool`; `launch::a_cockpit_reaches_the_yulius_tools_with_its_token_only_in_its_environment` |
| INS-001 | `instance` shows the Status panel and Settings' update section as the caller may see them: the build Yulius runs, an update offered or under way, the host's CPU load, memory and disk, the sessions and their bound, the sandboxes and subscriptions. | `cockpit::the_superagent_reads_the_instance_and_a_superadmins_updates_it` |
| INS-002 | `instance_update` runs, for a superadmin, the update buttons' functions: check now, install the offered update (or a given commit), restart with the build it runs, cancel an update under way. The ask is the confirmation the buttons ask for (the person's decision of 2026-11-13); the functions' refusals pass as they are. | the same test (the superadmin's four, against a server without an update helper) |
| INS-003 | Anyone else is refused with whose it is ("only a superadmin …"); an action that is none is refused. | the same test |
| INS-004 | Both are the person's project tools (SAR-001, SAR-002), to no task's agent; `instance` is pre-allowed; `instance_update` is the Superagent's only: not served through the remote MCP (a long-lived token's agent; the person decided it for a superadmin's Superagent) nor to the Global Superagent (text read in one sandbox must not pause everyone's work). The audit entry names the action. The instructions keep naming web pages among the text that is data. | the same test (through the tool as a stored superadmin, the audit's action); `remote::tests::every_persons_project_tool_is_served_by_both_callers_with_one_action`; `global_superagent::tests::it_is_not_served_change_merge`; `mcp::tests::the_superagent_is_told_to_do_what_its_person_may_do`; `manage_tools::the_cockpit_asks_before_every_changing_tool`; `launch::a_cockpit_reaches_the_yulius_tools_with_its_token_only_in_its_environment` |
| XRQ-001 | A request of the Superagent to its person (another sandbox's task created, changed, messaged or stopped; a `person_ask` question) stays on the panel until the person answers it or the Superagent withdraws it. When the call's wait (4 minutes) ends first, or the call is dropped, the call answers `{"outcome":"waiting","request":<id>}`; answered later, it runs as it would have. Nothing is audited until it is answered or withdrawn. `notifications/cancelled` for the call still withdraws it. | `cross_sandbox::an_unanswered_request_waits_and_is_answered_later`, `a_dropped_call_leaves_its_request_waiting`, `an_unanswered_change_waits_and_is_audited_when_answered` |
| XRQ-002 | The waiting requests and the newest 200 outcomes are kept in `superagent-requests.json` beside the database, written whole at every change, and put back at a start; a request whose target sandbox is gone is dropped. Nothing is written from the start until they are back, so a request asked meanwhile loses none. | `cross_sandbox::waiting_requests_are_kept_across_a_restart` (a second `App` on the same data), `a_request_asked_before_the_restore_loses_none` |
| XRQ-003 | `sandbox_request_amend` replaces what a waiting create, message or question asks, checked as a new request is, keeping its id and place, audited `amended` without its text; an update or a stop is refused (withdrawn and asked again). An amended request takes the next `revision`: the card is drawn again, and an answer naming another revision (none is 0) is refused (409) while it keeps waiting. `sandbox_request_withdraw` removes one, audited `withdrawn`. Another Superagent's request is not found. A call that ended no longer names its request, so a later `notifications/cancelled` with its id withdraws nothing. | `cross_sandbox::a_waiting_request_is_amended_or_withdrawn`, `an_amended_request_is_answered_only_as_read`; `cockpit::a_cancel_after_the_call_ended_withdraws_nothing`; `superagent-requests.e2e.ts` "an amended request is drawn again and approved as drawn" |
| XRQ-004 | `sandbox_requests` lists the Superagent's own waiting requests and its 20 latest outcomes (the call's answer: approved with what was done, declined, failed with why, withdrawn, cancelled), newest first. | `cross_sandbox::an_unanswered_request_waits_and_is_answered_later`, `a_waiting_request_is_amended_or_withdrawn` |
| XRQ-005 | `person_ask {question}` (1 to 4096 bytes, no character a person cannot see) waits on the Superagent's own panel: the question read to its end, **Yes** / **No**; the answer is `yes` or `no`, the call's or read later; nothing runs on it. | `cross_sandbox::a_decision_waits_on_the_panel_and_its_answer_is_kept`; `superagent-requests.e2e.ts` "a decision the Superagent leaves to its person is answered Yes or No on its panel" |
| XRQ-006 | `sandbox_tasks_list {sandbox}` lists another sandbox's tasks as `tasks_list` does there, without asking; its own sandbox is refused naming `tasks_list`, one the person cannot work in is not found. | `cross_sandbox::the_superagent_lists_the_tasks_of_another_sandbox` |
| XRQ-007 | The five are the Superagent's only (no task's agent, no remote MCP, not the Global Superagent); `sandbox_tasks_list` and `sandbox_requests` are pre-allowed, the other three ask in Claude Code first as `sandbox_task_create` does; none deletes or archives a task of another sandbox. The instructions say a request stays until answered and never to ask twice. | `cockpit::a_cockpit_manages_the_project_for_its_owner`; `manage_tools::the_cockpit_asks_before_every_changing_tool`; `mcp::tests::no_tool_deletes_or_archives_a_task_of_another_sandbox`; `launch::a_cockpit_reaches_the_yulius_tools_with_its_token_only_in_its_environment` |
| REL-001 | The Superagent's `cd_deploy` and `cd_rollback` of a production-stage environment (`prod`, or one whose stage is `prod`), `promote` and `onboarding_start` are requests on its own panel, lasting as XRQ-001 and XRQ-002 say (no amend; withdraw). A release's card names its action and environment (and a release or a part given) and takes a click only once the project's slug is typed; the server checks the typed slug, trimmed, before it takes the request (a slug mistyped is refused, 422, and the request keeps waiting). | `cd_rollback::the_superagent_releases_production_once_its_person_types_the_slug`; `superagent-requests.e2e.ts` "a production deploy asked by the Superagent takes the project slug typed" |
| REL-002 | Refused before anyone is asked: a person who is no administrator of the project (a release), an environment the project does not define, a dev-stage environment through this path, onboarding that does not wait to start, unknown arguments. | the same test; `cockpit::the_superagent_does_what_its_person_may_do_with_tasks_ci_and_cd` (over the MCP) |
| REL-003 | A dev-stage deploy or rollback runs at once: `cd_rollback` is a person's project tool (SAR-001, SAR-002), also through the remote MCP, dev-stage only there; the Global Superagent is not served it, as it is not served `cd_deploy` ("only a person rolls production back, on the project's CD page"). | `cd_rollback::the_superagent_releases_production_once_its_person_types_the_slug` (`superagent_production`), `a_custom_production_stage_environment_waits_for_the_person`; `cockpit::the_superagent_does_what_its_person_may_do_with_tasks_ci_and_cd`; `remote::tests::every_persons_project_tool_is_served_by_both_callers_with_one_action`; `global_superagent::the_global_superagent_works_in_every_sandbox_of_its_person` |
| REL-004 | The approval runs the CD page's `cd_deploy`, `cd_rollback` or `promote_sandbox` with the answering person's browser session and the slug they typed; their checks and refusals at that moment are the answer of the click and of the waiting call. Audited `superagent.cross_task` with the action, never the slug. | `cd_rollback::the_superagent_releases_production_once_its_person_types_the_slug` (production deployed to v2, rolled back to v2 from v3 with `main` unmoved, a promotion's own "nothing to promote") |
| REL-005 | `onboarding_start {notes?}`, approved by a click, runs the project page's Start onboarding (`approve_onboarding`) as that person. | `cross_sandbox::onboarding_starts_on_the_persons_click` (the start's own answer: this harness has no Git); `superagent-requests.e2e.ts` (the click) |
| REL-006 | The three new tools (`cd_rollback`, `promote`, `onboarding_start`) ask in Claude Code first as every changing tool does; the instructions say production, `promote` and `onboarding_start` wait on the panel. | `manage_tools::the_cockpit_asks_before_every_changing_tool`; `cockpit::a_cockpit_manages_the_project_for_its_owner`; `mcp::tests::the_superagent_is_told_to_do_what_its_person_may_do` |
| SES-001 | What the person types into the Superagent's panel while it restarts in place (`switched`: an update's tools, a usage-limit move, Restart) is kept in whole frames, up to 16 KiB, without Ctrl-C, and typed into the new session once its first screen shows; the ended terminal keeps the keyboard until then. A restart that fails drops it. | `agent-panel.e2e.ts` "what is typed while the Superagent restarts reaches it once it is back", "what is typed for a restart that fails is not sent to a later session" |
| SES-002 | The restart that gets an update's tools waits while the Superagent's prompt box holds text its person typed since it was last emptied, for at most 30 minutes after the last keystroke; its state reads `held`. Text, a paste (also split across frames), Alt-Enter and backslash-Enter type; Enter outside a paste and Ctrl-C empty the box; Esc, Tab, arrows and editing keys do neither. Keystrokes are ordered by their count; a draft's mark is kept with the Superagent's record, so a server restarted for the update knows it. | `cockpit::an_updates_restart_waits_for_text_left_in_the_prompt_box`, `a_draft_typed_before_the_servers_restart_holds_the_superagents`; `cockpit::tests::a_draft_is_text_typed_since_the_box_was_emptied` |
| CHK-001 | `superagent_check_set {name, every_minutes, prompt}` creates or replaces a check of the Superagent's (a name of 1 to 60 characters on one line, 5 to 1440 minutes, a prompt of 1 to 2048 bytes, no invisible characters, no unknown arguments; at most 10); `superagent_checks` lists them with when each runs next and ran last; `superagent_check_delete {name}` deletes one. Audited `superagent.check` without the prompt. | `cockpit::the_superagents_checks_are_held_by_yulius_and_typed_when_it_is_idle` |
| CHK-002 | The checks are kept in `superagent-checks.json` beside the database (owner-only, one writer at a time) and read back at a start before the server takes calls; a check out of bounds is dropped, and a file that cannot be read is kept as `.bad`. | the same test (a second `App` on the same data; the `.bad` file) |
| CHK-003 | A due check is typed into its Superagent's session when its conversation is idle, no restart is pending, its prompt box holds no draft and nobody typed in the last minute (checked again with its terminal held): one paste labelled `Yulius check "<name>"`, then Enter; the next interval counts from then, a minute if it could not be typed; not in the first 20 seconds after the Superagent started, nor within 30 seconds of the last check typed into it. | the same test (typed into the idle session once the clock passes its interval); `superagent_checks::tests::a_check_waits_for_a_quiet_prompt_box` |
| CHK-004 | Every minute, the Superagent of a due check that is not running is started as opening its panel would, with its person's rights then: not for a disabled person; a start that fails is tried again after 2, 4, … up to 60 minutes; the checks of a person or a sandbox that is gone are deleted. | the same test (`start_superagents_for_due_checks` after the Superagent ended, for a disabled person, for a removed sandbox) |
| CHK-005 | The three tools are the Superagent's only; `superagent_checks` is pre-allowed, the other two ask in Claude Code first; the instructions send standing jobs to `superagent_check_set`, never to a timer of the Superagent's own. | `cockpit::a_cockpit_manages_the_project_for_its_owner`; `manage_tools::the_cockpit_asks_before_every_changing_tool`; `launch::a_cockpit_reaches_the_yulius_tools_with_its_token_only_in_its_environment`; `mcp::tests::the_superagent_is_told_to_do_what_its_person_may_do` |
| SEC-001 | `secret_request_value {name, description?, scope?}` is a request on the Superagent's own panel (lasting; no amend); refused before anyone is asked for a name the vault takes from no agent (not capitals, digits and underscores, reserved, the Vidoc key), a scope other than `sandbox` (the default, named on the card; Configure needed) or `personal`, a description over 500 characters, unknown arguments. The card names the secret, the project, its scope, what it is for and, when it is so, that it replaces a value; it holds a masked multi-line field (a password field where the browser cannot mask one); Store takes a click once 8 characters are typed. | `cockpit::a_secret_value_typed_on_the_panel_reaches_the_vault_only`; `superagent-requests.e2e.ts` "a secret the Superagent asks for is typed masked on its panel and stored" |
| SEC-002 | The answer carries the value once; it is checked as the Secrets page checks a pasted value (a final line break dropped, a key's line breaks as literal `\n`, a cut or flattened key refused) before the request is taken: one these rules refuse, or none, leaves the request waiting and the value typed on the card. | the same test (no value, a short one, a cut key: refused, still waiting) |
| SEC-003 | Approved, the value is stored with the answering person's browser session as theirs: the new value of their secret of that name in the scope asked, or a new agent secret there; never another scope's or a global one; the outcome names the secret and `created` or `updated`. | the same test (`secret_verify` matches the multi-line key with its line breaks as literal `\n`; asked again: `updated`; the project's of the same name: `created` beside the personal one) |
| SEC-004 | No agent sees the value: not the Superagent's call or its outcome, not the requests file, not the audit; the page clears the field once sent. | the same test; the e2e test (nothing of it on the page) |
| VIS-001 | `task_screen {task_id \| number, lines?}` answers a task's screen as its page's terminal shows it: live while it runs (nothing while its run is starting), else the last screen its last run left (redacted of agent secrets again before it is read), its last 40 lines that hold something (at most 200), with how that run ended: state, exit code, reason, start and end. | `cockpit::the_superagent_sees_a_crashs_last_screen_a_tasks_usage_and_its_checkout`; `visibility_tools::tests::a_screen_is_cut_to_its_last_lines` |
| VIS-002 | `task_usage {number?, from?, to?}` answers the Dashboard's tokens per task of the project for the tasks created in a period (default: the last 7 days), the most first and at most 50, and the period's totals by model. | the same test (a default period set; one task's row only) |
| VIS-003 | `checkout` answers the project's repository checkout's branch and how many commits its head is behind and ahead of its integration branch on the remote as last fetched, and when that remote branch was fetched; a project not cloned, or a remote branch never fetched, is said so (409). It fetches nothing. | the same test (not cloned); `cd_rollback::the_checkout_says_how_far_it_is_from_the_integration_branch` (one behind more after a push and a fetch) |
| VIS-004 | The three are the person's project tools (SAR-001, SAR-002) with the view right, pre-allowed, served to the Superagent and through the remote MCP, to no task's agent. | `cockpit::a_cockpit_manages_the_project_for_its_owner`; `launch::a_cockpit_reaches_the_yulius_tools_with_its_token_only_in_its_environment`; `remote::tests::every_persons_project_tool_is_served_by_both_callers_with_one_action` |
| ABL-001 | `ability_report {ability, case, needed_for?}`, served to the Superagent and to every task's agent (its owner still working in the project), reports a missing ability in one call; refused: an ability empty, over 200 characters or on more than one line, a case empty or over 1500 characters (what the link takes of plain ASCII prose), `needed_for` over 1000, invisible characters, unknown arguments, a text naming the project (its slug, repository or owner as a word, also inside a compound such as `shop-api` or `ShopApi`, and a camelCase name as its words apart (`web app` for `WebApp`); the refusal names it and how it was written, so an agent rewords ordinary text a name catches, such as `real-time` for a project `time` or `GitHub` for `hub`; not for the Yulius repository's own project) or holding one of its agent secrets' values, a report whose filing link would pass 6000 bytes (GitHub's form refuses long ones; the refusal gives the link's size). | `cockpit::a_missing_ability_is_reported_in_one_call` (the Superagent and a task's agent); `ability_reports::tests` (`a_project_name_is_matched_as_a_whole_word`, `a_prose_case_at_its_cap_fits_the_link`, `a_link_past_the_bound_says_its_size`) |
| ABL-002 | Every report is audited `ability.reported` on the instance with the ability, the case and who reported it; the answer carries a link that opens a new issue in the Yulius repository titled `Missing ability: …`, labelled to be triaged, holding the agent's words and an acceptance criterion, for the person to submit: nothing is sent without them. | the same test (the link; nothing sent to GitHub; refused for the project's name and a secret); `ability_reports::tests::an_issue_holds_the_words_and_a_criterion`, `a_query_value_is_percent_encoded` |
| ABL-003 | At most 5 reports an hour per person, the Superagent's and their tasks' together. | the same test (the sixth refused); `ability_reports::tests::reports_are_bounded_per_hour_and_the_window_moves_on` |
| ABL-004 | The instructions send a missing ability to `ability_report`; it sends nothing outside the instance (the person files the link), and Claude Code asks before it as before every tool that is not read-only. | `mcp::tests::the_superagent_is_told_to_do_what_its_person_may_do`; `manage_tools::the_cockpit_asks_before_every_changing_tool` |
| VAR-001 | `variables_set {variables: [{name, value, description?}]}` and `variable_delete {name}` are the person's project tools with the Configure right: the Superagent is served them as the remote MCP already served them (the Global Superagent still withholds them). | `cockpit::the_superagent_sees_a_crashs_last_screen_a_tasks_usage_and_its_checkout` (set, listed, deleted); `remote::tests::every_persons_project_tool_is_served_by_both_callers_with_one_action` |

## Plan

- [x] A chained task waits while Yulius has pull request news for its predecessor (CHN-001 to CHN-003)
- [x] Tools read a start rule back as stored and take "after all current tasks" (CHN-004 to CHN-006)
- [x] Slice 1's documents: product, architecture, API, the decision and the changes log
- [x] Slice 1's independent review answered test-first
- [x] Slice 2: the person's project tools served to the Superagent (restart, pause, resume, start now, a task's delivery, CI, dev deploy), the rule's decision record, the instructions rewritten
- [x] Slice 3: task control that had no function (rename, a prompt not started, queue order, close out, bulk stop, pause and message)
- [x] Slice 4: tasks' questions (open ones listed, why a task needs attention, answered or dismissed for the person, the end of a session)
- [x] Slice 5: merges (a task's merge armed for the person, the sandbox's merge queue, why a pull request is not merging, CI order and hold)
- [x] Slice 6: subscriptions and model (which one a task is on, moved, model and effort changed)
- [x] Slice 7: the instance (version, health, update status, checks, Update now and Restart, sessions reconnect after a restart)
- [x] Slice 8a: requests to the person that last (cross-sandbox requests stored and amendable, a pending decision, another sandbox's task list)
- [x] Slice 8b: production deploy, promotion and onboarding behind one confirmation in the panel with the typed project slug
- [x] Slice 9a: nothing typed to the Superagent is lost in a restart (keys typed while it restarts reach the new session; an update's restart waits for text left in its prompt box)
- [x] Slice 9b: the Superagent's scheduled checks are held by Yulius (set, listed, deleted; kept across restarts; typed into its session when it is idle)
- [x] Slice 10a: a secret's value entered masked on the Superagent's panel, never seen by an agent
- [x] Slice 10b: visibility (a task's cost and load, a crash's exit and last screen, a checkout that is current)
- [x] Slice 11: a missing ability reported in one call, and the ability table in the product documentation
- [x] Slice 11's last review notes: a project name's compounds refused, the refusal names the word, the case's length as the link takes it

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
- AC-SAR-1: through its own token a Superagent sends a stopped task of its person through
  Continue (a running one is refused with "stop it first", a waiting one with
  `task_start_now`), pauses a running task (its pause is stored) and resumes it, starts a
  prepared one now, reads a task's delivery, the CI runs and the CD state, cancels a queued
  run, sends a dev deploy to the deploy, and is refused a production deploy with where a
  person does it.
- AC-SAR-2: the same calls on another person's task are refused with that task's owner named,
  whatever that task is doing, and change nothing; reading its delivery is allowed.
- AC-SAR-3: the changing calls leave exactly one `superagent.tool` audit entry each, in
  order, with the Superagent as actor, what the call was about and whether it went through;
  reads leave none.
- AC-SAR-4: a task's agent calling one of the steering tools gets "unknown tool".
- AC-TCT-1: through its own token a Superagent renames a running task (branch unchanged) and
  replaces the request of a queued one that has a file attached: the stored request is the
  new text followed by that file's mention, and the answer carries the old text. The same on
  a running task or another person's task is refused and changes nothing.
- AC-TCT-2: with three of the person's tasks and one of another person queued in that order,
  `tasks_reorder` of the third and the first makes the queue third, second, first, other;
  an order naming the other person's task, a running task or a task twice changes nothing.
- AC-TCT-3: `tasks_bulk {do: pause, all: true}` pauses both of the person's running tasks and
  not the other person's (counted); by number, a queued task, another person's task and a
  number that is no task come back refused with their reasons while the fourth is paused;
  one message to all reaches the requests of the tasks that have not started; `stop` with
  `all` cancels the waiting ones and leaves the other person's tasks alone.
- AC-TCT-4: with a live session whose person typed text and left it, the owner's message
  with `with_draft` is refused within a minute of their last key, refused while the task is
  someone else's, and then typed: the agent reads their text and the message as one prompt,
  and a plain message is typed after it.
- AC-TCT-5: a live, idle task is closed by `close_task_for_person` and ends `closed`; a
  working, waiting or ended one is refused with what to do.
- AC-QST-1: with a task whose agent asked a decision, one flagged by Yulius, one with a held
  prompt, one working and another person's with a request, `tasks_list {scope: needs_you}`
  through a Superagent's token lists the four that wait, each with its reason in the record
  and in the text; `task_get` of the working one says `working` and names no reason, and
  once it is paused, who paused it.
- AC-QST-2: a task's agent gets the same list, with "holds a typed prompt (new_scope) for its
  person's decision" and without the prompt's words, and "unknown tool" for the two deciding
  tools.
- AC-QST-3: `task_question_dismiss` on the person's task ends its request and answers what
  was asked; on another person's task it is refused with the owner's name and the request
  stays; a second call and a call on a flagged task are refused with what is the case.
- AC-QST-4: `task_held_prompt` with `dismiss` moves the hold to dismissed; `revert`, no
  action and a task without a hold are refused and change nothing.
- AC-QST-5: `session_read {tail: true, limit: 3}` of a five-entry session starts at its third
  entry, says `earlier: true` and `next_from: 5`, and shows a value read from a dotenv file
  before the tail redacted.
- AC-MRG-1: a person's task with an open pull request whose checks have not run: its merge
  state reads not armed and the queue is empty; `merge_for_person` arms it at the branch's
  head, the state and the queue show it with its owner, asking again keeps its place; once
  its run passed, the armed-merge pass merges it and the state says how it ended. A head
  whose run failed is refused with the reason.
- AC-MRG-2: through its token the Superagent reads `merge` in a task's delivery and an empty
  queue, is refused another person's task and a task without a pull request, puts the third
  of three queued runs first (another project's run keeps its place) and is refused a running,
  a first, another project's and an unknown run; a task's agent gets "unknown tool".
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
- Slice 2, integration over the MCP endpoint with a Superagent's own token
  (`tests/cockpit.rs`): every person's project tool once, with the page's refusals, another
  person's task, the audit; with a task's token: "unknown tool". The remote MCP's and the
  Global Superagent's tests run unchanged against the shared code. Unit: the instructions'
  statements, the tools served to each caller, the launch's pre-allowed tools.
- Slice 3, integration over the MCP endpoint with a Superagent's own token
  (`tests/cockpit.rs`, two tests) and with a person's operate token (`tests/remote_mcp.rs`);
  live sessions of the fake agent for the draft and the close (`tests/task_message.rs`); the
  store's two conditional writes (`yulius-store` `tests/store.rs`); unit tests of the title,
  the list of numbers, what `all` takes, the stop order and the kept attachments.
- Slice 4, integration over the MCP endpoint (`tests/cockpit.rs`: a Superagent's token and a
  task's token against seeded requests, a flag and a hold; `tests/sessions.rs`: the tail of
  a transcript on disk); unit tests of the audit's subject and the instructions.
- Unit: the news rule's every branch and its bound (`pr_watch::tests::news_is_due_until_the_agent_heard_it`,
  `typed_news_is_due_until_its_turn`).
- Slice 8a, integration through the functions the MCP calls (`tests/cross_sandbox.rs`): an
  unanswered and a dropped request answered later, amend and withdraw with their refusals, a
  second `App` on the same data directory, another sandbox's tasks, `person_ask` yes and no;
  e2e (`superagent-requests.e2e.ts`): the Yes / No card on the mock.
- Slice 8b, integration with the real Git adapter and ptyhost running the CD scripts
  (`tests/cd_rollback.rs`): production deployed and rolled back through the panel with the slug
  typed, refusals before asking, a promotion's own refusal; over the MCP (`tests/cockpit.rs`):
  the routing of dev and production; onboarding (`tests/cross_sandbox.rs`); e2e: the typed-slug
  card.
- Mutation: twelve mutants of the rule and its uses, each failed by a named test
  (`target/evidence/mutants.py`).
- Load: the three session tests 64 times each beside 96 busy loops (`target/evidence/r3-stress.log`).
- Not exercised by a test: the watch telling a merge to a run reattached after a restart.

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
- **Only a person's pause mutes news** (review rounds 1 to 3). A task a person paused is
  frozen: what the watch types into it waits with it, and that pause may never end. Waiting
  for it would hold a chain and keep a merged task from being archived, so such a task reads
  as before this change (an armed merge excepted). An update's hold ends by itself and the
  watch types right after, so a merge that lands during an update is still news due.
- **News typed while a turn runs is followed through Claude Code's queue** (review rounds 2
  and 3), which Yulius already reads for moves of a session (SPEC-471, SPEC-479), not by a
  second clock: the running turn may take any time, and another entry (a notification, a
  slash command) may get its turn first. No turn's start ends such news; the queue does, or
  two minutes of an idle session. Time alone decides only until the queue showed it.
- **The person's decisions (asked 2026-11-13, answered "ok").** For the later slices:
  production deploy, rollback and promotion to `main` stay behind one confirmation the person
  gives in the Superagent's panel (the typed project slug; stored, amendable, still there when
  they return); each cross-sandbox action keeps its approval, made lasting; Update now and
  Restart run directly for a superadmin's Superagent, with the buttons' refusals; no agent
  ever sees a secret's value, which the person types masked beside the chat.
- **The person's tools keep the remote MCP's names** (`task_restart`, `task_unpause`,
  `ci_runs`): one name for one function, whoever calls it. The descriptions carry the words
  people use ("continue", "resume"). A task's agent keeps other tools under three of these
  names (`ci_rerun`, `cd_status`, `cd_deploy`, about its own work); the Superagent is served
  the person's.
- **The instructions carry the rule, the descriptions carry the tools** (review of slice 2,
  F2). Claude Code 2.1.323 cuts an MCP server's instructions at 4096 characters; the first
  rewrite was 4921 long and lost its last five sentences. The text is 3574 characters now,
  per-tool guidance moved into the descriptions, and a test holds every served text within
  the bound. Raising the client's limit through its environment was rejected: it depends on
  the client's version.
- **Where the repository's rules stand** (review of slice 2, F3). The Superagent runs in the
  repository and loads its AGENTS.md as any agent there does. The instructions therefore do
  not say that those rules are for others: they bind work done in the repository, its own
  included, and do not decide what its person may ask Yulius to do; after the person's
  confirmation it acts with its Yulius tools, whose refusals stay. And what it reads is data.
- **`task_delivery` says what the record has** (F4). Whether a merge is armed while the
  checks still run, and why a green pull request nobody armed is not merging, are not in it:
  slice 5 adds them, and the texts no longer promise them.
- **`cd_deploy` for the Superagent is the dev-stage deploy only**, as for the remote MCP, and
  says where a person deploys production. The one confirmation in the panel for production
  comes with slice 8; until then the refusal names the CD page.
- **Project variables: the Superagent reads them** (`variables_list`); setting and deleting
  them comes with the secrets slice, where the vault's rules are decided together.
- **Slice 3: functions first, tools second.** None of these had a page. Each is written as
  a page's function would be (`task_edit.rs`: `AuthContext`, write access at the call, the
  owner only) and the tool calls it, so the rule of ADR-0330 holds and a page can call the
  same function later. No web UI comes with this slice: the wall is the Superagent's.
- **A request is replaced whole, by one conditional write** (TCT-002). An edit by ranges or
  by a patch was rejected: the Superagent would have to hold the exact old text, and
  `task_get` shows 4000 characters of it. The write compares the old text instead, so a
  message queued meanwhile makes it fail rather than vanish, and the answer returns the old
  text whole. The files attached to a request are the person's: they stay unless the new
  text places them itself (the workaround, delete and recreate, once lost four
  screenshots). The task keeps the mode and workflow chosen for it at creation.
- **The order is a swap of slots, not a new clock** (TCT-003). The queue is one order across
  all sandboxes (`COALESCE(queued_at, created_at)`, then the order of creation, which the
  query now says). Writing new times would move the person's tasks past other people's;
  swapping the slots they already hold moves nobody else. Two tasks can share a millisecond
  (each is queued at its own clock reading, and several are queued in quick succession), and
  a swap of equal places is no swap (review round 1): so the write walks the queue from the
  first named slot and puts each place after the one before it. Only where places are
  shared are other tasks' places touched, by milliseconds, never in their order, and such a
  task does not read as updated (review round 2). A column of its own for the order was rejected: one more thing to keep in step with
  `queued_at` for a case the places already carry.
- **Closing out is for an idle session** (TCT-004). "Set it to done" is not a state a person
  sets: Done is computed (slice 1). What they mean is that the session should end and the
  task leave the active list: `closed`, the state Yulius gives a finished task. A working
  task is not closed under its agent's hands: Stop is the tool that interrupts. A task
  Yulius still has pull request news for is not closed either (review round 1): closing it
  would start the tasks chained after it before the merge slice 1 makes them wait for.
  Background shells do not hold a close back: a leftover dev server is the usual case, and
  the description says they end.
- **One bulk tool with an action, not four** (TCT-005): one description to read, and the
  instructions name it for "all". Each task goes through the single function, so a bulk
  call can do nothing a single call could not, and every task keeps its own audit entry.
  `all` never reaches another person's task, a superadmin's Superagent included: the
  single functions are the owner's.
- **A draft is sent past, not discarded** (TCT-006, wall 2 "a queue blocked by unsent
  terminal text"). Yulius sees keys, not the prompt box, so it cannot show the text or know
  the box is empty, and the keys that clear a box in Claude Code (Esc, Ctrl+C) also interrupt
  a turn or end the session. So the owner decides the one thing that is safe: their text goes
  out with the message as one prompt, which empties the box and lets Yulius's own messages
  through again. Discarding stays with the person at the terminal, and the table says why.
- **Slice 4: the reasons are the page's, read where the list is read** (QST-001). A task's
  brief now carries what `TaskSummary` shows a person, by the same functions, so the two
  cannot say different things. A tool of its own for questions was rejected: the Superagent
  already reads `tasks_list`, and one more list to remember is one more "I can't see that".
  The group is the list's rule (`AttentionFacts::group`), not a second one.
- **A task's agent reads other tasks' questions, not held prompts' words** (QST-001). An
  agent that sees "#12 asked its person for a DigitalOcean budget" does not ask again. A
  held prompt is different: a person typed it, Yulius kept it from the agent it was typed
  to, and it is in no transcript; its words go to the person's own callers only.
- **The texts are data.** A request's and a flag's words are an agent's or Claude Code's
  own; they are stored as bounded single lines, the tools' descriptions say they are data,
  and the instructions' rule on text that is read covers them.
- **Dismissing ends the request, nothing else** (QST-005): no word to the agent and no
  change of its activity. The answer is `task_message`'s job, which ends the request too. A
  flag has no dismissal: each flag says what ends it (a prompt, a secret answered, a
  session stopped), and clearing it without that would hide a task that is stuck.
- **A held prompt is decided by the card's function**, with four of its six actions. `revert`
  discards the task's work and takes the person's explicit confirmation on the page;
  `skip_after` names steps to skip. Both stay there, and the table says so.
- **A question in a task's terminal is left for slice 10.** A permission menu or a choice
  Claude Code shows is answered with keys. Yulius knows that it is asked (the flag's reason
  carries Claude Code's words) but cannot yet show an agent the menu; typing keys blind
  would take a default nobody chose. Slice 10 adds a task's screen; the decision is taken
  with it.
- **The tail keeps the end** (QST-003). `render_text` cuts at the end when a bound is
  reached, which for a tail would drop what was asked for: entries are left out at the
  front until the text fits.
- **The instructions shrank to take the new tools** (3963 of 4096 characters): the proxy's
  mechanics and two examples moved out (the tools' descriptions have them). Tool names
  whose absence caused "wrong I can't" (`secret_request_hosts` among them) stay named.
- **Slice 5: the person's merge is the page's Merge, and the armed merge for a wait** (MRG-003).
  On green it is `merge_task_pr`, the button's function. While the checks run it arms the
  worktree's head for the person through the same armed-merge machinery an agent's
  `delivery_merge` uses, so the queue, the trains, a restart and a dropped arm behave the
  same. No independent review is asked for: that is an agent's discipline before it delivers
  its own work; the person decides about what is theirs, as the page lets them.
- **A person's arm says it is theirs** (review of slice 5, F2). The armed slot carries who
  armed it, saved with it across a restart, so the merge the pass makes is the person's in the
  audit and the outcome names them. The pass's sentences stay as they are: they are typed to
  the task's agent, whose next step they name; the person's Superagent reads them with
  `armed_by` beside them. A person arming a newer head than an agent's armed one replaces it
  (their decision, at their head); asking for the same head keeps the agent's arm and place.
- **Why a pull request is not merging is the armed slot's own words** (MRG-001): the train's
  sentence or the last decision's note, which `yulius wait` already shows the agent. No new
  reasoning about checks or GitHub: one source for the agent, the page and the Superagent.
- **CI order moves one run; holding the queue is left out** (MRG-004). Each project has lanes
  of its own and starts its queued runs oldest first, so a place a millisecond before the
  project's earliest queued run is "first" without touching another run or project. A hold
  would stop every task's checks and merges; no case in the read conversations needed more
  than cancel and run again, and no page holds it either. The table says so.
- **Slice 6: a subscription is named as people name it** (SUB-003). The person says "Team
  Max", not an id, and only administrators see ids; the label is matched in any case, and a
  label two subscriptions share is refused rather than guessed.
- **A model change waits for the session's next start** (SUB-004). Typing `/model` into a
  running session would change it mid-turn behind its agent and is not what Yulius types;
  the launch already takes the task's model. The answer says when it applies and that
  `task_switch` starts the session again now. No page sets a task's model after its creation:
  this is a person's function written as a page's would be (ADR-0330).
- **What a task runs on is in every brief, and the Global Superagent may move a task**
  (review of slice 6, F3, F4). A subscription's label is what the Subscriptions panel shows
  everyone signed in, so a task's agent reading the list learns nothing a person of the
  project could not; ids and emails stay an administrator's. `task_switch` and `task_model`
  change only the person's own task and merge no code, so the Global Superagent keeps them,
  unlike `task_merge` and `change_merge`.
- **Excluding a subscription from the pool is not a person's ability** because the pool has
  no such setting; the table says so rather than adding one here.
- **Slice 7: the ask is the confirmation, for a superadmin only** (INS-002). The person
  decided on 2026-11-13 that Update now and Restart run directly for a superadmin's
  Superagent; the buttons' functions keep every refusal (a move or an update under way, no
  helper). The Global Superagent is not served `instance_update`: it reads every sandbox, and
  a restart pauses everyone's work.
- **Logs and the doctor stay the host's.** No page shows them and the doctor runs in the CLI,
  outside the server; `instance` shows the health a page shows. Sessions coming back after a
  restart is the restart's own (tasks are frozen and resume, SPEC-432); the Superagent's own
  session is slice 9's.
- **Slice 8 is split in two** (8a, 8b): lasting requests and the panel's confirmation of
  production are each one concern and about one pull request's size.
- **Slice 8a: a request lasts until answered** (XRQ-001, ADR-0333). The wait stays 4 minutes
  (Node's 300 s headers timeout bounds a call); after it the request waits on the panel, not
  in the call. Kept in a JSON file beside the database as armed merges are, not in a table:
  runtime state, at most 3 per person and source sandbox and 200 outcomes. No expiry: a request
  nobody wants is declined in one click or withdrawn.
- **Only a create, a message or a question is amended** (XRQ-003): an update's start and a
  stop were resolved against the task as it was when asked (`starts_now`, the card's words);
  withdrawing and asking again resolves them anew.
- **`person_ask` asks in Claude Code too** (XRQ-007), as `sandbox_task_create` does: each tool
  that records something for the person stays out of the pre-allowed list, which holds only
  tools that read; on a contained server the bypass skips that prompt anyway.
- **Slice 11: a missing ability goes to the Yulius repository through its person** (ABL-001 to
  ABL-004, ADR-0337): kept on the instance, and a link the person submits; no issue is posted
  without them. The project variables, still a page's alone after slice 10, joined the person's
  tools here (VAR-001): the remote MCP already had them, so it is one implementation.
- **Slice 10b: what the page shows, read; nothing measured anew** (VIS-001 to VIS-004). A
  task's screen, its tokens and the checkout's distance are what the page, the Dashboard and Git
  already know. A task's load is left open: Yulius measures the host and the sessions per
  sandbox, not each task's processes, and `instance` shows the host's; the evidence's case
  (a task of its own blamed on other projects) is now answered by the host's load beside its
  sessions and the task's tokens. Cost is answered as tokens: no price per task is kept, and a
  price per model would be a figure of its own. The checkout fetches nothing: a fetch would change
  the person's working copy's view under them; the answer says when it was last fetched.
- **Slice 10a: a secret's value is typed on the panel** (SEC-001 to SEC-004, ADR-0336), the
  person's decision. The rest of wall 10 stays as decided before: binding a secret to hosts and
  copying it into `.env` keep a person's allowance (`secret_request_hosts`, `secret_copy_to_env`
  ask once each: the guard they already are), and `secret_verify` keeps refusing production
  secrets (ADR-0144). The "invisible sandbox-scoped secret" of the evidence was not reproduced
  here; it is left to its own report if it comes again.
- **Slice 9b: standing jobs are checks Yulius holds** (CHK-001 to CHK-004, ADR-0335): the
  conversation survives a restart, its schedulers do not, so the schedule moves out of the
  session. A due check may start a Superagent: that is what "keep monitoring" means to the
  person, and 10 checks of at least 5 minutes bound it. The instructions lost two parentheticals
  (`session_read`'s tail and a browser tab's link, which their tools' descriptions say) to make
  room for the rule within 4096 characters.
- **Slice 9a: an update still restarts the Superagent; nothing typed is lost to it** (SES-001,
  SES-002). Claude Code 2.1.325 handles `notifications/tools/list_changed`, but the Superagent's
  MCP instructions are read only when it connects, and Yulius's MCP endpoint answers JSON with no
  stream for the server's notifications (`GET /mcp` is 405, `listChanged: false`): a stream held
  open through each sandbox's forward for every Superagent is a change of its own, for a restart
  that already resumes the conversation. What a restart lost was the person's text and the
  Superagent's own schedulers; the text is kept here, the schedulers move to Yulius in slice 9b.
- **A draft holds a restart for at most half an hour** after its last keystroke: one left for
  longer is taken as abandoned, so an update reaches every Superagent within the hour.
- **Slice 8b: the typed slug on the panel is the CD page's confirmation** (REL-001,
  ADR-0334). The person decided it on 2026-11-13; ADR-0110 holds (a person decides production and
  types the slug). The slug is checked by the server before the request is taken, so a slip
  costs nothing. The release runs with the answering person's browser session, not with the
  Superagent's, so the functions' checks of administration, a GitHub identity and the drill are
  those of the CD page.
- **Onboarding takes a click, not the slug** (REL-005): it releases nothing to production; a
  person's own agent already starts it with an operate token (ADR-0188).
- **A release request is not amended**: the card the person confirms never changes under them;
  one that changes is withdrawn and asked again.
- **A request refused before anyone is asked writes no audit row**, as every cross-sandbox
  request refused before asking; the CD page's own audit records each run.
- **The rule is stored in its canonical words** (`#12 finished && #13` reads back as
  `#12 done and #13 done`): one form for the editor, the tools and the evaluation.

## Evidence

### Slice 1 (2026-11-13 and 14)

- **The 13 November case, reproduced before the fix.** `merge_train::a_task_chained_after_an_armed_green_pull_request_waits_for_its_merge`
  and the first version of the CHN-002 test both failed with the chained task `Queued`
  (expected `Chained`); `manage_tools::a_start_rule_reads_back_whole` failed with
  `"runs_after":1` and no rule for a rule naming three tasks.
- **Tests first, each seen to fail without its change:** the armed, green pull request; the
  merge not told; the news typed and its prompt hook held at its check; a server started again
  before the armed merges are back; an armed merge dropped on a green head; news typed while a
  turn runs; a queue entry that gets its turn before the news; a merge during an update's
  hold; a merge GitHub reports first; a supertask and Home's counts; the rule read back;
  `after_all_tasks` (what it names and leaves out, nothing left, the 64 bound, access first).
- **Mutation:** `python3 target/evidence/mutants.py`: twelve mutants of the news rule and its
  uses, each failed by a named test, `survived=0`.
- **Load:** the three session tests of `tests/task_done.rs` 64 times each beside 96 busy loops
  on 64 CPUs: 64 of 64 iterations passed (two earlier versions of the fix failed 1 of 48 and
  1 of 96 there, which is how the watcher reading a turn's prompt late was found).
- **Gate:** `scripts/dev ci` (repository check, cargo check, fmt, clippy, the macOS compile,
  the workspace's tests, doctests, deny, shear, cargo doc) passed on each head given to the
  review; on the head before the last round's delta: 4296 tests passed, 17 skipped.
- **End-to-end, web:** `npx e2e run tests/intake.e2e.ts`: 18 passed (the rule editor's note).
- **Security scan:** Vidoc is switched off for this project in Yulius: `yulius security-scan`
  reports "disabled", which is not a pass.
- **Independent review** (a fresh session, read-only, its own build and gate run): three
  rounds of findings, all in this slice's own code and all answered test-first. Round 1: the
  window after the news was typed, the restart before the armed merges are restored, what
  `after_all_tasks` named. Round 2: news typed while a turn runs, an update's hold, two
  readings of the delivery in one decision (its load run failed the CHN-002 test 3 of 96).
  Round 3: an entry that leaves Claude Code's queue alone before the news. Round 4 approved
  that head with one low note and two nits, which the commit after it answers: the two
  minutes of an idle session count only while the task reads idle (a Stop that Yulius's gate
  blocked moves the turn's end and the turn goes on), and two consequences are recorded in
  ADR-0328. Not filed as an
  issue: an armed merge's outcome is kept in memory only (older code); after a restart the
  watch still tells a green head that no merge is armed and why.
- **Not tested:** the watch telling a merge to a run reattached after a restart of the
  server; real Claude Code's queue records (the fake agent's follow the records seen with the
  real tool for SPEC-471 and SPEC-479).

### Slice 2 (2026-11-14)

- **Slice 1 is merged:** pull request #1270 into `dev` at `28b0f9687`, whose Yulius CI run
  passed (squash `a89c3c60a`). The note left by its review (D-1: the idle time that bounds
  queued news) is the first commit of this slice, with `pr_watch::tests::a_session_is_idle_since_its_last_turns_end_only`.
- **Tests first:** `cockpit::the_superagent_does_what_its_person_may_do_with_tasks_ci_and_cd`
  failed with "unknown tool: task_restart" and `cockpit::a_cockpit_manages_the_project_for_its_owner`
  on the list of served tools, against the code before the change (`target/evidence/s2-red.log`);
  both pass with it.
- **The shared code keeps its other callers:** the remote MCP's, the Global Superagent's, the
  cross-sandbox, browser, management, CD and message test files pass (206 tests), and so do
  the unit tests of `yulius-app` and `yulius-harness` (719).
- **Independent review, round 1** (a fresh session): request changes. Its findings, all in
  this slice's code, are answered: the gate's `cargo doc` (a link to a private module); the
  instructions cut by the client at 4096 characters (found with Claude Code 2.1.323 against
  a stub MCP server: "truncated from 4921 to 4096 chars"), now 3574 with a test of the bound
  for every served text; the wording on the repository's rules and on text that is data;
  what `task_delivery` says; tests that show the abilities (a stored pause and its end, a
  queued run cancelled, a dev deploy sent to the deploy, a named production-stage
  environment refused for a person's agent) and the exact audit entries; `task_restart`'s
  advice by the task's state; the audit written when the client went away, with what the
  call was about; the two tool lists tied by a test; the ability table's rows for variables
  and onboarding. Pre-existing and left: a Superagent's bearer has no request bound as a
  remote token has (120 a minute).
- **Round 2:** request changes, for what round 1's answers added: the audit entry took the
  call's raw arguments (a megabyte of text of the caller's choosing per call); it now names
  only what has the shape of the tool's task, run or environment. Also answered: a test that
  drops the request once the pause is stored and waits for the audit entry (it fails with
  "the dropped call left no audit entry" when the call is not detached,
  `target/evidence/s2-mutant-detached.log`); a queued task is told that it starts by itself
  (`task_start_now` takes a chained or prepared task only); Continue's advice is decided on
  the task's state, and the onboarding refusal by one constant. The reviewer ran the real
  client with the new instructions: no truncation.
- **Round 3:** approved, with two minor notes and a nit, which the commit after it answers:
  whose task it is comes before any advice about its state (another person's running or
  prepared task is refused with its owner's name, tested); the audit's ids are trimmed as
  the call reads them, and the test compares the exact record; the idle rule of queued news
  has no parameter that did nothing.
- **Gate:** `scripts/dev ci` on the head given to the review; its result is in the review's
  summary and Yulius CI's run on the pull request.
- **Security scan:** Vidoc is switched off for this project in Yulius ("disabled", not a pass).
- **Not tested:** a real Claude Code session calling the new tools (the MCP endpoint is called
  with a Superagent's own token; the client's cut of the instructions was seen with the real
  client by the reviewer); the restart of an open Superagent that the changed tool list
  causes at the update (the launch digest's existing rule); `task_restart` on the onboarding
  task of a project that has not started onboarding, through the endpoint (its answer is
  unit-tested); queued news behind a session that went on from a clean Stop without a prompt
  (the goal's verdict "not met") and that an error then ended: the idle time still counts
  from that earlier Stop, so for the seconds until Claude Code hands the entry on, the
  two-minute bound may read as over.

### Slice 3 (2026-11-14)

- **Slice 2 is merged:** pull request #1273 into `dev` at `01793e80c`, whose Yulius CI run
  passed (squash `dbc33d88f`). The two comment notes of its last review are this slice's first
  commit.
- **Tests and code:** the integration tests of the four tools, of `with_draft` and of the
  close were written before the application code they call, and first run after it; the
  store's two writes and their tests were written together. That each test fails without
  the behaviour it names is shown by mutation instead: fifteen one-line mutants (the
  compare of the old request, the unstarted rule, the queue's swap and its stored start,
  the kept attachments, the owner check of an order, a close in the first turn, what `all`
  takes, the stop order, the owner and the minute of `with_draft`, the audit's tasks), each
  failed by a named test (`target/evidence/mutants-slice3.py`; eleven more after the review).
- **Found by the tests while writing:** a draft typed seconds ago was typed over by
  `with_draft` (the rule reads a fresh draft as a draft, not as "at the keyboard"): the
  minute now holds for the owner too. A bulk stop started queued tasks only to stop them (a
  cancelled task's slot went to the next one): waiting tasks are stopped first, and the
  answer shows the order. A running task with no activity yet (its first turn) counted as
  idle for a close: it is working, as the lists show it.
- **The shared code keeps its other callers:** the Superagent, management, remote MCP,
  Global Superagent, message and queued-message test files pass (159 tests), and the unit
  tests of `yulius-app`, `yulius-domain`, `yulius-store` and `yulius-harness` (1341).
- **Independent review, round 1** (a fresh session, head `8cdf484f9`): request changes. All
  its findings are in this slice's code and are answered with tests: five of the seven integration
  tests written for them fail on that head (`target/evidence/s3-r1-red.log`; the other two
  cover the provider-session case and the 100 bound, which held already and had no test)
  and all pass now (`s3-r1-green.log`); the close rule's unit test came with the rule:
  - F1, medium: after a restart of the server `with_draft` sent the message and answered
    "the box is empty again", but the draft, known from its mark alone, stayed: every later
    message was refused and the task was flagged after ten minutes. The submit Yulius makes
    for the owner now counts as the prompt that follows the draft (the reviewer's probe is
    the regression test).
  - F10 (rated a nit, found to matter): two queued tasks that share a millisecond were not
    swapped. The order is now written along the queue, and the queue's query names its
    second key.
  - F2, F3: a close is decided by one rule over the task's state, a turn in progress, pull
    request news still to come and whether it has started (a held task with a live session
    was told it "has not started"; a flag standing mid-turn read as idle; a task with an
    armed merge could be closed and start its chain early).
  - F4: a bulk unpause listed tasks nobody paused as done. F6: `with_draft` could carry a
    prompt Yulius held for a decision to the agent. F7: through the remote MCP a bulk call
    stopped when its client went away. F9: the text and the label were glued.
  - F5: the mutants it found surviving (ties, a provider session, the 100 bound) and the
    audit's new arm have tests; "a bulk call of more than 100 tasks" is tested now.
  - F8: `docs/operations.md` and SPEC-480 DRF-007 say who may send past a draft.
  Twenty-six mutants now, each failed by a named test (`mutants-slice3.log`: `survived=0`).
- **Gate:** `scripts/dev ci` passed on `8b1d5a019` (4346 tests, 17 skipped; clippy, docs,
  repository contracts; `target/evidence/gate-slice3-r1.log`); the commit after it changes
  this specification only. (The first run of that gate failed at clippy on an import a test
  did not need; removed.)
- **Security scan:** Vidoc is switched off for this project in Yulius ("disabled", not a pass).
- **Not tested:** a real Claude Code session calling the new tools, and its prompt box with
  a real draft behind a pasted message (the fake agent echoes what it is sent as one prompt;
  that Claude Code submits a draft and a paste behind it as one prompt is SPEC-480's own
  finding, issue #1199); two orders of the same queue at the same moment (one transaction
  each; the second reads the places the first wrote); a close-out refused for a turn in
  progress or for pull request news, through a live session (the rule is unit-tested with
  every case; the facts are read from the functions the task list uses); a prompt whose hook
  is still being answered when the close is decided (up to the three seconds its scope check may take,
  or as long as an update or a pause holds the prompt: the close is a graceful shutdown, and
  the task can be continued).
- **Round 2** (head `9e8118bd9`): approved; the reviewer's own gate run passed. Its notes
  are answered in the commit after the merge of pull request #1275 (into `dev` at
  `ae5370970`, whose Yulius CI run passed): a close-out of a task with an armed, green pull
  request is refused and its chained task keeps waiting, shown through the real functions
  (`merge_train::a_task_chained_after_an_armed_green_pull_request_waits_for_its_merge`); an
  order writes nothing before its first named slot and leaves `updated_at` of tasks it did
  not name alone (`store::an_order_leaves_the_tasks_it_does_not_name_as_they_were`); the
  limit of the mark that ends a draft sent past is said where it is kept; the wording of
  this section and of the news refusal. Left as it is: the `turn` fact of a close through a
  live session.

### Slice 4 (2026-11-14)

- **Slice 3 is merged:** pull request #1275 into `dev` at `ae5370970`, whose Yulius CI run
  passed (squash `9ad06fdb2`). The notes of its approving review are this slice's first
  commit (see round 2 above).
- **Tests first:** the Superagent test, the session test's tail and the list of served tools
  failed against the code before the change (`target/evidence/s4-red.log`: every task listed
  for scope `needs_you`; no tail; two tools not served) and pass with it (`s4-green.log`).
- **Found on the way:** the instructions grew to 4193 characters with the new sentence, past
  the 4096 the client reads; the bound's test caught it, and they are 3963 now. A held
  prompt's words would have reached every task's agent through the list: they go to the
  person's own callers only, tested with a task's token.
- **Mutation:** eleven one-line mutants (the scope's filter, the group, a held prompt's
  words for agents, the owner of a dismissal, the four actions, the tail's three rules, the
  two lines of text, the audit's decision), each failed by a named test
  (`target/evidence/mutants-slice4.py`; four more after the review). One of them
  survived at first (the tail's cut at the front had no test): a long session now shows it.
- **The shared code keeps its other callers:** every test of `yulius`, `yulius-app` and
  `yulius-harness` passes (2957), the remote MCP's and the Global Superagent's among them.
- **Independent review, round 1** (a fresh session, head `5af19aa93`; the reviewer was cut off
  by its usage limit after confirming two findings by probe, its notes kept): both confirmed
  findings and its candidate notes are answered, with tests that fail on that head
  (`target/evidence/s4-r1-red.log`) and pass now (`s4-r1-green.log`):
  - C1: the tail was bounded by what the transcript holds, so twenty 12 000-character tool
    results left four entries of a 24 000-character read; it is bounded by the text shown
    now (a long session's end shows its last eight steps).
  - C2, C3: a held prompt's raw text could forge a line of the task list (a line reading
    like another task's, a colour escape, a character that reverses text); a request's words
    kept characters that hide text. Each is one clean line now, also for rows stored before,
    and a stored request drops such characters.
  - C4: a task's agent's `task_get` and its own `you` brief are tested to carry no held
    prompt's words. C9: an ended task still shows a prompt held before its end, as its card.
  - C5, C6, C7: the time zone advice moved into `automation_create`'s description; a
    dismissal ends any request of the task (its agent's or one Yulius opened), said in the
    tool, the API and ADR-0111. C8 (a dead branch) went with C1's rewrite.
  - Merged on the way: `dev` at `435111b4d` (one typist at a time), cleanly; the suites that
    type into sessions pass.
  Fifteen mutants now, each failed by a named test (`mutants-slice4.log`: `survived=0`); one
  survived at first because the test's forged prompt itself held the mark it looked for.
- **Round 2** (a fresh session on another model, head `b43ebdb3f`; its own gate run passed):
  request changes, one medium: the answer to C9 showed an ended task's request left from
  before its end, and let it be dismissed, while its page shows none (an ended task waits
  for nobody, ADR-0111). Answered with tests that fail on that head
  (`target/evidence/s4-r2-red.log`) and pass now (`s4-r2-green.log`): an ended task shows
  no request and none is dismissed; a session's tail keeps the last entry that shows text
  past a system note at its end (R2-2); one text test covers every reason a task gives
  (R2-3); a request's and Claude Code's words are quoted as data, the dismissed request is
  answered as one clean line, and the list reads each task's group once (R2-4 to R2-6).
  Nineteen mutants now (`mutants-slice4.log`: `survived=0`).
- **Gate:** `scripts/dev ci` passed on `6651c0f91` (4368 tests, 17 skipped; clippy, docs,
  repository contracts; `target/evidence/gate-slice4-r2.log`); the commit after it changes
  this specification only.
- **Security scan:** Vidoc is switched off for this project in Yulius ("disabled", not a pass).
- **Not tested:** a real Claude Code session reading the new fields and calling the two
  tools; `task_held_prompt` with `send`, `parallel` and `after` through the tool (the card's
  function is called with the action as given, and its own tests cover each; `dismiss` and
  the refusals are run here); a request that changes between its read and its dismissal
  (the store's conditional write has its own test,
  `needs_you::tests::a_request_ends_by_its_action_only_while_it_is_the_open_one`).

### Slice 5 (2026-11-14)

- **Slice 4 is merged:** pull request #1280 into `dev` at `7fd991af7`, by its merge train
  (squash `a431b2f77`), after the first CI run failed in `tests/cd.e2e.ts` "a needs_you decision
  reads Decide" (a file the change does not touch; the known flake of open issue #1175, which
  has this occurrence now) and its rerun passed. The nit of its approving review (a tail's
  `limit` counted entries that show nothing) is this slice's first commit, with its test.
- **Tests and code:** the merge-train test (`a_person_merges_their_tasks_pull_request_when_its_checks_pass`,
  on real repositories with the production Git adapter and the armed-merge pass) and the
  Superagent test (`the_superagent_reads_the_merge_queue_and_orders_ci`) were written before
  the code, which they call by name, so they could not build without it. That each fails
  without the behaviour it names is shown by ten one-line mutants (the merge field, the slot
  read, the queue's filter, the owner check, a merge on red, an arm kept in its place,
  another project's run, a running run, the project's earliest place, the audit's run),
  each failed by a named test (`target/evidence/mutants-slice5.py`, `mutants-slice5.log`:
  `survived=0`); two survived at first: one was an equivalent clause, now gone from the
  query, the other showed the test did not pin the run's new place, now it does.
- **The shared code keeps its other callers:** every test of `yulius`, `yulius-app`,
  `yulius-harness` and `yulius-store` passes; the harness's list of pre-allowed tools now
  names `merge_queue`.
- **Gate:** `scripts/dev ci` passed on `445f5af23` (4377 tests, 17 skipped; clippy, docs,
  repository contracts; `target/evidence/gate-slice5.log`); the commit after it changes this
  specification only.
- **Security scan:** Vidoc is switched off for this project in Yulius ("disabled", not a pass).
- **Not tested:** a merge on green through `task_merge` (the page's `merge_task_pr`, tested by
  its own suite; the merge here goes through the arm and the pass, which merges under the
  owner's authority); a merge train's members in `merge_queue` (`train_summary` reads the
  train file the train tests write; no train formed in these two tests); the dispatcher
  starting the run put first (it starts each project's queued runs oldest first, the order
  tested here).
- **Independent review, round 1** (a fresh session; two reviewers before it were cut off by
  their usage limits before writing a finding, so this one ran on a third model): request
  changes, four medium findings, all answered with tests that fail on that head
  (`target/evidence/s5-r1-red.log`) and pass now (`s5-r1-green.log`):
  - F1: a task whose run had ended was told its merge was armed, and the next pass dropped it.
    Such a task is refused with what to do (merge at once once its checks pass, or continue it).
  - F2: a merge the owner armed was audited as an agent's, "reviewed independently". The arm
    now carries who made it (saved across a restart), the merge is `delivery.person.merge`
    under their name, and `merge.last.armed_by` names them.
  - F3: the Global Superagent was served `task_merge`; it is withheld, as `change_merge` is.
  - F4: a run put first could go before an earlier run of its own commit, and the latest run of
    a commit is read by place; it now stops right after it.
  - F5, F6: a playground's task goes to its Merge; a head the pull request does not have yet is
    refused (read from GitHub, not a local ref, which Yulius's push does not move); the branch
    policy is checked before arming; the merge on green and its cleared arm are tested.
  - F7: one being decided is first in the queue; a task without a runtime is skipped.
  Sixteen mutants now (`mutants-slice5.log`: `survived=0`); the queue's order became a
  tested rule when its mutant survived.
- **Gate, round 1:** `scripts/dev ci` passed on `8c0a45a8b` (4378 tests, 17 skipped;
  `target/evidence/gate-slice5-r1.log`). Its first run there failed one test outside this
  change, `task_done::a_stop_settles_a_run_no_server_holds` ("the run's host still runs"), the
  open flake of issue #1190: it passes alone ten times in a row and failed once in four beside
  48 busy loops; the issue has this occurrence. The run before that stopped at clippy (a
  vector in a test). The commit after it changes this specification only.

### Slice 6 (2026-11-14)

- **Slice 5 is merged:** pull request #1285 into `dev` at `50ed73e90`, after its first merge
  train failed `pool_supervision::a_reattached_run_at_a_limit_is_restarted_past_it` (a timeout
  under load; it fails the same way on plain `dev` beside busy loops: issue #1286, filed). The
  notes of its approving review are this slice's first commit: `ci_prioritize` says when a
  run is not first, and who armed a merge is tested across a save.
- **Tests and code:** the Superagent test (`the_superagent_reads_subscriptions_and_sets_a_tasks_model`)
  was written before the code (it calls tools the server did not serve yet); it was first run
  after it. That it fails without each behaviour is shown by ten one-line mutants (the owner
  of a move and of a model change, nothing asked, a shape not taken, `default`, a shared name
  guessed, a name's case, the note for a running task, the brief's subscription, the pool),
  each failed by a named test (`target/evidence/mutants-slice6.py`, `mutants-slice6.log`:
  `survived=0`); one survived at first (the owner check before a name is read), and the test
  has that case now. Unit tests: the shapes of a model and an
  effort, the audit's subject of the three tools, the instructions (3 more tools named, within
  the bound at 4071 characters after two sentences were shortened).
- **The shared code keeps its other callers:** every test of `yulius`, `yulius-app`,
  `yulius-harness` and `yulius-store` passes (3096).
- **Gate:** `scripts/dev ci` passed on `cb2069c46` (4380 tests, 17 skipped; clippy, docs,
  repository contracts; `target/evidence/gate-slice6.log`); the commit after it changes this
  specification only.
- **Security scan:** Vidoc is switched off for this project in Yulius ("disabled", not a pass).
- **Not tested:** a move through `task_switch` that reaches a session (the page's
  `switch_task`, called with the resolved id, is tested by its own suites with real sessions;
  here its refusals and the name's resolution are); a model taken at a real launch (the launch
  takes the task's model, tested by the launch's suites); `subscriptions` with usage readings
  (the panel's `capacity_view`, tested by its suites).

### Slice 7 (2026-11-14)

- **Slice 6 is merged:** pull request #1288 into `dev` at `6fef988dc`. The notes of its
  approving review are this slice's first commit: a model change is audited as asked (a reset
  as `default`), a task without a live run costs no subscription read, the text line of a
  task's subscription is tested, and the Decisions say why task agents see labels and why the
  Global Superagent keeps `task_switch` and `task_model`.
- **Tests and code:** `the_superagent_reads_the_instance_and_a_superadmins_updates_it` was
  written before the code and first run after it. The superadmin's four actions are run
  through the function directly (the test server has no superadmin's Superagent); their
  refusals are the functions' own on a server without an update helper.
- **Mutation:** six one-line mutants (any action, anyone's update, an unconfirmed restart,
  restart taken for cancel, no status, the Global Superagent restarting), each failed by a
  named test (`target/evidence/mutants-slice7.py`, `mutants-slice7.log`: `survived=0`).
- **Gate:** `scripts/dev ci` passed on `0efe23d26` (4382 tests, 17 skipped;
  `target/evidence/gate-slice7.log`); the commit after it changes this specification only.
- **Security scan:** Vidoc is switched off for this project in Yulius ("disabled", not a pass).
- **Not tested:** an update or a restart that runs (the test server has no update helper; the
  buttons' functions, with real helpers, are tested by the update suites); the Superagent's
  own session across the restart (slice 9).
- **Independent review, round 1** (head `83258419`): request changes, one medium: shortening
  the instructions had turned "web pages" into "pages", the browser being the Superagent's
  main source of text that is data; restored, and a test holds it. Answered too: a task being
  stopped reads its subscription again (`is_live`, not the armed-merge rule); `instance_update`
  is the Superagent's only (no remote MCP: a long-lived token's agent, which the person's
  decision did not cover); the audit entry names the action; the superadmin's path is tested
  through the tool with the flag in the store; the API row names `GET /api/update`.
  Eight mutants now (`mutants-slice7.log`: `survived=0`); the Global Superagent's withholding
  of `instance_update` went, since no remote tool serves it any more (its test still holds).
- **Gate, round 1:** `scripts/dev ci` passed on `1acba6b10` (4382 tests;
  `target/evidence/gate-slice7-r1.log`); the commit after it changes this specification only.

### Slice 8a (2026-11-14)

- **Slice 7 is merged:** pull request #1293 into `dev` at `e269af6ee`.
- **Tests and code:** the six cross-sandbox tests of the old lifetime were rewritten to the new
  one first (an unanswered request waits and is answered later, a dropped call leaves it
  waiting, no expiry is audited) and failed; then amend and withdraw, the restart (a second
  `App` on the same data directory), another sandbox's tasks and `person_ask` were written
  before their code. The panel's `ask` card: the e2e test failed before the card and the mock
  answered it (`superagent-requests.e2e.ts`, 16 of 16, then 17 of 17 with round 1's).
- **Mutation:** ten one-line mutants, each failed by a named test
  (`target/evidence/mutants-slice8a.py`, `mutants-slice8a.log`: `survived=0`).
- **Gate:** `scripts/dev ci` passed on `3e500b0cc` (4409 tests, 17 skipped;
  `target/evidence/gate-slice8a.log`).
- **Security scan:** Vidoc is switched off for this project in Yulius ("disabled", not a pass).
- **Independent review, round 1** (head `3e500b0cc`): request changes, two medium and four minor
  findings, all in this slice's code, each answered test-first:
  - F1 (medium), the Superagent could amend a request after the person read it and before the
    click: an amended request takes the next `revision`, the card is drawn again, and the answer
    names the revision it showed (409 otherwise; `an_amended_request_is_answered_only_as_read`,
    the e2e "an amended request is drawn again and approved as drawn").
  - F2 (medium), a call that answered `waiting` kept its call key, so a later
    `notifications/cancelled` with the same id withdrew the request: the key goes with the call
    (`Waiting::stop_waiting`; `cockpit::a_cancel_after_the_call_ended_withdraws_nothing`).
  - F3, "withdrawn" was described but never kept: withdrawn and cancelled outcomes are kept.
  - F4, a crash between taking an answer and keeping its outcome could show the request again:
    the file is saved when the answer is taken (not covered by a test: it needs a crash at that
    point).
  - F5, blocking writes and unbounded options: answered in words. The file holds at most 3
    waiting requests per person and source sandbox, each checked as a request is (a prompt's
    and a message's bounds, `task_create`'s argument names); it is written whole, a few
    kilobytes, as `armed-merges.json` is, on the rare change of a request.
  - F6, an amendment was not audited and a question was audited twice in its own sandbox: an
    amendment is audited `amended` without its text, a question once.
  - F7, the bound: `person_ask` shares the bound of 3 on purpose (each is a card for the person
    to read); the Superagent withdraws one. The note on the restore's order hid a defect:
    restoring ran after the listeners were bound, so a request asked before it wrote the file
    without the saved ones, which the restore then read: they were lost. Now nothing is written
    from the start until the saved requests are back, the restore merges them (outcomes too) and
    writes them all, and it runs first in the background start
    (`a_request_asked_before_the_restore_loses_none`). Until then the bound of 3 can be passed
    by the requests asked meanwhile: they all wait, nothing is refused or lost.
  Seventeen mutants now (`mutants-slice8a-r1.log`: `survived=0`).
- **Gate, round 1:** `scripts/dev ci` passed on `09a08e1bd` (4412 tests, 17 skipped;
  `target/evidence/gate-slice8a-r1.log`).
- **Independent review, round 2** (head `09a08e1bd`): approve; four nits: N1, a request taken
  and then lost to a crash leaves no outcome (answered in words: the save right after the take
  is the guard, an "interrupted" outcome would need a second write per answer); N2, an
  unreadable file was overwritten by the save after the restore (fixed in slice 8b, test-first:
  it is kept as `.bad`, `a_saved_requests_file_that_cannot_be_read_is_kept`); N3, a crash while
  the restore runs loses the requests asked in that moment (answered in words: the restore is
  the first step of the start, before any task runs); N4 as F5.
- **Merge train:** the merge candidate failed twice on `cd.e2e.ts` "a needs_you decision reads
  Decide" (#1175), the sidebar still "Loading tasks…" at a one-shot read; the test polls now
  (`b9533c662`), approved in round 3 (head `b9533c662`). Merged into `dev` by pull request #1298
  at `0ed362b97`, whose own Yulius CI run passed.

### Slice 8b (2026-11-14)

- **Tests and code:** `the_superagent_releases_production_once_its_person_types_the_slug`
  (`tests/cd_rollback.rs`: the real Git adapter and ptyhost run the CD scripts) was written
  before the code: production deployed through the panel (v2), a slug left out or mistyped
  refused with the request kept, then rolled back from v3 to v2 with `main` unmoved, and a
  promotion whose own answer ("nothing to promote") reaches both the click and the call; the
  refusals before asking (a writer, a dev-stage environment, an unknown one or argument,
  onboarding that does not wait). Over the MCP (`tests/cockpit.rs`) a dev rollback goes to the
  CD page's at once and production, `promote` and `onboarding_start` are refused before asking
  for a writer. `onboarding_starts_on_the_persons_click` (`tests/cross_sandbox.rs`) shows the
  approval running Start onboarding as the person. The card: the e2e test failed before the
  card and the mock's check (`superagent-requests.e2e.ts`, 18 of 18).
- **Review of slice 8a, N2:** an unreadable `superagent-requests.json` is kept as `.bad`
  (`a_saved_requests_file_that_cannot_be_read_is_kept`, failed first).
- **Mutation:** ten one-line mutants, each failed or timed out in a named test (a refusal
  removed makes the request wait for the person, so the test's call never ends:
  `target/evidence/mutants-slice8b.py`, `mutants-slice8b.log`, `mutants-slice8b-rest.log`:
  `survived=0`).
- **Not tested:** a promotion that promotes (the CD fixture's production deploys promote by
  themselves, so nothing is left; a promotion that runs is the delivery suites'); a Start
  onboarding that starts (this harness has no Git; the onboarding suites start it).
- **Gate:** `scripts/dev ci` passed on `1016737d8` (4414 tests, 17 skipped;
  `target/evidence/gate-slice8b.log`).
- **Security scan:** Vidoc is switched off for this project in Yulius ("disabled", not a pass).
- **Independent review, round 1** (head `1016737d8`): approve, with one medium and minor
  findings, all in this slice's code, answered:
  - M1 (medium), the Global Superagent was served `cd_rollback` (it is not served `cd_deploy`):
    withheld now (`global_superagent::the_global_superagent_works_in_every_sandbox_of_its_person`
    failed first), and REL-003 says so.
  - m1, a production attempt refused before asking writes no audit row: kept, as every request
    refused before asking (the decision above); the API row says so.
  - m2, an unreadable CD overview refused even a dev deploy: it now leaves the decision to the
    tool, whose deploy refuses production without the typed slug anyway.
  - m3, `expect` was taken but no tool schema offered it: dropped from the request, the card and
    the documents.
  - m4, the slug is checked against the project's slug when the request was asked: answered in
    words. A project renamed meanwhile makes the CD page's own check refuse the run with why, and
    the Superagent asks again.
  - m5, a production-stage environment of another name, a declined release, an answer from no
    browser session: tested now (`a_custom_production_stage_environment_waits_for_the_person`,
    and the first test); an administrator who lost the right meanwhile is the CD page's own check
    (`a_waiting_production_deploy_honours_what_was_asked`).
  - n1: a release's amend refusal names releases (tested), the onboarding card's hint, the mock's
    double announcement, the ADR's "amendable", the Plan box.
  Two more mutants (`mutants-slice8b-r1.log`: `survived=0`).

### Slice 9a (2026-11-15)

- **Slice 8b is merged:** pull request #1304 into `dev` at `5de33aa65`, whose own Yulius CI run
  passed.
- **Tests and code:** `an_updates_restart_waits_for_text_left_in_the_prompt_box` (an adopted
  Superagent due a refresh, a draft typed through its terminal's socket during a turn; the turn
  ends and the restart reads `held`; the Enter lets it go on) failed first with the check taken
  out (it timed out waiting for `held`); a first form of it that waited for `due` passed without
  the fix and was replaced. The rule's unit test covers Enter, Ctrl-C, a pasted line break and
  the half hour. The panel: `agent-panel.e2e.ts` "what is typed while the Superagent restarts
  reaches it once it is back" failed before the change (the panel swapped the terminal for its
  skeleton, so the keys went nowhere); the panel, Global Superagent, request and terminal specs
  pass (112 of 112).
- **Mutation:** five one-line mutants, each failed by a named test
  (`target/evidence/mutants-slice9a.py`, `mutants-slice9a.log`: `survived=0`).
- **Not tested:** a real Claude Code's prompt box (the fake agent echoes keys; what empties it is
  read from the keys, Enter and Ctrl-C, not from its screen); a usage-limit move with keys typed
  meanwhile (it is the same `switched` path the e2e test drives).
- **Gate:** `scripts/dev ci` passed on `4ae29eedc` (4430 tests, 17 skipped;
  `target/evidence/gate-slice9a.log`).
- **Security scan:** Vidoc is switched off for this project in Yulius ("disabled", not a pass).
- **Independent review, round 1** (head `4ae29eedc`): request changes, all in this slice's code,
  answered test-first:
  - F1 (major), a draft typed before the server restarts for the update was forgotten (the marks
    lived in memory): the draft's mark is kept in `shell.json` when a draft starts or ends
    (`a_draft_typed_before_the_servers_restart_holds_the_superagents`, failed first).
  - F2, a paste split across frames read a line break as Enter, and two keystrokes in one
    millisecond tied: the paste is followed across frames, and keystrokes are ordered by their
    count, the clock kept only for the half hour.
  - F3, F4, Alt-Enter and backslash-Enter read as sent, and Esc, Tab or arrows as a draft:
    `read_box_keys` reads them as Claude Code does (unit test). Shift-Enter reaches the terminal
    as a plain Enter, so it still reads as sent (the draft is then not held; nothing else changes).
  - F5, a held refresh redid its costly steps every pass: the draft is checked first, in
    `cockpit_refresh_now`.
  - F6, a restart that failed kept the ended terminal and its held keys for a later session:
    dropped (e2e "what is typed for a restart that fails is not sent to a later session", failed
    first). F7: Ctrl-C is left out of held keys, which are kept until sent. F8: held keys are
    whole frames.
  - F9, `draft_holds` duplicated `agent_cli::is_draft`: it uses it. F10, F11: the comments.
  - F12, tests: the paste, Alt-Enter, Esc, a draft across the server's restart and a failed
    restart are tested now; the half hour in an integration test is not (the unit test holds it).
  - Docs: "nothing typed is lost" said more than this does; the documents say what is kept.
  Ten mutants now (`target/evidence/mutants-slice9a-r1.py`, `mutants-slice9a-r1.log`); nine are
  failed by a named test, after Alt-Enter on its own was added; one survives on purpose: taking
  out the early draft check in `cockpit_refresh_now` leaves the same behaviour (the check before
  the end still holds the restart) and only costs the work F5 named.

### Slice 9b (2026-11-15)

- **Slice 9a is merged:** pull request #1308 into `dev` at `9a890224d`, whose own Yulius CI run
  passed. Its last full gate, on `4c5038482`, failed one test outside this change:
  `yulius-git` `credential::tests::localhost_reaches_a_server_on_the_second_address` (from
  #1305), which passes alone (5 of 5) and is filed as #1307 (another test's listener on
  `127.0.0.1` at the same port).
- **Review of slice 9a, round 2 notes**, answered first, test-first: R1, a backslash and the Enter
  after it come as two frames from a browser: the reader keeps the backslash across frames; R2,
  a paste whose end marker never came ends after 5 seconds without keys (`PASTE_STALE_MS`); R3, a
  long paste typed during a restart is kept whole; R4, a growing draft's mark is kept once a
  minute; R5, the comment. The unit test failed first for R1 and R2.
- **Tests and code:** `the_superagents_checks_are_held_by_yulius_and_typed_when_it_is_idle` was
  written first: a check set (and refused out of bounds), listed, kept in the file, typed into
  the idle session when the clock passes its interval, typed again into an idle conversation,
  read back by a second `App` on the same data, a Superagent that ended started for it, and
  deleted. The rule of a quiet prompt box has its unit test.
- **Mutation:** twelve one-line mutants, each failed by a named test, after the test got its
  second delivery (an idle conversation) and one mutant was rewritten to compile
  (`target/evidence/mutants-slice9b.py`, `mutants-slice9b.log`: `survived=0`).
- **Not tested:** a check held back while a turn runs or a draft is there (the unit test holds the
  rule; the watch calls it only when idle); a Superagent that cannot be started for its check (no
  capacity: logged, tried again the next minute); an unreadable checks file (kept as `.bad`, as
  the requests file, whose test covers the same code shape).
- **Gate:** `scripts/dev ci` passed on `fe68dc671` (4436 tests, 17 skipped;
  `target/evidence/gate-slice9b.log`).
- **Security scan:** Vidoc is switched off for this project in Yulius ("disabled", not a pass).
- **Independent review, round 1** (head `fe68dc671`): request changes, all in this slice's code:
  F1, two saves at once could write an older list last: one writer at a time now, the list read
  under it; F2, a check typed into a Superagent that had just started could be swallowed: none in
  its first 20 seconds; F3, a check that could not be typed waited a whole interval: a minute;
  F4, a keystroke between the check of the prompt box and the paste: checked again with the
  terminal held; F5, checks due together went in one a second: 30 seconds apart; F6, a
  Superagent that would not start was tried every minute and gone people's or sandboxes' checks
  stayed: a back-off up to an hour, and their checks are deleted (tested: a disabled person, a
  removed sandbox); F7, a clock set back pushed checks past their interval: clamped; F8, a kept
  check is checked again when read, the file is owner-only, and the `.bad` file is tested; F9,
  control characters in a name are refused; R1's nit (a paste ending in a backslash, then
  Enter) is fixed. F10's other cases stay untested and listed under "Not tested": a check held
  by a draft end to end, the Enter after the paste (the shell runs it: the screen shows the label
  as a command), the Global Superagent (its token is not a Cockpit's), two due together, saves at
  once, the panel's long paste.
  Mutants of the fixes: thirteen, each failed by a named test once the rules were drawn out into
  `ready_for_check`, `backing_off`, `backoff_wait` and `sound` with their unit test
  (`checks_wait_their_turn_and_are_checked_when_read`) and the integration test read back a
  hand-edited check out of bounds (`target/evidence/mutants-slice9b-r1.log`: `survived=0`).

### Slice 10a (2026-11-15)

- **Slice 9b is merged:** pull request #1310 into `dev` at `4aa43cdca`, whose own Yulius CI run
  passed.
- **Review of slice 9b, round 2 notes**, answered first: G1, the 20-second grace is called a
  heuristic in ADR-0335; G2, a check pasted but not sent is not pasted again; G3, a retry leaves
  a check replaced meanwhile alone, a gone Superagent's back-off goes with its checks, checks out
  of bounds are logged when dropped; G4, G5 answered in words (a few milliseconds of a file write
  under the terminal held; after a clock set forward, the 30-second gap spaces the checks).
- **Tests and code:** `a_secret_value_typed_on_the_panel_reaches_the_vault_only` was written
  first: refusals before asking, the card (its title is the secret's name), no value, a short
  one and a cut key refused with the request kept, a multi-line key stored (`secret_verify`
  matches it with its line breaks as literal `\n`), nothing of it in the call, its outcome, the
  requests file or the audit, and asked again: `updated`. The panel: the e2e test failed before
  the card (19 of 19 now). The answer's fields became one `RequestAnswer` (clippy's bound on
  arguments).
- **Mutation:** seven mutants (`target/evidence/mutants-slice10a.py`, `mutants-slice10a.log`):
  five failed by a named test; `empty-value-taken` is equivalent (the vault's own check refuses
  an empty value too); `check-repasted` (slice 9b's G2) survives: it needs a terminal whose
  Enter fails after its paste went in, which no test fixture gives.
- **Not tested:** a Superagent's call that waited past its 4 minutes and the value stored later
  (the same lasting request as XRQ-001, tested there); a person without the right to manage the
  scope (the vault's own refusal, answered to both the click and the call).
- **Gate:** `scripts/dev ci` passed on `db1442752` (4438 tests, 17 skipped;
  `target/evidence/gate-slice10a.log`).
- **Security scan:** Vidoc is switched off for this project in Yulius ("disabled", not a pass).
- **Independent review, round 1** (head `db1442752`): request changes; no leak found; all in this
  slice's code:
  - M1 (major), a value replaced the person's secret of that name in any scope (a personal one
    for a project request, a global one for an administrator): only their secret of that name
    in the scope asked is replaced (`secret_replaced`); the test creates the project's
    `DEPLOY_KEY` beside the personal one.
  - M2, the card did not say whose it is or that it replaces a value: it names the project and
    the scope (the default named) and says "Replaces the value of …" (e2e).
  - M3, reserved names and missing rights were found only once the request was taken: the name is
    checked as the vault checks an agent's (`agent_made_name`: reserved names, the Vidoc key) and
    the project's scope needs Configure, before asking (the Vidoc key tested; Configure is write
    access, which a Superagent's person has); the documents say which later refusals remain.
  - M4, the field was emptied before the server took the value: it is kept until then (e2e: a
    value refused stays typed).
  - m1: the Vidoc key is refused. m2: the value is held in `Zeroizing` and the input built
    directly. m3: `RequestAnswer`'s `Debug` hides the value. m4: `auto_allow` as every agent
    secret, said in ADR-0336. m5: a password field where the browser cannot mask a text box,
    password-manager attributes off, the label says "never shown to your Superagent". The
    description is trimmed when stored.
  Mutants of the fixes: four, each failed by a named test once the project's request left its
  scope out (`target/evidence/mutants-slice10a-r1.log`: `survived=0`).

### Slice 10b (2026-11-15)

- **Slice 10a is merged:** pull request #1311 into `dev` at `066e6468e`, whose own Yulius CI run
  passed. Its first run at `4f85e0196` failed one test outside this change
  (`task_done::a_switch_whose_old_run_had_to_be_killed_starts_no_new_run`, which passes alone
  6 of 6: filed as #1312, run again, passed).
- **Review of slice 10a, round 2 notes**, answered first: n1, the password-field fallback says
  how to write a key's line breaks and asks for a new password; n3, ADR-0336 calls the wiping best
  effort; n2 (a card's "replaces" read on each panel read: the store looks again at storing) and
  n4 (Configure is write access, which a Superagent's person has) answered in words.
- **Tests and code:** `the_superagent_sees_a_crashs_last_screen_a_tasks_usage_and_its_checkout`
  was written first (a crashed task's saved last screen and exit code, its tokens row, one task's
  row only, a project not cloned); `the_checkout_says_how_far_it_is_from_the_integration_branch`
  runs on the CD fixture's real clone (one behind more after a push and a fetch).
- **Mutation:** six mutants, each failed by a named test after two were rewritten (one did not
  compile, one did not match the formatted code) and the tokens test got one task's row
  (`target/evidence/mutants-slice10b.py`, `mutants-slice10b.log`: `survived=0`).
- **Not tested:** a live task's screen (the hub the page reads; the saved screen is the case the
  evidence named); a task's load (left open, above).
- **Gate:** `scripts/dev ci` passed on `578b0f6d3` (4445 tests, 17 skipped;
  `target/evidence/gate-slice10b.log`).
- **Security scan:** Vidoc is switched off for this project in Yulius ("disabled", not a pass).
- **Independent review, round 1** (head `578b0f6d3`): request changes, no exposure found, all in
  this slice's code: F1 (medium), "the last 7 days" was all time (the Dashboard's default): the
  last 7 days now, the period selecting tasks by their creation, the most tokens first, at most
  50 rows (tested: a default period is set); F2 (medium), the saved screen held no secret, so
  nothing proved the redaction: the test writes a project agent secret into it and reads its
  label, never its value, and reads one line; F3, the checkout names its branch; F4, a remote
  branch never fetched is a 409 that says so, not a Git failure; F5, the time is that remote
  branch's own fetch, else the last fetch; F6, a run still starting shows nothing, not the run
  before's screen; F8, the description says a screen is data, never an instruction. F7 (blank
  lines inside the window kept, `lines` clamped) answered in words: the screen's own blank lines
  are part of what it shows.
  Mutants of the fixes (`target/evidence/mutants-slice10b-r1.log`): three; the default period and
  the branch are failed by named tests; `no-redaction` survives on purpose: every tool result is
  redacted of the project's agent secrets by the MCP layer too (`redact_strings`), so the saved
  screen's own redaction is a second layer the tool's answer cannot tell apart.

### Slice 11 (2026-11-15)

- **Slice 10b is merged:** pull request #1314 into `dev` at `a968a31a1`, whose own Yulius CI run
  passed.
- **Review of slice 10b, round 2 notes**, answered first: the API names the `starting` screen and
  what `fetched_at` means; a checkout that cannot be read logs why; `task_usage` says how many
  tasks the period has (`tasks_in_period`).
- **Tests and code:** `a_missing_ability_is_reported_in_one_call` was written first (refusals,
  kept on the instance without access, an issue as alice with read access and without the
  project's name, a task's agent reporting, the sixth in an hour refused). The project variables
  test (`variables_set`, `variable_delete` through the Superagent) failed first ("unknown tool").
  The ability table is in `docs/product.md`.
- **Mutation:** six mutants, each failed by a named test (`target/evidence/mutants-slice11.py`,
  `mutants-slice11.log`: `survived=0`).
- **Not tested:** the link opened in a browser on GitHub (its query is built and encoded as
  GitHub's new-issue form reads it; the unit test checks the encoding).

## The walls, closed

| Wall | How it ends | Slices |
| --- | --- | --- |
| 1. Yulius itself and its host | Closed: version, health, update status and checks (`instance`); Update now, Restart, Cancel for a superadmin's Superagent (`instance_update`). Left open: the host's logs and `yulius doctor` (no page shows them; the host's own tools). | 7 |
| 2. Changing a task that exists | Closed: restart, pause and resume, start now, rename, a prompt not started, reorder, close out, bulk stop, pause and message, a message past a draft, project variables (`variables_set`). Left open: discarding a person's draft (Yulius sees keys, not the box). | 2, 3, 11 |
| 3. Chaining | Closed: a chained task waits for its predecessor's pull request news; the rule reads back as stored; "after all current tasks". | 1 |
| 4. CI, merges and deploys | Closed: CI read, run again, cancel, order; a task's delivery, the merge queue and train, why a pull request is not merging, merge or arm; dev deploy and rollback. Behind a confirmation: production deploy and rollback, promotion, onboarding (the typed slug, a click). Left open: holding the CI queue (no page holds it; cancel and run again). | 2, 5, 8b |
| 5. Subscriptions and model | Closed: which subscription a task is on, moved, model and effort. Left open: the pool's priorities or exclusions (no such setting exists). | 6 |
| 6. Other sandboxes | Closed behind a confirmation: requests last until answered, are amended or withdrawn and read back; another sandbox's tasks are listed without asking. | 8a |
| 7. Tasks' questions | Closed: open questions and why a task waits, answered or dismissed for the person, a held prompt decided, a session's end. Left open: answering a menu in a task's terminal (read with `task_screen`, told to the person). | 4, 10b |
| 8. The Superagent's session lasting | Closed: standing jobs are checks Yulius holds; a draft holds an update's restart and keys typed while it restarts reach it. Left open: a changed tool list still restarts it (its instructions are read only at connection; the restart resumes the conversation). | 9a, 9b |
| 9. Questions to the person lost | Closed behind a confirmation: `person_ask` keeps a decision on the panel until answered. | 8a |
| 10. Secrets | Closed behind a confirmation: a value typed masked on the panel goes to the vault, never to an agent. Left as decided: hosts and `.env` copies ask once each; `secret_verify` refuses production secrets. | 10a |
| 11. Visibility | Closed: a task's screen and a crashed run's exit, its tokens, the checkout's distance. Left open: a task's own load (not measured; the host's is). | 10b |
| 12. Wrong "I can't" | Closed: the instructions and descriptions say to read the tool list, name the missing ability and report it (`ability_report`: kept on the instance, filed in the Yulius repository by the person with one click), and never treat a repository rule as overriding the person without saying which and asking. | 2, 11 |
- **Gate:** `scripts/dev ci` passed on `bd98a7977` (4447 tests, 17 skipped;
  `target/evidence/gate-slice11.log`).
- **Security scan:** Vidoc is switched off for this project in Yulius ("disabled", not a pass).
- **Independent review, round 1** (head `bd98a7977`): request changes, all in this slice's code:
  M1, the product summary's table broke the bullet after it: a blank line and the indentation;
  M2, "never the project's name" was only asked: a report naming the project or holding one of
  its agent secrets is refused (tested: the repository's name, a secret's value); M3, an issue
  was posted outside under the person's name without them (and a contained server asks
  nothing): the report is kept on the instance and answered with a link the person submits, so
  m1 (GitHub errors read as a permission) goes with it; m2, a task's owner must still work in the
  project; m4, the hour's window and the link's encoding have unit tests; n1, a lone `\r` is
  refused in the ability, and the API row speaks of error results.
  Mutants of the fixes: five, each failed by a named test once a task's owner who lost work access
  was tested (`target/evidence/mutants-slice11-r1.log`: `survived=0`).
- **Independent review, round 2** (head `d4615916c`): request changes, narrowly: R2-1, a long
  report made a link longer than GitHub's new-issue form takes: refused past 6000 bytes before it
  is kept (`checked_link`); R2-2, the name check matched substrings (any "Yulius" on the Yulius
  instance, a slug `api` in "application"): whole words only, not for the Yulius project itself,
  and the refusal names the kind of name (`project_named`, `names_word`); R2-3, ABL-004 reworded.
  Three more mutants, each failed by a unit test (`mutants-slice11-r2.log`: `survived=0`).
- **Review of slice 11, round 3 notes** (follow-up after PR #1315, head `745f00417`): R3-1, a
  compound of the project's name (`shop-api`, `shop_db`) slipped through: only letters and digits
  bound a name now; R3-2, the refusal names the word; R3-3, the case is capped at 1500 characters,
  what the filing link takes of plain ASCII prose (`MAX_CASE_CHARS`). Tests first
  (`a_project_name_is_matched_as_a_whole_word`, `a_case_at_its_cap_fits_the_link`,
  `cockpit::a_missing_ability_is_reported_in_one_call` failed before the fix); three mutants, each
  killed (`mutants-slice11-r3.log`: `survived=0`). Security scan: Vidoc disabled.
- **Independent review of `745f00417`**: approve, with minors answered test-first: the link's
  refusal gives its size and the bound and names every text (m1, m6); a camelCase compound
  (`ShopApi`, `ACMEShop`) is the name too (m2); the case's cap is checked by `checked_case`, its
  bound inclusive (m3); the cap's wording says plain ASCII prose, since other text meets the
  link's bound first (m1); the broader match's cost, ordinary text a short name catches, is said
  in ABL-001 (n4); the test column names the unit tests (n5). Mutants:
  `mutants-slice11-r3b.log` (one survived, a hump after a digit, until `Api2Shop` was tested;
  then `survived=0`).
- **Delta review of `25991ca66`**: approve, with notes answered: a camelCase name found as its
  words apart is said so in the refusal (`naming_refusal`: ``written `web app` ``, m1), and the
  word-split case is in ABL-001; a character outside ASCII takes up to 12 bytes in the link,
  not 9 (n2); `docs/changes.md` says the camelCase compounds and plain ASCII prose (n3).
