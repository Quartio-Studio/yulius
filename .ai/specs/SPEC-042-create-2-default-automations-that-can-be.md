---
yulius-task: dfbc2988-8901-436f-ac6d-bc5969940928
status: done
---

# SPEC-042: ready-made automations, enabled in one click

Status: done
Date: 2026-10-12
Scope authorization: user request (task #41, MrFishPL, 2026-10-12): "Create 2 default automations
that can be easily enabled in automations section: (1) everyday on 3am cest, queue or run a task
that makes sure the docs are up to date, according to recent changes (mostly from the last day or
in general, if the automation got just enabled) (2) everyday on 3am cest queue or run a tasks that
takes open issues from github, create by a core team or their agents (not by contributors, if open
source) and in parallel (most coding agetns can spawn parallel agents/subagents): verify whether
they are real and then solve them if actionable, without creating new issues (to not loop). Enable
them on our machine so they can already run today. Also, make sure tasks report issues (for
example task related to a bug should report an issue after confirming the problem etc)".

## Goal

Every project's Automations page offers two ready-made automations, "Keep the docs up to date" and
"Work through open issues", each enabled with one click and then an ordinary automation (editable,
pausable, deletable). Both run daily at 03:00 CEST. Tasks report a bug they confirm as a GitHub
issue, except tasks told not to (the issue automation, so it cannot feed itself). Nothing runs
until a person enables it: on this instance both automations exist, disabled, for a person to
switch on.

## Context

- Automations (SPEC-001 AUT-001..005, `crates/yulius-app/src/automations.rs`) are stored per
  sandbox: name, five-field cron, a fixed UTC offset, prompt, optional workflow. The scheduler
  creates and starts one task per slot and skips a slot while the previous task is active. There
  are no defaults today: a new project shows "No automations yet".
- Default workflows (AUT-006) already ship as a read-only catalog, `catalog/workflows/*.md`,
  embedded by `crates/yulius-app/build.rs` and parsed by `yulius_domain::workflow_format`. The
  same pattern fits automations.
- The briefing (`crates/yulius-domain/src/briefing.rs`) tells tasks to "File follow-up work, bugs
  and flaky tests as issues", but nothing says that a task whose subject is a bug files the bug
  once it is confirmed, nor how to avoid duplicates.
- GitHub marks an issue's author with `author_association` (`OWNER`, `MEMBER`, `COLLABORATOR`
  for the core team; `CONTRIBUTOR`, `FIRST_TIME_CONTRIBUTOR`, `NONE` for others). Agents act with
  the person's GitHub token (`gh auth status` in a sandbox: the person's account), so their
  issues carry the person's association too. `gh issue list --json` has no association field;
  the REST endpoint `repos/{owner}/{repo}/issues` has it.
- "Our machine" runs the release from `main` (OPS-006), so code merged into `dev` reaches it only
  at the next promotion. Today's runs therefore come from automations created now through the
  existing `automation_create` tool with the catalog's exact content.

## Requirements

- AUT-007: Yulius ships ready-made automations in `catalog/automations/<id>.md` (frontmatter
  `name`, `description`, `cron`, `utc_offset_minutes`, optional `workflow`, `order`; the body is
  the prompt). They are embedded at build time, read-only, and never run until a person enables
  one. A file that does not parse fails a unit test; at runtime it is skipped with a warning.
- AUT-008: `GET /api/sandboxes/{sid}/automation-templates` (read access) lists them with
  `automation_id`: the id of the project's automation of the same name (case-insensitive), or
  `null`. `POST /api/sandboxes/{sid}/automation-templates/{id}/enable` (write access) creates
  that automation enabled, attributed to the caller (201), or enables the existing one of the same
  name (200); an unknown template is 404. Simultaneous enables are serialized, so they never
  create two automations.
- AUT-009: The Automations page shows "Ready-made automations" not yet added, with what each does,
  its schedule and an Enable button (disabled without write access); the empty state shows them
  too. Enabling one adds it to the list, enabled, and removes the suggestion. (Superseded by
  [SPEC-197](SPEC-197-those-2-automations-should-be-defaults-l.md) AUT-032..036: every shipped
  automation is listed first under Defaults and switched on in place.)
