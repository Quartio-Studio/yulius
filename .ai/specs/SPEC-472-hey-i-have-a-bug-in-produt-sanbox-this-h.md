---
yulius-task: 3ce7fca4-c720-4005-ab26-1fb3c4348632
status: in-progress
---

# SPEC-472: A task flagged "needs attention" says why, and Reply shows where the answer goes

Status: in progress
Date: 2026-11-12
Owner: Yulius task #455
Scope authorization: the person's request of 2026-11-12 ("I have a bug in product sandbox, this
happened and I can't click reply and don't see any question. Please find root cause and fix it"),
with a screenshot of a task of the `product` sandbox on the running instance. Issue
[#1221](https://github.com/Quartio-Studio/yulius/issues/1221).

## Goal

Whoever opens a task that reads "Needs your action", at any time, reads in its bar why it was
flagged and what to do. Only a flag raised by Claude Code's own question says that the question is
in the terminal. Reply shows that the terminal took the keyboard.

## Context

- Symptom (the screenshot, 2026-11-12 07:27): the task's strip reads "Needs your action" and
  "Goal"; the bar over the terminal reads "The agent needs your attention. Its question is in the
  terminal below." with Reply. The terminal under it shows the agent's ordinary work (a thinking
  summary, "… · summary"), no menu and no question. Clicking Reply changes nothing on screen.
- The running instance is on `main` (`ecc21e884`); the code paths below are the same on `dev`.
- How the bar is worded: `renderAttention` in `crates/yulius-server/web/assets/taskview.js` shows
  that sentence for every running task whose activity is `needs_attention` and that has no
  `needs_you` request and no browser request. Its Reply (`terminalReply`) calls
  `terminal.focus()`; Claude Code hides the terminal's cursor and draws its own, so the page looks
  the same after the click (SPEC-156 kept this for a question in the terminal, where the answer is
  a menu choice).
- Root cause: one flag, many causes, and the cause is not kept. Claude Code's notice (a
  permission menu, a question: `NOTICES_ASKING_A_PERSON` in `crates/yulius-app/src/hooks.rs`) is
  one way a task becomes `needs_attention`, and the only one that puts a question on the
  terminal. Yulius raises the same flag itself, with no question there:

  | Where | Why | Said how |
  | --- | --- | --- |
  | `hooks.rs` `hook_stop` | the Stop gate gave up after `MAX_GATE_BLOCKS` blocks | notice |
  | `supervisor.rs` `goal_not_taken` (five callers) | the task's goal was never set | notice |
  | `supervisor.rs` `TRUST_UNANSWERED` (two callers) | Claude Code still asks whether to trust the folder | notice |
  | `supervisor.rs` `opening_never_sent` | a restart cut the typing of the first message | notice |
  | `context_full.rs` `give_up_on_full_context` | a full context could not be recovered | notice (not when re-flagged) |
  | `supervisor.rs` (a run's host could not be terminated) | the session could not be ended | notice |
  | `vault/users.rs` `flag_secret_wait` (four callers) | the agent asked for a secret; a person answers on the Secrets page | nothing |

  A notice is `Hub::notice` (`crates/yulius-app/src/hub.rs`): a frame broadcast to the viewers
  connected at that moment, which the page shows for 6 s (`showNotice` in `terminal-core.js`).
  Nothing is stored. Whoever opens the task later, or looks away for 6 s, gets the bar's sentence
  about a question that does not exist. Two of the paths run at a start of Yulius, when no viewer
  is connected at all, so nobody ever reads their notice. The task row's second line
  (`activityLine` in `status.js`) assumes the same: "Approve or answer in the terminal".
- Why the flag stands over ordinary work: the agent's work lifts only a flag a notice raised
  (`activity_on_work`, STA-003). After the gate gave up, a goal session goes on (Claude Code's
  goal check continues the turn) and the flag stays until a clean Stop or a typed prompt; a
  secret request's flag stays until a person answers it (SPEC-213), while the agent "goes on with
  other work" as the tool tells it.
- Which path the screenshot's task took cannot be read from here (another sandbox's data is not
  visible to this one). A goal session at work under the flag fits the gate and a secret request;
  the fix does not depend on which.
