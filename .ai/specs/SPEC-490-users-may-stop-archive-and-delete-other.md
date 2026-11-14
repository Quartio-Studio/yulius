---
yulius-task: 42c13112-2302-432f-ab26-08ee3802b561
status: in-progress
---

# SPEC-490: anyone who may work in a project stops, archives and deletes its tasks; control passes only by the owner's consent

Status: in-progress
Date: 2026-11-14
Owner: Yulius task #473
Scope authorization: the request of task #473 (by MrFishPL, 2026-11-14): "users should be able
to delete stop and archive tasks of other users, but not take control without the consent".
Amends SPEC-070 R3, R4 and R7 and SPEC-066 MGT-008..010; the decision is
[ADR-0331](../../docs/decisions/0331-anyone-tidies-a-task-and-only-its-owners-consent-passes-control.md),
which amends [ADR-0060](../../docs/decisions/0060-a-task-is-owned-by-one-person.md).

## Goal

A person who may work in a project can stop, archive, restore and delete any of its tasks,
whoever owns them, from every place that offers these actions: the task page, the lists, Home,
the sidebar, the API and the tools their Superagent and agents use. Nobody steers a task they do
not own: typing into it, sending it messages, answering what it asks, changing how it runs or
delivering it stays its owner's, and passes to someone else only by the owner's consent. One
rule decides both, in one place, and the UI says why control is refused and how to ask for it.

## Context

- **Today.** A task has one owner (`tasks.owner_id`, SPEC-070, ADR-0060), whose GitHub token it
  acts with. `App::load_task(ctx, id, action)` (`crates/yulius-app/src/tasks.rs`) refuses every
  action above `Action::View` to anyone else with 403 "Task #N is @owner's: it acts on GitHub
  as them. Ask them for control." Stop, archive, restore and delete go through it with
  `Action::Work`, so they are refused too. ADR-0060 rejected the alternative "leave stop,
  archive and delete open to any writer"; the person now decides the opposite.
- **Why it hurts.** A task of a teammate who is away keeps a session, a subscription seat and
  its place in the lists, and nobody else can end it or put it away. Asking for control to do
  that takes the teammate's GitHub identity for a task one only wants gone.
- **Consent exists.** A person with write access asks for control
  (`POST /api/tasks/{tid}/control-request`), the owner hands over
  (`…/control-request/accept`) or declines, the asker withdraws
  (`DELETE …/control-request`); a request to an owner who can no longer act (disabled, deleted,
  no write access) passes control at once (`crates/yulius-app/src/task_control.rs`). What it
  lacks: the owner sees a request only on the task's own page; an owner who handed a task over
  cannot change their mind; and the task page is the only place to ask.
- **The owner check is written six times.** Beside `load_task`, the same comparison
  `task.owner == ctx.user.id` stands in `terminal.rs` (`terminal_open`, `can_work`),
  `shells.rs` (`owns_shell_task`), `files.rs` (`sandbox_worktree`, `files_roots`),
  `task_edit.rs` (`reorder_queue`, `bulk_tasks`) and `task_control.rs`. A new way to steer a
  task has to remember one of them.
- **Tools follow the person.** `task_stop`, `task_archive` and `task_delete`
  (`App::tool_task_change`, `crates/yulius-app/src/manage.rs`), `task_close`, `tasks_bulk` and
  the remote MCP call the web UI's functions with the `AuthContext` of the person the caller
  acts for (SPEC-066, ADR-0330), so the rule of the function is the rule of the tool.
- **The web UI has one rule too.** `taskActions(task, access, pr, me)`
  (`crates/yulius-server/web/assets/status.js`) decides every task action shown: the task
  menu (sidebar, lists, Home, task page), the sidebar's quick archive and "Archive all", the
  task page's bars. Today `write = allows(access, 'write') && !otherOwner(task, me)` for all.
- **Stop and archive by a page are not audited**; by a tool they are (`task.stopped`,
  `task.archived`, `task.unarchived`). A deletion is audited either way (`task.deleted`).
