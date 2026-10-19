---
yulius-task: 650b7384-9fa7-4049-bb69-b663c3cfa6cd
status: in-progress
---

# SPEC-230: The Superagent sends messages to tasks

Status: in-progress
Date: 2026-10-19
Owner: Yulius task #216
Scope authorization: task #216: "superagent should be able to prompt tasks" (screenshot: the
person tells their Superagent "you can use the budget from DO"; the Superagent answers that the
tasks "don't know yet. Their prompts are fixed, and #4 and #5 are still queued, so I can't message
them until they start", and that once they run it could only message them as other Claude Code
sessions on the machine, which Yulius does not provide).

## Goal

When a person asks their Superagent to tell a task something ("tell #5 it may use the DO budget"),
the Superagent does it in one tool call, `task_message`, whatever state the task is in that can
take a message: a task that has not started gets it in its request (the queued message a person
adds in the task view), a running task gets it typed into its session as the person would type it.
A task that cannot take it says why, so the Superagent can tell the person what to do instead.

## Context

- Queued messages (SPEC-052, `App::queue_task_message`): a person adds a message to the request of
  a task that never started (queued, chained, prepared, waiting for capacity, no run and no
  session); it is refused once the task started ("type into its session instead").
- A running task's session is a Claude Code TUI in a PTY (`App::send_input`). People type into it
  from the task view (`terminal.rs`, which records who typed, `record_typing`). Yulius itself types
  into it: the first message (`type_opening`, bracketed pastes then Enter), the goal (`spawn_goal`)
  and `/compact` for a full context (`context_full.rs::type_line`). Each leaves a person's draft
  alone and sends Enter separately, 300 ms after the text, so the TUI sees typed text and then a
  submit. Claude Code takes a message submitted while it works and gives it to the agent as soon
  as it can (queued messages), so a busy agent needs no wait.
- The Superagent's management tools (SPEC-037, SPEC-066, ADR-0029, ADR-0049) act for its person
  (`ProjectCaller`, `manage_context`), resolve a task by `task_id` or `number`
  (`agent_task_ref`), and are audited with the caller (`cockpit:<login>`). New tools reach open
  Superagents through the launch digest (they restart once idle). Tools not in
  `COCKPIT_READ_ONLY_TOOLS` are asked about on an uncontained server (ADR-0165).
- `needs_you` (SPEC-134): a prompt a person types into the task ends the task's open request.

## Requirements

- TMS-001 (tool): `task_message {task_id | number, text}` is served to the Superagent only (not to
  task agents) and is not among the tools it runs without asking.
- TMS-002 (not started): a task that has not started takes the message into its request exactly as
  the task view's queued message does (same rules, size bound and wording, as the Superagent's
  person). The answer says `queued`.
- TMS-003 (running): a task that is `running` with a live run gets the message typed into its
  session: one bracketed paste of `Message from <login> (through their Superagent):` and the text,
  then a separate Enter. The answer says `sent`. Control characters other than line breaks and
  tabs are removed first (no escape sequence reaches the TUI, nor the end of the paste); CRLF
  becomes LF.
- TMS-004 (refusals): nothing is typed, and the answer says why, when the text is empty or longer
  than 64 KiB, the task is the caller's own (not possible for a Superagent, kept for safety), a
  person typed into the task's terminal in the last minute (their draft is theirs), Yulius is still
  typing the task's first message, another message to the task is being typed, or the task is
  neither waiting to start nor running (stopped, done, failed, starting, switching, paused): it
  names the state and what to do (continue the task, or create a new one).
- TMS-005 (authority and audit): the person needs write access to the project (as for the task
  view); each sent or queued message is audited `task.messaged` with the caller and the task, never
  the text.
- TMS-006 (answers what the task asked): a message typed into a running task ends the task's open
  `needs_you` request, as a person's typed prompt does.
- TMS-007 (instructions): the Superagent's MCP instructions say to use `task_message` when the
  person wants a task told something, only when asked.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] A message typed into a running task's session reaches its agent, guarded
- [ ] task_message is served to the Superagent, routes queued and running tasks, audited
- [ ] Docs: ADR, architecture, API, product and the Superagent's instructions name the tool
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-1: A running task (real PTY host, fake agent) given a message by `task_message` echoes
  `Message from admin (through their Superagent):` followed by the text, multi-line text included;
  the answer says `sent`, and the audit holds `task.messaged` without the text.
- AC-2: Escape sequences and other control characters in the text never reach the session (an
  embedded `ESC[201~` does not end the paste).
- AC-3: A person who typed into the running task within the last minute makes `task_message`
  refuse, and nothing is typed.
- AC-4: A queued task given a message has it in its request under "Added by <login> before the
  task started"; the answer says `queued`.
- AC-5: A stopped or done task refuses with its state named; an empty text is refused; a task of
  another project is not found.
- AC-6: `task_message` is in the Superagent's tools, not a task agent's, and not among
  `COCKPIT_READ_ONLY_TOOLS`.
- AC-7: A running task with an open `needs_you` has it ended by a sent message.

## Test plan

- Unit (`crates/yulius-app/src/task_message.rs`): AC-2's sanitising of the text and the paste
  framing.
- Integration, real processes (`crates/yulius/tests/task_message.rs`, the fake agent through the
  PTY host, as `context_full.rs`): AC-1, AC-3, AC-7.
- Integration (`crates/yulius/tests/task_message.rs`, the in-memory harness): AC-4, AC-5.
- Integration (`crates/yulius/tests/manage_tools.rs`): AC-6.

## Decisions

- **One tool for both states.** The Superagent should not need to know whether a task has
  started; the state decides between the request and the session, and the answer says which.
  ADR-0178.
- **Type into the session, as a person would.** Alternatives: a hook that adds the message as
  context at the agent's next tool call or Stop (never reaches an idle session, which no hook
  wakes), or Claude Code's agent messaging (only local sessions the Superagent can see, not
  Yulius tasks). Typing reaches idle and busy sessions alike, and Claude Code queues it while the
  agent works.
- **The Superagent only.** Task agents prompting each other would let one task's content steer
  another without a person; the person's own Superagent is the person speaking.
- **A person's draft wins.** As for the goal and `/compact`, a person who typed in the last minute
  owns the prompt box; the Superagent is told to try later rather than mixing its text into theirs.
- **The text is labelled.** The agent and anyone reading the transcript see the message came from
  the person through their Superagent, not from the task's own flow.

## Evidence

To be completed.