- Reproduced on `dev` (`92ba83cc2`) with a probe test in `crates/yulius/tests/agent_hooks.rs`:
  after the gate's fourth Stop the task reads `needs_attention`; a receiver subscribed before got
  "… stopped 3 times without fixing …"; a viewer that subscribes afterwards gets no notice; the
  task's summary has `needs_you: null` and nothing else that says why.
- Why the tests missed it: the tests of these paths assert the flag and, where there is one, the
  notice on a receiver subscribed before it. None opens the task afterwards. The e2e tests set
  `activity: 'needs_attention'` on a mock task and expect the terminal sentence.
- Earlier work on the same bar: SPEC-156 (Reply opens a field for a text answer; a question in
  the terminal keeps focusing it), SPEC-213 (an answered secret request ends its flag; its
  screenshots show this very bar over a working agent), SPEC-424 (a click is not lost to a
  rebuild). Requests Yulius opens itself (a goal pause, no subscription left, a host move) already
  say what to do, as `needs_you` (ADR-0304).
- What else the cause could break: every reader of `needs_attention` that assumes a terminal
  question. In the web UI these are the bar and the row's second line; both are in this change.
  The server's own readers (`AttentionFacts`, the pull request watch, the outage wake-up) only ask
  whether a person must act, which stays true.

## Requirements

- ATN-001: a `needs_attention` flag keeps its reason with the task: `kind` (`terminal`: Claude
  Code asks in the task's terminal; `yulius`: Yulius flagged the task itself), `problem` (what is
  wrong, one line), `action` (what the person does) and `link` (a page of this Yulius where they
  do it). It is written by the statement that raises the flag (`tasks.attention`, migration 0053)
  and survives a restart of Yulius.
- ATN-002: a reason speaks only for the flag it was written with: the task reads
  `needs_attention` and has not changed state since. Once the task works again, settles, ends, or
  is flagged anew by a path that gives no reason, the old reason is not shown. A flag raised on
  a task that is already flagged changes neither the flag (as before) nor its reason.
