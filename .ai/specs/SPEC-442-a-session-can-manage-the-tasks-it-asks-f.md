---
yulius-task: 97eaeae6-84e2-4a59-9b02-85351470a9bc
status: done
---

# SPEC-442: The Superagent manages tasks in its person's other sandboxes, each change approved

Status: done
Date: 2026-11-08
Owner: Yulius task #424
Scope authorization: task #424 (`/goal`), requested by the owner through quartio-v2's
Superagent: "also create a fix for 'I can't change it from here'."
Extends [SPEC-335](SPEC-335-superagents-should-be-able-to-talk-to-ot.md) (XSR-001..010) and
[ADR-0227](../../docs/decisions/0227-the-superagent-starts-tasks-in-other-sandboxes-only-on-its-persons-approval.md);
decided in [ADR-0286](../../docs/decisions/0286-the-superagent-changes-tasks-in-other-sandboxes-on-its-persons-approval.md).

## Goal

A Superagent that asked for a task in another of its person's sandboxes can follow it up from
where it is: read it, start a prepared one now, re-chain it, tell it something and stop it, each
change approved by the person on the same panel as `sandbox_task_create`. "Prepared, but should
be queued" never again needs a delete and a new task, and new requests are queued unless the
person asks for prepared.

## Context

- 2026-11-07: quartio-v2's Superagent created a task in the yulius sandbox with
  `sandbox_task_create(prepared: true)`. The person then wanted it queued; `task_update`,
  `task_message`, `task_stop` and `task_get` act only in the Superagent's own sandbox, so the task
  was deleted and created again, which the "never recreate a task" rule (SPEC-396 ATR-006) exists
  to prevent and which risks the task's attachments.
- SPEC-335 built the one cross-sandbox write: an in-memory request (`App::cross_requests`) that
  the call waits on (at most 4 minutes, 3 per person and source sandbox), answered only by the
  person's click in a browser session, executed in the target through `tool_task_create` with a
  `ProjectCaller` pinned to the target, audited `superagent.cross_task` on the target only.
- The same-sandbox tools take a `ProjectCaller` whose `sandbox` is where they act:
  `tool_task_get`, `tool_task_update` (SPEC-396), `tool_task_message` (SPEC-230, labels a running
  task's paste "Message from <login> (through their Superagent)") and `tool_task_change(Stop)`
  (SPEC-066). Each already checks the person's write access, the task's state and audits the
  change on the task.
- The Superagent's tools and instructions are digested into its launch (SPEC-186 SAV-006):
  running Superagents restart once idle to get new ones.
- The Global Superagent (SPEC-427) reaches projects through the remote tools as its person; it
  is not changed here.

## Requirements

- XST-001: the Superagent is served `sandbox_task_get {sandbox, task_id | number}`: what
  `task_get` answers for that task of another sandbox the person can work in. It only reads:
  no approval, pre-allowed like `sandboxes_list`, not audited. Its own sandbox is refused (use
  `task_get`); a sandbox the person cannot work in and a task not in that sandbox are not found.
- XST-002: the Superagent is served three requests that change a task of another sandbox, each
  naming it by `sandbox` and `task_id | number`:
  - `sandbox_task_update {…, after_task?, start_rule?, supertask?, prepared?, start_at?}`:
    `task_update`'s change of how a task that has not started starts; none of them starts it now;
  - `sandbox_task_message {…, text}`: `task_message`'s message, labelled as the person's;
  - `sandbox_task_stop {…}`: `task_stop`.
  Unknown arguments, an empty or over-long text, characters the person cannot see (SPEC-335
  XSR-008), its own sandbox, a sandbox the person cannot work in and a task not in it are refused
  before anyone is asked.
- XST-003: each of them is a request as `sandbox_task_create` is (XSR-003, XSR-004, XSR-006): it
  waits for the person's answer on the panel, at most 4 minutes, within the same bound of 3 per
  person and source sandbox; only the person, in a browser session, answers it; approved, the
  same-sandbox tool runs in the target for the person with exactly the arguments shown, and its
  refusal (the task started meanwhile, a personal-secret task, …) is the answer of the click and
  of the call; declined, unanswered, dropped or cancelled, nothing changes.
- XST-004: no cross-sandbox delete, archive or restore exists: no tool is served for them and a
  request can only be one of create, update, message and stop.
- XST-005: every answer is audited `superagent.cross_task` in both sandboxes: on the target with
  `from_sandbox`, on the source with `to_sandbox`, each with the `action` (`create`, `update`,
  `message`, `stop`), the task's number when it names one, the outcome and the caller; never the
  prompt or the message.
- XST-006: `GET …/superagent/requests` gives each request its `action` and, for a request about a
  task, the `task` (number, title, status as they are now); a message's text comes as `prompt`, an
  update's arguments as `options`, exactly as they will run (`false` flags and blank texts are
  dropped before asking), and `starts_now` says whether an update starts the task now (no start
  option, or a supertask). An update that cannot apply (the task started, a start New task
  refuses) is refused before anyone is asked. The panel's card says what is asked: "Your
  Superagent asks to start #N now / to change how #N starts / to send #N a message from you / to
  stop #N" in the target, with
  the task's title, and the button says it (**Start now**, **Change start**, **Send message**,
  **Stop task**). A message's text is shown and read as a prompt is (XSR-008: invisible
  characters named, the button waits until its end was on screen).
