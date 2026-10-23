---
yulius-task: 13901ebb-10d3-43f6-94e1-c2f040a7869a
status: done
---

# SPEC-283: First-run onboarding says what each step does and what comes next; GitHub opens in a new tab

Status: done
Date: 2026-10-23
Owner: task #269
Scope authorization: task #269 (requested by Noaksyy), with four screenshots of the setup wizard
and the GitHub connection page: GitHub must open in a new tab, each step must say what it does
and what comes next, the layout must not break, the first subscription becomes a skippable step,
and the sandbox creation actions must show when expected. Decision: ADR-0211.

## Goal

A person setting up a fresh instance always knows where they are, why a step exists and what to
do next, and never loses the Yulius tab: the setup wizard numbers its steps and explains each,
ends with the first Claude subscription (skippable), Home lists the getting-started steps with
their state, and creating the GitHub App happens in a new tab while Yulius waits and then says
what comes next.

## Context

- `pages/auth.js` `mountSetup`: setup token + password → recovery code (`recoveryCodeCard`) →
  Jev key → Vidoc key (`instancekeys.js` `keySetupCard`, labelled "Optional · 1 of 2"/"2 of 2")
  → Home. Nothing says why a setup token exists or what follows each card.
- `pages/admin.js` `manifestForm` posts the GitHub App manifest with a hidden form in the same
  tab (`form.submit()`), so Yulius is replaced by GitHub, then by the App's installation page
  (the callback redirects there, `routes/admin.rs`), and nothing leads back. The connected card
  ends with "Next: admit users and add subscriptions." with no way to act on it.
- `pages/home.js` `emptyView`: Connect Claude banner (SPEC-246), the playground drop zone
  (SPEC-099) and the GitHub section. It does not say which of these is done or next.
- Layout: `.recovery-card { border: 0; padding: 0 }` (meant for the administration page's
  modal) also strips the setup page's card, so the recovery code sits flush against the edge of
  a borderless box (screenshot 1). The owner-kind toggle (`.seg`) in the GitHub card's `stack`
  stretches to the full width, its two options squeezed left (screenshot 4).
- `app.js` reads `/api/me` again whenever the tab becomes visible, so Home learns of a GitHub
  connection made in another tab.

## Requirements

- ONB-001: Create GitHub App on GitHub opens a new tab synchronously on the click (no pop-up
  blocker on the await that follows), cuts its `opener`, and posts the manifest form into it;
  the Yulius tab stays on the GitHub connection page. A blocked tab says so (allow pop-ups) and
  posts nothing; a failed manifest request closes the tab and shows the error.
