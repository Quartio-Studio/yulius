---
yulius-task: 25decd3b-ed8e-4fcf-ab8e-a49ca9c2b1c2
status: in-progress
---

# SPEC-466: Run migration: Yulius checks its GitHub App callback, Settings → Instance shows the move, the remote CLI follows it

Status: in-progress
Date: 2026-11-11
Owner: Yulius task #446 (slice C of #442)
Scope authorization: task #446, asked by the project's owner: "After a move changes the public
URL, GitHub sign-in failed ('redirect_uri is not associated with this application') until someone
edited the App by hand, and every laptop still pointed at the old URL. Make Yulius notice, say
exactly what to do, and let clients follow." It delivers requirements MIG-042 and MIG-043 of
[SPEC-460](SPEC-460-run-migration-one-command-moves-the-whol.md)
([ADR-0297](../../docs/decisions/0297-one-root-command-moves-an-instance-and-drives-the-new-host-over-a-one-time-key.md)).

## Goal

An instance knows whether GitHub accepts its own sign-in address. When it does not, the superadmin
sees one link and the two values to paste, in Settings → Instance and in the move's checklist,
before the cutover; the sign-in page says why GitHub sign-in cannot work. After a move the
Instance page says where the instance came from and what its first start did with the tasks, and
`yulius remote` on a laptop follows the old URL's redirect to the new one by itself.

## Context

- Production moved on 2026-11-11 to a new URL. GitHub sign-in answered "redirect_uri is not
  associated with this application" until someone added the callback to the App by hand, and
  nothing in Yulius said so. `yulius remote` on laptops kept the old URL: the old host's Caddy
  answers 307 with `X-Yulius-Moved-To` (SPEC-460 MIG-041), which the client reports as "HTTP 307".
- `yulius instance migrate` prints a GitHub note without the App's link
  (`migrate.rs::github_note`): the root command cannot open the sealed App credentials
  (`instance.github_app`), which hold the slug.