- ATN-003: the task's API view carries the reason as `attention` (`{kind, problem, action,
  link}` or null) wherever a task summary is sent: the task lists, a task's detail and the
  `task.updated` event.
- ATN-004: each flag Yulius raises itself gives its reason, with the words of its notice:

  | Flag | Problem | To do |
  | --- | --- | --- |
  | The gate gave up | The agent stopped 3 times without fixing: (the gate's reasons) | Tell it how to go on, in the terminal. |
  | The goal was never set | (why). The task has no goal yet. | Type `/goal` and the request in the terminal, or stop and continue the task. |
  | The folder-trust question at a first message | Claude Code still asks whether to trust this folder, so Yulius typed nothing. | Answer it in the terminal, then type the task's first message. (Or: the task starts by itself once it is answered.) |
  | A restart cut the first message | Yulius restarted while it typed this task's first message, so the first message was never sent and may be half typed in the prompt box. | Complete it (…) and press Enter. (A goal task: then type `/goal` and the request.) |
  | A full context | (why). | Find what fills it (…), then type `/compact` or `/clear` in the terminal, or stop and continue the task. |
  | The session could not be ended | Yulius could not end this session ((error)). It still counts as running, so the task cannot be deleted. | Stop the task again to retry. |
  | A secret request | The agent asked for a secret, and a person has to answer its request. | Answer it on the Secrets page. (link: the sandbox's Secrets page, at the secret's tab) |

- ATN-005: a flag raised by a Claude Code notice has kind `terminal` and Claude Code's own words
  as its problem. Every problem and to-do is one line of at most 400 characters (a longer one
  is cut and ends in `…`), control characters and runs of whitespace as one space.
- ATN-006: the notices stay as they are, for the viewers connected when the flag is raised.
- ATN-007: the task view's bar shows a `yulius` reason as a request is shown (SPEC-327): "Needs
  you", Problem, To do, an Open link when the reason has one, and Reply for the owner. A
  `terminal` reason reads "The agent needs your attention. Its question is in the terminal
  below.", with Claude Code's words when given. A flag without a reason (raised before this
  change, or by a path whose own card says it) reads "The agent needs your attention. Look at its
  terminal below.": it no longer claims a question. A `needs_you` request and a browser request
  keep their place before the reason.
- ATN-008: the task row's second line says a `yulius` reason's to-do (its problem when it has
  none) instead of "Approve or answer in the terminal".
- ATN-009: Reply that gives the terminal the keyboard says so where the person looks: the
  terminal's notice reads "The terminal has the keyboard: answer there." A terminal that refuses
  input says why instead (read-only), as it does for typing. Phones keep the quick input.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] A flag keeps its reason: the store column, the API field and the gate's flag
- [x] Every flag Yulius raises says why, and a notice's flag carries Claude Code's words
- [ ] The task view shows the reason and Reply says the terminal took the keyboard
- [ ] Docs, ADR-0306 and the changes entry true to the fix
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- The new integration tests fail on `dev` (the task's view has no `attention`) and pass with the
  fix, one per flag of ATN-004 and one for a notice.
- A task flagged by the gate, read after a restart of Yulius, still says why.
- A task that worked again after its flag, and one flagged anew without a reason, show no reason.
- The new e2e tests fail on `dev` (the bar reads the terminal sentence for a `yulius` reason; no
  notice after Reply) and pass with the fix; the existing reply, watching and needs-you e2e tests
  pass.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- `crates/yulius-store`: the flag and its reason are one statement; a reason is read only while
  the task reads that flag (after another activity, after a flag raised without a reason, for a
  task that is not running, and for a row that is not JSON it is not); a flag from a state the
  task left is not raised.
- `crates/yulius/tests/agent_hooks.rs`: the gate's flag says why to a viewer that comes later
  and after a restart of Yulius (a second app on the same database); the reason goes when a
  prompt is typed; a notice's flag is `terminal` with Claude Code's words; a notice on a task the
  gate flagged keeps the gate's reason.
- `crates/yulius/tests/first_message.rs`, `context_full.rs`, `task_delete.rs`, `secrets_env.rs`,
  `secrets_copy.rs`, `secrets.rs`, `cd.rs`: the existing test of each flag also reads the task's
  `attention` (it fails on `dev`).
- e2e `reply.e2e.ts`: a `yulius` reason shows as Needs you with Problem, To do, Open and Reply; a
  `terminal` reason shows the sentence and Claude Code's words; no reason shows the neutral
  sentence; Reply shows the terminal's notice; a read-only terminal says so; the row's second
  line. The page has no axe violations with a reason shown.
- Mutation checks: each part of the fix is removed in turn; each must fail a test (Evidence).

## Decisions

- The reason is the flag's, not a request. A `needs_you` request stays "whatever the agent does
  meanwhile" and has its own ends (ADR-0304); a flag ends at a typed prompt, a clean Stop or its
  answer. Opening a request for each flag would need a new end for each, and would replace the
  agent's own open request (one per task). A reason bound to the flag needs no new lifecycle.
- Bound by `state_at`, not cleared by each writer: every statement that changes a task's status
  or activity already stamps `state_at` (six statements in the store). The reason carries the
  time of its flag and is read only while `state_at` is that time, so no writer has to know of
  it, and a future one cannot forget to clear it.
- One reason per flag, the first: a second matter on a flagged task does not change the flag
  today, and the first matter still stands when the second is answered. Taking the latest would
  leave "its question is in the terminal" behind once that question is answered, which is this
  defect again.
- Reply keeps focusing the terminal for every flag, never a text field (SPEC-156 RPL-006): a
  Claude Code menu may have opened since the reason was written, and typed text would pick its
  options unseen. The click's visible result is the terminal's own notice.
- A notice in the terminal's corner, not a focus ring on every terminal: the ring would change
  every terminal of the product (shells, the Superagent) for one button.
- The three flags with their own card or request (a browser request, a goal pause, a host move)
  give no reason: their card says it, before the reason's place in the bar.
- [ADR-0306](../../docs/decisions/0306-a-needs-attention-flag-keeps-its-reason.md): a flag
  Yulius raises keeps its reason with the task.