- XST-007: every approval is per request: a create's approval grants nothing later. (The optional
  "skip a second approval for a task this session created" is not built; see Decisions.)
- XST-008: guidance: the Superagent's instructions and `sandbox_task_create` say a task in
  another sandbox is created queued (starting by itself) unless the person asks for it prepared,
  and name `sandbox_task_get`, `sandbox_task_update`, `sandbox_task_message` and
  `sandbox_task_stop` for following it up, never deleting and recreating it.

## Plan

- [x] Spec, ADR and plan for cross-sandbox task requests
- [x] A Superagent reads a task of another sandbox with sandbox_task_get
- [x] A Superagent starts or re-chains a task of another sandbox once its person approves
- [x] A Superagent messages or stops a task of another sandbox once its person approves
- [x] Every cross-sandbox answer is audited in both sandboxes
- [x] The Superagent panel shows start, change, message and stop requests
- [x] Instructions and docs: queued unless asked for prepared, the new tools named
- [x] Shown end to end: a prepared task in another sandbox started and messaged from a Superagent
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, changes log updated

## Acceptance criteria

- AC-XST-1: a Superagent in A creates a prepared task in B (approved), then calls
  `sandbox_task_update` with no start argument; the person approves; the task leaves `prepared`
  and runs (its agent answers its prompt).
- AC-XST-2: `sandbox_task_message` to that running task, approved, is typed into its session as
  "Message from <login> (through their Superagent)".
- AC-XST-3: a declined update, message or stop changes nothing: the task's status and request are
  as before, nothing is typed.
- AC-XST-4: no tool deletes, archives or restores a task of another sandbox: the Superagent is
  served none, and a call by such a name is an unknown tool.
- AC-XST-5: every answer (approved, declined, expired) is in the audit of both sandboxes, with
  the action and without the text.
- AC-XST-6: `sandbox_task_get` reads a task of B without asking; its own sandbox, a sandbox the
  person only reads and a task of another sandbox are refused.
- AC-XST-7 (e2e): the panel shows a message request's text and **Send message**, and a start
  request's **Start now**; answering removes the card.

## Test plan

- Integration (`crates/yulius/tests/cross_sandbox.rs`, the SPEC-335 world of alice's studio and
  website): XST-001, XST-002 refusals, XST-003 approve/decline of update and stop, XST-004,
  XST-005 audits in both sandboxes, XST-006 request listing.
- Integration with real processes (`crates/yulius/tests/cross_sandbox_live.rs`, the
  first-message harness: supervisor, ptyhost and fake agent): AC-XST-1 and AC-XST-2 from a
  Superagent in a second sandbox.
- Unit (`mcp.rs` tests, `manage_tools.rs`): served tools, the read-only allowlist, guidance text.
- e2e (`e2e/tests/superagent-requests.e2e.ts`): AC-XST-7 against the mock.

