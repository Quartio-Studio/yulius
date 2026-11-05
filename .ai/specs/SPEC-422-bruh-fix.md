---
yulius-task: 2d37216d-f24f-4bfe-887f-666d3d161f10
status: in-progress
---

# SPEC-422: Sign-in without a GitHub App never shows a raw error

Status: in-progress
Date: 2026-11-05
Owner: Yulius task #404
Scope authorization: task #404, asked by the project's owner with two screenshots ("bruh, fix"):
the sign-in page offers "Sign in with GitHub", and the click shows
`{"error":{"code":"conflict","message":"the GitHub App is not configured yet"}}`.

## Goal

On an instance whose GitHub App is not connected yet, the sign-in page does not offer a GitHub
sign-in that cannot work, says why, and points the superadmin at the password; a GitHub sign-in
link opened anyway lands on the sign-in page with that sentence, never on a JSON error body.

## Context

- Symptom: signed out, `/signin` shows "Sign in with GitHub" and "Use a different GitHub account";
  both open `GET /api/auth/github/start`, which answers 409 JSON (`App::github_app`,
  `crates/yulius-app/src/auth.rs`) because no GitHub App is stored for the instance.
- Root cause: (1) `github_start` (`crates/yulius-server/src/routes/setup_auth.rs`) turns every
  error into the API's JSON body, while its sibling `github_callback` already redirects refusals to
  `/signin?error=<reason>` (ACC-004: "never to a raw error"); (2) the sign-in page cannot know the
  App is missing: only `GET /api/me` reports `instance.github_configured`, and it needs a session.
  The public `GET /api/setup/status`, which the page already reads at boot, says only
  `setup_required`.
- Why tests missed it: `http_auth.rs` starts GitHub sign-in only after `configure_github`; the
  e2e mock's `/api/setup/status` and sign-in page never model `github=0` signed out.
- Same pattern elsewhere: Home's "Sign in with GitHub" banner and the Secrets page's re-auth link
  open the same route signed in; they get the redirect too (on an instance without an App those
  are not shown, as `github_configured` is false there).

## Requirements

- GSU-001. `GET /api/auth/github/start` on an instance without a GitHub App answers 303 to
  `/signin?error=github_unconfigured[&next=<next>]` (the `next` kept only when it is a local
  path), with no JSON body; any other failure to start answers 303 to
  `/signin?error=failed[&next=<next>]` and is logged.
- GSU-002. `GET /api/setup/status` (public) also says `github_configured: bool`: whether the
  instance has a GitHub App. No other instance detail is exposed.
- GSU-003. The sign-in page, when `github_configured` is false, shows no GitHub button and no
  "Use a different GitHub account"; it says "GitHub sign-in isn't set up on this instance yet. The
  superadmin signs in with the password below and connects GitHub under Administration." and opens
  the superadmin password disclosure. The `github_unconfigured` reason says the same sentence,
  once; with GitHub connected that reason is stale and not shown.
- GSU-004. The page follows GitHub being connected after it loaded: at sign-out or an expired
  session it uses what the session knew, then reads the setup status again and shows or hides
  GitHub sign-in in place when that changed: a typed password, the open form and focus stay;
  an answer for a sign-in page no longer shown changes nothing (review rounds 1-3).

## Plan

- [x] Reproduce the defect and find its root cause
- [x] GitHub sign-in start redirects to the sign-in page instead of a JSON error
- [x] Setup status says whether GitHub is configured
- [x] Sign-in page hides GitHub sign-in until the App is connected
- [ ] Independent review answered and docs current
- [x] Sign-in page follows GitHub being connected after load (review round 1)
- [ ] Gates green and delivered

## Acceptance criteria

- Signed out on an instance without a GitHub App, `/signin` shows no "Sign in with GitHub", says
  why, and the password form is open; the password still signs in.
