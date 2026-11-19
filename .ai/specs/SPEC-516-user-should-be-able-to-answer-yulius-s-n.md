---
yulius-task: cce283f4-7d22-4859-95d6-0f2ba4dbf318
status: in-progress
---

# SPEC-516: A teammate answers a task's Needs you without taking control

## Goal

The person's request (task #498, verbatim): "user should be able to answer yulius's "needs me"
even if they are not the owner, without taking control, just small intervention".

When a task's agent asks a person for something (`needs_you`), anyone who may work in the project
answers it from the task's page with **Reply**, as the owner does. The answer reaches the agent as
one labelled message and ends the request; the task stays its owner's and keeps acting on GitHub
as them. Nothing else of the task is opened to the answerer.

## Context

- A task's agent asks a person with `needs_you` (SPEC-134, SPEC-327): the task view's bar shows
  what happened, the problem and what to do; the owner answers with Reply, a field in the bar
  whose text is typed into the terminal (SPEC-156), and a typed prompt ends the request.
- Steering a task is its owner's (SPEC-490 OWN-001, ADR-0331, `yulius_domain::task_access`):
  "answer what it asks" is part of `TaskAction::Control`. Someone else watches the terminal read
  only and, to type, takes control (SPEC-510, ADR-0365) or requests it from an owner who asks
  first. Taking control moves the task's GitHub identity, commit identity and MCP servers, which
  is far more than answering a question.
