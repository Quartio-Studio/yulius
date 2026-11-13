# 0316. An access rule holds whatever the way in: token scopes, the sign-in's browser, the merge's base, production

## Status

Accepted. Specified in [SPEC-483](../../.ai/specs/SPEC-483-deep-security-and-quality-audit-fix-ever.md)
(slice 4); task #465. Applies [0188](0188-remote-control-uses-scoped-personal-api-tokens.md)
(token scopes) and [0110](0110-only-a-person-deploys-production.md) (production) to entry points
they did not reach; extends [0206](0206-the-mock-is-not-shipped-and-a-real-instance-ignores-mock-1.md) (what a
real instance ships) and [0047](0047-device-sign-in-start-is-limited-and-bounded.md) (bounded
unauthenticated starts).

## Context

The audit of task #465 found rules that held on the way they were written for and not on
another way to the same thing:

- **A `read` API token** reads "only what view access allows" (SPEC-247 RMT-001). The remote
  MCP checked each tool's action against the scope; the REST API refused a read token only by
  method. A GET that needs Work access (the working tree with its ignored `.env` and the
  secrets copied into it, other people's unsaved editor text, the person's shells, previews
  and Superagent) was answered.
- **A revoked API token** "is refused at once" (RMT-001). New requests were; a terminal socket
  it had opened kept typing for up to 30 seconds, because only logouts grew the access epoch.
- **GitHub sign-in** checked the state GitHub echoes back, nothing of the browser. A person who
  started a sign-in and authorized could hand the callback link to someone else, who was then
  signed in as them (what they entered as "themselves" landed in that account). The start is
  open to anyone and stored a state per request, without a bound.
- **The brand lab**, a design aid, was embedded in the binary and loaded for `?lab=`: it
  builds a stylesheet from values in the address, unchecked. A link to the device approval
  page with such values covered the page with its one button: an approval by one blind click,
  which mints an API token of the victim.
- **A task's pull request** is merged "into the integration branch". Nothing but the branch
  policy compared its base, and the policy can be `none`. An agent's own token may change a
  pull request's base: pointed at the production branch, `delivery_merge` brought the branch
  up to date with production and merged it there with the owner's token, a promotion ADR-0110
  excludes.
- **A production deploy** is an administrator's, with the project's slug typed, "by the rule
  every entry point uses" (ADR-0110). The command of a delivery environment that follows
  production ran for anyone with write access and `confirm: true`.

## Decision

- **The scope narrows every access check of a request.** `App::require_for(ctx, sandbox,
  action)` is `require` plus the token's scope, and every check of a request's caller goes
  through it. A `read` token gets 403 where the action is above View, whatever the method.
- **Revoking an API token grows the access epoch**, as a logout does: a socket it opened asks
  again before its next key.
- **A GitHub sign-in is bound to the browser that started it.** The start sets a cookie with
  the state (`__Host-yulius_oauth`, HttpOnly, SameSite=Lax, 10 minutes); the callback is
  honoured only where that cookie holds the state it carries, else it answers `expired`
  without spending the state. Starts are limited to 10 a minute per client and 1000 waiting
  states in all; past either the sign-in page says `busy`.
- **The brand lab is not shipped.** `lab.js` and `lab-proposals.js` are left out of the
  embedded assets as the mock is, and the page loads the lab only in mock mode.
- **A task's change merges only into the integration branch of its own repository.** The
  merge rule in the domain has the fact (`DeliveryFacts::base_is_integration`), so every merge
  goes through it; `delivery_merge` refuses to arm, and an armed merge is dropped with what to
  do, when the pull request goes elsewhere. Nothing is brought up to date with another base.
- **The command of an environment that follows production is a production deploy**:
  `cd::may_deploy` decides it (an administrator, the slug in `slug`), as for the CD page and a
  promotion.

## Alternatives

- **Refuse a read token on a list of GET routes.** The next route needing more than View is
  forgotten; the check belongs where the action is named.
- **Spend the state when a callback arrives without its cookie.** Anyone holding a link could
  then cancel the starter's sign-in.
- **Store the browser binding with the state** (a hash of a second nonce). The state is in
  the starter's address bar already; the cookie carrying it proves the same browser.
- **Check the lab's values instead of not shipping it.** The lab is for mock mode; every
  value it takes from the address would stay a way to style a real page.
- **Keep the base check in the branch policy only.** The policy is a project's choice
  (`none` is one); where a task's change may land is not.
- **Let a writer run a production-stage command with the slug typed.** ADR-0110 says who
  deploys production; a second, weaker way in is what this record removes.

## Consequences

- A dashboard or script with a `read` token that read files, shells, previews or the
  Superagent over REST gets 403 and needs an `operate` token.
- A sign-in started in one browser cannot be finished in another, and third-party cookie
  blocking does not matter: the cookie is first-party on a top-level navigation.
- `?lab=` does nothing on a real instance; the e2e suite's lab test runs in mock mode.
- A project whose environment after production has a command needs an administrator for
  it, with `"slug"` in the request; a manual checkpoint (no command) is confirmed as before.
- The server's response headers still carry no script or style policy
  (`Content-Security-Policy` is `frame-ancestors 'none'` only); that is finding W1b of
  SPEC-483, for a later slice with a browser run against a real server.
