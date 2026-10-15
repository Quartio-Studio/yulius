---
yulius-task: d52601d9-b519-4553-b399-d84a2d8c34bf
status: in-progress
---

# SPEC-141: agents cannot push or merge into the production branch

Status: in-progress
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
  read, metadata: read. Never the owner's user token; no administration, no workflows. Commit
  authorship stays the owner's (the Git identity in the run is unchanged).
- R2. Without an installed App the agent gets no token (409 "the GitHub App is not installed"); a
  playground still gets none.
- R3. The production ruleset Yulius creates for new projects also restricts updates (`update`
  rule) and keeps deletion and force pushes blocked, with repository admins (and, for an
  organization, organization admins) as the only bypass actors. The App is never a bypass actor.
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
- [ ] Agents' Git and gh get a narrowed App token, never the owner's
- [ ] New projects' production ruleset restricts updates to admins, and the fake GitHub enforces it
- [ ] Yulius checks whether agents could write a branch through GitHub's rules
- [ ] Onboarding reports a production branch agents could write
- [ ] An integration test proves an agent-side push or merge into production is refused
- [ ] Docs, independent review and gates are done and the change is delivered

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
  pushed by Yulius (the owner's token) at the end of the turn, not by the agent's own `git push`.
