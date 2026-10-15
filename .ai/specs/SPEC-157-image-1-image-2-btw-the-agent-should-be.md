---
yulius-task: b4e6ad04-732b-4a0e-8067-4ecd223f179e
status: done
---

# SPEC-157: the onboarding agent protects the production branch itself

Status: done
Date: 2026-10-15
Owner: Yulius task #148
Scope authorization: task #148: two screenshots and "btw the agent should be able to do that on
onboarding, fix this as well". Screenshot 1: an onboarding task of Quartio-Studio/product shows
"Needs you: Add a GitHub ruleset on Quartio-Studio/product for main: Restrict updates, Restrict
deletions, Block force pushes; bypass: Repository admin only". Screenshot 2: the person (a
repository admin) asked the agent to do it; the agent ran `gh api -X POST …/rulesets`, Claude Code's
auto-mode safety check blocked the call because it changes repository access rules, and the agent
handed the command back to the person.

## Goal

When onboarding finds a production branch the agents could write (SPEC-141 R5), the onboarding agent
protects it itself, through Yulius, with the rights of the person who started onboarding. A person is
asked (`needs_you`) only when that person cannot do it either (not a repository admin, no GitHub
sign-in, GitHub refuses the ruleset).

## Context

- ADR-0114 (SPEC-141): an agent's Git and `gh` hold a narrowed App installation token without
  Administration, so the agent's shell cannot create a ruleset, by design (it must not be able to
  delete one either). Onboarding warns about an unprotected production branch
  (`production_branch_warning`) and its prompt told the agent "Only a person can apply it: ask one
  with `needs_you` … never change the repository's protection yourself."
- ADR-0114 also chose "Yulius does not add it to existing repositories itself: changing a
  repository's protection is that repository's admins' decision".
- The person who starts onboarding owns the onboarding task (`task.owner`); Yulius already acts on
  GitHub for a task with its owner's user token (`person_github_token`, pushes, merges).
- Yulius already creates rulesets for new projects (`create_ruleset`, `rulesets_for`); the GitHub
  client writes the `update`, `deletion`, `non_fast_forward` rules and the Repository admin bypass.
- The screenshots show the person wanted it done and was an admin; the friction came from the agent
  having to run an access-rules change in its own shell, which the harness rightly blocks.

## Requirements

- R1. A task tool `production_protect` (no arguments) adds, on the sandbox's repository, a branch
  ruleset for the delivery profile's production branch: enforcement active, rules Restrict updates,
  Restrict deletions and Block force pushes, the Repository admin role as the only bypass actor; no
  pull-request rule (existing repositories keep their own merge rules). Its name is
  `yulius-people-only-<branch>`, distinct from the `yulius-production-<branch>` ruleset new projects
  get.
- R2. It works only in the sandbox's onboarding task (origin `onboarding`), and acts as the task's
  owner, with their GitHub user token, only when their account is enabled and GitHub says that
  person is a repository admin; never with the App's installation token and never with the agent's
  token. In another task, for a disabled owner, without a GitHub identity or sign-in, without admin
  rights, or when GitHub refuses (its error passed on, not guessed at), it is
  refused with what a person must do (the same setting the onboarding warning names) and the hint to
  ask with `needs_you`.
- R3. It only adds protection: when the agents' token already cannot write the branch it changes
  nothing and says so; it never removes or edits a ruleset. After adding, it checks again with the
  agents' token and reports whether the branch is now guarded.
- R4. A playground (no GitHub repository) is refused.
- R5. Each ruleset added is audited (`task.production_protect`).
- R6. The onboarding preparation prompt, when the production branch is unprotected or unchecked,
  asks the agent to call `production_protect`, and only if it is refused to ask a person with
  `needs_you`, quoting the setting; the agent still never changes protection from its shell.
- R7. ADR-0129 records the change to ADR-0114's "Yulius does not add it itself"; ADR-0114,
  `docs/operations.md`, `docs/architecture.md` and the briefing's tool list say the same.

## Plan

- [x] Spec and ADR-0129 record that onboarding agents protect production with the onboarding person's rights
- [x] Agents get a production_protect tool that adds the people-only ruleset as the task owner
- [x] The onboarding prompt asks the agent to protect production itself and needs_you only on refusal
- [x] Docs, independent review and gates are done and the change is delivered

## Acceptance criteria

