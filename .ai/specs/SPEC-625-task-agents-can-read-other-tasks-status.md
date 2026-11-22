---
yulius-task: 9a60dad8-697d-4edb-92fe-729524e293ae
status: in-progress
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
  environment for `env_done`, `pr` number or null, `merge_armed`), `claims` (the task's live scope
  claims: pattern and note), `waits_for` (Yulius's own one-line reasons: an armed merge, its
  checks or delivery news, staging, the queue or supertasks, capacity, a pause, its person) and
  `screen` (`from`: live, last run, starting or none; `text`: its last 20 non-empty lines,
  redacted of agent secrets as the page's terminal is). Nothing is written by the read.
- **PSV-002** A task's agent reads only tasks of its own project: a task of another project, by
  number or id, is not found, and no list or claim of another project appears.
- **PSV-003** A task's agent reads what its owner may view: `tasks_list` and `task_get` from a
  task refuse when its owner is disabled or has lost read access to the project (the rule of
  SPEC-490: viewing needs read access, owner or not).
- **PSV-004** When the peer reaches its owner's personal agent secrets or personal MCP servers and
  the reading task does not hold that same person's (another owner's task, or an automation's
  task), every text an agent or person wrote that could carry what those reached is withheld from
  `tasks_list`, `task_get`, `session_read` and `sessions_search`: the request, step titles and
  notes, the current step, wait history, the waiting texts (`needs_you`, `needs_you_notice`,
  `attention`, `held_prompt`, `wait_reason`), claim notes, the screen and the transcript. The
  answer says `withheld` and why. Its number, title, status, `shows`, branch, spec path, step
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
| request (`prompt`, 4000 chars) | unless PSV-004 | The person's request, readable by every member; may name personal matters for a personal task. |
| `needs_you`, `attention`, `held_prompt`, `wait_reason`, `needs_you_notice`, wait history | unless PSV-004 | What the task page shows every member; free text. |
| `owner` (login) | always | Shown on every task row; a GitHub login, no email. |
| `delivery`: stage, PR number, merge armed | always | The pull request is public to the repository's readers; the stage is Yulius's own label. |
| `claims`: patterns | always | Repository paths, already in `scopes_list`. |
| `claims`: notes | unless PSV-004 | Agent text. |
| `waits_for` | always | Yulius's own fixed sentences; they carry no agent or person text. |
| `screen` (20 lines) | unless PSV-004 | What the task page's terminal shows members, redacted of agent secrets; withheld when personal tools could have printed personal data. |
| transcript (`session_read`, `sessions_search`) | unless PSV-004 | Readable by members today, redacted of agent secrets; never part of `task_get`. |
| subscription, model, effort | always | As before; account emails are never shown (UI-022). |
| account email, secrets' values, personal secret names | never | Values never reach agents (placeholders, SPEC-245); emails are administrators' only. |

## Plan

- [x] Write the spec and the decision for the read-only peer view
- [ ] Tests that fail without the peer view: status, other project, no write path, private fields
- [ ] task_get answers owner, delivery, claims, waits_for and screen
- [ ] A task reads only what its owner may view, in its own project
- [ ] Withhold free text of a peer holding another person's personal secrets
- [ ] Render the new fields and say read-only in the instructions and descriptions
- [ ] Skills and catalog: look before overlapping work, no relay requests
- [ ] Docs: api, product, architecture, changes
- [ ] Independent review approves the exact head

## Acceptance criteria

- AC-1 Alice's task reads Bob's task of the same project with `task_get` and sees its owner
  (`bob`), its PR number and stage, its claims, `waits_for` and its screen.
- AC-2 A task of another project is not found by number or id, and its claims never appear.
- AC-3 A task whose owner is disabled gets `tasks_list` and `task_get` refused.
- AC-4 With Bob's personal secret switched on in the project, Alice's task sees none of Bob's
  task's request, step titles, notes, screen, claim notes or waiting texts in `task_get` and
  `tasks_list`, and `session_read` of Bob's task is refused; Bob's other task sees them.
- AC-5 `task_get` on a peer changes nothing (status, updated time, steps, messages, audit), and
  the task agent's tools still hold no `task_message` and no person tool.
- AC-6 The skills, catalog and instructions say what PSV-006 says.

## Test plan

- `crates/yulius/tests/peer_status.rs` (new; declared in `tests/main.rs`): AC-1 to AC-5 through
  the internal MCP route with task tokens, two people, two projects, an automation's task and a
  personal agent secret; each test fails without the change (new fields absent, private text
  present).
- `crates/yulius-app/src/mcp_text.rs` unit tests: the text of `task_get` shows the owner, the
  delivery, claims, `waits_for`, the screen and `withheld`.
- `crates/yulius-app/src/mcp.rs` unit test: the instructions and descriptions say read-only and
  stay within `CLIENT_TEXT_MAX`; the existing MSR-001 check (no "message" in the instructions).
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
- Out of scope: removing the existing tidy tools of task agents (`task_stop`, `task_archive`,
  `task_delete`, `task_update`); notifying a task when a neighbour changes (no push, read only).