- **Three ways in did not ask the owner at all** (found by reading every route and every tool
  down to its check, 2026-11-14):
  1. `task_message` (the Superagent, the remote MCP, `tasks_bulk {do: "message"}`, an approved
     `sandbox_task_message`): `App::tool_task_message` typed into another person's *running*
     session and ended its `needs_you` request; it refused only a task holding its owner's
     personal secrets or MCP servers (SPEC-230 allowed the rest on purpose). The web UI's
     message route and the terminal refused the same person.
  2. `attachments_trash {restore, into}`: files went into the attachment folder of any task
     of the project, while `POST /api/tasks/{tid}/attachments` is the owner's.
  3. `POST /api/sandboxes/{sid}/onboarding/approve` (and `onboarding_start`): anyone with write
     access started the onboarding task, which is its sandbox creator's and acts on GitHub as
     them, and added a hand-over text to its request. Continue on the same task was refused.

### Every way to change a task, and its class

The inventory this change was built from (the routes of
`crates/yulius-server/src/routes/mod.rs`, the tools of `mcp.rs`, `person_tools.rs`,
`manage.rs`, `remote.rs`). "Tidy" is open to anyone who may work in the project; "control" is
the owner's.

| What | Entry points | Class |
| --- | --- | --- |
| Stop a task, cancel one that waits to start | `POST /api/tasks/{tid}/stop`; `task_stop`, `sandbox_task_stop`; `tasks_bulk {do: stop, numbers}` | tidy |
| Close out an idle running task | `task_close` | tidy |
| Archive, restore | `POST …/archive`, `…/restore`; `task_archive`; the sidebar's quick archive | tidy |
| Delete | `DELETE /api/tasks/{tid}`; `task_delete` | tidy |
| Type into the session, resize it | `WS /api/tasks/{tid}/terminal` | control |
| Send or queue a message, decide a held prompt | `POST …/queued-messages`, `…/held-prompts/{hid}`; `task_message`, `sandbox_task_message`, `task_held_prompt`, `tasks_bulk {do: message}` | control |
| Answer or dismiss what the task asks (`needs_you`, a question) | `task_questions.rs`; `task_question_dismiss` | control |
| Continue, restart, start now, schedule, change how it starts | `POST …/continue`, `…/start-now`, `PUT …/schedule`, `…/start`; `task_restart`, `task_start_now` | control |
| Pause, resume | `POST …/pause`, `…/resume`; `task_pause`, `task_unpause`, `tasks_bulk {do: pause}` | control |
| Move to another subscription | `POST …/switch` | control |
| Rename, replace the request, order in the queue | `task_edit`, `tasks_reorder` | control |
| Attach a file | `POST …/attachments`; `attachments_trash {restore, into}` | control |
| Start its onboarding task | `POST /api/sandboxes/{sid}/onboarding/approve`; `onboarding_start` | control |
| Change how it starts | `task_update`, `sandbox_task_update` | control |
| Open or merge its pull request | `POST …/delivery/pr`, `…/delivery/merge` | control |
| Write into its worktree (editor, upload) | `PUT /api/sandboxes/{sid}/files/*` with `root=task:<id>` | control |
| A shell in its worktree | `POST /api/sandboxes/{sid}/shells` with a task, `WS /api/shells/{id}/terminal` | control |
| Hand over, decline a request | `POST …/control-request/accept`, `DELETE …/control-request` | control |
| Read: detail, prompt, transcript, delivery, changes, feedback | the `GET` routes, `POST …/feedback` | view |

Not a task's, and so outside this rule (each is a decision below): the sandbox's one browser,
CI runs (rerun, cancel) and the repository's cleanup, which are the project's; a shell in the
sandbox repository, which reaches every worktree on disk (ADR-0060's known limit: containment
separates sandboxes, not tasks); and automations, which any writer may change and whose tasks
are their creator's (reported as issue #1289, see Decisions).

## Requirements

- **OWN-001** One rule. `yulius_domain::task_access` names what a person does to a task
  (`TaskAction`: `View`, `Tidy`, `Control`) and decides it from two facts, their access to the
  project and whether they own the task: view needs read access; tidy needs write access;
  control needs write access and ownership. `App::load_task` and `App::require_control` are
  its only callers in the application, and every owner comparison listed in the Context goes
  through them. A sandbox-level action passed to `load_task` above `View` means `Control`, so a
  new path that does not say otherwise is the owner's (fail closed).
