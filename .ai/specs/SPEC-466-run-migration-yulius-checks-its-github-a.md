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
  secret, a fixed made-up code and the URI to the token endpoint, within the client's request
  timeout (10 s), never retried. `redirect_uri_mismatch` is **missing**. `bad_verification_code`
  is **registered** only when GitHub also refuses a control URL that cannot be registered
  (`https://callback-check.invalid/…`, a second request): a GitHub that takes that one too does
  not compare callback URLs for this App, so it does not say, and the answer is unknown with
  that reason, never "registered". Anything else (another OAuth error, an HTTP error, a timeout,
  no network) is an error whose text never holds the secret. No person's token is used.
- MVC-002 **Three answers, kept.** The instance keeps the last answer for its own callback in
  memory: registered, missing, or unknown with the reason, and when it was given. Without a GitHub
  App there is no answer and GitHub is not asked.
- MVC-003 **When it asks.** At start (in the background) and when the manifest flow has stored the
  App; on demand (MVC-011's "Check again" and the return to the page, MVC-020); and,
  while the answer is not "registered", when it is read (the Instance page, the sign-in page) and
  is due: 30 s old at first, so a missing callback is re-checked until it is registered, and
  nothing asks GitHub while nobody looks. A page left open does not ask for ever at that pace:
  after eleven answers in a row that say the same and were asked for by reads, the wait doubles
  with each one, up to 4 minutes (so the kept answer is at most that far behind a callback added
  since, for whoever reads next); a demand or a changed answer makes it 30 s again, also a demand
  that took the answer of a check already on its way. A clock that steps back silences nothing:
  a demand asks whatever the clock says, and an answer dated after now is due. A GitHub sign-in that completes proves the
  callback: GitHub exchanged a code for it, so the answer is "registered" at once, without
  another question, and an answer that was on its way meanwhile does not undo it. One check runs
  at a time: a second request waits for it and takes its answer, and a check that dies does not
  stop the next ones.

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
  After a move: where the instance came from, when, and what the first start did with the tasks
  (back in the queue, left stopped, could not be queued).
- MVC-012 **Sign-in page.** `GET /api/setup/status` says `github_callback_missing` (true only for
  the answer "missing"). The page then says, in a fixed sentence, that GitHub says it does not
  know this address, that GitHub sign-in will not complete until the superadmin adds it to the
  App (Settings → Instance says how) and that the superadmin's password works; the password form
  is open. While it says so the page reads the status once more 3 s later (reading it made the
  instance ask GitHub again), and the sentence goes in place once the callback is there.
  Password sign-in is unchanged.
- MVC-013 **The check never blocks sign-in.** `GET /api/auth/github/start` goes to GitHub whatever
  the kept answer is and asks GitHub nothing: GitHub's own page stays the authority, no visitor
  can make the instance ask, and the sign-in that completes proves the callback (MVC-003).

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
  cannot ask, the general note as before. `ok` is said only of the URL the move goes to: an
  instance that answers for another one (an environment file still names the old URL) is
  `manual`, with that said. What the instance or GitHub said reaches the checklist without
  control characters and without characters that reorder or hide text.

Remote clients

- MVC-030 **Follow a move.** `yulius remote` and its shortcuts (`project`, `task`, `ci`, `vars`,
  `secrets list`, `secrets delete`): an answer that is a redirect with `X-Yulius-Moved-To: <new
  origin>` and a `Location` that is the request's own path at that origin moves the saved sign-in
  to the new URL (same token), prints `yulius: <old> moved to <new>; the saved sign-in follows
  it.` on standard error and repeats the call once there. A redirect answered again is an error.
- MVC-031 **Nothing else is followed.** The new origin must be `https` (plain `http` only when the
  old URL was itself a loopback development server), with no credentials, path, query or
  fragment, and another URL than the one asked. Without the header, with another `Location`, or
  with a header that is not such an origin, the redirect is the error it is today and the token
  is sent nowhere else. A move onto a URL where another sign-in is saved is not followed and
  changes nothing (an instance cannot make itself a name of another one); the same sign-in
  saved under both names becomes one.
- MVC-032 **The old name keeps working, and there is a way back.** The store remembers the move
  (`moved` in `remote-credentials.json`), so a command that names the old URL (`--server`,
  `YULIUS_SERVER`) uses the sign-in at the new one. When the instance a sign-in followed does
  not answer, the error says where the sign-in came from and how to go back; the token is sent
  nowhere to find out. `yulius remote login <old URL>` brings the sign-in back after a move that
  was undone: an empty token takes the one that followed the move (tokens are shown once), what
  followed is forgotten, and the names that led there lead to the old URL.

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
  HTTP/1.0 request to the loopback listener (connecting, sending and the whole answer each
  bounded at 45 s, above the 40 s the server needs at worst: two questions of 10 s behind a
  check of two; a
  `public_url` that is not `https://host` is never sent); `line` makes the checklist's "GitHub
  App" line of the answer. The plan asks through `World::github` (the old instance, the new URL);
  before the finish the flow sends `Request::Callback` to the new host, whose step asks the
  instance there about its own URL, so that instance keeps the answer too.
- **Remote client.** `RemoteClient` returns `RemoteError::Moved(new base URL)` for a valid move
  (`moved_base`) and never follows anything itself; `remote_cli::following` moves the sign-in
  (`TokenStore::relocate`, which refuses another sign-in at the new URL), says the line and asks
  once more; `client` resolves a name that moved (`TokenStore::moved_to`); `failed` adds where a
  sign-in came from (`moved_from`) to a transport error; `TokenStore::save` at a URL that had
  moved brings the sign-in back.

## Plan

- [x] Spec SPEC-466 and ADR-0302
- [x] The GitHub client asks whether a callback URL is registered, against the fake GitHub
- [x] The instance keeps its callback's answer: at start, on demand, re-checked while missing
- [x] Address API: the answer, the App's link and values, the move's report
- [x] Settings → Instance shows the Address card, with e2e on the mock
- [x] The sign-in page says when GitHub does not know the address; the check never stops a sign-in
- [x] Internal endpoint; the move's plan and final checklist check the new URL's callback
- [x] `yulius remote` follows a move's redirect and the store remembers it
- [x] Operations guide §5 and §7, remote control, API, architecture, product summary, changes
- [x] Gates green and the independent review answered
- [x] The first review's findings fixed, each with a test that fails without it
- [ ] The second review's findings fixed: the saved token goes nowhere without being typed

## Acceptance criteria

- Against the fake GitHub with registered callbacks: a registered URI answers registered (after
  the control question was refused), another one missing in one request, a wrong client secret
  an error naming GitHub's code and not the secret, a GitHub that does not answer an error within
  the request timeout, and a GitHub that takes every URI no word at all (unknown with the reason
  for the superadmin), never registered.
- A server whose App's callback is not registered answers `state: "missing"` on the address API
  with the organization's (and a person's) settings link and both values; after the callback is
  registered at GitHub a read 30 s after the last answer makes it `registered` without a restart,
  and "Check again" does at once. A callback that stays missing is asked for at 30 s for eleven
  reads, then at 60 s, 120 s and 4 minutes from then on; after "Check again", or an answer that
  changed, at 30 s again. A GitHub sign-in that completes makes the answer `registered` without
  asking GitHub; one GitHub refuses changes nothing. Without an App: `github: null` and GitHub was
  never asked. A member is refused (403).
