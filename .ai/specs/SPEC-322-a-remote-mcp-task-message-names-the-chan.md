---
yulius-task: 06524c84-a27d-49ae-8bfd-6558cbdd3b25
status: in-progress
---

# SPEC-322: A remote MCP task_message names the channel it came through

Status: in-progress
Date: 2026-10-26
Owner: Yulius task #307
Scope authorization: task #307 (`/goal`), fixes #789. Amends
[SPEC-230](SPEC-230-superagent-should-be-able-to-prompt-task.md) TMS-003 for the remote MCP of
[SPEC-247](SPEC-247-drive-yulius-fully-from-a-local-agent-cl.md); the Superagent's label of
[ADR-0178](../../docs/decisions/0178-the-superagent-messages-tasks-by-queueing-or-typing.md) is
unchanged.

## Goal

A message typed into a running task through the remote MCP (a person's own agent, with an
`operate` token) is labelled with that channel, `Message from <login> (through their own agent):`,
instead of naming the Superagent, which was not used. The Superagent's label stays as it is.

## Context

- **Symptom.** A laptop agent calls remote `task_message {project, number: 12, text}` on a
  running task. Task #12's agent reads `Message from alice (through their Superagent):`. The audit
  actor (`remote:alice`) is right; only the label the agent reads is wrong.
- **Root cause.** `message_paste(login, text)` (`crates/yulius-app/src/task_message.rs`) hard-codes
  "(through their Superagent)", and `type_task_message` receives only the login. The remote MCP
  (`remote.rs`, `"task_message"`) calls the same `App::tool_task_message` with a `ProjectCaller`
  whose only trace of the channel is its audit `detail` (`{"via": "remote", …}`), which nothing
  reads for the label. The comment before `end_needs_you` also assumes the Superagent.
- **Why tests missed it.** The remote MCP test (`remote_mcp.rs`,
  `a_repository_is_onboarded_end_to_end_from_a_laptop`) only reaches a task that has not started
  (`queued`); the typed label is tested only with a Superagent caller.
- **Callers.** `ProjectCaller` is built in four places: a task's agent (`AgentContext::caller`),
  a person's Cockpit, which is their Superagent (`cockpit_caller`), and the remote MCP
  (`remote_call`). Task agents are never served `task_message` (TMS-001).

## Requirements

- RML-001: `ProjectCaller` names its channel explicitly (a typed `channel` field set where each
  caller is built: task agent, Superagent, remote MCP), not read back from its audit detail.
- RML-002: a message typed into a running task through the remote MCP is one bracketed paste of
  `Message from <login> (through their own agent):`, a blank line and the text, then Enter.
- RML-003: through the Superagent (the Cockpit) the label stays
  `Message from <login> (through their Superagent):`.
- RML-004: everything else about the message (refusals, audit, `needs_you` ending, queued
  messages) is unchanged; a message typed through either channel ends the task's `needs_you`.
- RML-005: `docs/api.md`'s `task_message` row, SPEC-230 TMS-003 and the product summary state both
  labels.

## Plan

- [x] Spec records the remote label and the channel it is read from
- [ ] A remote MCP message to a running task names the person's own agent
- [ ] Docs state both labels
- [ ] Independent review findings answered
- [ ] Gates green and evidence recorded

## Acceptance criteria

- AC-1 (RML-002): an integration test sends `task_message` through the remote MCP
  (`App::remote_mcp_handle`, `tools/call`, an `operate` token's caller) to a running task, and the
  task's screen shows `Message from admin (through their own agent):` and the text; the answer
  says `sent`, the audit actor is `remote:admin` and the task's `needs_you` ends. It fails on dev
  91785a67.
- AC-2 (RML-003): the existing Superagent test still sees `(through their Superagent):`.
- AC-3 (RML-001, RML-002, RML-003): the unit test of `message_paste` checks both labels.
- AC-4 (RML-005): the documents state both labels.

## Test plan

- Integration (`crates/yulius/tests/task_message.rs`, real processes: supervisor → ptyhost → fake
  agent): `a_remote_message_names_the_persons_own_agent` covers AC-1, written first and run
  against the unfixed code; `a_running_task_gets_the_message_typed_into_its_session` covers AC-2.
- Unit (`crates/yulius-app/src/task_message.rs`): `a_message_is_one_labelled_bracketed_paste`
  covers AC-3.
- `scripts/dev ci` (the gate) and `scripts/dev security`.

## Decisions

- **An explicit channel on `ProjectCaller`, not `detail["via"]`.** The audit detail is free-form
  JSON for histories; reading behaviour back from it would couple the label to audit wording and
  make a missing key silently fall back. A `Channel` enum (`Task`, `Superagent`, `Remote`) is set
  by the code that authenticates each caller, so the compiler makes every new caller choose one.
- **Wording "their own agent".** It names what the remote MCP is for (SPEC-247: "Connect your
  local agent", a person's own coding agent on their computer) without naming a product (Claude Code,
  Codex) Yulius cannot verify. Suggested by the issue.
- **The Cockpit keeps "their Superagent".** The Cockpit is the Superagent (its caller's actor is
  `cockpit:<login>`; ADR-0178), so its label already names the channel used: no third label.
- **Task agents get a label too.** They are never served `task_message` (TMS-001), but the
  `Task` channel needs a wording for `message_paste` to stay total: `their task's agent`. No ADR:
  the choice is local to this tool and recorded here and in ADR-0178's amendment note.
