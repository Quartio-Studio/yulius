# 0178. The Superagent messages tasks by queueing or typing

## Status

Accepted. Specified in [SPEC-230](../../.ai/specs/SPEC-230-superagent-should-be-able-to-prompt-task.md);
task #216. Amends SPEC-037 MGT-006 (one more Superagent tool) and extends SPEC-052 (queued
messages) to the Superagent.

## Context

A person told their Superagent "you can use the budget from DO", meaning two tasks it had
created. The Superagent could not pass it on. Two of the tasks were still queued, so their
requests were fixed as far as it could tell. It could only offer to message them later as other
Claude Code sessions on the machine, which Yulius tasks are not. People can already add a queued
message to a task that has not started (SPEC-052) and type into a running task's terminal. The
Superagent, which acts for that person, could do neither.

## Decision

- **One tool, `task_message {task_id | number, text}`.** The task's state picks how the message
  arrives, and the answer says which (`delivered: queued | sent`):
  - a task that has not started (no run and no session yet) gets it in its request, through
    `App::queue_task_message`, as the person's queued message;
  - a `running` task gets it typed into its session. That is one bracketed paste,
    `Message from <login> (through their Superagent):`, a blank line and the text, then a
    separate Enter 300 ms later. Claude Code queues a message submitted while the agent works,
    so it arrives mid-turn as well as when the session is idle.
  - every other state is refused, with the state named and what to do instead.
- **Only the Superagent.** The tool is not in `tool_descriptors()`, so task agents are not served
  it. Tasks do not prompt each other: a person speaks to their tasks through their own
  Superagent. It is not among `COCKPIT_READ_ONLY_TOOLS`, so on an uncontained server Claude Code
  asks the person first (ADR-0165).
- **Nothing is typed over a person.** A person who typed into the terminal in the last minute
  (`typed_recently`) owns the prompt box, as for the goal and `/compact`. So does Yulius while it
  types the first message (`typing_opening`), and another message being typed (one per run, a
  flag cleared on drop). In each case the call is refused, not delayed.
- **Clean text.** CRLF and CR become LF. Every control character except LF and tab is removed, so
  an ESC can neither end the paste nor drive the TUI. The text is trimmed, at most 64 KiB, and
  never empty.
- **Write access, audited without the text.** The call acts for the Superagent's person
  (`manage_context`). It is audited `task.messaged` with the caller and `delivered`. A typed
  message ends the task's open `needs_you`, since the person answered through their Superagent.

## Alternatives

- **Add the message as hook context** (`additionalContext` at the next tool call, or a Stop-hook
  block). An idle session runs no hook, so the message would wait until someone typed anyway.
  Rejected.
- **Claude Code's own agent messaging.** It reaches local sessions the Superagent can list, not
  Yulius tasks in their sandboxes, and it bypasses Yulius's attribution and audit. Rejected.
- **Wait for the person's draft, then type.** That would need a background typist with its own
  bounds and give no answer to report. Refusing tells the Superagent to try again or tell the
  person. Rejected for now.
- **Serve it to task agents too.** One task's content could steer another without a person.
  Rejected; a task that needs another task's output waits for it (`after_task`, start rules).

## Consequences

- The Superagent can answer the screenshot's case directly. "Tell #4 and #5 they may use the DO
  budget" queues the line into the waiting tasks and types it into the running ones.
- An open Superagent gets the tool after an update by restarting once it is idle (the launch
  digest covers tool descriptors).
- Typing into a session that shows a dialog puts the text into the dialog. Tasks run
  without approval prompts, so this is limited to Claude Code's own dialogs. Yulius does not
  type while the first message is pending. In the first seconds of a goal-mode run, a message
  can still arrive before the goal is typed. The goal then follows it, so both reach the agent.
