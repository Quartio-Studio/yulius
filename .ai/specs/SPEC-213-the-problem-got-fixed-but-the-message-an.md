---
yulius-task: a604d603-742b-4b73-bc40-be923fb2a698
status: draft
---

# SPEC-213: an answered secret request ends the task's "Needs your action"

Status: draft
Date: 2026-10-17
Owner: Yulius task #200
Scope authorization: the person's request (task #200), "the problem got fixed but the message and
status still were there, fix", with two screenshots. Fixes #633.

## Goal

When a person answers what a task's agent asked for on the Secrets page (a `.env` copy, a CD
secret, hosts for a secret), the task stops reading "Needs your action" as soon as nothing it asked
for still waits: it reads Working again, its terminal banner "The agent needs your attention" goes,
and it leaves the sidebar's Needs you list.

## Context

- Screenshots: the task header shows "Needs your action" and the terminal the banner "The agent
  needs your attention. Its question is in the terminal below.", while the terminal shows the agent
  saying "The key copy has been approved", working on for nearly four minutes (`Processing… 3m
  52s`); the sidebar keeps the task under Needs you ("waiting 3m").
- Symptom → cause. `secret_copy_to_env`, a CD run's secret use and `secret_request_hosts` call
  `flag_secret_wait` (`crates/yulius-app/src/vault/users.rs`) when a secret waits for a person:
  the task becomes `needs_attention`, and the tool tells the agent to print the link and go on with
  other work. The person answers with `answer_env_files` (`vault/env_files.rs`) or
  `answer_host_request` (`vault/requests.rs`); neither touches the task. The agent's later work
  does not either: `activity_on_work` (`finish.rs`, SPEC-032 STA-003) only lifts a flag raised by
  a Claude Code notice (`notice_attention_ms`), deliberately not one raised by a request. So the
  flag lasts until the turn's Stop settles the task, minutes later in a goal session.
- Why the tests missed it: `secrets_env.rs` and `secrets.rs` assert that the request flags the
  task, and that the answer lets the copy or the hosts through, but never read the task's activity
  after the answer.
- The askers of a request are kept on the secret as text (`env_requested_by`, `requested_by`:
  `#N`, `#N in <slug>` for an instance-wide secret, or `<login>'s Superagent`, joined by `, `;
  `requester()` in `vault/requests.rs`). A task's activity changes stamp `state_at`.
- What else the cause could break: any other flag raised for a request a person answers outside
  the terminal. The browser request (`browser_request_user_action`) is the only other one; the
  person acts in the tab and the agent goes on, which already reads as the agent's work in the tab;
  it is not part of this fix.

## Requirements

| ID | Requirement |
| --- | --- |
| SRA-001 | When a person answers a secret's `.env` request (allow or dismiss) or its host request (allow or dismiss), every running task of the secret's sandbox (every sandbox for an instance-wide secret) whose `needs_attention` was raised by a secret wait, and that has no other request still waiting, reads Working again, published to the UI at once. |
| SRA-002 | A task with another `.env` or host request still waiting (on this or another secret) stays `needs_attention`. |
| SRA-003 | A `needs_attention` flag that was already set when the secret wait came (a permission prompt, the gate, a browser request), or that was replaced since (the activity changed after the secret wait), is not cleared by an answer. |

## Plan

- [ ] Reproduce the defect and find its root cause
- [ ] Write the fix spec and the regression test that fails
- [ ] Fix the cause until the regression test and every gate pass
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- After a person allows the only secret a task asked to copy into `.env`, the task's activity is
  `working` (SRA-001); after they dismiss it, likewise.
- After a person allows the hosts a task asked for, the task's activity is `working` (SRA-001).
- A task that asked for two secrets stays `needs_attention` after the first answer and is
  `working` after the second (SRA-002).
- A task flagged by something else before it asked keeps its flag after the answer (SRA-003).

## Test plan

- `crates/yulius/tests/secrets_env.rs`: a new test drives the MCP tool and the HTTP answer for two
  secrets (SRA-001, SRA-002), dismissal included, and a task flagged before it asked (SRA-003);
  the existing copy test reads the activity after its answer.
- `crates/yulius/tests/secrets.rs`: the host request test reads the activity after the answer.
- The tests fail on `dev` before the fix (the task stays `needs_attention`).

## Decisions

- **Clear at the answer, not at the agent's next work.** The agent is told to go on with other
  work right after asking, so its next work says nothing about the person; the answer does, and it
  clears the banner at once, even while the agent sits in a long tool call. Alternatives: arm
  `notice_attention_ms` at the flag (the flag would vanish within seconds, before anyone saw it),
  or at the answer (the banner would wait for the agent's next line).
- **Which tasks: by state, not by parsing the askers.** At an answer the server looks at the
  running tasks of the secret's sandbox (all sandboxes for an instance-wide secret) flagged by a
  secret wait, and for each checks whether any agent secret still waits for it (its `#N`, or
  `#N in <slug>`, among the askers of a waiting request). That needs no new mapping from the
  askers' text back to tasks and covers every request kind with one rule.
- **Which flag is the secret wait's: the runtime remembers it.** `flag_secret_wait` records, in
  the task's runtime, the `state_at` its flag got, only when it raised the flag itself (the task
  was not already `needs_attention`). An answer clears only a flag whose `state_at` still matches,
  so a flag replaced since, or raised by another cause, stays (SRA-003). The runtime is memory: after
  a Yulius restart the flag ends at the turn's Stop, as before this fix. A stored column would
  survive restarts but costs a migration for a few minutes of a banner; not worth it.
- No ADR: this restores the documented meaning of the flag (SPEC-129: the task is flagged while it
  waits for a person) and changes no contract.
