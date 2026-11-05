---
yulius-task: 77eb0740-3b1f-475b-8cf3-8b00a09feb33
status: in-progress
---

# SPEC-426: A code edit waits for the task's spec and its Plan

Status: in-progress
Date: 2026-11-05
Owner: Yulius task #408
Scope authorization: task #408 (`/goal`), with a screenshot of task #2 of a project in a free
private GitHub organisation ("Harden firmware: quality gate, deep audit, edge-case te…", goal mode,
paused, "The agent has not written a plan yet"). The request: "Yulius in repo when the org doesnt
have paid features completely ignores (paid org) gates. For example, it didnt declare a plan … We
need to be completely independent from github infra … Fix." and, while the work ran, "also make
sure this is realy the cause and if not, report the real one".

## Goal

An agent cannot change a task's code before the task has its spec with a Plan, whatever the
repository's GitHub plan: Yulius itself refuses the first code edit (Claude Code's `Edit`, `Write`,
`MultiEdit` and `NotebookEdit`) of a task that needs a spec while it has none with a Plan
checklist, and says what to write. The plan is then on the task from the start of the work, not
discovered at the end of a turn.

## Context

- **The GitHub plan is not the cause.** The discipline gate (SPEC-002 DSC-*, ADR-07) is Yulius's
  own: the `Stop` hook (`App::hook_stop` → `spec_gate`, `crates/yulius-app/src/hooks.rs`) reads
  the task's workflow, the worktree's Git state and the spec file. None of it calls GitHub, and
  none of it differs between a free and a paid organisation. What a free private organisation
  lacks (rulesets, SPEC-423; branch protection) guards the production branch, not the plan.
  Yulius's notices (Needs you, attention, the sidebar) are its own too: nothing is sent through
  GitHub notifications or mail.
- **The real cause: the gate only runs when the agent ends a turn.** The spec and Plan are
  checked at `Stop` alone (and at delivery). Claude Code's hooks Yulius registers before a tool
  runs are local only: `PreToolUse` for Bash, answered in the sandbox (`guard_client`). A goal-mode
  task (the default for new tasks, TSK-010) works through one long turn, so its agent can read,
  audit and change code for a long time before its first `Stop`; until then it is told the rule
  only in its briefing ("before changing code, write the task spec …"), and nothing holds it. The
  screenshot is that state: a goal task, in the middle of its work, with no spec and so no steps.
- Why tests missed it: every discipline test (`crates/yulius/tests/agent_hooks.rs`) drives the
  `Stop` hook; no test asks what happens to an edit before it.
- What else the cause touches: the step strip and the progress of every such task stay empty
  until the first turn end; an agent that never ends a turn before its work is done meets the gate
  only once everything is written, and writes the plan after the fact.

## Requirements

- **EDG-001 Edits reach Yulius.** Claude Code's `PreToolUse` for `Edit`, `Write`, `MultiEdit` and
  `NotebookEdit` goes to Yulius (`yulius hook claude PreToolUse` forwards it, failing open like
  every hook, and never held: a `202` fails open at once); `Bash` keeps its local check and is not
  sent. Only the event, the tool, the agent's folder (`cwd`) and the edited path leave the sandbox,
  never the file's content; the client reads up to 16 MiB of the input so a large `Write` is still
  judged.
- **EDG-002 A code edit waits for the Plan.** Yulius denies such an edit
  (`permissionDecision: deny` with a reason) when all hold: the edited file (`file_path`, or
  `notebook_path`, a relative one from the agent's `cwd`) is inside the task's worktree, after
  resolving `.` and `..`, or through links to it; it is not
  documentation (`is_documentation_path`, the rule the `Stop` gate uses for DSC-013); the task needs
  a spec for a change beyond documentation (its workflow's spec gate, or no workflow; not an
  onboarding task, not a workflow without the spec gate); and the task's spec does not exist or has
  no top-level Plan item. Everything else passes untouched, and any failure to decide passes.
- **EDG-003 The reason says what to do.** The reason names the spec path (or the specs directory
  when the task has no path), the `yulius-task` frontmatter, the Plan checklist and `spec_check`,
  says that documentation edits are not held, and tells the agent to make the edit again once the
  Plan is there.
- **EDG-005 It gives up as the Stop gate does.** Denials are counted per turn (reset by the next
  prompt); after three, the rest of that turn's edits pass and the task's terminal says once that
  the agent's edits were refused three times for want of a Plan. A spec Yulius cannot read (a
  link) or an agent told to go on without one is therefore not stuck.
- **EDG-004 The rest of the gate is unchanged.** The `Stop` gate still requires a valid spec;
  the edit gate asks only for a Plan, so a draft spec being written does not hold the work.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Yulius refuses a code edit of a task whose spec has no Plan yet
- [x] Claude Code sends edit tools' PreToolUse to Yulius
- [x] ADR, architecture and docs describe the edit gate
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, Evidence and the changes log updated

## Acceptance criteria

- AC1: a task without a workflow and without a spec: `PreToolUse` `Edit` of `<worktree>/api.rs`
  is denied with a reason naming the spec path; `Write` of the spec itself and of `README.md` pass;
  a `Read` and a file outside the worktree pass; `<worktree>/docs/../api.rs` is denied.
- AC2: a spec without a Plan item keeps the edit denied; once the Plan has an item, the edit
  passes.
- AC3: a task whose workflow has no spec gate (research) edits code freely (an onboarding task
  has the same policy, `SpecPolicy::NotRequired`).
- AC4: the launched Claude Code settings register `PreToolUse` for the edit tools with the
  `yulius hook claude PreToolUse` command; the hook client forwards an `Edit` to Yulius and prints
  its answer, and still answers a self-matching `pgrep -f` Bash command locally.

## Test plan

- `crates/yulius/tests/agent_hooks.rs` `a_code_edit_waits_for_the_spec_plan` (AC1, AC2) and
  `edits_without_a_spec_gate_pass` (AC3), through the internal HTTP hook route with a real Git
  worktree. Fails before the fix (the route answers `PreToolUse` with nothing).
- `edits_are_placed_from_the_agents_folder_and_through_links` (relative paths from `cwd`, a link
  to the worktree, a nested-only Plan, a task without a spec path) and
  `the_edit_gate_gives_up_after_three_denials_and_says_so` (EDG-005).
- `crates/yulius-harness` unit tests: the launch plan's `PreToolUse` entries (AC4) and the hook
  client's dispatch (an `Edit` reaches a loopback test server; a Bash self-match is denied without
  one); `an_edit_check_fails_open_and_is_never_held` (no server, a 500, a 202, a Write over 1 MiB
  sent trimmed).

## Decisions

- [ADR-0270](../../docs/decisions/0270-a-code-edit-waits-for-the-tasks-spec-plan.md): the
  discipline gate also runs before a code edit, on Yulius, through Claude Code's `PreToolUse`.

## Evidence

(filled in at the end)
