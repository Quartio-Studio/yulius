# 0110. Only a person deploys production, typing the project's slug; agents never promote

## Status

Accepted. Specified in [SPEC-134](../../.ai/specs/SPEC-134-implement-cd-it-should-write-cd-bash-scr.md);
task #113. Changes DLV-008's "an agent never promotes unless its task asks": agents never promote.
Amended by [0334](0334-the-superagent-asks-for-production-on-its-panel-with-the-typed-slug.md)
(SPEC-487): the person may type the slug on their Superagent's panel, which then runs the CD page's
deploy, rollback or promotion as them.

## Context

"Dev deployment can be done by agent but main deployment should require strong confirmation and be
always done by humans." For Yulius, pushing to `main` is the production deployment: instances update
themselves from it. Until now `delivery_promote` let an agent promote under its task owner's
authority, and only the tool's description asked it not to unless its task asked for a release.
`delivery_deploy` ran any command-verified environment, production-stage ones included.

## Decision

- A production deploy (`[cd.prod]`) is started only by a person with administrator access to the
  project, on the CD page or through the API, with the project's slug typed as confirmation. The
  check is in the domain rule every entry point uses; agents (task tokens, MCP) are refused
  whatever they pass.
- With `promote = true`, the production deploy first promotes the integration branch with the
  profile's method, under the person's GitHub identity; a promotion that cannot complete now (its
  pull request's checks still run or failed) refuses the deploy and says so. The superadmin, who
  has no GitHub identity, promotes through the GitHub App, and the run and the audit say so
  ([SPEC-677](../../.ai/specs/SPEC-677-urgent-the-superadmin-can-deploy-product.md)); every other
  rule here still applies.
- Agents have no promotion tool any more (`delivery_promote` is gone), and `delivery_deploy`
  refuses production-stage environments. An agent that needs production asks a person with
  `needs_you`.
- The promotion API (`POST /api/sandboxes/{sid}/delivery/promote`) follows the same rule: an
  administrator and `{"confirm": "<slug>"}`; for Yulius a promotion is the release.
- An administrator's `auto_promote` in the delivery profile stays: a person chose it.
- The limit of this decision is Yulius's own paths: an agent's shell held its task owner's GitHub
  token, which could write the production branch on GitHub directly. [ADR-0114](0114-agents-hold-a-token-github-refuses-on-production.md)
  closes that: agents hold a narrowed App token, and a ruleset only people bypass protects the
  production branch.

## Alternatives

- **A confirm checkbox.** One click is not a strong confirmation.
- **The person's second factor.** Not every account has one, and the superadmin's recovery flow
  would block production.
- **Agents promote when their task asks.** The person said production is always done by humans.

## Consequences

- A release is a person's click and typed slug on the CD page; tasks end at the merge into the
  integration branch (and a dev deploy when they ask for one).
- Agent tasks that used to promote on request now ask a person, who sees what will be promoted
  before typing.