## Decisions

- **Separate `sandbox_task_*` tools, not a `sandbox` argument on `task_*`.** The same-sandbox
  tools run at once; a cross-sandbox one waits minutes for a click. One name per behaviour keeps
  the allowlist (read-only `sandbox_task_get` pre-allowed, the others never), the descriptions and
  the audit unambiguous. Recorded in ADR-0286.
- **One request type with an action**, reusing SPEC-335's waiting set, bound, answer route and
  card, so every guarantee there (browser-only answer, invisible characters, read-to-the-end,
  arming) holds for the new actions without a second mechanism.
- **Reads are not approved.** `sandbox_task_get` shows the person's own task to the person's own
  Superagent, as `sandboxes_list` does; asking for every look would train the person to click
  through cards.
- **No standing approval for tasks the session created** (the optional item). Starting a task is
  one click on a card that says "start #N now"; a grant would need storing who created what and
  until when, and a session that reads injected text could use it unseen. Every change stays a
  click.
- **Audit in both sandboxes**: the target's admins see who reached in, the source's what its
  Superagent did elsewhere.

## Evidence

- Tests written before the code: the integration tests did not compile without `CrossAction`
  and `tool_sandbox_task_request`; the guidance test failed ("the instructions miss `queued`");
  review round 1's tests failed before their fix (`an_update_card_says_whether_the_task_starts_now`,
  `an_update_that_cannot_apply_is_refused_without_asking`, `the_card_names_the_task_as_it_is_now`).
  Mutation check: without the source-side audit row, `every_cross_sandbox_answer_is_audited_in_both_sandboxes`
  and `an_unanswered_change_is_audited_as_expired_in_both_sandboxes` fail.
- AC-XST-1, AC-XST-2 (shown end to end): `cross_sandbox_live::a_prepared_task_of_another_sandbox_is_started_and_messaged_from_a_superagent`,
  with real processes (supervisor, detached `yulius ptyhost`, fake agent): the Superagent of
  `home` asks for a prepared task in `pool` (approved), starts it with `sandbox_task_update`
  (approved; the agent answers its prompt, no second task), then messages it (approved;
  typed as "Message from admin (through their Superagent):"); 3 audit rows in each sandbox,
  none holding the message.
- AC-XST-3..6: `cross_sandbox::` (29 tests: reads without asking; start, re-chain, message, stop
  approved; declined changes nothing; refusals before asking, including updates that cannot
  apply; refusal on approval and the personal-secret refusal audited `failed` in both
  sandboxes; expiry and every answer audited in both; a task named twice; the card's current
  task state), `cockpit::the_cross_sandbox_task_tools_over_the_mcp` (over `/mcp`: get, a stop
  request, cancellation audited in both) and `cockpit::…` `sandbox_task_delete`/`_archive` are
  unknown tools; `mcp::tests` (served set, action of each tool, no delete/archive, guidance).
- AC-XST-7: `e2e/tests/superagent-requests.e2e.ts` 15/15 (`npx e2e run tests/superagent-requests.e2e.ts`).
- `scripts/dev ci` at 16866429: exit 0 (repository contracts, fmt, clippy, macOS targets,
  3233 tests passed, docs); the merge of `origin/dev` (a1268e51) was clean, then the affected
  tests passed again (156/156).
- `yulius security-scan`: "Security scan disabled" (Vidoc is off for this project): not run,
  not passed.
- Independent review (fresh session, read-only): round 1 REQUEST CHANGES (1 major: an update
  card could say "change" while it starts the task now; 4 minor; 2 nits), all fixed test-first;
  round 2 APPROVE at f6cbd7c9. Two nits left by decision: a card whose task was deleted while it
  waits keeps the task's last title and status (approving it answers not found, nothing
  changes), and stop and message requests are refused on approval, not before asking, when the
  task ended meanwhile (XST-003 allows that; an update is checked earlier because its card
  wording depends on it).
- Not tested here: the live instance. The tools reach running Superagents after Yulius' own
  update deploys this (their launch digest covers the tools and instructions); promotion to
  `main` is a person's.