- `GET /api/auth/github/start?next=/x` without an App → 303 `/signin?error=github_unconfigured&next=/x`.
- `GET /api/setup/status` → `{"setup_required": false, "github_configured": false|true}`.
- With an App, sign-in is unchanged (existing `http_auth.rs` and `sign-in.e2e.ts` pass).

## Test plan

- Rust (`crates/yulius/tests/http_auth.rs`): start without an App redirects with the reason and
  `next`; setup status reports `github_configured` false then true after `configure_github`.
- e2e (`e2e/tests/sign-in.e2e.ts`): mock `signedout=1&github=0` shows no GitHub button, the
  sentence, an open password form that signs in; `/signin?error=github_unconfigured` says it.
- `signInProblem('github_unconfigured')` covered by the e2e above.
- Review round 1: Rust, a stored App that cannot be read → `/signin?error=failed&next=…`
  (checked to fail with the `failed` branch mutated back to the JSON error); e2e, GitHub connected
  while signed in (`finishGithubApp`) is offered after Sign out, and `error=github_unconfigured`
  with GitHub connected shows no reason.
- Review rounds 2-3 (e2e, `/api/setup/status` held through `__yuliusMock.holdPath`/`held`, each
  hold proven by `heldWaiting`): a password typed while the read is held survives GitHub showing
  in place; GitHub connected in this session is offered at sign-out while the read is held
  (fails with the teardown capture removed); a late answer for an earlier sign-in page leaves the
  current page right. Positive checks assert the GitHub button, the other-account link and the
  divider are visible (fail when `setGithub(true)` leaves them hidden).

## Decisions

- Redirect (server) and hide (page) both: the redirect alone still offers a dead button; hiding
  alone leaves raw JSON for old links, bookmarks and Home's banner. Small and reversible: no ADR.
- `github_configured` on the public setup status rather than a new endpoint: the page already
  reads it at boot (one round trip, PERF-11) and the fact is already observable by clicking.

## Review

Round 1 (fresh-session reviewer, head 7fc9ab35): request changes.

| # | Finding | Kind | Answer |
| --- | --- | --- | --- |
| 1 | The page kept the GitHub state read at load; after connecting GitHub and signing out it still hid GitHub sign-in | own code | Fixed test-first (GSU-004) |
| 2 | The `error=failed` branch had no test | own code | Test added, proven by mutation |
| 3 | Docs said `failed` drops `next` | own code | Docs and GSU-001 say `[&next=…]` |
| 4 | Plan boxes unticked | own code | Ticked as steps finish |
| 5 | A stale `github_unconfigured` reason showed beside a working GitHub button | own code | Fixed test-first |
| 6 | A malformed query (`next=%FF`) on start/callback answers axum's plain 400 | pre-existing | Outside this change (present on dev before it): #1070 |

Round 2 (fresh-session reviewer, head a9bf2353): request changes.

| # | Finding | Kind | Answer |
| --- | --- | --- | --- |
| 1 | The re-read redrew the page, dropping a typed password, the open form, focus or a password error | own code | Fixed test-first: the page shows or hides GitHub in place (`setGithub`) |
| 2 | No test failed without the teardown capture | own code | Test with the status read held after connecting GitHub in the session; checked to fail with the capture removed |
| 3 | e2e README row missed the new tests | own code | Updated |

Round 3 (fresh-session reviewer, head c2a1bc37): request changes.

| # | Finding | Kind | Answer |
| --- | --- | --- | --- |
| 1 | Positive checks passed on a hidden GitHub button | own code | `githubOffered` asserts visibility of the button, the link and the divider; checked to fail when `setGithub(true)` keeps them hidden |
| 2 | A late status answer for a replaced sign-in page could leave the current page stale | own code | Fixed test-first: answers for a page no longer shown are ignored; each page compares with what it shows |
| 3 | The sign-out test did not prove its hold took effect | own code | Polls `heldWaiting` before judging the page |
| 4 | Test plan and GSU-004 out of date | own code | Updated |