- After a start with the move's mark the address API names the old URL, the time and the three
  task counts; without `moved.json`, `moved: null`.
- `GET /api/setup/status` says `github_callback_missing: true` only while the answer is missing;
  `GET /api/auth/github/start` redirects to GitHub whatever the answer is and asks GitHub
  nothing; password sign-in works throughout.
- `/internal/github-callback` answers 401 without the key, `{"github": …}` with it (`null`
  without an App), checks another `public_url` without changing the kept answer, and refuses
  `public_url=http://x` and `https://x/path` (422).
- A move whose URL changes: the plan's "GitHub App" line is `ok` when the old instance says the
  new callback is registered, `manual` with the exact link and both values when missing, and
  `manual` with the general note when the instance cannot be asked; the final checklist's line
  is `ok` when the new instance says registered, and never `ok` when the instance answers for
  another URL than the new one.
- `yulius remote` against a server that answers 307 with the header and the same path at another
  server: the call is answered by the new server, the store holds the token under the new URL
  only, the line is on standard error; `--server <old>` works afterwards. A redirect without the
  header, to another path, to `http` from an `https` server, or with a header carrying a path is
  not followed and the store is unchanged; a second move in a row is an error. A move onto a URL
  where another sign-in is saved is not followed and the file is unchanged byte for byte. After
  a move that is undone, the failing command names the URL the sign-in came from without
  contacting it, and `yulius remote login <old URL>` with an empty token brings the sign-in
  back: the file holds the token under the old URL only.
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

## Evidence

