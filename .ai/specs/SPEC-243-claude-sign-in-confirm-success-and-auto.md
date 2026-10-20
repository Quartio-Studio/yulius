---
yulius-task: 76c9a44e-2a03-4eab-8bca-95164aa83cd2
status: done
---

# SPEC-243: Claude sign-in confirms success and closes by itself

Status: done
Date: 2026-10-20
Owner: task #229
Scope authorization: task #229, feedback from an early external user: "Login works great, I'd only
add auto-close after logging in. Right now you log in and still have the terminal in front of you
even after a successful login. It's a bit misleading the first time, because you don't know
whether it worked or not." Refines SPEC-001 ROT-002 (connecting an account is a button followed
by the provider's own sign-in) and the login terminal of SPEC-002 §7.2 "Login".

## Goal

When Claude's sign-in succeeds, the sign-in dialog closes by itself. The person sees that the
subscription is connected and which one it is. When the sign-in fails, times out, is cancelled or
is lost, the dialog says so plainly and offers to try again. Closing a sign-in that is still
running cancels it instead of leaving it running in the background.

## Context

Confirmed against `dev` at 33410359.

- **Flow.** "Add subscription" or "Sign in again" (`pages/admin.js`) opens
  `openLoginTerminal`, a full-size dialog with the login terminal
  (`WS /api/admin/accounts/login/{lid}/terminal`). The server runs `claude setup-token` in a
  server-owned PTY (`yulius-app/src/accounts.rs`, ADR-05). Each output chunk is kept raw in a
  bounded buffer for token extraction and masked before it reaches a viewer. When the process
  exits, `finish_login` extracts the token, seals it into the account, and publishes
  `account.updated` and the `exit` frame (`ok`, `error` or `cancelled`).
- **Symptom.** On `exit` the dialog only replaces a one-line muted status text ("Signed in: … You
  can close this window."). The terminal stays in front with Claude's own output, and the only
  button is "Close". Nothing tells the person, at a glance, that the subscription is connected.
  A failure reads as the same small line, in red, with no way to retry. "Close" while the CLI
  still runs leaves it running until its 15-minute timeout.
- **How success is detected.** `claude setup-token` stores no credential file and leaves no state
  `claude auth status` could read. It prints the long-lived token once and ends by itself. Claude
  Code 2.1.292 calls its completion callback 500 ms after its `success` state, which ends the
  process. The server already reads the token from the raw PTY stream (not from the browser and
  not from rendered text) and decides on the process's exit. That stays the success signal. A
  CLI that kept running after printing the token would leave the sign-in `running` until the
  15-minute timeout, so the token itself now also ends the wait (LGN-001).
- **Which account.** The stored `setup-token` credential has only the `user:inference` scope, so
  Claude's profile endpoint refuses it and Yulius does not know the sign-in's email today
  (`docs/api.md`: `email` is planned for UI-022, "the server does not report them yet"). Task
  #230 ("Rename Claude sessions and show account email") owns reading and storing the email. This
  change names the subscription by its label, and also by `email` once the account view reports
  one.

## Requirements

| ID | Requirement |
| --- | --- |
| LGN-001 | The server ends a Claude sign-in once its output holds a complete token. If the CLI has not exited within `TOKEN_EXIT_GRACE` (5 s) after the token first appeared, the server stops it as it stops a timed-out sign-in, then seals the token, and the sign-in ends `ok`. A CLI that exits on its own (the normal case) is not affected. |
| LGN-002 | `POST /api/admin/accounts/login/{lid}/cancel` (instance administrators) cancels a running sign-in and waits (at most 15 s) for its end, answering how it ended: `{"status": "cancelled"}` normally. The CLI is stopped, its temporary directory removed, and the account keeps its state, as for a sign-in replaced by another one. A complete token already captured is sealed instead (`ok`): the sign-in had succeeded. A sign-in that has already ended is left as it is and its ending is answered; 404 for an unknown or expired sign-in, 403 for others. The cancel is audited (`account.login_cancelled`). |
| LGN-003 | When the login terminal reports `ok`, the dialog closes by itself, the subscriptions reload, and a success toast names the subscription: "Claude #3 is connected" and "as <email>" when the account reports an email. It also says tasks can use it now. |
| LGN-004 | When the sign-in ends `error`, the dialog stays open with Claude's output. A banner says "Sign-in failed" with the reason Yulius recorded (the account's `last_error`, for example "the sign-in timed out after 15 min"), and offers "Try again" and "Close". |
| LGN-005 | "Try again" starts a new sign-in for the same subscription (`relogin`) in the same dialog. The banner goes away and the new terminal replaces the old one. |
| LGN-006 | While the sign-in runs, the dialog's button reads "Cancel sign-in". Closing the dialog by that button, ×, Esc or a click outside cancels the sign-in on the server (LGN-002). A toast then says how it ended, from the server's answer and the account's state: connected (a token was already captured), or cancelled with the subscription not signed in, or cancelled while it keeps its working sign-in. A sign-in started by Try again while the dialog closes is cancelled too. |
| LGN-007 | A sign-in that ended `cancelled` on the server's side (replaced by another sign-in, or its subscription removed), or a login terminal the server no longer has, shows a banner that says so, with "Try again" (except for a removed subscription, and for a viewer signed out of Yulius) and "Close". The server refuses a terminal for an unknown or expired sign-in in band (`not_found`, close 4003), so the browser does not retry it. A connection that cannot be re-established says it was lost, with Try again. |