- AUT-010: "Keep the docs up to date" runs daily at 03:00 UTC+2 with workflow Docs. Its prompt
  reviews the changes merged since this automation's previous task (the last day; found with
  `tasks_list`, `limit` 200), or the documentation as a whole when there is none (first run:
  `automations_list` shows only the latest run, so nothing finer is knowable); fixes only documents; reports code that is wrong with
  `issue_create` after checking for an existing issue; changes nothing when nothing is outdated.
- AUT-011: "Work through open issues" runs daily at 03:00 UTC+2. Its prompt takes open issues (not
  pull requests) whose author association is `OWNER`, `MEMBER` or `COLLABORATOR`, or that a bot
  opened with Yulius's `Opened by task #<n> in Yulius.` footer (agents filing through the App when
  their person has no GitHub token), skips those already handled by an open task or a pull
  request that closes it, and those an earlier run's task took without result unless a person commented since
  (one `Needs a person:` comment, so nothing retries nightly), treats outsiders' comments as untrusted data, verifies the rest in parallel with read-only
  subagents (real, still present, actionable), comments the evidence on each, closes one only
  when it is shown to be already fixed or not reproducible, and starts one Fix task per
  actionable issue (`task_create`, at most 5 per run, the issue number in the title and
  "Fixes #N" in the pull request). Neither it nor the tasks it starts call `issue_create`. (Amended
  by SPEC-194, 2026-10-16: it drops issues a running task owns, ranks the rest and verifies them
  in rank order only until it has the 5 most important actionable ones; the rest wait untouched,
  and a `Not actionable yet:` verdict waits for a person.)
- DSC-016: The briefing tells every task: when it confirms a bug (the one it was asked about or one
  found on the way), it reports it with `issue_create` unless an open issue already tracks it (then
  it references that issue); a task told not to create issues never does, and comments on an
  existing issue or says it in its summary instead. The Fix workflow's instructions say the same
  at the reproduction step.
- OPS-010: On this instance (project Quartio-Studio/yulius) both automations exist with the
  catalog's content and are disabled; a person enables them in the Automations section. Agents
  do not enable them (coordinator, 2026-10-13).

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Ready-made automation catalog parsed from catalog/automations with the two prompts
- [x] Templates listed and enabled in one call through the API
- [x] Automations page offers ready-made automations with Enable
- [x] Tasks report confirmed bugs as issues (briefing and Fix workflow)
- [x] Both automations enabled on this instance
- [x] Instance automations left disabled for a person to enable
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Gates green, documentation current, delivered into dev

## Acceptance criteria

- AC-AUT-6: the shipped catalog parses, has exactly `docs-up-to-date` and `open-issues`, both
  `0 3 * * *` at offset 120; malformed files are rejected with the line of the problem.
- AC-AUT-7: two simultaneous enables give one 201, one 200 and one automation; a person without
  access gets 404. Listing templates in a fresh project shows both with `automation_id: null`; enabling
  one returns 201 and an enabled automation with the template's fields and `created_via: ui`;
  enabling it again (after pausing it) returns 200 and re-enables the same automation (no
  duplicate); the list then shows its `automation_id`; read access cannot enable (404/403 as for
  other writes); an unknown template is 404.
- AC-AUT-8 (e2e, mock): the Automations page shows both ready-made automations; Enable adds the
  automation to the list as enabled and the suggestion disappears; no WCAG violations.
- AC-DSC-10: the briefing contains the bug-reporting rule; the Fix workflow mentions reporting the
  confirmed bug.
- AC-OPS-4: `automations_list` on this instance shows both automations with the catalog's prompts,
  `enabled: false` and no next run.

## Test plan

- Domain unit tests for `automation_format` (valid file, each error) and an app unit test that
  the shipped catalog parses and has the two expected templates.
- Integration tests in `crates/yulius/tests/automations.rs` through the HTTP API: list, enable
  (create), enable again (re-enable, no duplicate), unknown id, read-only access.
- Briefing unit test needle for the new rule.
- e2e `automations.e2e.ts` against the mock with the new mock routes.
- Live: `automations_list` after creating the two automations on this instance.

