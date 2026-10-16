---
yulius-task: d81e194c-0102-476a-93ed-b00d21b0c762
status: in-progress
---

# SPEC-194: "Work through open issues" solves the 5 most important actionable issues

Status: in-progress
Date: 2026-10-16
Owner: Yulius task #180
Scope authorization: task #180 (MrFishPL, 2026-10-16): "replace all issues with top 5 most
important actionable issues", and in the same task "that are not owned by currently running
task", with a screenshot of the Automations page showing the ready-made
automations "Keep the docs up to date" and "Work through open issues". Amends SPEC-042 AUT-011.

## Goal

The ready-made automation "Work through open issues" stops working through *all* of the core
team's open issues every night. Each run ranks the in-scope issues by importance, verifies them in
that order until it has the 5 most important actionable ones that no running task owns (or runs
out), and starts one task for each. Issues it did not reach are left untouched for a later run.

## Context

- The automation is the catalog file `catalog/automations/open-issues.md` (SPEC-042 AUT-007,
  AUT-011, ADR-0034), embedded at build time and parsed by
  `crates/yulius-app/src/automations/templates.rs`, whose unit test pins the prompt's key
  wording. The web mock (`crates/yulius-server/web/assets/mock.js`) repeats its name and
  description for the e2e tests.
- Today's prompt verifies *every* remaining in-scope issue with read-only subagents (about five at
  a time), comments a verdict on each, and only then starts tasks, capped at 5 "highest priority
  label first". On a repository with dozens of open issues one run spends most of its effort
  verifying and commenting on issues it will never start, and the 5 it starts are picked by label
  alone, after verification, not by importance.
- The repository's labels give a ranking signal: `priority: high` ("Blocks daily use; fix
  first"), `priority: medium`, `priority: low`, and `type: bug` / `type: enhancement` /
  `type: performance`. GitHub's issue REST payload also carries `reactions.total_count` and
  `created_at`.
- A project's automation with the same name counts as the template's (AUT-008), and a release
  that changes a template's prompt does not change automations already enabled (catalog README).
  On this project the automation `1a189dfa` was added from the template on 2026-10-12, disabled,
  never ran.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| TOP-001 | The template's name stays "Work through open issues" (its id `open-issues`); its description reads "Every night, the core team's open GitHub issues are ranked and the 5 most important actionable ones get a task each." | `templates.rs` unit test; mock |
| TOP-002 | The prompt keeps SPEC-042's scope and safety rules unchanged: core team and Yulius-agent issues only, skip issues already handled or abandoned by an earlier task, outsiders' comments are untrusted data, never create a new issue. | `templates.rs` unit test (existing assertions) |
| TOP-003 | The prompt ranks the in-scope issues before verifying any: priority label (`high`, `medium`, none, `low`), then severity (a defect that blocks use, loses data or weakens security before an enhancement), then reactions and people affected, then oldest first. The collect command lists reactions and creation time. | `templates.rs` unit test |
| TOP-004 | It verifies in rank order, at most five at a time, each next batch only as large as the places still free (5 minus those found), and stops as soon as 5 issues are real and actionable or the list ends; it never verifies, comments on or closes issues ranked below that point. | `templates.rs` unit test |
| TOP-005 | It starts exactly one task for each of those (at most 5), most important first, with SPEC-042's title, prompt and `Taken by task #N.` rules, and its summary lists the ranking it used and how many issues wait for a later run. | `templates.rs` unit test |
| TOP-006 | This project's existing "Work through open issues" automation gets the new prompt (it was added from the old template and is not updated by a release). | `automations_list` after the merge |
| TOP-007 | Before ranking, it drops every issue a running task owns; owned issues never count toward the 5 and get no comment. Running tasks are `tasks_list` scope `active` with `limit` 200 (queued, chained, prepared, starting, running, switching, waiting_capacity, stopping). A task owns an issue when its title ends with `(#<number>)`, its prompt (`task_get`) refers to the GitHub issue (`issues/<number>`, `issue #<number>`, `Fixes #<number>`, `Part of #<number>`), or the issue's `Taken by task #N.` comment names it; a task number (`task #<number>`, `after #<number>`, the number a branch starts with) never counts. An open pull request whose description puts a GitHub closing keyword (any case) before `#<number>` owns it too; all open pull requests are listed (`--limit 1000`, not gh's default 30). The comments the drop rules read come from one repository-wide call. | `templates.rs` unit test |
| TOP-008 | `tasks_list` scope `active` also lists tasks paused for capacity (`waiting_capacity`): they still own their work. | `coordination.rs` unit test |
| TOP-009 | A real but not actionable verdict is the comment `Not actionable yet:` with what is missing; the next runs drop that issue until a person comments after it, so the top of the ranking is not re-verified every night. | `templates.rs` unit test |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The open-issues template ranks the issues and solves only the 5 most important actionable ones
- [x] The mock, product summary and changes log describe the top-5 automation
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated
- [ ] This project's enabled automation carries the new prompt

