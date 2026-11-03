# 0260. A playground's guests come in through an expiring link

## Status

Accepted. Specified in [SPEC-410](../../.ai/specs/SPEC-410-playgrounds-teammates-try-a-playground-w.md);
task #392, issue [#412](https://github.com/Quartio-Studio/yulius/issues/412). **Amends SPEC-001
ACC-005 for playgrounds only**, by the product owner's decision of 2026-11-03 ("#412 - ok"), and
PRJ-012 / [SPEC-099](../../.ai/specs/SPEC-099-on-onboarding-we-want-a-mode-to-play-aro.md) PLY-005
(a playground was its maker's and the superadmin's alone). Builds on
[ADR-0081](0081-a-playground-keeps-its-git-on-the-server.md).

## Context

ACC-005, a founder requirement, says "Every user authenticates with GitHub; there is no user
password." A playground lets a team try Yulius before it connects GitHub, but only its maker could
use it: teammates without an admitted GitHub login had no way in, so "the team tries it" meant one
person. Issue #412 was left for the product owner since 2026-10-15 because an agent cannot overturn
a user requirement; on 2026-11-03 they accepted it.

What a guest must not reach is everything outside the one playground. Yulius's permissions grow
from GitHub's: write access also configures a sandbox (automations, MCP servers), manages its
agent secrets and variables, and several instance-wide routes (team notes, team secrets, status,
API tokens) are open to any signed-in person. So a guest is not simply a writer.

## Decision

- **ACC-005 stays for everyone but a playground's guests.** A guest is a person a playground's
  admin (its maker, the superadmin) lets in without GitHub. A guest has no password either: the
  rest of ACC-005 ("there is no user password") still holds.
- **The link is the credential.** The admin creates a link `<public_url>/join?code=<code>` with a
  256-bit random code, valid 1, 7 or 30 days (7 by default). Only its SHA-256 is stored, and the
  link is shown once. Anyone holding it joins by typing a name: Yulius makes a guest account (no
  GitHub link, no password, tied to the link in `playground_guests`) and a browser session that
  ends when the link does. Bounds: 5 live links per playground, 20 guests per link, 10 join
  attempts per client a minute (ACC-013's client key).
- **A guest works in one playground and nothing else.** Access gives a guest write on the link's
  playground while the link lives and the sandbox is still a playground; none elsewhere.
  `App::require` refuses a guest `Configure` and `Administer`. The server's `Auth` extractor lets a
  guest's request through only on an allowlist (the playground's tasks, sessions, shells, files,
  Git, browser, attachments, notes, previews, stages, CI, intake; the task, session and shell
  routes; `/api/me`, the event stream, logout and their own notes): every other route answers 403
  (a route added later outside those prefixes too). The event stream gives a guest only their
  playground's events, and their tasks' agents get no tool that configures (`automation_*`).
- **A guest shares the playground's workspace, so it holds no secret from then on.** Inside the
  playground a guest uses its files and worktrees, terminals and browser as its maker does. So once
  a guest has joined, no run of the playground (theirs, the maker's, a CI job's) gets an agent
  secret and no saved agent state is restored into or saved from it, also after the link ends. The
  Invite dialog says what is shared.
- **Revoking ends it at once.** The admins list the live links with their guests and revoke one:
  the link stops admitting, its guests' sessions are revoked (401 on the next request, open sockets
  close at their next re-check, ACC-008), their access ends and their live and waiting tasks and
  terminals are stopped. Expiry does the same: the session ends then, and the background sweep stops
  what its guests still run.
  Moving the playground to GitHub (SPEC-176) or removing it ends guest access, as access then
  mirrors GitHub, which a guest has not.
- **A guest never becomes a GitHub user.** A teammate who later signs in with GitHub is admitted
  by login like anyone (ACC-004) and is a different account; the guest's tasks keep their owner and
  can be handed over (SPEC-070). Wherever a person is named, a guest is "<name> (guest)", never
  "superadmin".

## Alternatives

- **Local accounts with a password, limited to playgrounds** (the issue's first framing). A second
  password system, with its own recovery, rate limits and storage, for people who only try a
  playground for days; ACC-005's "no user password" would fall too.
- **Email invitations.** Yulius sends no mail and an instance has no mail server.
- **One shared guest account per playground.** No way to tell guests apart in tasks and audit, or
  to end one link without the others.
- **A sandbox of their own per guest, or containment per person inside the playground.** A new
  isolation boundary; trying the maker's code means working in the maker's workspace.
- **Guests as writers, with a denylist of dangerous routes.** Fails open: every new route would be
  reachable by guests until someone remembered to deny it.
- **A guest flag on `UserRecord`.** 32 literal sites and parallel tasks that construct it; the
  `playground_guests` row ties a guest to its link, which revocation needs anyway.
- **Let the link stay valid and only stop new joins on expiry.** A guest would then outlive the
  window the maker chose; the session ending with the link needs no sweeper.

## Consequences

- A team tries a playground together; its maker decides who, for how long, and can stop it.
- Anyone holding a live link joins: the maker shares it as a secret, and revokes it when it leaks.
- The allowlist must grow when a playground page needs a new route for guests; the fence's unit
  test lists it.
- A guest's tasks use the instance's Claude accounts like the maker's, and every agent run's
  environment carries that account's token (as before this change, ADR-0005), so a guest's task can
  read it and it outlives the link. Hiding it behind the egress proxy is a separate change.
- A guest reaches what the playground's workspace holds: its files (a `.env` written earlier), its
  browser's signed-in sites. Make links only for people you would let work in it, and keep nothing
  there you would not show them. Its agent secrets stop applying once a guest joined.