- AC1. A task owned by a repository admin calls `production_protect` on an unprotected sandbox: one
  ruleset `yulius-people-only-main` is created with that person's token, `restrict_updates`, no
  pull-request rule; the tool answers `protected: true`; the onboarding status warning is gone.
- AC2. Calling it again changes nothing (no second ruleset) and answers that the branch is already
  protected.
- AC3. The onboarding task is refused when its owner is not a repository admin, when the owner's
  account is disabled, and when GitHub refuses the ruleset (its error shown); another task of the
  sandbox is refused; nothing is created and each message names the ruleset and `needs_you`.
- AC4. The onboarding task's prompt for an unprotected production branch names `production_protect`
  and `needs_you`, and no longer says only a person can apply it.

## Test plan

- Integration (`crates/yulius/tests/import_server.rs`, the import World's GitHub stub records
  rulesets with their token): AC1-AC3 through the MCP endpoint with a live attempt's token; AC4 from
  the onboarding task's prompt.
- Unit: the tool's descriptor is served; the briefing names it.
- Not testable here: GitHub's real enforcement and a real admin's user token (no real GitHub call
  in tests); the in-process fake GitHub of `production_branch.rs` already proves a ruleset with these
  rules refuses the agents' token.

## Decisions

- [ADR-0129](../../docs/decisions/0129-onboarding-agents-protect-production-as-the-onboarding-person.md):
  a Yulius tool, acting as the task's owner when they are a repository admin, adds the protective
  ruleset; the agent's own token stays without Administration.
- Owner's token only, never the App's installation token (which has Administration: write): GitHub
  then checks that a repository admin decided, which keeps ADR-0114's principle that protection is
  the admins' decision; a superadmin or non-admin owner gets `needs_you`.
- No pull-request rule: the warning and the screenshots ask for exactly Restrict updates, Restrict
  deletions and Block force pushes, and an existing repository's merge policy is its own.
- A distinct name (`yulius-people-only-<branch>`) so it never collides with a `yulius-production-*`
  ruleset an older Yulius created without the update rule; rulesets layer, so both apply.
- Only the onboarding task may use it (review round 1): ADR-0129's consent is a person starting
  onboarding; any other task (an automation run, a task an agent created, a prompt injected through
  an issue) must not tighten a repository's protection unasked. Served to every task so the
  refusal can say so; a later request goes through a person or a new onboarding.
- A disabled owner is never acted for (review round 1), as `owner_github_token` and the merge train
  already refuse.

## Evidence

Run on 2026-10-15 in this task's worktree.

- `scripts/dev ci` at `a8a88e31`: exit 1 on one test only, `first_message::a_launch_goal_unrecorded_across_a_restart_is_watched_again`
  ("timed out waiting for the flag" under the full run's load): the known flake tracked by open issue
  #514, outside this change. 1812 tests run: 1811 passed, 17 skipped; every gate before nextest
  (repository checks, Python tests, `cargo check`, fmt, clippy `-D warnings`) passed. Run alone
  (`cargo nextest run -p yulius -E 'test(/a_launch_goal_unrecorded/)'`) it passed 3 times of 3.
- `cargo test -p yulius --test main -- import_server::`: 8 passed, among them
  `the_onboarding_agent_protects_production_as_its_admin_owner` (AC1-AC4). It failed before the
  change on the prompt assertion (the prompt said "Only a person can apply it") and, in round 2,
  on the "onboarding task" refusal before that check existed.
- `cargo test -p yulius-app --lib -- production_protect mcp::`: 3 passed (the ruleset's rules; the
  served descriptors match the briefing's tool list). `cargo test -p yulius-domain --lib briefing`:
  15 passed (tool list unique and rendered, briefing within its budget).
- `scripts/dev security` (Vidoc): **not run**, exit 2: Vidoc rejected the key (HTTP 401, invalid or
  expired token), as since 2026-10-14 (ADR-0104: unrun, not passed).
- Independent review in a fresh session (`om-code-review`): round 1 at `520f4e09` asked for changes
  (major: a disabled owner was acted for; minors: scope beyond onboarding, a guessed cause in
  GitHub's refusal, missing refusal tests), all fixed in `a8a88e31`; round 2 approved `a8a88e31`.
  The last commit only rewords the test's doc comment and fills this section.
- Not testable here: GitHub's real enforcement and a real admin's user token creating the ruleset
  (no real GitHub call in tests). A playground's refusal and an owner without a GitHub identity rely
  on the shared, already tested `refuse_in_playground` and `person_github_token` guards.