- ONB-002: while the App is being created elsewhere the page shows a waiting card ("Finish on
  GitHub in the new tab …") with Start again, reads `/api/admin/github` when the tab comes back
  into view and every 5 s while visible, for at most 15 minutes, stopping when the page goes
  away; once configured it shows the connected card, a "GitHub connected" toast, and updates
  `me` (`instance.github_configured`) for Home.
- ONB-003: the connected card lists the next steps: install the App on repositories (new tab),
  then create a sandbox from a repository (Import repository, Create from template, opening the
  sandbox dialog; Connect Claude first while no Claude account is usable), and admit users.
- ONB-004: every setup card names its place ("Step N of M") and says in one or two sentences
  what it does and why: the setup token ("proves you run this server: only someone who can read
  its console can claim the instance, so nobody who merely reaches this address can take it
  over"), the recovery code, each optional key, and the subscription; each card says what comes
  after it.
- ONB-005: the wizard's last step is "Add your first Claude subscription": Add subscription ends
  setup on Subscriptions with its sign-in open (`/admin/accounts?add=1`); Skip for now ends it on
  Home. Its text says agents run on it and that it can be added later.
- ONB-006: Home's empty view shows an administrator "Getting started": Connect Claude, Create
  your first sandbox, Connect GitHub (optional), each with a one-line explanation and a state
  (Done, Next, To do, Optional), the first not done marked Next.
- ONB-007: the setup page's recovery card keeps the auth card's border and padding (only the
  modal's drops them); the owner-kind toggle keeps its content width.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] GitHub App creation opens in a new tab and the page follows it
- [x] The GitHub page says what comes next once connected
- [x] Setup wizard numbers its steps and explains each one
- [x] Setup ends with the first Claude subscription, skippable
- [x] Home lists the getting-started steps with their state and next action
- [x] Onboarding cards keep their layout (recovery card, owner toggle)
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- On `/admin/github` (mock, `github=0`), Create GitHub App on GitHub calls `window.open` with
  `_blank`, the URL stays `/admin/github`, the waiting card shows; once the mock App exists the
  connected card with its next steps shows without a reload, and Import repository opens the
  sandbox dialog. A blocked `window.open` shows the pop-up message and no waiting card.
- Setup shows "Step 1 of …" with the token's explanation, then the keys as later steps, then
  "Add your first Claude subscription"; Skip for now lands on Home, Add subscription on
  Subscriptions with the sign-in dialog.
- Home's empty view (mock without sandboxes) lists the three getting-started steps with Next on
  the first not done.
- The setup recovery card has a border and padding; the owner toggle is narrower than the card.
- No WCAG violations on the new states.

## Test plan

- e2e `onboarding-flow.e2e.ts` (new): the GitHub new tab, blocked tab, waiting card and
  connected next steps; the wizard's step labels, token explanation and subscription step (both
  exits); Home's getting-started list; the recovery card padding and toggle width (computed
  style), axe on each. Each written first and failing before its change.
- e2e `update.e2e.ts` setup tests: follow the new last step.
- Full gate `scripts/dev check` and the whole e2e suite.

## Decisions

- ADR-0211: the GitHub App manifest is posted into a new tab opened on the click, with the
  opener cut, and the Yulius tab waits by reading the connection; the setup wizard ends with the
  first subscription as a skippable step. Alternatives (same tab with a return link via a
  `setup_url`, a pop-up window, embedding the subscription sign-in in the wizard) are in the ADR.

## Evidence

- `e2e/tests/onboarding-flow.e2e.ts` (14 tests): each written before its change and seen failing
  for the right reason (no `window.open` call; no waiting heading; no Next steps region; no step
  labels; no Getting started list; recovery card padding 0; manifest form absent; closed tab not
  detected), then passing. Mutation check: removing the page's `destroy` cleanup makes
  "stops reading the connection … when the page is left" fail (3 reads, expected 2).
- `update.e2e.ts` setup tests updated for the new last step; `password-setup.e2e.ts`,
  `home.e2e.ts`, `claude-connect.e2e.ts`, `loading-shapes.e2e.ts` pass unchanged.
- `scripts/dev ci`: exit 0. Whole e2e suite: 66 files passed, 1 skipped (696 tests passed,
  1 skipped) before the review fixes; rerun after them is recorded in the pull request.
- Screenshots of setup steps 1, 2 and 5, Home's Getting started, the GitHub form, waiting and
  connected cards checked by eye (mock, Chromium).
- `scripts/dev security`: **not run** (exit 2): Vidoc answered HTTP 429, this key has used its
  50,000,000 tokens. No finding was checked; CI's security job reports the same state.
- Independent review (fresh session, om-code-review) at 1a442dc4: request changes on one medium
  (no test for the failed-manifest path) and lows/nits. Fixed test-first: failure-path test,
  closed-tab guard with test, mock builds the real form (asserted), Check now after the 15-minute
  limit, focus moved to each new card (asserted), empty action row, polling stop tested, heading
  class, docs. Kept: step 1 says "of 5" while the 4-step path exists only in the mock (the server
  always issues a recovery code).
- Review round 2 at 72eeca8a: approve, with lows on Check now. Fixed test-first (two tests with a
  stubbed page clock, both failing before): past the limit the page reads once more (an App that
  exists shows connected at once), then shows Check now with focus; Check now is off while it
  reads.
- Review round 3 at d8e0aaf2: approve, with a low (Start again during the last read was undone by
  its answer) and a nit (Check now's off state untested). Fixed test-first: a read made before
  Start again or before the page went away changes nothing (the test failed before: the Check now
  card came back over the form); Check now is asserted off while its read is held (the mock's
  `holdGithub`). `onboarding-flow.e2e.ts`: 14 passed, twice.
- Not tested automatically: the real post to GitHub (needs a GitHub account and a public URL).
