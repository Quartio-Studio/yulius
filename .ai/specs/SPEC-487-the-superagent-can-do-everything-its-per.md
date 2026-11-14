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
[ADR-0328](../../docs/decisions/0328-a-task-is-not-done-while-yulius-has-pull-request-news-for-it.md),
the rule for the Superagent is
[ADR-0330](../../docs/decisions/0330-the-superagent-does-what-its-person-may-do.md).

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
| Start a project's onboarding (also by continuing its onboarding task before it started) | behind one confirmation in the panel (slice 8); until then the person, on the project's page | no | SPEC-051 OBD-002, ADR-0038: a person decides that a repository is adapted |
| Project variables: read | yes (slice 2) | in its environment (had) | They are not secrets |
| Project variables: set, delete | yes (slice 10) | no | Decided with the vault's rules; a person's page today |
| Rename, edit a prompt not started, reorder, close out, bulk actions | yes (slice 3) | no | As above |
| Send a message past text the person left unsent in a task's prompt box | yes, for the task's owner (slice 3) | no | SPEC-480: the text is the person's; they decide that it goes out with the message |
| Discard text a person left unsent in a task's prompt box | no | no | Yulius sees keys, not the box, and the keys that clear it (Esc, Ctrl+C) also interrupt a turn or end the session; the owner's message goes past it instead |
| Message a task | yes (had) | no | ADR-0178 |
| List open questions, see why a task needs attention | yes (slice 4) | yes, read only, without a held prompt's words (slice 4) | A task that sees another's question does not ask it again; a prompt Yulius held reached no agent |
| Answer or dismiss a task's question for the person; decide a held prompt (send, dismiss, a new task now or after) | yes (slice 4) | no | The question is to a person; their Superagent answers when they say so |
| Decide a held prompt by skipping steps or reverting the task | no: the person, on the task's page | no | Reverting discards the task's work and asks for the person's own confirmation there (TSK-013) |
| Answer a question Claude Code asks in a task's terminal (a permission menu, a choice) | not yet (decided with slice 10, which shows a task's screen) | no | Yulius cannot tell an agent what the menu shows; keys typed blind would take its default |
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
| CHN-002 | A running task whose pull request is merged is Done only once the pull request watch, when it saw that pull request open, has told its agent of the merge and the turn that news starts has ended. While the news is typed and that turn has not begun, it still is not Done: typed into an idle session, until the task goes from idle to Working, for at most two minutes; typed while a turn runs (Claude Code queues it), as long as Claude Code's queue says it waits for its turn, whichever entry gets its turn first, for at most two minutes of an idle session, and for two minutes from the typing while the queue has not shown it yet. The same holds for an armed merge that ended without merging until the watch said how, and for a merge GitHub reports before the record says so. One decision reads the task's delivery once. A task the watch tells nothing is Done as before: a playground's, one whose pull request it never saw open, and one a person paused. An update's hold or a pause for capacity changes nothing: the news is typed right after. A supertask's wait and Home's counts follow the same rule. | `task_done::a_chained_task_waits_until_its_predecessor_heard_of_its_merge` (real hooks, the prompt hook held at its check), `task_done::news_typed_while_a_turn_runs_is_due_until_its_own_turn` (a held turn, Claude Code's queue, a held hand-over), `task_done::queued_news_stays_due_while_another_entry_gets_its_turn_first`, `merge_train::a_merge_github_reports_first_is_news_too`, `merge_train::a_task_chained_after_a_dropped_armed_merge_waits_for_its_news`, `supertask::a_supertask_waits_for_a_task_that_has_not_heard_of_its_merge`, `supertask::a_merge_during_an_updates_hold_is_still_news_due`, `pr_watch::tests::news_is_due_until_the_agent_heard_it`, `pr_watch::tests::typed_news_is_due_until_its_turn` |
| CHN-003 | An achieved goal is Done as before, and so are a closed task and a task with no pull request news left: CHN-001 and CHN-002 remove no Done state that was final. | the tests of TSK-012 and TSK-014 unchanged (`intake::chained_tasks_start_on_done_and_wait_for_a_person_otherwise`, `task_done::a_chain_starts_when_its_predecessor_achieves_its_goal`) |
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

## Plan

- [x] A chained task waits while Yulius has pull request news for its predecessor (CHN-001 to CHN-003)
- [x] Tools read a start rule back as stored and take "after all current tasks" (CHN-004 to CHN-006)
- [x] Slice 1's documents: product, architecture, API, the decision and the changes log
- [x] Slice 1's independent review answered test-first
- [x] Slice 2: the person's project tools served to the Superagent (restart, pause, resume, start now, a task's delivery, CI, dev deploy), the rule's decision record, the instructions rewritten
- [x] Slice 3: task control that had no function (rename, a prompt not started, queue order, close out, bulk stop, pause and message)
- [x] Slice 4: tasks' questions (open ones listed, why a task needs attention, answered or dismissed for the person, the end of a session)
- [ ] Slice 5: merges (a task's merge armed for the person, the sandbox's merge queue, why a pull request is not merging, CI order and hold)
- [ ] Slice 6: subscriptions and model (which one a task is on, moved, model and effort changed)
- [ ] Slice 7: the instance (version, health, update status, checks, Update now and Restart, sessions reconnect after a restart)
- [ ] Slice 8: requests to the person that last (cross-sandbox requests stored and amendable, a pending decision, production behind one confirmation, another sandbox's task list)
- [ ] Slice 9: the Superagent's session lasts (standing instructions survive a restart, a message typed during one is kept, a changed tool list restarts nothing)
- [ ] Slice 10: secrets entered masked beside the chat, and visibility (cost, load, a crash's exit and last screen, a current checkout)
- [ ] Slice 11: a missing ability reported in one call, and the ability table in the product documentation

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
