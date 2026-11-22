---
yulius-task: 9a60dad8-697d-4edb-92fe-729524e293ae
status: done
---

# SPEC-625 — Task agents read the status of their project's other tasks, read-only

ADR: [0460](../../docs/decisions/0460-a-task-reads-its-neighbours-status-and-never-steers-them.md)

## Goal

A task's agent can read where each other task of its project stands (its plan, pull request,
delivery stage, claimed paths, owner, what it waits for and its recent screen) so that it stops
duplicating or colliding with its neighbours and stops asking a person to relay a status it can
read. It gets no way to message, start, stop or change another task through this: the owner
approved the read view on 22 Nov 2026 and rejected messaging between tasks ("messaging makes too
much chaos"). Task #609.

## Context

- Why: tasks #538 and #539 delivered parts 4 and 5 of task #494's work this week. They could not
  see whether #494 knew or what it was doing, so each raised a Needs you asking the owner to relay
  a message. iTermX gives every chat a read view of the others (`itermx-tabs list/read/log`); it
  was read as a reference only.
- What task agents read today through the Yulius MCP (`crates/yulius-app/src/mcp.rs`,
  `PROJECT_TOOLS`; `coordination.rs`):
  - `tasks_list`: every task of the agent's own sandbox (the project): number, title, status and
    activity, `shows` (needs_you, working, queued, done, ended), branch, spec path, step progress
    with the current step, start rule, subscription, model, the waiting texts (`needs_you`,
    `attention`, `held_prompt`, `pause`, `wait_reason`) and `parked_pr`;
  - `task_get`: the same for one task plus its request (`prompt`, cut at 4000 characters), its
    step list with notes and its wait history;
  - `scopes_list`: every live task's claims; `session_read`/`sessions_search`: transcripts of any
    task of the project (redacted of agent secrets).
  - Missing for a peer: its owner, its pull request and delivery stage, its own claims in one
    place, what it waits for in Yulius's words (its armed merge, its checks, staging, the queue)
    and its recent screen or last words.
- Task agents already may stop, archive and delete other tasks (`task_stop`, `task_archive`,
  `task_delete`, SPEC-066) and change how a waiting task starts (`task_update`, SPEC-396). This
  spec adds nothing of the kind and changes none of them. `task_message` is the Superagent's and
  the remote MCP's only (SPEC-230, ADR-0178): only a person sends a task a message.
- Visibility today (SPEC-490, ADR-0331, `task_access.rs`): viewing any task of a project needs
  read access to it; a task's agent acts for its owner (SPEC-070). The owner's personal agent
  secrets and personal MCP servers act as them outside GitHub (SPEC-245, SPEC-332): another
  person's words never drive such a task (`App::reaches_owners_personal`).
- The Superagent reads a task's screen with `task_screen` (SPEC-487 VIS-001,
  `visibility_tools.rs`); this spec reuses that read.

## Requirements

- **PSV-001** `task_get` answers, for every caller, besides what it answered before:
  `owner` (the owner's login), `delivery` (`stage` as the delivery record's wire name with the
  environment for `env_done`, `pr` number or null, `merge_armed`), `claims` (the scope claims it
  holds while live or waiting, pattern and note; an ended task holds none, though nothing released
  them), `waits_for` (Yulius's own one-line reasons: an armed merge, its checks or delivery news,
  staging, the queue or supertasks, capacity, a pause, its person) and `last_words` (the end of
  its agent's last message in its transcript, 300 characters, redacted as the transcript is, or
  null). A person's own callers (the Superagent, the remote MCP) also get `screen` (`from`: live,
  last run, starting or none; `text`: its last 20 non-empty lines, redacted of agent secrets); a
  task's agent never does: a prompt Yulius held (SPEC-487 QST-001) and a person's unsent draft
  (SPEC-480) show on the screen. The read changes nothing of the task (status, plan, owner,
  messages); Yulius's own wait bookkeeping (`sync_wait_message`) runs as for every brief.
- **PSV-002** A task's agent reads only tasks of its own project: a task of another project, by
  number or id, is not found, and no list or claim of another project appears.
- **PSV-003** A task's agent reads the status of tasks only while its owner may view the
  project: `tasks_list` and `task_get` from a task refuse when its owner is disabled, has lost
  read access, or Yulius cannot settle that access now (a cached decision within the staleness
  bound, else GitHub; fail closed, with a sentence naming the cause). The rule of SPEC-490:
  viewing needs read access, owner or not. Transcripts and claims (`session_read`,
  `sessions_search`, `scopes_list`) keep their existing rule.
