---
yulius-task: d52601d9-b519-4553-b399-d84a2d8c34bf
status: done
---

# SPEC-141: agents cannot push or merge into the production branch

Status: done
Date: 2026-10-15
Owner: Yulius task #139 (part of task #118, SPEC-138)
Scope authorization: task #139: "Fix GitHub issue #486 […] Make it so only a person can move the
production branch. Weigh, in an ADR: tokens handed to agents that cannot write the production
branch […]; branch protection / rulesets that only Yulius's production deploy satisfies, checked
and reported at onboarding; a credential helper or egress proxy rule that refuses writes to the
production ref. Acceptance: an agent's shell cannot push to, merge into or fast-forward the
production branch, even with `--no-verify` or `gh`; onboarding reports a production branch that
agents could write; an integration test proves an agent-side push to the production branch is
refused. If the chosen design needs a GitHub setting that only a repository admin can apply […]
build and test everything Yulius does, and leave the issue open with a comment naming exactly
which setting a person must apply. Do not change repository settings yourself."

## Goal

Only a person moves a project's production branch. An agent's Git and `gh` hold a GitHub token
that GitHub itself refuses for any update of the production branch (push, merge, fast-forward,
deletion), and that cannot loosen that protection; Yulius tells people when a repository is not
protected that way.

## Context

- ADR-0110 makes production a person's on Yulius's own paths, but names its limit: an agent's shell
  holds its task owner's raw GitHub user token (`App::agent_git_credential`, `yulius gh-token`), and
  the pre-push guard (`crates/yulius-git/src/hooks.rs`) is a local hook that `--no-verify` skips.
- The owner's token is a user-to-server token of the Yulius App: capped by the App's permissions
  (Administration: write included) and the person's role. For a repository admin it can push to
  `main`, merge a dev → main pull request, and delete the rulesets that would stop it.
- Agents' network is slirp4netns egress; `HTTPS_PROXY` is advisory, so a proxy rule cannot bind an
  agent that holds a real token.
- New projects get rulesets (`rulesets_for`, DSC-007): pull requests required, force pushes and
  deletion blocked, no update restriction and no bypass list.
- Yulius's own GitHub calls for a task (its push, pull request, merge, promotion) run on the
  server with `task_github_token` (the owner's token); they stay as they are.

## Requirements

- R1. `/internal/git-credential` (the agent's Git credential helper and `yulius gh-token`) issues an
  agent token: an installation token of the App restricted to the sandbox's repository and to the
  permissions contents: write, pull requests: write, issues: write, checks: read, commit statuses:
  read, metadata: read, as far as the installation grants each (GitHub refuses a request for more;
  a write granted only as read is asked as read; without Contents: write no token). Never the
  owner's user token; never administration or workflows. Commit authorship stays the owner's (the
  Git identity in the run is unchanged).
- R2. Without an installed App the agent gets no token (409 "the GitHub App is not installed"); a
  playground still gets none.
- R3. The production ruleset Yulius creates for new projects also restricts updates (`update`
  rule) and keeps deletion and force pushes blocked, with the Repository admin role as the only
  bypass actor (organization owners hold it on every repository). The App is never a bypass actor.
- R4. Yulius can tell whether its agents could write a branch: through GitHub's branch rules and
  each applying ruleset's `current_user_can_bypass`, asked with an agent token. The branch is
  guarded only when an active `update` rule and an active `deletion` rule apply from rulesets the
  agent token can never bypass; otherwise it is writable, naming what is missing; a GitHub
  failure is "could not be checked", never guarded.
- R5. Onboarding reports it: the onboarding status (CLI `yulius onboard`, import, API) carries a
  warning naming the production branch and the exact ruleset a repository admin must add when
  agents could write it, or that it could not be checked; the preparation task's prompt asks its
  agent to raise it with a person (`needs_you`) when that is the case.
- R6. ADR-0114 records the decision and the alternatives; ADR-0110's limit, `docs/operations.md`
  (GitHub App permissions), architecture and product summary say the same.

## Plan

- [x] Spec and ADR-0114 record the design: narrowed agent tokens plus a production ruleset only people bypass
- [x] Agents' Git and gh get a narrowed App token, never the owner's
- [x] New projects' production ruleset restricts updates to admins, and the fake GitHub enforces it
- [x] Yulius checks whether agents could write a branch through GitHub's rules
- [x] Onboarding reports a production branch agents could write
- [x] An integration test proves an agent-side push or merge into production is refused
- [x] Docs, independent review and gates are done and the change is delivered

## Acceptance criteria

- AC1. The credential an agent run gets is the narrowed installation token even when its owner is
  signed in with GitHub (R1, R2).
- AC2. Against the fake GitHub, with the production ruleset Yulius creates, the agent token's merge
  of a dev → main pull request, fast-forward of `main`, deletion of `main` and deletion of the
  ruleset are refused; a repository admin's token still promotes (R1, R3).
- AC3. The branch check reports guarded with that ruleset, writable without it (or when the App
  may bypass), and fails (not guarded) when GitHub refuses the read (R4).
- AC4. The onboarding status of a sandbox whose production branch agents could write carries the
  warning; a guarded one carries none (R5).
- AC5. On Quartio-Studio/yulius a person must still add the ruleset: the issue stays open with a
  comment naming it (task rule), until then GitHub accepts the narrowed token's push to `main`.
- AC6 (known limit, not met here). Yulius's own server-side Git in a task's worktree still carries
  the owner's token in an agent-writable repository (issue #54 part B, set aside by a person on
  2026-10-09); ADR-0114 records it and the issue stays open for it.

## Test plan

- Unit: `rulesets_for` production rules and bypass actors; the GitHub client's ruleset body and
  agent-token request body; the guard's decision over rule lists.
- Integration (`crates/yulius/tests/production_branch.rs`, the production `GitHubAdapter` over the
  in-process fake GitHub): AC2 and AC3.
- Integration (`agent_hooks.rs`): AC1 replaces "the agent's Git acts as the task creator".
- Integration (onboarding status through the import plan): AC4.
- Not testable here: GitHub's real enforcement (no repository settings may be changed by this
  task, and no real GitHub call runs in tests); the fake models GitHub's documented semantics.