- **OWN-002** Tidy. A person with write access to the project stops (also cancels a task that
  waits to start), closes out, archives, restores and deletes a task they do not own, with the
  checks and effects these actions have for an owner (a running task is stopped before it is
  archived or deleted; a finished one is closed first). Read access is refused as before (403),
  and no access answers 404. The same holds for `task_stop`, `task_archive`, `task_delete`,
  `task_close` and `tasks_bulk {action: "stop"}` with numbers, for the person the caller acts
  for, through the Superagent, the remote MCP and a task's agent; a caller's own task stays
  refused.
- **OWN-003** "All" stays one's own. `tasks_bulk` with `all` and the sidebar's "Archive all"
  take the person's own tasks only (the tool says how many of other people it left, the
  button's title reads "Archive your Done tasks"); another person's task is tidied by naming
  it.
- **OWN-004** Control. Everything in the inventory's control rows is refused to anyone but the
  owner with 403, whatever the way in (page, API token, Superagent, remote MCP, task agent),
  with one message: "Task #N is @owner's and only they steer it: it acts on GitHub as them. Ask
  them for control on the task's page. You can still stop, archive or delete it." There is no
  exception for administrators or the superadmin. Start onboarding is control of the
  onboarding task: one that exists is started by its owner; a sandbox without one gets it
  created for the person who starts onboarding (never as a side effect of a refused start).
- **OWN-005** The owner learns who tidied. A stop, close-out, archive, restore or deletion of a
  task by someone who does not own it is audited under that person's name with the owner's
  (`task.stopped`, `task.closed_by_person`, `task.archived`, `task.unarchived`,
  `task.deleted`; detail `{"task", "owner"}`), and a stop or close-out says in the task's
  terminal "Stopped by @login." or "Closed by @login." The audit of an owner's own page action
  is unchanged (none for stop and archive), and a tool's entry keeps its actor and gains the
  owner when the task is not its person's.
- **OWN-006** The web UI offers tidy on another person's task wherever it offers it on one's
  own: Stop or Cancel, Archive or Restore and Delete in the task menu (sidebar, lists, Home,
  task page), the sidebar's quick archive, and Cancel on the task page's waiting bars. Stopping
  and deleting another person's task ask first and name the owner ("Stop @anna-q's task #3?").
  Every control item stays disabled with the title "@owner owns this task: request control to
  do this".
- **OWN-007** Why and how. For a person with write access the task page's bar on another
  person's task reads "You are watching @owner's task. You can stop, archive or delete it; to
  type in it or change it, ask @owner for control." with Request control. The task menu of
  another person's task has Request control (or Withdraw request) wherever the menu opens, so
  control can be asked for from a list.
- **OWN-008** The owner hears of a request wherever they are. A request for control of a task
  reaches its owner as a notification (the bell, a toast, the desktop notification when
  switched on) that opens the task, and the task's row in the sidebar says "@login asks for
  control" until it is answered. A request waits without a time limit: silence is not consent.
