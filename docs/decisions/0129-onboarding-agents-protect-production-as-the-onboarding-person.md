# 0129. Onboarding agents protect production through Yulius, as the person who owns the task

## Status

Accepted. Specified in [SPEC-157](../../.ai/specs/SPEC-157-image-1-image-2-btw-the-agent-should-be.md);
task #148. Amends [ADR-0114](0114-agents-hold-a-token-github-refuses-on-production.md)'s "Yulius does
not add it to existing repositories itself".

## Context

ADR-0114 gives agents a token without Administration and makes onboarding report a production branch
the agents could write, asking a person (`needs_you`) to add the ruleset. In practice the person who
started onboarding is usually a repository admin and simply answered "do it": the agent then ran
`gh api -X POST …/rulesets` in its shell, Claude Code's auto-mode safety check blocked it (it changes
repository access rules), and the command went back to the person to paste. The request (task #148):
the agent should be able to do that on onboarding.

## Decision

- **A task tool, `production_protect`, adds the protection server-side.** It creates one branch
  ruleset on the sandbox's repository for the production branch: Restrict updates, Restrict
  deletions, Block force pushes, the Repository admin role as the only bypass actor, named
  `yulius-people-only-<branch>`, and no pull-request rule.
- **It acts as the task's owner, only when they are a repository admin.** Yulius uses the owner's
  GitHub user token, asks GitHub whether they are an admin first, and refuses otherwise (also
  without a GitHub identity or sign-in), telling the agent to ask with `needs_you`. Never the App's
  installation token, never the agent's token: the decision stays a repository admin's, as ADR-0114
  wants, and starting onboarding (a person in the browser) is that admin's consent to the
  discipline it sets up.
- **It only tightens.** Nothing happens when the agents' token already cannot write the branch; no
  ruleset is ever edited or removed; the result is checked again with the agents' token.
- **The onboarding prompt uses it.** With an unprotected or unchecked production branch the
  preparation agent calls `production_protect`, and asks a person only when it is refused. The agent
  still never changes protection from its shell; its token stays without Administration.

## Alternatives

- **Onboarding adds the ruleset itself, without the agent.** Silent: the person never sees it
  happen in the task, and a refusal has nowhere to go but a log. The tool keeps it in the task's
  transcript and falls back to `needs_you` naturally.
- **Use the App's installation token** (it has Administration: write): works for every owner,
  but then anyone who can start onboarding changes a repository's protection without GitHub checking
  that they may.
- **Give the agent's token Administration, or allow the `gh api` call in the agent's settings**:
  the same token could then delete the ruleset, which is exactly what ADR-0114 prevents.
- **Keep asking a person**: the friction the request is about.

## Consequences

- An admin who starts onboarding gets the production branch protected without leaving Yulius; an
  owner who is not an admin (or the superadmin, who has no GitHub identity) still gets "Needs you".
- GitHub shows the person as the ruleset's creator. A private repository on a free plan refuses
  rulesets; the tool then reports the refusal and the agent asks a person.
- Repositories may end up with two Yulius rulesets on the production branch (an older
  `yulius-production-*` without the update rule, and `yulius-people-only-*`); rulesets layer, so the
  stricter rules apply.