## Decisions

- [ADR-0114](../../docs/decisions/0114-agents-hold-a-token-github-refuses-on-production.md): agents
  get a narrowed installation token; the production branch is protected by a ruleset only people
  bypass; Yulius checks and reports it. A proxy or credential-helper rule was rejected because an
  agent holding a real token bypasses both.
- Agents keep no Workflows permission: the onboarding commit that removes `.github/workflows` is
  pushed by Yulius (the owner's token) at the end of the turn, not by the agent's own `git push`;
  the preparation prompt and `ci_setup` say so (review round 1, major 2).
- Review round 1, major 3 (owner's token through Yulius's own Git in the worktree): not fixed in
  this change. It is #54 part B, which a person set aside; rebuilding server-side Git on a mirror
  is a change of its own. Recorded in ADR-0114 and on #486.
- One production ruleset with the admin bypass is kept (review minor 3): a fast-forward promotion
  needs the bypass for the pull-request rule too.

## Evidence

Run on 2026-10-15 in this task's worktree.

- `scripts/dev ci` at `ba90129c`: exit 0. Repository checks, Python unit tests, `cargo check`,
  `cargo fmt --check`, `cargo clippy -D warnings`, `cargo nextest run --profile ci` (1751 tests run:
  1751 passed, 16 skipped), doctests, `cargo deny`, `cargo shear`, `cargo doc`. The first run at
  `f781aee0` failed at clippy (`cmp_owned` in the fake), fixed in `ba90129c`. The final commit only
  adds a prompt clause with its assertion, comment rewraps and this section:
  `cargo test -p yulius-app --lib onboarding` passed after it.
- `cargo test -p yulius --test main -- production_branch::`: 2 passed. With the production
  ruleset's `restrict_updates` turned off, `an_agents_push_or_merge_into_production_is_refused`
  fails (`branch_guard … is_guarded()`), so the test proves the fix (AC2).
- `agent_hooks::the_agents_git_never_holds_the_owners_token`, `task_ownership::` (20 tests),
  `import_server::`, `sessions::`, `projects::`, `playground::`: passed (AC1, AC4).
- `cargo test -p yulius-github --features fake --test client`: `agent_tokens_*`,
  `a_branch_is_guarded_only_by_rules_the_token_cannot_bypass`, the ruleset body test: passed (AC3).
- `cargo test -p yulius-import`: the CLI prints the onboarding warnings: passed.
- `scripts/dev security` (Vidoc): **not run**, exit 2: Vidoc rejected the key (HTTP 401, invalid
  or expired token), as since 2026-10-14 (ADR-0104: unrun, not passed).
- Independent review in a fresh session (`om-code-review`): round 1 at `f781aee0` asked for changes
  (3 majors); round 2 at `ba90129c` approved, with major 3 accepted as the recorded known limit
  AC6 (#54 part B, issue #486 left open).
- Not testable here: GitHub's real enforcement of the ruleset and of narrowed tokens (no real
  GitHub call in tests; this task may not change repository settings). The fake models GitHub's
  documented semantics; the `current_user_can_bypass` reading and the admin-role bypass for
  organization owners are not verified against GitHub.