- Facts of SPEC-460, verified again for this spec (2026-11-11, GitHub's live endpoint, the GitHub
  CLI's public OAuth client): `POST https://github.com/login/oauth/access_token` with a made-up
  code answers `redirect_uri_mismatch` for a URI that is not registered,
  `bad_verification_code` for one that is, and `incorrect_client_credentials` for a wrong secret
  (HTTP 200 each, about 0.2 s). GitHub has no API to read or change an App's callback URLs or
  homepage.
- The App a manifest creates holds two URLs of the instance: the homepage (`url`) and one callback
  (`{public_url}/api/auth/github/callback`); no webhook and no setup URL
  (`yulius-github::app_manifest`).
- What exists and is built on: `GitHubClient::exchange_code` (the same token request),
  `yulius_github::fake::FakeGitHub`, the loopback listener's `/internal/leftovers` with the key
  the server writes to `<data>/leftovers-cli.key` at every start (SPEC-215), `<data>/moved.json`
  (`host_move::MoveReport`), the move's `World` and target `Env` (fakes in its tests), the
  Instance page's cards, `GET /api/setup/status` (what the sign-in page reads before sign-in).

## Requirements

The check

- MVC-001 **Ask GitHub.** `GitHubClient::callback_registered(uri)` posts the App's client id and
  secret, a fixed made-up code and the URI to the token endpoint, once, within the client's
  request timeout (10 s). `bad_verification_code` is **registered**, `redirect_uri_mismatch` is
  **missing**; anything else (another OAuth error, an HTTP error, a timeout, no network) is an
  error whose text never holds the secret. No person's token is used.
- MVC-002 **Three answers, kept.** The instance keeps the last answer for its own callback in
  memory: registered, missing, or unknown with the reason, and when it was given. Without a GitHub
  App there is no answer and GitHub is not asked.
- MVC-003 **When it asks.** At start (in the background) and when the manifest flow has stored the
  App; on demand (MVC-011's "Check again" and the return to the page, MVC-013, MVC-020); and,
  while the answer is not "registered", when it is read (the Instance page, the sign-in page) and
  is due: 30 s old at first, so a missing callback is re-checked until it is registered, and
  nothing asks GitHub while nobody looks. A page left open does not ask for ever at that pace:
  after eleven answers in a row that say the same and were asked for by reads, the wait doubles
  with each one, up to 32 minutes; a demand or a changed answer makes it 30 s again. One check
  runs at a time: a second request waits for it and takes its answer.

Settings → Instance and the sign-in page

- MVC-010 **API.** `GET /api/admin/instance/address` (superadmin) answers the public URL, the
  GitHub answer (`state`, `reason`, `callback_url`, `homepage_url`, `settings_url`, `checked_at`;
  `null` without an App) and the move (`from`, `to`, `at`, `queued`, `left`, `failed` of
  `moved.json`; `null` when the instance never moved). `settings_url` is
  `https://github.com/organizations/<owner>/settings/apps/<slug>` for an organization and
  `https://github.com/settings/apps/<slug>` for a person. `POST /api/admin/instance/address/check`
  asks GitHub now and answers the same view.
- MVC-011 **The card "Address".** On the Instance page: the public URL; registered: one line;
  missing: a warning with the one link "Open the App's settings on GitHub", the callback URL and
  the homepage URL, each with a copy button, the note that the old callback may stay until the old
  host is retired, and a re-read every 5 s until it is registered (said in place, no reload; the
  latest read wins, and one that fails keeps what the card shows); coming back to the page
  (another tab, GitHub's settings) asks GitHub at once; unknown: the reason and "Check again".
  After a move: where the instance came from, when, and
  what the first start did with the tasks (back in the queue, left stopped, could not be queued).
- MVC-012 **Sign-in page.** `GET /api/setup/status` says `github_callback_missing` (true only for
  the answer "missing"). The page then says, in a fixed sentence, that GitHub says it does not
  know this address, that GitHub sign-in will not complete until the superadmin adds it to the
  App (Settings → Instance says how) and that the superadmin's password works; the password form
  is open. While it says so the page reads the status once more 3 s later (reading it made the
  instance ask GitHub again), and the sentence goes in place once the callback is there.
  Password sign-in is unchanged.
- MVC-013 **The check never blocks sign-in.** `GET /api/auth/github/start` goes to GitHub whatever
  the kept answer is: GitHub's own page stays the authority. A start while the answer is
  "missing" and older than 5 s makes the instance ask GitHub again beside it (a demand), so a
  callback registered since is known at the next look.

The root command

- MVC-020 **Internal endpoint.** `GET /internal/github-callback[?public_url=https://host]` on the
  loopback listener, with `Authorization: Bearer <key of <data>/leftovers-cli.key>`: asks GitHub
  now about the callback of that public URL (default: the instance's own, whose kept answer it
  replaces) and answers `{"github": …}`: `state`, `reason`, `callback_url`, `homepage_url`,
  `settings_url`, `checked_at`, or `null` without an App. 401 without the key; 422 for a
  `public_url` that is not `https://host`. No credential is in the answer.
- MVC-021 **The plan.** When the URL changes, the plan of `yulius instance migrate` asks the old
  instance (it still runs) about the new URL's callback: registered is `ok`; missing is `manual`
  with the exact link and both values; no App is `ok` (nothing to register); when the instance
  cannot be asked or GitHub gives no answer, `manual` with the reason and what is known (the
  exact link when the instance answered, else the general note).
- MVC-022 **The final checklist.** Before it removes its key the flow asks the new instance, on
  the new host, through the staged build, and the checklist's "GitHub App" line is `ok` when the
  callback is registered and `manual` with the exact link and both values when it is not; when it
  cannot ask, the general note as before.

Remote clients

- MVC-030 **Follow a move.** `yulius remote` and its shortcuts (`project`, `task`, `ci`, `vars`,
  `secrets list`, `secrets delete`): an answer that is a redirect with `X-Yulius-Moved-To: <new
  origin>` and a `Location` that is the request's own path at that origin moves the saved sign-in
  to the new URL (same token), prints `yulius: <old> moved to <new>; the saved sign-in follows
  it.` on standard error and repeats the call once there. A redirect answered again is an error.
- MVC-031 **Nothing else is followed.** The new origin must be `https` (plain `http` only when the
  old URL was itself a loopback development server), with no credentials, path, query or
  fragment, and another URL than the one asked. Without the header, with another `Location`, or with a header that is not such an
  origin, the redirect is the error it is today and the token is sent nowhere else.
- MVC-032 **The old name keeps working.** The store remembers the move (`moved` in
  `remote-credentials.json`), so a command that names the old URL (`--server`, `YULIUS_SERVER`)
  uses the sign-in at the new one. `yulius remote login` at a URL forgets a move recorded from it.

## Design

- **Port and adapter.** `GitHubPort::callback_registered(app, redirect_uri) -> AppResult<bool>`
  (default: unavailable); `GitHubAdapter` maps it to the client. The fake GitHub server gets a
  list of registered callback URLs (`set_callback_urls`; unset: every URI is registered, as
  before) and checks it after the client credentials and before the code, as GitHub does.
- **Application** (`yulius-app::github_callback`): the kept answer (`CallbackKept`: the state,
  when it was given, and how many read-triggered answers in a row said the same), the check
  behind one async lock, the read that starts a background check when the answer is due
  (`Kept::stale`, `recheck_after`), `settings_url`, and the views.
  States: no answer yet (owner: the start's or a read's background check; exit: its answer),
  `unknown`/`missing` (owner: whoever reads next once it is due, or a demand; exit: `registered`),
  `registered` (kept until the next start or demand).
- **Move** (`instance/migrate/github.rs`). `asked_instance` reads the key file and sends one
  HTTP/1.0 request to the loopback listener (connecting, sending and each read bounded at 30 s,
  above the 20 s the server needs at worst for a check behind another; a
  `public_url` that is not `https://host` is never sent); `line` makes the checklist's "GitHub
  App" line of the answer. The plan asks through `World::github` (the old instance, the new URL);
  before the finish the flow sends `Request::Callback` to the new host, whose step asks the
  instance there about its own URL, so that instance keeps the answer too.
- **Remote client.** `RemoteClient` returns `RemoteError::Moved(new base URL)` for a valid move
  (`moved_base`) and never follows anything itself; `remote_cli::following` moves the sign-in
  (`TokenStore::relocate`), says the line and asks once more; `client` resolves a name that moved
  (`TokenStore::moved_to`).

## Plan

- [x] Spec SPEC-466 and ADR-0302
- [ ] The GitHub client asks whether a callback URL is registered, against the fake GitHub
- [ ] The instance keeps its callback's answer: at start, on demand, re-checked while missing
- [ ] Address API: the answer, the App's link and values, the move's report
- [ ] Settings → Instance shows the Address card, with e2e on the mock
- [ ] The sign-in page says when GitHub does not know the address, and sign-in start re-checks
- [ ] Internal endpoint; the move's plan and final checklist check the new URL's callback
- [ ] `yulius remote` follows a move's redirect and the store remembers it
- [ ] Operations guide §5 and §7, remote control, API, architecture, product summary, changes
- [ ] Gates green and the independent review answered

## Acceptance criteria

- Against the fake GitHub with registered callbacks: a registered URI answers registered, another
  one missing, a wrong client secret an error naming GitHub's code and not the secret, a GitHub
  that does not answer an error within the request timeout.
- A server whose App's callback is not registered answers `state: "missing"` on the address API
  with the organization's (and a person's) settings link and both values; after the callback is
  registered at GitHub a read 30 s after the last answer makes it `registered` without a restart,
  and "Check again" does at once. A callback that stays missing is asked for at 30 s for eleven
  reads, then at 60 s, 120 s and so on up to 32 minutes; after "Check again", or an answer that
  changed, at 30 s again. Without an App: `github: null` and GitHub was never asked. A member is
  refused (403).
- After a start with the move's mark the address API names the old URL, the time and the three
  task counts; without `moved.json`, `moved: null`.
- `GET /api/setup/status` says `github_callback_missing: true` only while the answer is missing;
  `GET /api/auth/github/start` redirects to GitHub whatever the answer is and, while it is
  missing and older than 5 s, makes the instance ask GitHub again, after which the status no
  longer says missing once GitHub accepts the callback; password sign-in works throughout.
- `/internal/github-callback` answers 401 without the key, `{"github": …}` with it (`null`
  without an App), checks another `public_url` without changing the kept answer, and refuses
  `public_url=http://x` and `https://x/path` (422).
- A move whose URL changes: the plan's "GitHub App" line is `ok` when the old instance says the
  new callback is registered, `manual` with the exact link and both values when missing, and
  `manual` with the general note when the instance cannot be asked; the final checklist's line
  is `ok` when the new instance says registered.
- `yulius remote` against a server that answers 307 with the header and the same path at another
  server: the call is answered by the new server, the store holds the token under the new URL
  only, the line is on standard error; `--server <old>` works afterwards. A redirect without the
  header, to another path, to `http` from an `https` server, or with a header carrying a path is
  not followed and the store is unchanged; a second move in a row is an error.
- e2e on the mock: the Address card shows the warning, the link, both values and the move; after
  GitHub accepts the callback the card says so without a reload; "Check again" works while the
  card reads by itself and a read that answers late changes nothing; coming back to the page asks
  GitHub at once; a re-read that fails keeps the card; the sign-in page shows the sentence and the
  open password form.

## Test plan

- `yulius-github` (`tests/client.rs`): MVC-001 against `FakeGitHub`, the timeout against a
  listener that never answers.
- `crates/yulius/tests/github_callback.rs` (the app harness with its GitHub port fake): MVC-002,
  MVC-003, MVC-010, MVC-012, MVC-013, MVC-020, with failure cases (GitHub down, no App, a member,
  a bad key, a bad URL).
- `instance::migrate` unit and flow tests (`tests/github.rs`, `tests/flow.rs`): MVC-021, MVC-022
  with fake hosts; `asked_instance` against a loopback listener, and against the real loopback
  router in `github_callback.rs`.
- `yulius-import` unit tests (`remote.rs`, `token_store.rs`) and a test of the `yulius` binary
  with real servers that redirect (`remote_mcp.rs`): MVC-030 to MVC-032.
- e2e (`e2e/tests/instance-address.e2e.ts`, `sign-in.e2e.ts`) on the mock.
- Gates: `scripts/dev ci`, the e2e suite, `yulius security-scan` (Vidoc is off for this project:
  disabled, not passed).
- Not testable here: GitHub's answer for a real GitHub App (no App secret in a sandbox; the
  endpoint and the three answers were measured with an OAuth app's public client), and a real
  move between two hosts with a GitHub App.

## Decisions

- [ADR-0302](../../docs/decisions/0302-the-instance-asks-github-about-its-own-callback-and-clients-follow-only-a-moves-redirect.md)
  records the lasting ones: the token endpoint as the question; asked on demand and while someone
  looks, never in a loop; the loopback endpoint behind the CLI key so root's move gets the facts
  of the sealed credentials; clients follow only a redirect that names itself a move.
- **One key file for root's commands.** `/internal/github-callback` takes the key of
  `leftovers-cli.key`: the same trust (root or the service user on the host, while the server
  runs), one file to write at start and to explain.
- **The check is advice; it never stops a sign-in.** The page says what is wrong and the button
  stays and leads to GitHub. Returning a start to the sign-in page while the answer is "missing"
  (as a first version of this change did) would explain more than GitHub's error page, but it
  would let the check lock every member out if GitHub ever answered a made-up code otherwise for
  a GitHub App than for the OAuth app it was measured with (not testable here). A wrong answer
  now costs a wrong sentence, never a sign-in.
- **One pull request.** The four parts are one slice of SPEC-460 as its owner cut it, and the
  chained task #447 builds on all of it; each part is small and has its own tests.
- **Not in this change.** `yulius login`, `yulius onboard` and `yulius secrets import` (the
  import client and its own credentials file) do not follow a move: MIG-043 names `yulius
  remote`; they say the redirect and take `--server <new URL>`. Claude Code's own MCP connection
  (`claude mcp add`) cannot be changed from the server: the checklist keeps its line.