## Plan

- [x] Spec for the sign-in that confirms itself
- [x] Server ends a sign-in once its token is captured
- [x] Cancel endpoint for a running sign-in
- [x] Sign-in dialog closes itself on success with a toast naming the subscription
- [x] Failed, timed-out and lost sign-ins show a banner with Try again
- [x] Closing a running sign-in cancels it
- [x] Docs, ADR-0184 and changes entry match the flow
- [x] Review round 1 findings fixed test-first
- [x] Gates green on the fixed head
- [x] Review round 2 approves the head
- [x] Merge into dev on green CI

## Acceptance criteria

- A scripted CLI that prints a token and keeps running ends `ok` with the token sealed, and its
  process got `Terminate` (`accounts::a_sign_in_ends_once_its_token_is_captured`). Fails on
  `dev`: the sign-in stays `running`.
- The cancel endpoint stops a running sign-in and answers `cancelled`, again once it ended, 404
  for an unknown one and 403 for a member (`accounts::a_running_sign_in_can_be_cancelled`). A
  cancel after the token was printed keeps it (`accounts::a_cancel_after_the_token_keeps_it`).
- A terminal for an unknown sign-in gets an in-band `not_found` and close 4003
  (`accounts::login_terminal_websocket_streams_masked_output_and_accepts_input`).
- e2e (mock): finishing the sign-in closes the dialog and shows "Claude #9 is connected"; the new
  subscription reads as signed in. A failing sign-in shows "Sign-in failed" with the reason,
  "Try again" starts a new one in the same dialog, and that one succeeds. Closing a running
  sign-in cancels it (the mock records the cancel) and says so. A sign-in replaced elsewhere, one
  whose subscription was removed (no Try again) and one the server no longer has each say so.
- `docs/api.md`, `docs/architecture.md`, ADR-0184 and `docs/changes.md` describe the flow.

## Test plan

- Integration (`crates/yulius/tests/accounts.rs`, scripted fake process host):
  `a_sign_in_ends_once_its_token_is_captured` (LGN-001) and
  `a_running_sign_in_can_be_cancelled` (LGN-002). Waits are on the `exit` frame and
  `account.updated`, bounded; the grace period is shortened in the test with a setter, as the
  timeout already is.
- Route contract: `crates/yulius/tests/ui_contract.rs` checks the mock's new route against the
  server router.
- e2e (`e2e/tests/subscriptions.e2e.ts`, mock backend): success auto-closes (LGN-003); failure
  and Try again (LGN-004, LGN-005); cancel on close (LGN-006); a sign-in replaced elsewhere
  (LGN-007). Each waits on visible state, never on time.
- Browser check of the dialog states on the mock (`?mock=1`) in the Yulius browser.
- Full gate `scripts/dev ci`, `scripts/dev security` and the e2e suite.

## Decisions

- **Success stays server-side: the token captured from the CLI's own PTY, then the exit.**
  Considered:
  - A credentials file. `setup-token` writes none.
  - `claude auth status`. It cannot see a token that only went to the terminal.
  - Scraping the rendered screen in the browser. Viewers only get masked output, and it would be
    fragile.

  The raw server-side stream is where the token already comes from (ADR-05). The new grace end
  (LGN-001) covers a CLI that lingers after printing it. Recorded as ADR-0184.
