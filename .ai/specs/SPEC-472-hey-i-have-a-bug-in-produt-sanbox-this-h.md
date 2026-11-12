---
yulius-task: 3ce7fca4-c720-4005-ab26-1fb3c4348632
status: done
---

# SPEC-472: A task flagged "needs attention" says why, and Reply shows where the answer goes

Status: done
Date: 2026-11-12
Owner: Yulius task #455
Scope authorization: the person's request of 2026-11-12 ("I have a bug in product sandbox, this
happened and I can't click reply and don't see any question. Please find root cause and fix it"),
with a screenshot of a task of the `product` sandbox on the running instance. Issue
[#1221](https://github.com/Quartio-Studio/yulius/issues/1221).

## Goal

Whoever opens a task that reads "Needs your action", at any time, reads in its bar why it was
flagged and what to do. Only a flag raised by Claude Code's own question says that the question is
in the terminal, and that flag ends once the question is answered and the agent goes on, also
after a restart of Yulius. Reply shows that the terminal took the keyboard.

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
  | `browser.rs` `browser_request_user_action` | the agent asked for an action in the sandbox browser | an event, to the pages connected then |

  A notice is `Hub::notice` (`crates/yulius-app/src/hub.rs`): a frame broadcast to the viewers
  connected at that moment, which the page shows for 6 s (`showNotice` in `terminal-core.js`).
  Nothing is stored. Whoever opens the task later, or looks away for 6 s, gets the bar's sentence
  about a question that does not exist. Two of the paths run at a start of Yulius, when no viewer
  is connected at all, so nobody ever reads their notice. A browser request has its own row in
  the bar, but only on a page that has the request: from the event, or from the tab list that
  its Browser panel reads while open. A page opened later, with the panel closed, has neither
  and showed the terminal sentence too. The task row's second line (`activityLine` in
  `status.js`) assumes the same: "Approve or answer in the terminal".
- Why the flag stands over ordinary work: the agent's work lifts only a flag a notice raised
  (`activity_on_work`, STA-003). After the gate gave up, a goal session goes on (Claude Code's
  goal check continues the turn) and the flag stays until a clean Stop or a typed prompt; a
  secret request's flag stays until a person answers it (SPEC-213), while the agent "goes on with
  other work" as the tool tells it.
- A second cause of the same sight: a flag that Claude Code's own question raised is no longer
  lifted after a restart of Yulius. The agent's work answers such a flag (STA-003) by the time it
  was raised, `notice_attention_ms`, which lives in the server's memory (`TaskRuntime`). An
  instance update freezes every agent mid-turn (SPEC-160) and the new server reattaches the runs,
  so a task flagged before the update is still flagged after it, and its time is gone: the
  person answers, the agent works on, and the task reads "Needs your action" with "Its question
  is in the terminal below." until the turn's Stop or a typed prompt, which in a goal session can
  be hours. SPEC-469 restored that time for a goal pause's flag (from its request); a notice's
  flag had nothing to restore it from. Reproduced: the fake agent is flagged by a
  `permission_prompt` notice, Yulius restarts, the agent goes on with a tool call, and the task
  stays `needs_attention` ("timed out waiting for Working with the agent's work").
