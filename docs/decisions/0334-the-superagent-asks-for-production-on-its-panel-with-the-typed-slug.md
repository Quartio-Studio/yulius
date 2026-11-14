# 0334. The Superagent asks for production on its panel, with the project's slug typed

## Status

Accepted. Specified in
[SPEC-487](../../.ai/specs/SPEC-487-the-superagent-can-do-everything-its-per.md) (REL-001 to
REL-006), task #469, slice 8b; the person's decision of 2026-11-13. Keeps
[0110](0110-only-a-person-deploys-production.md) (only a person deploys production, typing the
project's slug; agents never promote): the person still types it, now on the Superagent's panel.
Builds on [0333](0333-the-superagents-requests-to-its-person-last-until-answered.md) (requests
that last) and [0330](0330-the-superagent-does-what-its-person-may-do.md).

## Context

Asked to deploy production, roll it back or promote, the Superagent could only refuse and send
its person to the CD page, though it had prepared everything (wall 4 of task #469). The person
decided that production deploy and rollback, and promotion, go behind one confirmation in the
Superagent's panel with the typed project slug, stored, amendable and lasting; and that a
project's onboarding starts the same way.

## Decision

- **A request, not a refusal.** The Superagent's `cd_deploy` and `cd_rollback` of a
  production-stage environment (`prod`, or one whose `[cd.<name>]` has stage `prod`), `promote`
  and `onboarding_start` record a request on its own panel, as `person_ask` does (0333): it
  waits until the person answers or the Superagent withdraws it, across a restart. A dev-stage
  deploy or rollback runs at once, as the CD page's (`cd_rollback` is now a person's project
  tool, also through the remote MCP, dev-stage only there).
- **Refused before anyone is asked** what the page would refuse the person now: a person who is
  no administrator of the project (production, promotion), an environment the project does not
  define, a dev-stage one through this path, onboarding that does not wait to start, unknown
  arguments.
- **The person's confirmation is the CD page's.** The card names the action and the environment
  (and a release, a part or a commit when given); its button takes a click only once the
  project's slug is typed, and the server checks the typed slug before it takes the request (a
  slug mistyped leaves it waiting). Onboarding releases nothing, so it takes a click.
- **Run as that person.** The approval, from a browser session, runs `cd_deploy`,
  `cd_rollback`, `promote_sandbox` or `approve_onboarding` with that person's session and the
  slug they typed: administration, the rollback drill, a moved branch, a GitHub identity for the
  promotion are checked there at that moment, and a refusal is the answer of both the click and
  the waiting call. Audited `superagent.cross_task` with the action, never the slug.
- **Only the Superagent.** No task's agent and no remote MCP token asks this way: a token's
  agent is still refused production ("only a person … on the CD page"); the Global Superagent
  is not served these requests.

## Alternatives

- **Claude Code's permission prompt as the confirmation.** Skipped on a contained server and
  answered once with "always allow"; not the typed slug the CD page asks for. Rejected.
- **A link to the CD page.** What the Superagent did before: the person leaves the
  conversation for a page that asks again. Rejected by the person.
- **Typing the slug in the chat.** The Superagent would hold the confirmation it is meant to
  get; text the agent reads is data, never a person's consent. Rejected.
- **Amending a release request.** A deploy names only an environment and a commit; one that
  changes is withdrawn and asked again, so the card the person confirms never changes under it.

## Consequences

- "Deploy production" and "roll production back" are one sentence to the Superagent and one
  typed slug for the person, who stays an administrator deciding each release.
- A request can be approved long after it was asked; the CD page's checks run at approval, so a
  branch that moved or a drill that did not pass refuses it with why.