- **A cancel keeps a token already captured.** In the 5-second grace the dialog is still open, and
  a person unsure whether it worked is most likely to close it then. The token is valid, so it is
  sealed and the toast says the subscription is connected. Found by the independent review.
- **Closing a running sign-in cancels it.** The other option was to keep it running for a reload
  to reattach. Nobody can reach an abandoned sign-in from the UI: the page offers "Sign in again",
  which replaces it anyway. Leaving it running kept a PTY and a temporary config directory alive
  for up to 15 minutes and counted against the 8 concurrent sign-ins. A reload still leaves it
  running until its timeout, which is bounded.
- **A toast, not a lingering success screen.** The user asked for the dialog to close. The toast
  names the subscription, and its card on the page reads signed in, which is the lasting state.
- **The email comes from #230.** The success message uses the account view's `email` when it
  is there. It does not read the email itself: a `setup-token` credential cannot read Claude's
  profile, and storing the email belongs to #230's migration.

## Evidence

- Regression tests failed before their fixes:
  - `accounts::a_sign_in_ends_once_its_token_is_captured` failed with `Elapsed(())`: the sign-in
    stayed `running` (setter added first, behaviour after).
  - `accounts::a_running_sign_in_can_be_cancelled` failed with `left: 404, right: 403` (no route).
  - `accounts::failed_logins_record_errors_and_relogin_failures_keep_working_accounts` failed on
    "exit code Some(2)" once it expected "(exit code 2)".
  - The `not_found` part of `accounts::login_terminal_websocket_streams_masked_output_and_accepts_input`
    failed with a handshake `Http(404)`.
  - Of the four first e2e tests, the four SPEC-243 ones failed on the old dialog (no
    "closes by itself" text, no banner).
  - `accounts::a_cancel_after_the_token_keeps_it` answers `ok` with the token sealed. On the
    round-1 code it answered 204 and discarded the token (the reviewer's analysis).
  - `accounts::a_partial_token_does_not_end_the_sign_in` also passes on `dev`. It is a guard on
    the new grace end, not a regression test.
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & (test(/^accounts::/) | test(/^ui_contract::/))'`:
  19 passed.
- `scripts/dev ci` (check, lint, macOS, test, doc, deps): exit 0 on the final head. Earlier runs
  failed and were fixed: a product name in a string (use `BRAND.name`), and a doc link to a
  private constant.
- e2e:
  - `npx e2e run tests/subscriptions.e2e.ts tests/consistency.e2e.ts`: 25 passed.
  - The full suite (`npx e2e run`): 638 passed, 1 skipped, 1 failed. The failure was
    `secrets.e2e.ts` "a failed copy shows no check…", an axe target-size check on a Close
    button: the known flake #640, outside this change. It passed 3 of 3 runs on its own.
- Browser check on the mock (`?mock=1&ambient=0`, served on port 20107, Yulius browser), on
  Claude #3's "Sign in again":
  - The code `fail` showed the "Sign-in failed" banner with the reason and Try again.
  - Try again opened a new sign-in with "Cancel sign-in".
  - A code then closed the dialog. The toast read "Claude #3 is connected. Tasks can use it now."
    and the card turned signed in.
- `scripts/dev security` (Vidoc vidoc-1): exit 0, with no critical, high or medium finding in the
  change. One medium finding is outside it (`routes/mod.rs:410`, internal router authentication):
  CI files it as an issue.
- Independent review (fresh session, `om-code-review`):
  - Round 1 requested changes with 2 major and 5 minor findings, all own code. All were fixed,
    with tests where the behaviour is observable: the lost terminal is refused in band, a removed
    subscription gets no Try again, a Try again started while the dialog closes is cancelled,
    the cancel toast follows the server's answer, a token already captured is kept on cancel,
    and the token scan is incremental.
  - Round 2 approved `a96ea5db`. Its three nits were fixed in `910b931f`: a dialog closed after
    a lost connection cancels the sign-in, a `running` answer says the sign-in is still stopping,
    and a failed audit of a cancel is logged.
- Not tested against a real Claude sign-in. That needs a person's Claude account in the
  browser. The success detection relies on `claude setup-token` 2.1.292 exiting 500 ms after
  printing the token, read from its bundled source. The 5-second grace end covers a version that
  does not exit.
- Pre-existing, outside this change: on the real server a signed-out viewer is refused at the
  handshake (401), so the dialog says "connection lost" rather than "signed out" (only the mock
  sends the in-band frame).