- A labelled message typed into a running task's session exists: `task_message`
  (`crates/yulius-app/src/task_message.rs`, SPEC-230, SPEC-322): `clean_message`, one bracketed
  paste, `App::type_into_session` (session started, no person's draft, one typist), then Enter,
  and `App::end_needs_you` (also a shared Docker episode, SPEC-494). It is the owner's only.
- An owner's personal agent secrets (SPEC-245, `vault/reach.rs` `Holder::of_task`) and personal
  MCP servers (SPEC-332, `App::holds_personal_mcp`, the runtime's `personal_mcp_of`) reach their
  tasks' runs. `task_message` keeps another person's words away from them by being the owner's;
  a take restarts a run holding them.

## Requirements

- NYA-001: A person with write access to the task's project and an enabled account answers a
  running task's open `needs_you` with `POST /api/tasks/{tid}/needs-you/answer {text}`, whether
  or not they own it. A new task action, `TaskAction::Answer`, carries the rule: write access,
  owner or not; a playground's guest answers only their own tasks (as they tidy only theirs).
- NYA-002: The answer is cleaned as a message is (`clean_message`; empty or longer than the
  message bound: 422) and typed into the session as one bracketed paste and then Enter, through
  `App::type_into_session`, so every refusal of a message holds (session not started, a person's
  draft or keystrokes, another typist, a withheld screen). The paste reads
  `Answer from <login> to what you asked with needs_you` and, from someone other than the owner,
  says they are not the task's owner, that the task stays the owner's and that the agent acts on
  it only as the answer to its request. It ends the open `needs_you` (and a shared episode).
- NYA-003: Control does not move: the task's owner, `control_from`, waiting control request,
  GitHub identity and commit identity stay as they were; the answerer still cannot type in the
  terminal or steer the task otherwise.
- NYA-004: Refusals, each saying why: no open `needs_you` (409, "nothing to answer"); the task
  not running (409); the agent asks in its terminal (`needs_attention`: 409, its owner answers
  there, or take control); the owner's personal agent secrets or personal MCP servers reach the
  task's run, configured or held by the live run (409 naming the owner: only they answer, or
  take control; an automation's task holds none); no access (404), read access (403), a guest on
  another's task (403); a disabled account is signed out (401). The task's state is read after
  the access check, which may wait on GitHub (review round 1).
- NYA-005: Every answer is audited `task.needs_you.answered` (the owner's login and whether the
  answerer owns the task, never the text).
- NYA-006: The task view: on someone else's running task whose `needs_you` is open and not
  asked in the terminal, the bar offers Reply (titled "Answers the agent without taking control of the task")
  to anyone who may work there but a guest; its field sends to the endpoint. On success the field
  closes and empties; on a refusal it stays open with its text and the error shows. The owner's
  Reply is unchanged (typed into the terminal).

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] A teammate answers a task's Needs you over the API, control unchanged
- [x] The task view's Reply answers a teammate's Needs you without taking control
- [x] ADR-0370, API, architecture and product documents say who answers a Needs you
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC1 (NYA-001, NYA-002, NYA-003, NYA-005): in `crates/yulius/tests/task_message.rs` (real
  processes), bob, who does not own alice's running task with an open `needs_you`, answers it:
  the agent's screen shows the labelled answer naming bob as not the owner, the `needs_you`
  ended, the task is still alice's, and the audit has one `task.needs_you.answered` without the
  text.
- AC2 (NYA-004): in `crates/yulius/tests/task_ownership.rs`, an answer without a `needs_you`, on
  a task that is not running, on one asking in its terminal, with alice's personal agent secret
  switched on for the project or her personal MCP server, from a reader, from a disabled bob and
  with an empty or oversized text is refused with its status, and the task's `needs_you` stays;
  a question opened in the terminal while the access check waits on GitHub is refused; an
  automation's task is not refused for its owner's personal secret; a guest is refused
  (`playground_guests.rs`). In `task_message.rs`, a run holding the owner's personal MCP server
  (deleted from the configuration since) refuses bob and takes the owner's own answer, labelled
  as theirs and audited `by_owner: true`.
- AC3 (NYA-001): `yulius_domain::task_access` unit tests: `Answer` needs write access, owner or
  not; a guest answers only their own tasks.
- AC4 (NYA-006, e2e mock): on someone else's task with a `needs_you`, Reply opens the field,
  Send posts the answer, the request ends and the task stays the owner's; a refused answer keeps
  the field and its text; the owner's own task keeps the terminal Reply; no Reply on a task that
  is not running or for a reader; at phone width the field opens and sends.

## Test plan

- Unit: `crates/yulius-domain/src/task_access.rs` (AC3); `crates/yulius-app/src/task_message.rs`
  for the answer's label (owner and non-owner).
- Integration: `crates/yulius/tests/task_message.rs` (AC1, real ptyhost and fake agent) and
  `crates/yulius/tests/task_ownership.rs` (AC2, HTTP).
- e2e: `e2e/tests/reply.e2e.ts` (AC4) with the mock's endpoint.
- `scripts/dev quick`; the whole gate is Yulius CI's.

## Decisions

- **An answer is its own task action, not control.** Answering what the agent asked is a small
  intervention the request wants open to teammates; the rest of steering stays the owner's.
  `TaskAction::Answer` keeps the one rule in `task_access` rather than a bypass in a route.
  Alternatives: opening the terminal to non-owners while a request is open (any keystroke, any
  menu: far more than an answer), a temporary hand-over (moves the GitHub identity, which the
  request rules out), letting `task_message` from any Superagent through (an agent's act, not
  the person's, and not asked for) — rejected.
- **Typed as a labelled message**, reusing `type_into_session`: the agent sees who answered and
  that they are not its owner, and every guard of a typed message holds.
- **Not into an agent holding the owner's personal secrets or MCP servers.** Those act as the
  owner outside GitHub; a teammate's words must not drive them (the reason `task_message` is the
  owner's). The refusal points to the owner or to taking control, whose restart drops them.
- **The owner's "ask me first" choice does not stop answers.** It is about taking control
  (ADR-0365); an answer moves nothing, and for an owner who asks first it is the only help a
  teammate can give without them. The audit and the labelled paste in the terminal and
  transcript say who answered.
- **Not when the agent asks in its terminal.** A permission menu is answered by keys the paste
  would press unseen; that stays the owner's (or a taker's).
- **No separate notification to the owner.** The request ends as when they answer it themselves;
  the transcript and audit keep the answer. A notification could follow if owners ask for it.