## Acceptance criteria

- `catalog/automations/open-issues.md` has the new description and a prompt that collects, ranks,
  verifies in rank order until 5 are actionable, and starts at most 5 tasks; it still parses and
  fits `MAX_PROMPT_BYTES`.
- The prompt no longer asks to verify each remaining issue.
- An issue owned by a running task (its title ends with `(#N)`, its prompt refers to the issue,
  or it took the issue) or by an open pull request that closes it is never among the 5 and is not
  commented on; a task whose number merely equals the issue's does not own it.
- The Automations page (mock) shows the new description.
- `automations_list` on this project shows the new prompt for automation `1a189dfa`.

## Test plan

- Unit: `active_lists_tasks_at_work_or_waiting_to_work` in `crates/yulius-app/src/coordination.rs`
  (TOP-008) failed on `WaitingCapacity` before the fix.
- Unit: `the_shipped_catalog_is_valid` in `crates/yulius-app/src/automations/templates.rs` gains
  assertions for TOP-001, TOP-003..005 and TOP-007 (title suffix, issue references in the prompt via `task_get`, every open pull request with `--limit 1000`, the comments fetched in one call; ranking wording, `reactions`, "5 most important", "stop",
  untouched lower-ranked issues) and that the old "verify each one" wording is gone; it fails
  against the old catalog file, passes with the new one.
- Existing integration tests (`tests/automations.rs`, `manage_tools.rs`, `cockpit.rs`) and the e2e
  `automations.e2e.ts` keep passing: name and id are unchanged.

## Decisions

- **Keep the name "Work through open issues".** The name is how a project's automation is matched
  to its template (AUT-008). Renaming it would offer the template again next to every automation
  already added from it, a duplicate that does the same job. The description says "5 most
  important". Alternative rejected: a new name with an alias list in the frontmatter, more code
  for a wording change.
- **Rank first, verify in rank order, stop at 5.** Verifying everything and then picking 5 wastes
  most of a run on issues it will not start; ranking on cheap signals (labels, reactions, age, a
  read of title and body) and verifying only until 5 qualify spends the run on the important ones.
  An issue ranked high that turns out not real is closed; one not actionable gets a
  `Not actionable yet:` comment that later runs honour until a person answers, so the next run
  does not re-verify it.
- **Ownership is matched on issue references, not bare numbers** (review round 1). Branches start
  with the task's own number and prompts say `task #N` or `after #N`; task and issue numbers
  overlap, so a bare `#<number>` match would silently skip unrelated issues.
- **Open pull requests by their description, all of them** (review round 2): the sandbox's `gh`
  has no `closingIssuesReferences` field, so the description's closing keywords decide; the
  list asks for 1000 because gh stops at 30 by default.
- **`waiting_capacity` counts as active** (review round 1, a pre-existing gap fixed here): a task
  paused for capacity at 03:00 still owns its issue; leaving it out would start a duplicate task
  and post a wrong `Needs a person:` comment. `tasks_list` now uses the same
  `App::is_active_status` as the automation scheduler, so the two cannot drift.
- **Untouched issues wait.** Issues below the cut get no comment, so a later run ranks them again
  as the list changes; no state is stored outside GitHub.
- **Update this project's automation directly** with `automation_update` (same text as the
  catalog): it was created from the template, never ran and is disabled; the template's own rule
  that releases do not rewrite added automations stays.