Head of the gates: `c84185d3` (2026-11-11), which holds `dev` up to `4b59bce3` (#1173); the
commits after it change this document only.

- `scripts/dev ci`: exit 0; nextest 3681 tests run, 3681 passed, 16 skipped; clippy,
  doctests, docs and the repository check.
- e2e: the whole suite (`cd e2e && npx e2e run`) at `8e687075`: 79 files passed, 1 skipped; 940
  tests passed, 1 skipped. At the gates' head, where only a comment of the sign-in page changed
  since: `sign-in`, `instance-address`, `instance-limits` and `claude-code`, 36 tests passed.
  New: `instance-address.e2e.ts` (8) and three in `sign-in.e2e.ts`.
- The tests of this change, by requirement:
  - MVC-001: `yulius-github` `tests/client.rs`
    `a_callback_url_is_registered_missing_or_not_known` (with the control question and a GitHub
    that takes every URL), `a_callback_question_ends_when_github_does_not_answer`; through the
    production adapter, `github_callback::the_adapter_asks_the_fake_github`.
  - MVC-002, MVC-003, MVC-010, MVC-012, MVC-013, MVC-020: `crates/yulius/tests/github_callback.rs`
    (19 tests) and `yulius-app` `github_callback::tests` (3, the schedule of re-checks among
    them).
  - MVC-011, MVC-012: the e2e tests above.
  - MVC-021, MVC-022: `instance::migrate::tests::github` (8), four flow tests
    (`a_callback_github_accepts_is_ok_in_the_plan_and_at_the_finish` and the three after it),
    `github_callback::the_move_s_question_is_answered_by_the_running_server`.
  - MVC-030 to MVC-032: `yulius-import` `remote::tests` (3), `token_store::tests` (4 new), and
    with the `yulius` binary against real servers `remote_mcp::the_cli_follows_an_instance_that_moved`,
    `the_cli_does_not_follow_a_move_onto_another_sign_in` and
    `the_cli_finds_its_way_back_after_a_move_that_was_undone`.
- Each behaviour's test was seen to fail without it: before its code was written, or by a
  mutation of it (73 mutations of the kept answer and its schedule, the routes, the pages, the
  move's question and line, the client's control question and the remote client with its
  store). Two survived at first and got their tests (a URL that is not a host reaching the
  request line; the count of equal answers not reset by a changed one), a third was an
  equivalent change. The first review found a fourth that no mutation had been tried on: the
  schedule's test asked only "is GitHub asked by then", never "not before". It now reads at
  29 999 ms and at each longer wait minus one, and ten mutations of the schedule, run against
  the integration tests alone, are all caught.
- `yulius security-scan`: "Security scan disabled" (Vidoc is off for this project): disabled, not
  passed.
- Real runs (2026-11-11):
  - GitHub's live token endpoint through `GitHubClient::callback_registered`, with the GitHub
    CLI's public OAuth client, at `9262383b` (with the control question): its registered callback
    `Ok(Some(true))` after two requests (405 ms), another URL `Ok(Some(false))` after one, a
    wrong secret `Err(OAuth("incorrect_client_credentials"))`. So GitHub refuses the control URL
    for an OAuth app.
  - At `f9590767`, before the later fixes: a real `yulius serve` on loopback (`GET
    /api/setup/status` has `"github_callback_missing":false`; `/internal/github-callback`
    answers 401 without the key, `{"github":null}` with the key of `leftovers-cli.key` and 422
    for `public_url=http://…`; the address API shows a `moved.json`); `yulius instance
    migrate-target` with the step `callback` (the command the move runs on the new host)
    answered `ok` "no GitHub App is connected" from that server, and `manual` with the reason
    for a data directory without the key.
  - At `f9590767`: a real Caddy 2.11.9 with the site block the move writes (`header
    X-Yulius-Moved-To`, `redir … 307`) in front of that server: `yulius remote status` printed
    `yulius: http://127.0.0.1:20110 moved to http://127.0.0.1:20108; the saved sign-in follows
    it.`, answered from the new URL (exit 0), said nothing the second time, worked with
    `--server <old URL>`, and left `remote-credentials.json` (0600) with the token under the new
    URL and the old name in `moved`.
- Independent review, in a fresh read-only session:
  - Round 1 at `401d401f`: changes requested. One major (the schedule's test could not fail
    without the schedule), five minors (a clock that steps back silenced "Check again"; no test
    for the questions at start and after the manifest flow; no way back for a laptop after a
    move that is undone; an instance could make itself a name of another saved one; `ok` said
    without looking at which URL was asked about) and seven nits. All are answered in code with
    a test each, or in the documents; the control question came from its remark that the
    checklist now turns to `ok` on an answer measured only with an OAuth app.
  - Round 2: asked for the head that holds this section. The merge (`delivery_merge`) is
    refused without its approval of the exact head and records its verdict.

Changed while it was built, and why:

- A sign-in start first returned to the sign-in page while the callback was known as missing.
  The answer rests on what GitHub says to a made-up code, measured with an OAuth app and promised
  nowhere for a GitHub App, so it may not be able to stop a sign-in: a start always goes to
  GitHub and asks nothing, a sign-in that completes proves the callback, and "registered" is
  believed only after the control question.
- Reads asked GitHub again every 30 s for as long as a page stayed open; the wait now doubles
  after eleven equal answers, up to 4 minutes.

Not tested here:

- GitHub's answer for a real GitHub App: a sandbox holds no App secret. The endpoint, the order of
  its checks and the answers were measured with an OAuth app's public client. If an App is
  treated otherwise, the control question turns the answer into "unknown" (GitHub takes every
  URL) or the pages say "missing" wrongly (GitHub refuses every URL) while sign-in works all the
  same and the first sign-in that completes corrects it. On an instance with an App, Settings →
  Instance → Address shows the answer at once.
- A real move between two hosts whose instance has a GitHub App, and the plan's question on a real
  old host (the root command needs systemd and a unit to freeze): covered by the fake-host flow
  tests and by the real `migrate-target` step above.
- `https` between the CLI and a moved instance (the real run used loopback `http`, which the
  client allows only there); the `https`-only rule is covered by `no_other_redirect_is_a_move`.
- That the sign-in page and the Address card ask nothing in a loop over hours (the e2e runner has
  no clock to turn): the single delayed read and the 5 s re-read are covered, their absence
  afterwards is not.