- **OWN-009** Consent can be withdrawn. A hand-over on request records who gave control
  (`tasks.control_from`, schema version 59). That person takes the task back without asking
  (`POST /api/tasks/{tid}/control/take-back`): one conditional update of the owner and the
  giver, then what a hand-over does (commit identity, fingerprints, a restart for personal MCP
  servers, the terminal notice "Control of this task returned to @a: it acts on GitHub as @a
  again.", audit `task.control_taken_back`, the published summary). It needs write access and
  an enabled account. The right ends when control moves again: the owner hands it on (the new
  giver is that owner), hands it back (nobody), or an absent owner's task passes (nobody).
  `TaskSummary.control_from` names the giver, or is null.
- **OWN-010** What each side sees after a hand-over. The giver's bar reads "You handed this
  task to @b. It acts on GitHub as them." with Take back; the new owner's terminal notice says
  "@a can take it back."; when it is taken back the former owner's view follows the event
  (transcript instead of terminal) and they are notified "@a took back control of #N".
- **OWN-011** An owner who is gone. As before (SPEC-070 R5): a request to an owner whose account
  is disabled or deleted, or who lost write access to the project, passes control at once and
  is audited; a failure to tell counts as present. An owner who is only away keeps the task:
  the request waits, and the task can be tidied meanwhile.

## Plan

- [x] The spec and ADR-0331 state the rule: tidy for anyone who may work, control by consent
- [x] One rule in the domain decides view, tidy and control, and every owner check goes through it
- [x] Stop, close-out, archive, restore and delete work on another person's task over the API and the tools
- [x] A tidy by someone else is audited with the owner and said in the task's terminal
- [x] The web UI offers tidy on another person's task everywhere and says why control is refused and how to ask
- [x] The documents and the tools' descriptions say the new rule (api, product, architecture, changes)
- [ ] Independent review of the first pull request (the rule and the UI), findings fixed
- [ ] The owner hears of a request anywhere, and control can be asked for from the task menu
- [ ] The person who handed a task over takes it back (schema 59, the route, the UI)
- [ ] Independent review of the second pull request (consent), findings fixed
- [ ] Gates green on the final head, evidence recorded

## Acceptance criteria

- **AC-1** (OWN-002) With two people who have write access, B stops A's running task (it ends
  `stopped`), cancels A's queued task, archives and restores A's resting task and deletes it;
  each over the HTTP router. A Superagent acting for B does the same with `task_stop`,
  `task_archive` and `task_delete`, and `tasks_bulk {action: "stop", numbers: [A's]}` stops it.
- **AC-2** (OWN-002) A person with read access gets 403 for stop, archive and delete of A's
  task; a person without access gets 404.
- **AC-3** (OWN-004) B gets 403 with the message of OWN-004 for: continue, switch, pause,
  resume, start-now, schedule, change of start, a queued message, a held-prompt decision, an
  attachment, opening and merging the pull request, `task_edit`, `tasks_reorder`,
  `tasks_bulk` with `pause` and `message`, `task_message`, a restore of attachments into A's
  task, Start onboarding of A's onboarding task, a file write and a shell in A's worktree;
  terminal input from B does not reach the run. A can do each. An instance administrator and
  the superadmin are refused the same.
- **AC-4** (OWN-001) The domain rule's table: each of the three actions against each access
  level, owning and not owning.
- **AC-5** (OWN-003) `tasks_bulk {action: "stop"}` without numbers by B leaves A's tasks alone
  and counts them.
- **AC-6** (OWN-005) After B stops and archives A's task the audit holds `task.stopped` and
  `task.archived` under B's login naming A, and the task's terminal carries "Stopped by @b.";
  after A stops their own task from the page there is no such entry.
- **AC-7** (OWN-006, OWN-007; e2e) On another person's task the menu's Stop, Archive and Delete
  are enabled and Continue is disabled with the title of OWN-006; Stop asks naming the owner
  and stops the task; the sidebar row has its quick archive; the bar reads as OWN-007; the
  menu's Request control asks and turns into Withdraw request; "Archive all" leaves it.
- **AC-12** (OWN-004) B starts onboarding of a sandbox without an onboarding task: the task
  is B's. A start refused for want of a Claude account still creates nothing.
- **AC-8** (OWN-008; e2e) A request for control on the owner's task, while they look at another
  page, adds a notification that opens the task, and the sidebar row says who asks.
- **AC-9** (OWN-009) A hands over to B on B's request: `control_from` is A. A takes back: owner
  A, `control_from` null, B refused control again, audit `task.control_taken_back`, the notice.
  Refused: C (never the giver) 403; A after B handed on to C on C's request (409, the giver is
  B); A disabled or without write access (403); a stale race (409).
- **AC-10** (OWN-009) B hands the task back to A on A's request: `control_from` is null (B
  cannot take it back). An absent owner's task passing to C clears it.
- **AC-11** (OWN-010; e2e) The giver sees Take back, uses it and gets the terminal back.

## Test plan

- Unit (`crates/yulius-domain/src/task_access.rs`): AC-4.
- Store (`crates/yulius-store/tests/store.rs`): migration 59; `pass_control` records the giver,
  `take_control` clears it, `take_back_control` refuses stale expectations.