- **PSV-004** When the peer reaches its owner's personal agent secrets or personal MCP servers and
  the reading task does not hold that same person's (another owner's task, or an automation's
  task), every text an agent or person wrote that could carry what those reached is withheld from
  `tasks_list`, `task_get`, `session_read` and `sessions_search`: the request, step titles and
  notes, the current step, wait history, the waiting texts (`needs_you`, `needs_you_notice`,
  `attention`, `held_prompt`, `wait_reason`), claim notes, the screen and the transcript; claim
  notes and the current step also from `scopes_list`, `scope_claim`'s overlaps and
  `task_create`'s planned overlaps; the same from the brief `task_stop`, `task_archive` and
  `task_update` answer, and the request from the briefing's recent changes. `last_words` is
  withheld too. The answer says `withheld` and why (the `tasks_list` text on the row). Its number, title, status, `shows`, branch, spec path, step
  counts, owner, PR and stage, claim patterns and `waits_for` stay.
- **PSV-005** No write path: the read view adds no tool and no argument that messages, starts,
  stops or changes a task; a task agent is still not served `task_message` or any person tool.
- **PSV-006** The task agent's instructions (MCP `instructions`, `tasks_list` and `task_get`
  descriptions, the `.apm` skill `yulius-spec` and the catalog skill `delivery-pipeline`) say to
  read the neighbours before starting overlapping work and never to ask a person to relay a
  status the agent can read; they keep that only a person, through the Superagent, sends a task a
  message.

### What a task agent sees of another task

