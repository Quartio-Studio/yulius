---
yulius-task: 154bb878-9296-4ee7-84d1-9c80b0a8ffd3
status: in-progress
---

# SPEC-070: a task is owned by one person, whose GitHub credentials it uses

Status: in-progress
Date: 2026-10-13
Scope authorization: task #66 (requested by MrFishPL): "Tasks should be owned by a specific
person because it uses their github credentials. Other users shouldnt be able to write to the
tasks to not abuse the credentials but they can request the owner to take control on the task.
Then, the credentials changes to the new user's." Extends SPEC-001 GIT-003 and ACC-007, SPEC-002
§8 and ADR-0007.

## Goal

Every task has one owner. The task acts on GitHub with the owner's token, and only the owner
changes the task. Anyone else who may work in the project sees it read-only and can ask the
owner for control; once the owner hands it over, the task acts on GitHub as the new owner.

## Context

- Since 2026-10-11 (GIT-003, ADR-0007) a task acts on GitHub with its creator's user-to-server
  token: `App::creator_github_token` (`crates/yulius-app/src/access.rs`) serves the agent's Git
  credential and `gh`, Yulius's pushes and pull requests, and the agent's `delivery_merge`,
  `delivery_promote` and `issue_create`. Commits carry the creator's name and no-reply address
  (`set_task_identity` in `supervisor.rs`, the import's identity in `imports.rs`). The agent's
  project-management tools (`AgentContext::caller`), `delivery_*` (`agent_authority`) and the
  armed-merge decision (`agent_wait.rs`) act for the creator.
- But every person with write access to the sandbox (`Action::Work`) may type into any task's
  terminal, send it messages and attachments, continue, switch, start, stop, archive or delete
  it and merge its pull request: they drive an agent that holds someone else's GitHub token. All
  those paths decide through `App::load_task(ctx, id, Action::Work)` (`tasks.rs`); the terminal
  through `terminal_open`'s `can_write` and `App::can_work`, rechecked on every input.
- `tasks.created_by` also names who asked (the request file's "Requested by", the session's
  author, the creator's composer-draft attachment folder `attachments/drafts/<user>/`), so it
  cannot simply be rewritten on a hand-over.
- The Cockpit and task agents stop, archive and delete tasks "for the person the caller acts
  for" (SPEC-066) through the same `load_task`.

## Requirements

- R1. Ownership. A task has an owner (`tasks.owner_id`, schema version 17), the person who
  created it when it is created; existing tasks get their creator. `created_by` stays who asked.
  `TaskSummary` gains `owner: {login}`.
- R2. Credentials follow the owner. Everything GIT-003 gives the creator's token, identity or
  authority uses the owner's instead: `creator_github_token` (renamed `owner_github_token`) and
  its fallback to the App when the owner has no usable token (with the terminal notice naming
  the owner), the worktree's commit identity, the agent's management tools, `delivery_*` and the
  armed-merge decision.
- R3. Only the owner writes. Every task write through the web API — terminal input and resize,
  `continue`, `switch`, `start-now`, `stop`, `archive`, `restore`, `DELETE`, queued messages,
  held-prompt decisions, attachments, opening and merging its pull request, writing into its
  worktree through the file API or an upload, and opening or typing into a shell in it (a shell
  opened before a hand-over stops taking input) — needs the owner as
  well as write access to the sandbox; anyone else gets 403 "Task #N is @owner's: it acts on
  GitHub as them. Ask them for control." The same holds for the Cockpit's and task agents'
  `task_stop`, `task_archive` and `task_delete` (they act for a person). Reading is unchanged.
  The terminal opens read-only for a non-owner (`can_write` false).
- R4. Asking for control. A person with write access to the sandbox who does not own the task
  asks for control (`POST /api/tasks/{tid}/control-request`). One request waits per task: a
  second person's request is 409 naming who asked; asking again is harmless. The owner sees
  "@login asks to take control" in the task. The requester may withdraw it, the owner declines
  it (`DELETE /api/tasks/{tid}/control-request`, by either), or the owner hands it over
  (`POST /api/tasks/{tid}/control-request/accept`). A hand-over whose requester can no longer
  work in the sandbox is refused (409) and the request is dropped.
- R5. An absent owner. When the owner can no longer act — the account is disabled or deleted,
  or it has lost write access to the sandbox — a request passes control at once, whatever request
  waits (asking again does it for someone who asked earlier). A waiting asker who can no longer
  work gives way to the next person who asks; both this and a refused hand-over drop the request,
  audited `task.control_dropped`. A request never locks a task. Only a definite answer drops
  a request (a disabled or deleted account, no write access); when GitHub cannot tell, the
  request stays and a hand-over fails as unreachable (502).
- R6. A hand-over. Control passes atomically only if the owner and the request are still the
  ones the decision saw (a race answers 409). Then: the owner is the requester and the request
  is gone; the worktree's commit identity is the new owner's; the next GitHub call of the task
  (also of a run in progress: Git, `gh`, delivery) uses the new owner's token; the terminal says
  "Control of this task passed from @a to @b: it now acts on GitHub as @b."; the App-fallback
  notice may show again; the change is audited (`task.control_requested`,
  `task.control_withdrawn`, `task.control_declined`, `task.control_passed`) and published.
- R7. The task view (`taskview.js`). For a non-owner with write access the terminal is
  read-only with the label "Read-only: @owner owns this task", the task menu's write items,
  the queued-message card, the exit actions and the phone quick keys are off (titled "@owner
  owns this task"), and a bar says "@owner owns this task: it acts on GitHub as them." with
  Request control (after asking: "You asked @owner for control." with Ask again and Withdraw).
  The owner with write access sees "@login asks to take control of this task: it will act on
  GitHub as them." with Hand over and Decline, on its own row above what else the task needs
  (a chained task keeps Start now). When control passes, both people's views follow the `task.updated` event without a
  reload, and the new owner's terminal reconnects and accepts input. The Tasks list and the
  sidebar quick archive offer archive only on the viewer's own tasks.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Tasks record an owner and act on GitHub as their owner
- [x] Only a task's owner writes to it, over the API, the terminal and the agent tools
- [x] People ask for control and the owner hands it over or declines; an absent owner's task passes at once
- [ ] The task view names the owner, offers Request control and the owner's hand-over
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC1. A new task's owner is its creator; a task from before the migration has its creator as
  owner; `TaskSummary.owner` names them.
- AC2. With fakes recording the token of each GitHub call: after a hand-over from A to B, the
  agent's Git credential and `gh` token are B's, and the worktree's commit identity is B's.
- AC3. User B with write access gets 403 for stop, continue, archive, delete, a queued message,
  an attachment and a merge on A's task, and terminal input from B does not reach the run; A
  can do each. A Cockpit acting for B cannot stop A's task.
- AC4. B asks; a second requester C gets 409; A declines → no request; B asks again and A hands
  over → owner B, request gone, A now gets 403, B writes; audit rows and the terminal notice are
  present.
- AC5. A's account is disabled → B's request makes B owner at once.
- AC6. A hand-over after the requester lost write access is 409 and drops the request; a
  withdraw by someone who is neither owner nor requester is 403.
- AC7. (e2e) A non-owner sees the read-only notice and Request control; the owner sees Hand
  over; after the hand-over the former owner's view shows the notice and the new owner's the
  composer, without a reload.

## Test plan

- Store (`crates/yulius-store/tests/store.rs`): the migration fills `owner_id`; conditional
  request, clear and transfer updates refuse stale expectations.
- Integration (`crates/yulius/tests/task_ownership.rs`, new): AC1-AC6 through `App` and the
  HTTP router with two GitHub users (the existing fake GitHub), the terminal input check, and
  AC2 with the recording fakes used by `agent_hooks.rs`.
- Existing tests that act on another person's task keep passing or are changed to the owner,
  each change named in the Evidence.
- e2e (`e2e/tests/ownership.e2e.ts`): AC7.

## Decisions

- A separate owner column instead of rewriting `created_by`: who asked stays true (request
  file, session author, the creator's draft attachments), and only who acts changes.
- Ownership is checked inside `load_task` for `Action::Work`, so every existing and future task
  write gets it, instead of at each route.
- Every write is the owner's, also stop, archive and delete: they are the owner's work too, and
  an absent owner is covered by R5 rather than by exceptions.
- One waiting request per task, stored on the task (two columns), not a request table: there is
  nothing to queue, and a hand-over clears it in the same update.
- The owner approves; nobody overrides a present owner, not even an administrator: the request
  is about protecting a person's credentials. Recorded in ADR-0051 (0050 is taken by task #54).
- Review round 1: the waiting request is read with a separate query per summary instead of
  widening `TaskRecord`; task lists are bounded and the query is by primary key. The mock does
  not model an absent owner or a requester who lost access (no account states in it); those
  rules are covered by the integration tests.
- Review round 2: the "waiting" colour of the attention bar is not kept on its second row when a
  request for control is shown above it (cosmetic; the row keeps its text and buttons).

## Evidence

(to fill)