- Which path the screenshot's task took cannot be read from here (another sandbox's data is not
  visible to this one). Sessions Yulius starts run with permissions bypassed
  (`--dangerously-skip-permissions`), so Claude Code's permission prompts are rare there and a
  flag of Yulius's own is the likelier one for a goal session at work: the gate, or a secret
  request. A dialog Claude Code still shows (a question, a plan to approve) with an update in
  between is the other. The fix covers both and does not depend on which.
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
- What stays as it is, and why:
  - A flag that the main session's work does not answer by design (a worker's permission prompt,
    an agent that needs input: `notice_answered_by_work`) still ends only at the turn's Stop or a
    typed prompt: nothing tells Yulius that such a dialog was answered. Its bar now carries
    Claude Code's words, so it says what was asked.
  - A second ask on a task that is already flagged does not move the flag's time or its reason
    (issue #1200, open: a work line read after the second ask lifts the flag). This change keeps
    the first reason for the same reason the time is kept; #1200 changes both together.
  - A task that is stopping when its session cannot be ended is not running, so it reads no
    flag and no reason, before and after this change: its strip says Stopping, the notice says
    to stop it again, and deleting it is refused (SPEC-065). The flag and its reason are for the
    running task whose close could not end the session.

## Requirements

- ATN-001: a `needs_attention` flag keeps its reason with the task: `kind` (`terminal`: Claude
  Code asks in the task's terminal; `yulius`: Yulius flagged the task itself), `problem` (what is
  wrong, one line), `action` (what the person does) and `link` (a page of this Yulius where they
  do it). It is written by the statement that raises the flag (`tasks.attention`, migration 0053)
  and survives a restart of Yulius.
- ATN-002: a reason speaks only for the flag it was written with: the task reads
  `needs_attention` and has not changed state since. Once the task works again, settles, ends, or
  is flagged anew by a path that gives no reason, the old reason is not shown. A flag raised on
  a task that is already flagged changes neither the flag (as before) nor its reason, unless the
  flag changes owner (ATN-011).
- ATN-003: the task's API view carries the reason as `attention` (`{kind, problem, action,
  link}` or null) wherever a task summary is sent: the task lists, a task's detail and the
  `task.updated` event.
- ATN-004: each flag Yulius raises itself gives its reason, with the words of its notice:

  | Flag | Problem | To do |
  | --- | --- | --- |
  | The gate gave up | The agent stopped 3 times without fixing: (the gate's reasons) | Tell it how to go on, in the terminal. |
  | The goal was never set | (why). The task has no goal yet. | Type `/goal` and the request in the terminal, or stop and continue the task. |
  | The folder-trust question at a first message | Claude Code still asks whether to trust this folder, so Yulius typed nothing. | Answer it in the terminal, then type the task's first message. (Or: the task starts by itself once it is answered.) |
  | The folder-trust question before a typed goal | Claude Code still asks whether to trust this folder, so Yulius typed nothing. The task has no goal yet. | Answer it in the terminal, then type `/goal` and the request, or stop and continue the task. |
  | A restart cut the first message | Yulius restarted while it typed this task's first message, so the first message was never sent and may be half typed in the prompt box. | Complete it (…) and press Enter. (A goal task: then type `/goal` and the request.) |
  | A full context | (why). | Find what fills it (…), then type `/compact` or `/clear` in the terminal, or stop and continue the task. |
  | The session could not be ended | Yulius could not end this session ((error)). It still counts as running, so the task cannot be deleted. | Stop the task again to retry. |
  | A secret request | The agent asked for a secret, and a person has to answer its request. | Answer it on the Secrets page. (link: the sandbox's Secrets page, at the secret's tab) |
  | A browser request | The agent asks you to act in the sandbox's browser: (the request's reason) | Open the browser tab and do what it asks. (link: the tab) |

- ATN-005: a flag raised by a Claude Code notice has kind `terminal` and Claude Code's own words
  as its problem. Every problem and to-do is one line of at most 400 characters (a longer one
  is cut and ends in `…`), control characters and runs of whitespace as one space, and without
  the invisible format characters that reorder or hide text (bidirectional marks, embeddings,
  overrides and isolates, zero-width spaces): the words come from Claude Code and from agents.
- ATN-006: the notices stay as they are, for the viewers connected when the flag is raised.
- ATN-007: the task view's bar shows a `yulius` reason as a request is shown (SPEC-327): "Needs
  you", Problem, To do, an Open link when the reason has one, and Reply for the owner. A
  `terminal` reason reads "The agent needs your attention. Its question is in the terminal
  below.", with Claude Code's words when given. A flag without a reason (raised before this
  change, or by a path whose own request says it) reads "The agent needs your attention. Look at
  its terminal below.": it no longer claims a question. A `needs_you` request, and a browser
  request the page has, keep their place before the reason.
- ATN-008: the task row's second line says a `yulius` reason's to-do (its problem when it has
  none) instead of "Approve or answer in the terminal"; for a flag without a reason it says the
  task's `needs_you` request when it has one (a goal pause), else "Look at its terminal".
- ATN-009: Reply that gives the terminal the keyboard says so where the person looks: the
  terminal's notice reads "The terminal has the keyboard: answer there." A terminal that refuses
  input says why instead (read-only), as it does for typing. Phones keep the quick input.
- ATN-010: a flag that the session's work answers (Claude Code's own question; not a worker's
  prompt or an agent that needs input, STA-003) is answered by that work after a restart of
  Yulius too. The reason kept with the flag says that work answers it, the store says when the
  flag was raised, and a run that is watched again takes that time back. Every other flag stays
  through the agent's work, as before: one that work does not answer, one of Yulius's own, and
  one another matter raised after the notice's flag was replaced.
- ATN-011: a reason follows the owner of its flag (review round 1). A secret request's flag
  ends with the request's answer; once another matter flags the task, that flag is the other
  matter's (the answer no longer ends it, SPEC-213 SRA-003), and it says the other matter's
  reason from then on, or none when that matter gives none: never the secret's, which would
  send the person to a Secrets page with nothing left to answer. The flag itself and its time
  stay as they were. A flag that waits for several secret requests points at one that still
  waits: after an answer that leaves another request open, its link is that request's tab of
  the Secrets page. Two more things hold (review round 2):
  - Whose flag it is survives a restart of Yulius. The reason kept with a secret request's flag
    says that the request's answer ends it; a server that started after the flag was raised
    reads that, so the answer still ends the flag (and its reason), and another matter still
    takes the flag over.
  - A Claude Code question that takes the flag over is the session's own to answer, as on an
    unflagged task: the agent's work after it ends the flag (the task reads Working), so the
    bar never keeps "its question is in the terminal" over an answered question. The secret
    request then waits on the Secrets page without a flag, as it does once a turn's end or a
    typed prompt ended its flag. A matter that work does not answer (a worker's prompt, a flag
    of Yulius's own) keeps the flag until the turn's end or a typed prompt, as before.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] A flag keeps its reason: the store column, the API field and the gate's flag
- [x] Every flag Yulius raises says why, and a notice's flag carries Claude Code's words
- [x] The task view shows the reason and Reply says the terminal took the keyboard
- [x] Docs, ADR-0306 and the changes entry true to the fix
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- The new integration tests fail on `dev` (the task's view has no `attention`) and pass with the
  fix, one per flag of ATN-004 and one for a notice.
- A task flagged by the gate, read after a restart of Yulius, still says why.
- A task flagged by a Claude Code notice before a restart of Yulius reads Working once the agent
  works after it (the test fails without the fix: it stays `needs_attention`); a worker's notice
  and another matter's later flag stay.
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
  `attention` (it fails on `dev`); a new one covers the folder-trust question in front of a typed
  first message.
- `crates/yulius/tests/task_done.rs` (the fake agent's `/notify <type>` and `/tool`, across a
  restart of Yulius on the same data directory): a `permission_prompt` notice's flag is answered
  by the agent's work after the restart (red before the fix); a `worker_permission_prompt`
  notice's flag is not; a flag another matter raised after the notice's was replaced is not.
- `crates/yulius/tests/browser.rs`: the browser request's test also reads the task's `attention`
  (red before the fix).
- Review round 1 (ATN-011, each red before its fix): `secrets_env.rs`, a Claude Code question
  on a task its secret request flagged shows the question, before and after the secret's
  answer, and so does a worker's prompt across a dismissed request; `secrets.rs`, a task that
  asked for a project's secret and one of every sandbox points at the second's tab once the
  first is answered, and reads no flag after both; `yulius-store`, a reason is replaced only on
  the flag it was read from, keeps that flag's time, and the same reason again changes nothing;
  `yulius-app`, format characters are dropped from a reason; e2e, the row of a flag with a
  request says the request. The store test came with the statement it tests.
- Review round 2 (each red before its fix): `secrets_env.rs`, on a second server on the same
  database a secret request's answer ends its flag and reason, and a worker's prompt there
  takes the flag over and outlives the answer; `task_done.rs`, a real run of the fake agent
  asks for a secret's hosts with its own token, Claude Code's notice takes the flag over, and
  the agent's next work makes the task Working with no reason, after which the request's
  answer changes nothing; `yulius-app`, the rule that keeps a notice's time, and `answer_ends`
  kept with the reason and out of the view.
- `crates/yulius-app`: a reason is one bounded line and its view carries the page; whether work
  answers a flag is kept with the reason and is no part of the view; a goal never set says in
  its terminal what it said before and keeps what is wrong apart from what to do, also behind an
  unanswered trust question (that path has no integration test, before or after this change).
- e2e `attention.e2e.ts` (new): a `yulius` reason shows as Needs you with Problem, To do and
  Reply, and with Open for a page; a browser request reads from the task until the page has the
  request, then from the request's own row; a `terminal` reason shows the sentence and Claude
  Code's words; no reason shows the neutral sentence; Reply shows the terminal's notice; a
  read-only terminal says why instead; a watcher reads the reason without Reply; the row's
  second line. The page has no axe violations with a reason shown.
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
- One reason per flag, its owner's: the matter whose end ends the flag. A second matter on a
  flagged task does not change the flag today, and the first matter still stands when the
  second is answered, so the first reason stays; taking the latest would leave "its question is
  in the terminal" behind once that question is answered, which is this defect again. A secret
  request's flag is the one flag that changes owner (SPEC-213: any other matter takes it over,
  and the secret's answer then leaves it), so there the reason changes with it (ATN-011, review
  round 1): the replacement keeps the flag's time. While both matters are open the bar shows
  the one that holds the flag; the secret's request is still on the Secrets page and in the
  agent's printed link.
- The reason is the record of whose flag it is (`answer_ends`, review round 2), beside the
  server's memory of it (`secret_wait_at`, SPEC-213). SPEC-213 left that memory to a restart
  because a stale flag then said nothing specific and a column was not worth a migration; this
  change adds the column and a specific reason ("Answer it on the Secrets page"), which must
  end with the answer after an update too. The memory stays as the fast path; when it holds
  nothing, the flag's reason is asked, under the same lock. A reason is read only while the
  task reads its flag, and a matter that takes the flag over writes its own, so the reason
  cannot name an owner the flag no longer has.
- A question that takes a secret request's flag over is answered by work (review round 2)
  rather than kept until the turn's end: the time a notice's flag is answered by is kept for
  it, and its reason says so for a restart. The alternative, the flag going back to the secret
  request once the question is answered, would flag a working task again for a request that
  already had its flag taken; a request without a flag is the state a turn's end leaves too.
- Reply keeps focusing the terminal for every flag, never a text field (SPEC-156 RPL-006): a
  Claude Code menu may have opened since the reason was written, and typed text would pick its
  options unseen. The click's visible result is the terminal's own notice.
- A notice in the terminal's corner, not a focus ring on every terminal: the ring would change
  every terminal of the product (shells, the Superagent) for one button.
- Two flags give no reason: a goal pause's and a host move's. Each comes with a `needs_you`
  request, which is kept with the task and shown before the reason's place in the bar. A browser
  request is not kept with the task for a page that comes later, so its flag does give its
  reason; a page that has the request shows the request's own row, with the button that opens
  the tab in place.
- Whether work answers a flag is kept with its reason (`work_answers`), not derived from the
  kind: a worker's prompt is a question in the terminal too, and the main session's work says
  nothing about it. It is no part of the API view: clients have no use for it.
- The restart case is fixed here and not filed: it shows the same bar over the same working
  session, and its fix is the reason this change starts to keep. The time is taken back when a
  run is watched (once per run), not looked up at every line of the agent's work.
- [ADR-0306](../../docs/decisions/0306-a-needs-attention-flag-keeps-its-reason.md): a flag
  Yulius raises keeps its reason with the task.

## Evidence

- **Reproduction.** A probe test on `dev` (`92ba83cc2`), not committed: after the gate's fourth
  Stop the task reads `needs_attention`, a receiver subscribed before got the notice "… stopped
  3 times without fixing …", one that subscribes afterwards gets none, and the task's view says
  nothing of why. Issue #1221. The committed tests failed first:
  - `bc318566b` (tests and spec only, on `dev`'s code): 14 integration tests failed, each at its
    `attention` assertion (`left: Null`), and the 9 e2e tests of `attention.e2e.ts` failed (the
    bar read the terminal sentence, no notice after Reply, the row's line unchanged).
  - `992e6ca66` (the restart test, before its fix):
    `a_notice_is_answered_by_the_agents_work_after_a_restart` failed with "timed out waiting for
    Working with the agent's work"; its two guards passed.
  - The browser request's test failed before `e33e15a6e` (`left: Null`).
  - `5861ce1cc` (review round 1's tests, before their fix): the two `secrets_env` tests read
    the secret's reason where the question's was expected, `secrets.rs` read the answered
    request's tab (`section=agents`), the unit test kept the bidi override and the zero-width
    space, and the e2e row test found "Look at its terminal".
- **Fix.** `5d6595ced` (the column, the statement that flags with a reason, the API field, the
  gate), `8d6bfc0fd` (every other flag of Yulius's own, and the notice's words), `d736a3871` (the
  bar, the row's line, Reply's notice), `15bca64fa` (the restart), `e33e15a6e` (the browser
  request), `db7bc1a22` (a goal never set behind the trust question; the mock drops a reason
  with its flag), `c80a37a3f` (documents), `c43b39cc9` (review round 1: a reason follows the
  owner of its flag, format characters, the row's line).
- **The real CLI.** Claude Code 2.1.322 was run in a terminal against a local stand-in for the
  API (nothing left this host; the scripts are kept outside the repository). The stand-in asks
  for one Bash call, so the permission dialog opens in the default permission mode. Read from
  that run:
  - the `Notification` hook ran 6.5 s after the dialog opened, with `notification_type:
    "permission_prompt"` and `message: "Claude needs your permission"` (beside `session_id`,
    `transcript_path`, `cwd`, `prompt_id`, `hook_event_name`; no `title`). The regression test
    `a_notices_flag_carries_claude_codes_words` posts an input of that shape, with made-up ids;
  - once the dialog was answered, the session wrote the tool's result and its next assistant
    line within 0.1 s, both dated after the notice: the work that answers the flag.
  The CLI's own code names the hook input's fields (`message`, `title`, `notification_type`) and
  waits 6 s before it notifies (`rDn=6000`).
- **Mutation checks** (each on a clean tree, at `db7bc1a22` and for the last seven rows at
  `8fe945493`, restored from `HEAD` afterwards). Every mutation fails a test:

  | Removed or broken | Failing test |
  | --- | --- |
  | A reason is read only while the state is of the flag's time | `a_flags_reason_is_read_only_while_the_task_reads_that_flag`, `a_reason_that_cannot_be_read_is_none`, `a_flag_the_gate_raised_says_why_to_whoever_comes_later`, `a_flag_raised_after_a_notices_is_not_lifted_after_a_restart` |
  | A flagged task keeps its flag and its reason | `a_flags_reason_is_read_only…`, `a_notice_on_a_flagged_task_keeps_the_flags_reason` |
  | A flag from a state the task left is not raised | `a_flag_from_a_state_the_task_left_is_not_raised` |
  | The store answers when the flag was raised | three store tests, `a_notice_is_answered_by_the_agents_work_after_a_restart` |
  | The gate's flag gives its reason | `a_flag_the_gate_raised…`, `a_notice_on_a_flagged_task…` |
  | A notice's reason says that work answers it | `a_notice_is_answered_by_the_agents_work_after_a_restart` |
  | A worker's notice does not say so | `a_workers_notice_is_not_answered_by_the_agents_work_after_a_restart` |
  | A run watched again takes the flag's time back | `a_notice_is_answered_by_the_agents_work_after_a_restart` |
  | Only a reason that work answers gives its time back | `a_workers_notice_is_not_answered…after_a_restart` |
  | The browser request's flag gives its reason | `user_action_requests_mark_the_task_notify_viewers_and_return_a_link` |
  | The bar shows a reason of Yulius's own | six tests of `attention.e2e.ts` |
  | Only Claude Code's question is "in the terminal" | "a flag Yulius raised says why…", "a flag with no reason claims no question" |
  | Words that end a sentence get no second full stop | "a question in the terminal reads so…" |
  | Reply says that the terminal took the keyboard | the three Reply tests |
  | A terminal that takes no input says why instead | "Reply on a terminal that takes no input says why instead" |
  | The row's line follows the reason | "the task's row says the reason's to-do…" |
  | A reason's page gets its Open link | the secret and browser request tests |
  | Reply stays the owner's | "a watcher reads the reason, and Reply stays the owner's" |
  | A flag handed over says its new owner's reason | `a_flag_raised_after_the_request_outlives_its_answer`, `a_flag_that_takes_a_secret_requests_over_says_its_own_reason` |
  | A reason handed over is kept as one that work does not answer | `a_flag_raised_after_the_request_outlives_its_answer` |
  | A flag that waits for another request points at it | `a_flag_points_at_the_secret_request_that_still_waits` |
  | A reason is replaced only on the flag it was read from | `a_flags_reason_is_replaced_only_on_the_flag_it_was_read_from` |
  | The same reason again is no change | the same store test |
  | A replaced reason keeps its flag's time | the same store test |
  | Format characters are dropped | `reasons_are_single_bounded_lines` |

- **Gate.** `scripts/dev ci` exited 0 at `8fe945493`, the head with review round 1's fixes and
  `dev` merged (`2a31d50ae`, no conflict): repository checks, the scripts' unit tests, `cargo
  check`, `fmt`, `clippy` for Linux and macOS, the tests (3974 run, 3974 passed, 16 skipped),
  doctests, `cargo deny`, `cargo shear`, `cargo doc`. It had exited 0 at `674e88784` too (3971
  passed), and the first reviewer ran it again at `7d6d66bcb` (3971 passed). The commit after
  `8fe945493` changes this spec only.
- **End-to-end suite** (a web change). The whole suite at `8fe945493`: 83 files passed, 1
  skipped; 995 tests passed, 1 skipped; the same at `674e88784`, and in the reviewer's own run
  at `7d6d66bcb`. A first whole run, at `d736a3871`, had one failure in a
  file this change does not touch: `cd.e2e.ts` "a scrolled needs_you list survives task updates
  and its to-do stays opaque" read the list's `tabindex` once, before the page had set it
  (`expected [null,40] to equal ["0",40]`). Its file passed alone three times, and the same
  single read fails 1 run in 3 with late `ResizeObserver`s on `dev`'s assets as on this
  branch's: a flaky test this change did not cause, filed as #1223.
- **By eye.** Screenshots of the bar from the mock in Chromium at 1440, 820 and 390 px: a
  reason of Yulius's own with and without a page, Claude Code's question with its words, a flag
  without a reason, and the terminal's notice after Reply. The buttons go under the text on the
  phone width, as a request's do.
- **Security scan.** `scripts/dev security` exited 0 as disabled: Vidoc is turned off in Yulius
  for this project, so nothing was scanned. That is not a pass.
- **Independent review** (fresh read-only sessions, `om-code-review`).
  - Round 1, on `7d6d66bcb`: a usage limit cut the first reviewer off after it had run the gate
    and the e2e suite (both passed) and before it reviewed; a second reviewer continued from
    its notes. Verdict: request changes, one major and two nits, all in this change's own code
    and all fixed test-first in `c43b39cc9` (tests first, `5861ce1cc`):
    - Major: a secret request's flag that another matter took over kept the secret's reason,
      also after the secret was answered ("Answer it on the Secrets page", with a link to a
      page with nothing to answer). The reason now follows the flag's owner (ATN-011). The
      reviewer asked for a case where the gate takes the flag over; the second test uses a
      worker's permission prompt instead, which goes through the same function
      (`flag_needs_attention`) and is a flag the session's work does not answer.
    - Found beside it, the same kind: a flag that waited for two secret requests kept the link
      of the answered one's tab. It now points at a request that still waits.
    - Nit: invisible format characters passed into the bar. Dropped (ATN-005).
    - Nit: the row of a flag whose `needs_you` request says it read "Look at its terminal". It
      reads the request (ATN-008).
    - Its notes on coverage: the reason when a secret flag is raised again after an answer is
      now read by `a_flag_points_at_the_secret_request_that_still_waits`' path for a request
      that still waits; a flag raised anew in the moment of an answer has no test.
  - Round 2 reviews the head with these fixes; its verdict is the review summary recorded with
    the merge.
- **Not tested.**
  - The task of the screenshot: it is another sandbox's, on the running instance, and nothing
    of it can be read from here. Which of the causes it met is therefore not known; both are
    fixed and each has its failing test.
  - A session of the real Claude Code inside a Yulius of this branch (it needs a subscription;
    the running instance is on `main` and gets the change when a person promotes `dev`). The
    server's side is tested with the fake agent, also across a restart on the same data
    directory, and the fake's notice and work lines have the shape the real CLI was seen to
    send.
  - An instance update itself (the freeze and the new binary): the restart tests start a second
    server on the first one's data directory and reattach the run, as an update does.
  - The folder-trust question in front of a typed goal has no integration test, before or
    after this change; its words are unit-tested.
  - Browsers other than Chromium, and a real touch screen (phones keep the quick input, which
    this change does not touch).
