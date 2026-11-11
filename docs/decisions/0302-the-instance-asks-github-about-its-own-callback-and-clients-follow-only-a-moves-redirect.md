# 0302. The instance asks GitHub about its own callback, and clients follow only a move's redirect

## Status

Accepted. Specified in
[SPEC-466](../../.ai/specs/SPEC-466-run-migration-yulius-checks-its-github-a.md); task #446, slice
C of [0297](0297-one-root-command-moves-an-instance-and-drives-the-new-host-over-a-one-time-key.md)
(SPEC-460 MIG-042, MIG-043).

## Context

- After a move changed the public URL, GitHub sign-in failed until someone added the new callback
  URL to the GitHub App by hand, and nothing in Yulius said so. GitHub has no API to read or change
  an App's callback URLs or homepage; its token endpoint answers `redirect_uri_mismatch` or
  `bad_verification_code` for a made-up code, depending on whether the URI is registered.
- The App's slug, which its settings link needs, is in the sealed credentials only the running
  server opens. `yulius instance migrate` runs as root beside the server and could only describe
  where the settings are.
- The old host answers every request with a 307 to the new URL and the header
  `X-Yulius-Moved-To`. `yulius remote` follows no redirect, by design: its token must not travel
  to wherever a response points.

## Decision

- **The token endpoint is the question, and a control question keeps it honest.** The instance
  posts its own client id and secret, a fixed made-up code and its callback URL:
  `redirect_uri_mismatch` means missing, anything unexpected unknown with the reason.
  `bad_verification_code` means registered only if GitHub refuses, in a second request, a URL
  that cannot be registered (under `.invalid`): a GitHub that took any URL would otherwise read
  as "registered" for ever. Bounded requests, no person's token, nothing changed at GitHub.
- **Asked on demand and while someone looks.** At start, when the App is stored, on a superadmin's
  "Check again" or return to the page, for the move's plan, and when a page
  reads an answer that is not "registered" and due: 30 s old at first, and, once eleven such
  answers in a row said the same, twice as old with each further one, up to 4 minutes (a demand
  or a changed answer makes it 30 s again). No loop asks GitHub while nobody looks, a page left
  open asks ever less, and one check runs at a time.
- **A sign-in that completes is the better witness.** When GitHub exchanges a real code for the
  instance's callback URL, the URL is registered: the kept answer becomes "registered" at once,
  whatever the question with the made-up code said before.
- **The check never stops a sign-in.** While the answer is "missing" the sign-in page says so and
  opens the password; "Sign in with GitHub" still leads to GitHub, whose own page stays the
  authority; a start asks GitHub nothing, so no visitor can make the instance ask. "Unknown"
  changes nothing at all.
- **Root's commands get the facts over the loopback listener.** `GET /internal/github-callback`
  takes the key the server writes for `yulius leftovers` (`leftovers-cli.key`) and answers the
  state, the settings link and the two values, for the instance's URL or the one named. The move
  asks the old instance in its plan and the new one, through the staged build, before it finishes.
- **Clients follow a move and nothing else.** `yulius remote` treats a redirect as a move only
  when it carries `X-Yulius-Moved-To` naming a bare `https` origin and its `Location` is the same
  path there. It then saves the sign-in under the new URL, remembers the old name, says one line
  and repeats the call once.

## Alternatives

- **Driving GitHub's settings page with a stored session**: forbidden (0297).
- **`GET /app` to compare the homepage**: it never returns the callbacks, and the homepage does not
  decide whether sign-in works.
- **A loop that checks every minute**: an instance whose callback stays missing would send GitHub
  a failed code exchange every minute for as long as nobody fixes it.
- **Checking only at start**: the superadmin fixes the App while the server runs; the page would
  stay wrong until a restart.
- **Hiding GitHub sign-in, or returning a sign-in start to the sign-in page, while the callback is
  missing**: kinder than GitHub's error page when the answer is right, but the answer rests on
  what GitHub says to a made-up code, measured with an OAuth app and promised nowhere for a GitHub
  App. An advisory check that can refuse sign-in could lock every member out of an instance whose
  sign-in works.
- **A second key file for the new endpoint**: the same people may read it (root, the service
  user), so it would add a file and no boundary.
- **Opening the sealed credentials in the root command**: the master key and the database would
  be read outside the server, while it runs.
- **Following every redirect in the CLI** (or every one to `https`): any response of a server, or
  of whoever answers for its name, could take the token elsewhere.
- **Leaving the old entry in the credentials file**: commands without `--server` would find two
  instances and stop.

## Consequences

- A "registered" answer is as old as the last start or demand: a callback removed at GitHub later
  shows on GitHub's own error page first, and on the Instance page after "Check again".
- The check's meaning rests on GitHub's answers for a made-up code, measured with an OAuth app.
  If GitHub renames the codes, or checks the code before the URI for an App (the control
  question then gets the same answer), the answer is "unknown" with the reason, and the move's
  checklist stays `manual`. If it ever refused a registered URI this way, the pages would say
  "missing" wrongly and sign-in would work all the same.
- Whoever can make the old URL answer over TLS can move a laptop's saved sign-in to another
  `https` origin: the old host stays trusted with the token until its name is retired, as it was
  before the move. It cannot move it onto another instance the laptop is signed in to: a move
  onto a URL with another saved sign-in is refused.
- After a move that was undone, laptops that had followed point at the stopped instance. Their
  CLI says where the sign-in came from and never sends the saved token to the old URL, neither
  to find out nor for a sign-in without a token (a retired name may belong to someone else by
  then). `yulius remote login <old URL>` needs a token typed: the one that followed brings the
  sign-in back, a new one leaves the sign-in that followed beside it until `yulius remote
  logout --server <new URL>` forgets it.
- `yulius login`, `yulius onboard` and `yulius secrets import` do not follow a move (SPEC-466).