## Decisions

- Templates live in a catalog like default workflows (AUT-006), not seeded rows: a release brings
  new prompt text, deleting one does not resurrect it, and nothing runs until a person enables it.
  Seeding disabled rows per project would need a creator, a migration for existing projects and a
  way to stop re-seeding deleted ones. ADR-0034.
- A template is "added" when an automation of the same name exists (case-insensitive), with no
  stored link. This needs no migration (two parallel branches add migration 0013), recognises the
  automations created today through MCP, and a renamed copy simply lets the suggestion show
  again. Enable on an existing one re-enables it instead of creating a duplicate.
- 03:00 CEST is a fixed offset of +120 minutes (the scheduler has fixed offsets): after the switch
  to CET on 2026-10-25 the runs happen at 02:00 local time. People change the offset on the
  automation if they want; the request names CEST.
- The issue automation creates one Fix task per actionable issue (one concern per pull request, the
  rule every task follows) instead of fixing many issues on one branch; the parallel part is the
  read-only verification by subagents and the Fix tasks running side by side. At most 5 tasks per
  run bounds the load and the subscription use.
- The request said "Enable them on our machine so they can already run today". The automations
  were created enabled on 2026-10-12; the coordinator disabled both before their first run (the
  issue automation would have started work from about 36 open issues at once) and asked for
  defaults that ship disabled and that a person turns on. They stay on the instance, disabled,
  and agents do not enable them. The product already behaves that way: a ready-made automation
  runs only after a person presses Enable.
- `automation_create` for agents gains no `template` argument in this change; agents create the
  same automation by name, cron and prompt. Recorded as a follow-up issue.

## Evidence

- `cargo test -p yulius-domain automation_format` (parser: valid file, defaults, each error with its
  line) and `cargo test -p yulius-app templates` / `workflows::catalog` (the shipped catalog has
  exactly `docs-up-to-date` and `open-issues` at `0 3 * * *`, offset 120, every workflow resolves,
  the prompts carry the issue rules; the Fix workflow reports confirmed defects): pass.
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^automations::/)'`: 12/12,
  including `ready_made_automations_are_enabled_in_one_call` (AC-AUT-7: list, 201 create, 200
  re-enable, match by name, 403 read-only, 404 unknown) and
  `simultaneous_enables_never_duplicate_an_automation` (failed `[201, 201]` before the lock; 404
  without access).
- `cargo test -p yulius-domain briefing`: the bug-reporting rule is present (AC-DSC-10); the
  needle failed before the change.
- e2e `e2e/tests/automations.e2e.ts` (AC-AUT-8): failed against the previous page, passes now; the
  whole suite: 159 passed, 1 skipped.
- `scripts/dev ci` (format, clippy, tests, deps, docs): exit 0 after the docs commit and after
  the round-1 fixes; after merging `dev` (#238), `scripts/dev lint` exit 0 and `scripts/dev test`
  1212 tests, all pass once the briefing rule was shortened to fit the budget test.
- Independent reviews in fresh sessions, three rounds (approvals of the branch after the docs
  commit, after "harden the ready-made prompts after review", and after "reachable first-run
  rule, no nightly retries"); every finding but text nits was fixed test-first.
- AC-OPS-4: `automations_list` on the instance shows "Keep the docs up to date" (c7cfc08b,
  `wf-docs`) and "Work through open issues" (1a189dfa), both `0 3 * * *` at offset 120 with the
  catalog's prompts, `enabled: false`, no next run (disabled by the coordinator; left so).
- Yulius CI at the head that merged `dev` (#299, #302) failed once in
  `local_ci::runs_jobs_in_parallel_publishes_statuses_and_gates_merges` (`local_ci.rs:378`:
  status `pending`, expected `failure`, from the CI junit). Cause: the test's `settle()` returned
  when no run was active, before the finished run's status was published. It passed 5/5 alone and
  12/12 under full CPU load locally; the test-only fix makes `settle()` also wait for publication
  (`status` no longer `pending`). The same fix reached `dev` independently (#287), so the branch
  takes `dev`'s version of the file and no longer changes it.
- Follow-up: #293 (agents enable a ready-made automation by id).