| Field | Shown | Why it is safe |
| --- | --- | --- |
| number, title, status, activity, `shows` | always | The task list of every project member shows them; the title is the request's label. |
| branch, spec path | always | Public in the repository's branches and pull requests. |
| step counts (done/total/percent) | always | Numbers only. |
| step titles and notes, current step | unless PSV-004 | The plan the project's members read on the task page; agent text, so withheld when it could carry personal data. |
| request (`prompt`, 4000 chars; its first line in the briefing's recent changes) | unless PSV-004 | The person's request, readable by every member; may name personal matters for a personal task. |
| `needs_you`, `attention`, `held_prompt`, `wait_reason`, `needs_you_notice`, wait history | unless PSV-004 | What the task page shows every member; free text. |
| `owner` (login) | always | Shown on every task row; a GitHub login, no email. |
| `delivery`: stage, PR number, merge armed | always | The pull request is public to the repository's readers; the stage is Yulius's own label. |
| `claims`: patterns | while it holds them | Repository paths, already in `scopes_list`; an ended task holds none. |
| `claims`: notes (also in `scopes_list`, `scope_claim` and `task_create` overlaps, with `their_step`) | unless PSV-004 | Agent text. |
| `waits_for` | always | Yulius's own fixed sentences; they carry no agent or person text. |
| `last_words` (300 chars) | unless PSV-004 | The end of the agent's last message, which members read in its transcript, redacted of agent secrets. |
| `screen` (20 lines) | never | The terminal shows a prompt Yulius held and a person's unsent draft, which reach no agent; the Superagent and the remote MCP, a person's own, get it. |
| transcript (`session_read`, `sessions_search`) | unless PSV-004 | Readable by members today, redacted of agent secrets; never part of `task_get`. |
| subscription, model, effort | always | As before; account emails are never shown (UI-022). |
| account email, secrets' values, personal secret names | never | Values never reach agents (placeholders, SPEC-245); emails are administrators' only. |

## Plan

- [x] Write the spec and the decision for the read-only peer view
- [x] Tests that fail without the peer view: status, other project, no write path, private fields
- [x] task_get answers owner, delivery, claims, waits_for and screen
- [x] A task reads only what its owner may view, in its own project
- [x] Withhold free text of a peer holding another person's personal secrets
- [x] Render the new fields and say read-only in the instructions and descriptions
- [x] Skills and catalog: look before overlapping work, no relay requests
- [x] Docs: api, product, architecture, changes
- [x] Independent review approves the exact head
- [x] Answer review round 1: last words not screen, held claims, tidy briefs, transcript tests
- [x] Review round 2 approves the fixes on the exact head

## Acceptance criteria

- AC-1 Alice's task reads Bob's task of the same project with `task_get` and sees its owner
  (`bob`), its PR number and stage, its claims, `waits_for` and its agent's last words, and
  nothing of its screen (where a held prompt's words show). An ended task's leftover claims are
  not shown.
- AC-2 A task of another project is not found by number or id, and its claims never appear.
- AC-3 A task whose owner is disabled gets `tasks_list` and `task_get` refused.
- AC-4 With Alice's personal secret in the project, Bob's task and the automation's task see none
  of Alice's task's request, step titles, notes, last words, claim notes or waiting texts in
  `task_get`, `tasks_list`, `scopes_list`, `scope_claim`, `task_create`'s overlap, the briefs of
  `task_archive` and the briefing's recent changes; `sessions_search` does not find its
  transcript and `session_read` refuses it by task number and by id. Alice's other task sees
  all of them.
- AC-5 `task_get` on a peer changes nothing of it (status, activity, updated time, owner, steps,
  messages), and
  the task agent's tools still hold no `task_message` and no person tool.
- AC-6 The skills, catalog and instructions say what PSV-006 says; the test pins the MCP
  instructions ("never ask a person to relay") and the `task_get` description ("read-only",
  "only a person").

## Test plan

- `crates/yulius/tests/peer_status.rs` (new; declared in `tests/main.rs`): AC-1 to AC-5 through
  the internal MCP route with task tokens, two people, two projects, an automation's task and a
  personal agent secret; each test fails without the change (new fields absent, private text
  present).
- `crates/yulius-app/src/mcp_text.rs` unit tests: the text of `task_get` shows the owner, the
  delivery, claims (a note on one line), `waits_for`, last words, the screen and `withheld`, and
  a `tasks_list` row says its texts are withheld.
- `crates/yulius-app/src/peer_status.rs` unit tests: what `withhold` removes and keeps, and that
  `waits_for` holds only Yulius's sentences.
- The `task_get` description says read-only (AC-5 in `peer_status.rs`); the existing checks keep
  every description within `CLIENT_TEXT_MAX` and the instructions free of "message" (MSR-001,
  `mcp_coordination.rs`).
- Run whole: `binary_id(yulius::main) & test(/^(peer_status|mcp_coordination|remote_mcp|cockpit|cross_sandbox|secrets_scopes)::/)`,
  the `yulius-app` unit tests, `scripts/dev quick`.

## Decisions

- Extend `task_get` (and filter `tasks_list`) instead of a new `task_status` tool: the agent
  already reaches for `task_get`, the Superagent and the remote MCP get the same answer, and a
  second read tool would be a second version of the same view (ADR-0460).
- The withholding rule is by holder, not by reader's owner alone: an automation's task holds no
  person's secrets (`Holder::Nobody`), so it never reads a personal task's text, and a person's
  own two tasks read each other in full (both already hold the same secrets).
- Persons (Superagent, remote MCP) keep their full view: their access rule is unchanged.
- A task reads a neighbour's last words from its transcript, not its screen (review round 1):
  Claude Code shows a prompt Yulius held ("Original prompt: …") and a person's draft in the
  terminal, and neither may reach an agent. The transcript holds neither.
- The holder rule reads today's configuration (`reaches_owners_personal`): once the owner
  switches the personal secret or MCP server off, their earlier tasks read in full again. Kept:
  recording "held personal" per run would need a migration and still not cover the files below.
- This is not a confidentiality boundary: every task of a sandbox shares its file system
  (SPEC-245 "Shared surfaces"), including other tasks' transcript folders and worktrees. PSV-004
  limits what Yulius's tools hand over (ADR-0460, Consequences).
- Out of scope: removing the existing tidy tools of task agents (`task_stop`, `task_archive`,
  `task_delete`, `task_update`); notifying a task when a neighbour changes (no push, read only).

## Evidence

Final head before review: the branch after merging `origin/dev` at 10d6ae619.

- Test first: `scripts/cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^peer_status::/)'`
  before the change, 5 of 5 failed: `owner` and `claims` were null, a disabled owner's task got
  `tasks_list`, and a personal task's text reached bob's task. After the change, 5 of 5 pass.
- Whole binaries: `scripts/cargo nextest run -p yulius -p yulius-app` after the dev merge,
  4030 tests run, 4030 passed, 24 skipped. A first run found 5 failures: 4 in
  `mcp_coordination` (its fixture owner had no GitHub token, so the new view check refused; the
  fixture now signs in as `seed_github_user` with write access) and
  `skills::tests::every_place_says_what_to_do_on_an_overlap` (the canonical overlap sentence
  is kept word for word in `delivery-pipeline`). Both are fixed.
- Mutation checks by hand, each one undo rerunning `peer_status::` and then restored. Six of them
  were run through a small loop over the edits, which AGENTS.md asks not to do; each loop step
  was still one undo:
  - without `withhold_from` in `task_get`, AC-4 fails;
  - without `require_reader` in `tasks_list`, AC-3 fails;
  - without the project filter in `agent_task_ref`, AC-2 fails;
  - without `require_transcript_reader` in `session_read`, AC-4 fails;
  - without the row withholding in `tasks_list`, AC-4 fails;
  - with `withholds_from` ignoring the holder comparison, AC-4 fails (alice's own task then
    reads nothing);
  - with `LiveClaims::note` always answering the note (done by hand), AC-4 fails.
- `scripts/dev quick` passed: the repository contracts and `cargo fmt --check`.
- `scripts/cargo clippy -p yulius-app -p yulius --all-targets -- -D warnings` passed.
- `RUSTDOCFLAGS='-D warnings' scripts/cargo doc -p yulius-app --no-deps` passed.
- `apm install` (apm-cli 0.37.0, as in `apm.lock.yaml`) regenerated `.agents/skills/yulius-spec`
  and `.claude/skills/yulius-spec`.
- Security scan: `yulius security-scan` printed "Security scan disabled: Vidoc is turned off in
  Yulius". It is disabled, not passed.
- Review round 1 (head 19511ec92) requested changes, all answered in d2917d80e:
  - The screen carried a held prompt's words. A task's agent now gets `last_words` from the
    transcript, and only a person's callers get `screen`.
  - Ended tasks' leftover claims were shown. Claims now show only while `holds_claims()`.
  - `task_stop`, `task_archive` and `task_update` briefs now withhold.
  - The transcript search and by-id read now have tests.
  - Minors: access errors are refused with a sentence naming the cause, a `tasks_list` row says
    its texts are withheld, claim notes stay on one line, and the briefing's recent changes
    leave out a personal request. PSV-003's wording is narrowed to the task status, the
    description says texts are data, and AC-5 no longer names the audit.
  - Nits: architecture wording fixed; the screen is no longer read for a task's agent.
  - Answered in words: the present-tense holder rule and the shared file system are recorded in
    Decisions and ADR-0460. Not changed (nit): the second `task_attention_facts` read, the
    pre-existing `runtime()` in `task_screen_of` (now reached by person callers only), and
    the per-row personal lookups of `tasks_list`.
- Round 1 tests first: the new checks failed before the fix:
  `an_ended_tasks_claims_are_not_shown`; AC-1 (screen present, no `last_words`); AC-4 (the
  `task_archive` brief carried the private step).
- Round 1 hand mutations, one at a time, each restored, each failing its test:
  - the screen for task readers (AC-1);
  - without the `holds_claims` filter (the ended-claims test);
  - without the withholding of the tidy brief (AC-4);
  - without the `sessions_search` filter (AC-4);
  - without the by-id `session_read` check (AC-4);
  - without the briefing's withholding (AC-4).
- After merging `origin/dev` at a94510e71, `scripts/cargo nextest run -p yulius -p yulius-app`:
  4045 passed. `scripts/dev quick`, clippy `-D warnings` and `cargo doc -D warnings` passed.
- Review round 2 approved head 2addfe8bb. Its minors and nits were fixed within the allowance
  (one product line), in the commit after it:
  - claims show while the task is not resting, so a chained task's claims are shown, as
    `task_create`'s planned overlaps count them; the test covers both a closed and a chained task;
  - a test now holds that a person's caller still gets `screen`;
  - a test now holds the `task_update` brief's withholding;
  - the ADR index line and ADR-0460's access wording are fixed.
  Both new tests fail when their line is undone (by hand, one at a time).
- Left as round 2 says, with reasons: `last_words` parses the transcript's tail like
  `session_read` with `tail` (bounded to one task); a store failure inside
  `settled_access_level` is answered with the same "cannot confirm" sentence.
- Not tested: a real Claude Code session reading a neighbour, and a personal MCP server held by a
  live run. The holder rule comes from `reaches_owners_personal`, which SPEC-245 and SPEC-332
  already test for personal MCP servers.