- Integration (`crates/yulius/tests/task_ownership.rs`): AC-1 to AC-3, AC-5, AC-6, AC-9, AC-10
  through `App` and the HTTP router with two GitHub users and the Superagent's tools; the
  existing refusal tests for stop, archive and delete become the new permissions, each named in
  the Evidence. `secrets_scopes.rs`, `task_message.rs`, `pool_supervision.rs` (a message),
  `attachment_trash.rs` (a restore), `projects.rs` (Start onboarding, AC-12), `cockpit.rs`
  (`tasks_bulk`, `task_close`).
- e2e (`e2e/tests/ownership.e2e.ts`): AC-7, AC-8, AC-11, with accessibility checks of the bar,
  the menu and the dialog.
- Contract (`crates/yulius/tests/ui_contract.rs`): the new route is called by the UI and
  documented.

## Decisions

- **Tidy is stop, close-out, archive, restore and delete, and nothing else.** They end a task
  or put it away, act on nothing outside Yulius and use nobody's GitHub token (a deletion
  leaves pushed branches and pull requests). Restore undoes an archive, so whoever may archive
  may undo it. Pause is not tidy: a paused task waits for its pauser to resume it, which is
  steering, and Stop already ends a task that must not run.
- **Write access, not read.** "Any user" is anyone who may work in the project: a reader changes
  nothing in a project today (SPEC-001 ACC-007), and a task of a project one cannot see stays
  invisible.
- **`load_task` keeps its signature; above `View` it means control.** Parallel work adds task
  functions (`task_setup.rs` of task #469); each stays the owner's without knowing this
  change. The three tidy functions ask for `TaskAction::Tidy` by name.
- **"All" takes one's own tasks.** A sweep over other people's tasks that nobody named is not
  what "stop that task" means; naming the task is the consent to its loss.
- **Consent is a hand-over that its giver can take back, not a shared grant.** A grant that
  lets two people type would have B steering an agent that acts on GitHub as A, which ADR-0060
  exists to prevent. Withdrawal therefore returns the task whole. One level is kept (the last
  giver), not a history: who handed on consented to the next owner, and their own giver's
  right ends there.
- **No time limit on a request, no administrator override.** Either would be control without
  consent. An owner who is away loses nothing but a task that others may now stop; an owner
  who is gone cannot consent to anything, and only then does a request pass control.
- **A message is control, whatever the task holds.** SPEC-230 let a Superagent type into
  another person's running task unless it held their personal secrets. A message decides what
  the agent does next under its owner's name; the request names it ("sending it messages").
  The secrets check is gone with it: nobody but the owner gets that far.
- **The onboarding task is the starter's.** Refusing everyone but the sandbox's creator would
  leave a sandbox whose creator is away with no task to ask control of; creating the task for
  the creator and then refusing would create it as a side effect of a refusal, which SPEC-246
  forbids. A task that does not exist is nobody's, so it is created for the person who starts
  onboarding and acts on GitHub as them; one that exists (a start that failed, a stopped
  onboarding) is its owner's like any task.
- **The sandbox's browser is the project's, not a task's.** One Chromium per sandbox is
  shared by its people and its agents (BRW-002); a tab has no owner, and a request to act in
  the browser reaches everyone who may work there. Acting in it types nothing into a task's
  session. Giving tabs owners is a design of its own; nothing here needs it.
- **CI runs and cleanup are the project's.** A rerun or a cancel of a run, and the removal of
  a clean worktree of a stopped task, change the project's shared state by rules of their own
  (SPEC-171, the hygiene rules) and steer no session.
- **Automations are reported, not changed here.** Any writer may change an automation's
  prompt and run it, and its task is its creator's: a person's prompt can run under another
  person's name. That is about who an automation runs as, with product choices of its own
  (the editor becomes its owner, or only its creator changes it), not about a task someone
  owns; it is filed as issue #1289 with both options.
- **No agent asks, grants or takes back.** Asking and consenting stay a person's own acts in
  the page or the API: a Superagent reads transcripts and web pages, and consent given on a
  sentence it read there is not the owner's.
