# 0114. Agents hold a token GitHub refuses on the production branch

## Status

Accepted. Specified in [SPEC-141](../../.ai/specs/SPEC-141-fix-486-agents-cannot-push-or-merge-into.md);
task #139, issue #486. Completes [ADR-0110](0110-only-a-person-deploys-production.md) outside
Yulius's own paths. Changes F 2026-10-11 for the agent's shell only: an agent's own `git push` and
`gh` act as the Yulius App; its commits stay authored by the task's owner, and Yulius's push, pull
request, merge and promotion for the task stay the owner's.

## Context

ADR-0110 makes production a person's, but an agent's shell held its task owner's GitHub user token
(the credential helper and `yulius gh-token`). For a repository admin that token can push to `main`,
merge a dev → main pull request, and delete the rulesets that would stop it (the App has
Administration: write). The pre-push hook is advice that `--no-verify` skips. The agent's network
is not forced through Yulius's egress proxy (`HTTPS_PROXY` is advisory under slirp4netns).

## Decision

- **Agents get a narrowed installation token.** The credential helper and `yulius gh-token` answer
  an App installation token restricted to the sandbox's repository and to contents: write, pull
  requests: write, issues: write, checks: read, commit statuses: read and metadata: read. Never the
  owner's token. Without Administration it cannot touch rulesets, branch protection, deploy keys or
  settings; without Workflows it cannot add a GitHub Actions workflow. Only what the installation
  grants is asked (an older App may lack Issues or Commit statuses); no Contents: write, no token.
- **The production branch carries a ruleset only people bypass.** Restrict updates (`update`),
  restrict deletions and block force pushes on the production branch, with the Repository admin
  role as the only bypass actor, always (organization owners hold that role on every repository;
  not verified against GitHub here). GitHub then refuses the App's token, and so the agent's
  `git push` (with or without `--no-verify`), `gh pr merge`, auto-merge, the merge and contents
  APIs, and workflows' `GITHUB_TOKEN`. A person's promotion (ADR-0110) runs with their own token and passes when they are a repository admin.
- **Yulius creates it and checks it.** New projects get that ruleset. For every repository Yulius
  asks GitHub, with an agent token, which rules apply to the production branch and whether that
  token can bypass them (`current_user_can_bypass`). Onboarding reports a production branch the
  agents could write (or one it could not check) and names the ruleset a repository admin must add.
  Yulius does not add it to existing repositories itself: changing a repository's protection is
  that repository's admins' decision. (Amended by [ADR-0129](0129-onboarding-agents-protect-production-as-the-onboarding-person.md):
  the onboarding agent adds it through Yulius as the task's owner when GitHub says they are a
  repository admin.)

## Alternatives

- **An egress proxy rule refusing writes to the production ref** (agents hold a placeholder, the
  proxy injects the real token): the only option with Yulius as the enforcer, but it must parse
  `git-receive-pack` bodies and the whole REST and GraphQL write surface (merges, auto-merge,
  contents, refs, rulesets, deploy keys, workflows), where every new GitHub endpoint is a hole, and
  it puts every agent's Git through TLS interception. Kept as possible defence in depth later.
- **A credential helper that refuses production pushes**: the helper never sees the ref, and the
  token it hands out works for everything. No protection.
- **Keep the owner's token and rely on a ruleset**: the ruleset cannot tell the agent from the
  person behind the same token, and an admin's token can delete the ruleset.
- **Make the App the bypass actor and give people's promotions to the App**: the agents' tokens are
  the App's, so they would bypass too.

## Consequences

- Until a repository admin adds the ruleset, GitHub still accepts the narrowed token's push to the
  production branch: the protection is complete only where the ruleset exists, and Yulius says
  where it does not. Quartio-Studio/yulius needs it added by a person (issue #486).
- Promotion (and `auto_promote`) needs a repository admin's token once the ruleset exists, or a
  bypass list the repository's admins choose.
- An agent's own pushes, comments and issues show the Yulius App on GitHub; commits keep the
  owner as author. An agent cannot push a change to `.github/workflows`: Yulius's own push of the
  task branch (the owner's token) carries the onboarding commit that removes them.
- A person's terminal keeps the person's own token, and the superadmin's (who has no GitHub
  identity) the App's full repository token: they are people's terminals, not agents'.
- **Remaining hole: Yulius's own Git in the task's worktree.** Yulius's push of a task branch and
  its merge of the integration branch into a worktree run `git` on the server with the owner's
  token in that one command's environment, inside a repository whose `.git/config` the agent can
  write. The fixed `-c` hardening (no hooks, fsmonitor or `ext::`) does not stop every
  config-driven command (for example `core.sshCommand` with an `ssh://` remote), so an agent can
  still obtain the owner's token, and run code on the host, that way. This is issue #54 part B
  (read-only repository config, or network Git from a mirror only Yulius writes), set aside by a
  person on 2026-10-09; until it is built, a repository admin's token can still reach an agent,
  and #486 stays open for it.
- Classic branch protection is not read: a branch protected only that way is reported as writable.
- A new project whose ruleset GitHub refuses (a private repository on a free plan) is only logged.
