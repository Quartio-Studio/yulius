---
yulius-task: 56839a7f-ee68-4994-99cf-5e6db3f7ee41
status: in-progress
---

# SPEC-510: Take control of a teammate's task, unless its owner chose to be asked

## Goal

The person's request (task #492, verbatim): "change "request control" for tasks to "take
control" - github changes then etc, unless user selects different option".

Someone who may work in a project takes control of a teammate's task at once with **Take
control**: from then on the task acts on GitHub as them (token, commit identity, personal MCP
servers), and the previous owner is told and may take it back. A person who wants to be asked
first selects that option in Settings; their tasks keep today's Request control, Hand over and
Decline.

## Context

- Today (SPEC-070, SPEC-490, ADR-0060, ADR-0331): a task acts on GitHub as its owner and only
  the owner steers it. Anyone else asks (`POST /api/tasks/{tid}/control-request`); the owner
  hands over (`…/control-request/accept`) or declines; the asker may withdraw. Control passes
  without consent only from an absent owner (disabled, deleted, without write access). The giver
  of a hand-over takes it back (`POST …/control/take-back`, `tasks.control_from`) and the person
  it is taken from hears of it (`task.control_taken_back`).
- One function passes control (`App::pass_task_control`, `crates/yulius-app/src/task_control.rs`):
  the conditional store update, the worktree's commit identity, the run's fingerprints, the restart
  of a run holding the previous owner's personal MCP servers, the terminal notice, the audit and
  the published summary. Taking control reuses it.
- The web UI: the task view's control row (`taskview.js`), the task menu (`actions.js`
  `openTaskMenu`, `status.js` `taskActions.askControl`), notifications (`notifications.js`), the
  Settings page (`pages/settings.js`) and the e2e mock (`mock.js`).
- The person's decision of 2026-11-14 ("not take control without the consent", ADR-0331) is
  reversed by this request for everyone who does not select the option; ADR-0365 records it.

## Requirements

- TKC-001: A person with write access to a project, an enabled account and no guest link takes
  control of another person's task there at once (`POST /api/tasks/{tid}/control/take`) when its
  owner has not chosen to be asked. The task then acts on GitHub as them exactly as after a
  hand-over (one conditional update; commit identity, fingerprints, MCP restart, notice, audit
  `task.control_taken`). A waiting request for control is dropped by the take.
- TKC-002: The previous owner becomes the giver (`control_from`) and may Take back while control
  has not moved since; they are told wherever they are (`task.control_taken` event to them only:
  a notification and a toast that opens the task). The task's terminal says
  "@to took control of this task from @from: it now acts on GitHub as @to. @from can take it back."
- TKC-003: Each person chooses in Settings whether teammates take control of their tasks or ask
  first: "Ask me before taking control of my tasks", off by default, kept on the server
  (`users.control_ask_first`, `PUT /api/me/control {ask_first}`, `Me.user.control_ask_first`).
  With it on, a take of their task is refused with 409 naming them and saying to request control;
  requests, hand-over, decline and withdraw work as before. With it off, requests still work.
- TKC-004: An owner who can no longer act never holds a task: a take or a request passes control
  at once whatever the owner chose (SPEC-070 R5, kept).
- TKC-005: Each task summary says whether its owner asks first (`owner_asks_first`). On someone
  else's task the task view's control row and the task menu offer **Take control** (after a
  confirmation naming the owner: it will act on GitHub as you, @owner is told and can take it
  back) when the owner does not ask first, and Request control / Withdraw / Ask again as today
  when they do. Disabled steering items say "@owner owns this task: take control to do this" (or
  "request control to do this"). The giver's row reads "@x has control of this task now" with
  Take back.
- TKC-006: Taking control is a person's own act: no agent, Superagent or MCP tool does it. The
  refusal for a non-owner's steering says to take control on the task's page (or ask, when the
  owner wants to be asked).
- TKC-007: Refusals: the owner taking their own task (409), a disabled account or one without
  write access (403/404), a guest (403), a task that changed meanwhile (409), a task starting with
  its owner's personal MCP servers (409, as for a hand-over).

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Owners choose in Settings whether teammates take control or ask first
- [x] Take control passes a teammate's task at once, its owner told and able to take it back
- [x] Task view and task menu offer Take control, or Request control when the owner asks first
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC1 (TKC-001, TKC-002): in `crates/yulius/tests/task_ownership.rs`, bob takes alice's task:
  it is bob's, `control_from` is alice, a waiting request of carol's is dropped, the audit has
  `task.control_taken`, alice gets `task.control_taken`, and alice takes it back.
- AC2 (TKC-003): with alice's option on, bob's take answers 409 naming alice and the task stays
  hers; bob's request still waits for her answer. The option round-trips through `PUT
  /api/me/control` and `GET /api/me`.
- AC3 (TKC-004): with alice's option on and alice disabled, bob's take passes control.
- AC4 (TKC-007): alice taking her own task, a reader, and a disabled bob are refused.
- AC5 (TKC-005, e2e): on someone else's task the control row and the menu show Take control; it
  confirms and the task becomes the viewer's; with the owner asking first they show Request
  control; the Settings toggle saves the option (no WCAG violations).
- AC6: store tests for the migration's default and the take's conditional update.

## Test plan

- Store: `crates/yulius-store/tests/store.rs`: the option defaults to off and is saved; a take
  passes control only from the expected owner, records the giver, drops a request.
- Integration: `crates/yulius/tests/task_ownership.rs` for AC1-AC4 through the HTTP API.
- e2e: `e2e/tests/ownership.e2e.ts` (mock) for AC5; existing ownership and watching specs updated
  to the new labels.
- `scripts/dev quick`; the whole gate is Yulius CI's.

## Decisions

- **The option is the owner's, on the server, default off.** "unless user selects different
  option": the person whose task would be taken is who may want consent, and the taker's page
  must know it, so a per-person server setting (not a browser preference). Default off, as the
  request makes taking the rule. Alternatives: per-task option (more clicks, nobody sets it in
  advance), per-project option (one person's preference decided for the team), taker chooses
  between take and ask (gives the owner no say) — rejected.
- **The previous owner can take it back.** A take is without consent, so the person who lost the
  task returns it with one click even when the taker asks first; this reuses `control_from`.
- **A confirmation before taking.** It moves whose GitHub name the task uses; Stop of someone
  else's task already confirms.
- **Requests stay** for owners who ask first; with the option off a request still works (an
  asker may prefer to ask), but the UI offers Take control.
- **Guests do not take.** A playground guest tidies only their own tasks (ADR-0331); taking the
  maker's task would be more than that.
