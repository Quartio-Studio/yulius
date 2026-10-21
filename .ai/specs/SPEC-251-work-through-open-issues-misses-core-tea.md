---
yulius-task: 3342f1bd-f982-4bf6-92d0-b8188785c87d
status: draft
---

# SPEC-251: "Work through open issues" takes in the core team's issues that GitHub calls CONTRIBUTOR

Status: draft
Date: 2026-10-21
Owner: Yulius task #237
Scope authorization: task #237 (MrFishPL, 2026-10-21), started by task #235: fix GitHub issue
[#594](https://github.com/Quartio-Studio/yulius/issues/594). Amends SPEC-042 AUT-011 and SPEC-194
TOP-002 (the scope rule only).

## Goal

The ready-made automation "Work through open issues" ranks every open issue written by a person
with write access to the repository, or by this project's agents through its Yulius App, and still
leaves contributors' and outsiders' issues untouched.

## Context

- Symptom (reproduced 2026-10-21 on dev at 4e796d87, 109 open issues): every issue reports
  `author_association` `CONTRIBUTOR`. 50 are by MrFishPL, a repository admin
  (`gh api repos/{owner}/{repo}/collaborators/MrFishPL/permission` → `admin`), 44 of them opened
  through the App on his behalf (`performed_via_github_app.slug` = `yulius-quartio-studio`, with
  the `Opened by task #<n> in Yulius.` footer); 59 are by the App's bot
  `yulius-quartio-studio[bot]`, 2 of them with the footer. The prompt's rule (association `OWNER`,
  `MEMBER` or `COLLABORATOR`, or a bot with the footer) keeps 2 of 109 in scope.
- Root cause: `catalog/automations/open-issues.md` step 1 trusts `author_association` alone for
  people. GitHub reports `CONTRIBUTOR` for anyone who has a commit in the repository and is not an
  organization member or invited collaborator, so an admin who holds access another way (a
  personal account owning the organization, a team, an outside admin) reads `CONTRIBUTOR`. The
  collect command does not list the author's login nor `performed_via_github_app`, so neither the
  repository permission nor the App can be checked. The unit test
  `the_shipped_catalog_is_valid` (`crates/yulius-app/src/automations/templates.rs`) pinned the old
  wording, so nothing caught it.
- Issues opened through the App: `agent_delivery.rs` (`issue_create`) appends the footer and opens
  the issue with the owner's GitHub token when they have one (author: the person,
  `performed_via_github_app` the App, since that token is the App's user access token,
  `access.rs` `user_github_token`), else with the installation token (author: `<slug>[bot]`).
- In a sandbox `gh` works through the same App: with the installation token
  `gh api graphql -f query='{viewer{login}}'` names the bot (`yulius-quartio-studio[bot]`, checked
  here); with a person's App user token it names the person, and `gh api user/installations` lists
  that App's installations with their `app_slug`.
- The comment fetch `gh issue list --json number,comments` returns at most 100 comments per issue
  (leftover nit from task #180's review).
- The project's own automation `1a189dfa` was added from the template and is not updated by a
  release (catalog README); it is updated with `automation_update` after the merge.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| OIS-001 | The collect command lists each issue's author login and the slug of the App that opened it (`performed_via_github_app`). | `templates.rs` unit test |
| OIS-002 | An issue whose author is a person with repository permission `write`, `maintain` or `admin` is in scope even when the association reads `CONTRIBUTOR` or `NONE`; the permission is asked once per author with `gh api repos/{owner}/{repo}/collaborators/<login>/permission`, and a failed lookup counts as no write access. | `templates.rs` unit test |
| OIS-003 | An issue this project's Yulius App's bot opened whose body ends with `Opened by task #<n> in Yulius.` is in scope; an issue filed through the App on a person's behalf counts by that person's permission alone (OIS-002); the prompt says how to tell this project's App's slug, and without it no issue is in scope by this rule. | `templates.rs` unit test |
| OIS-004 | Associations `OWNER`, `MEMBER`, `COLLABORATOR` stay in scope; contributors and outsiders without write access stay out, untouched. | `templates.rs` unit test |
| OIS-005 | The prompt says that an issue showing exactly 100 comments has more, read with `gh issue view <number> --json comments`. | `templates.rs` unit test |
| OIS-006 | This project's "Work through open issues" automation carries the new prompt. | `automations_list` after the merge |

## Plan

- [ ] Spec written and validated for the open-issues scope fix
- [ ] Test pins the new scope wording and fails on today's prompt
- [ ] The open-issues prompt takes writers and this project's App issues into scope
- [ ] ADR, changes log and docs describe the new scope
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green and the spec closed with evidence
- [ ] Merged into dev on green CI and this project's automation carries the new prompt

## Acceptance criteria

- `catalog/automations/open-issues.md` step 1 collects `author` and `app`, puts writers and this
  project's App's bot issues with the footer in scope, keeps the old association rule, and leaves
  contributors and outsiders without write access out; it still parses and fits
  `MAX_PROMPT_BYTES`, and its first paragraph is unchanged (SPEC-240 test).
- Applied by hand to today's open issues, the rule puts the 50 issues of MrFishPL (admin) and the
  2 footered bot issues in scope, and nothing by an author without write access.
- `the_shipped_catalog_is_valid` fails on the old prompt and passes on the new one.
- `automations_list` shows the new prompt for automation `1a189dfa`.

## Test plan

- Unit: `the_shipped_catalog_is_valid` pins the collect fields, the permission check and its
  levels, the App-slug lookup, the footer, the out-of-scope wording and the 100-comment note; run
  it on the old prompt first (fails), then on the new one (passes).
- Manual: run the new collect command and the permission check on this repository and count the
  issues in scope.
- Gate: `scripts/dev check` and `scripts/dev lint`.

## Decisions

- Writers are found by the repository permission, not by a wider association list: GitHub's
  association cannot tell an admin from a one-time contributor (both read `CONTRIBUTOR`), while
  `collaborators/<login>/permission` is exact and needs one call per distinct author
  ([ADR-0192](../../docs/decisions/0192-open-issues-scope-by-repository-permission-and-the-app.md)).
- The App rule needs this project's App's bot and the footer: the footer alone is plain text
  anyone can type, and any installed App's bot is not necessarily Yulius. The bot rule narrows
  from "a bot" to "this project's App's bot".
- An issue filed through the App on a person's behalf counts by the person's permission, not by
  the App (independent review, round 1): every person on any project of this instance holds a
  user token of the same App, so `performed_via_github_app` proves no write access here. #594's
  first criterion asked for it "whether the author is the bot or a user"; its third criterion
  (no outsiders) wins, and every writer's App issue is in scope by permission anyway.
- Yulius's security-finding issues (bot-authored, no footer, ADR-0172) stay out of scope as before:
  putting them in would change what the automation ranks every night, a product decision outside
  #594. Recorded on #594.
- The fix is a prompt change only; exposing the App slug through `project_context` was weighed and
  left out: `gh` already names the App it works through, and a new MCP field is a bigger change
  than the defect.
