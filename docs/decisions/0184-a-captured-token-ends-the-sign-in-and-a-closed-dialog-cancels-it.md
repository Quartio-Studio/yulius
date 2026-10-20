# 0184. A captured token ends the sign-in, and a closed dialog cancels it

## Status

Accepted. Specified in [SPEC-243](../../.ai/specs/SPEC-243-claude-sign-in-confirm-success-and-auto.md);
task #229, from an early external user's feedback. Refines SPEC-002 ADR-05 (the account login
flow) and SPEC-001 ROT-002.

## Context

- People connect a Claude subscription with `claude setup-token` in a server-owned PTY. The
  dialog showing it stayed open after a successful sign-in. Its only sign of success was one
  muted line, so the person could not tell whether it had worked.
- `setup-token` writes no credentials file and leaves nothing `claude auth status` could read. It
  prints the token once, and Claude Code 2.1.292 ends the process 500 ms later. The server
  already reads the token from the raw PTY stream (viewers only get masked output) and decides
  success on the process's exit.
- A CLI that kept running after printing the token would have kept the sign-in `running` until
  its 15-minute timeout.
- Closing the dialog left a running sign-in alive for up to 15 minutes, holding a PTY, a
  temporary config directory and one of the 8 concurrent sign-in slots. Nobody could reach it
  again from the UI.

## Decision

- **Success** is still decided on the server, from the raw PTY stream. The sign-in ends `ok` when
  the CLI exits after printing a complete token. Once the output holds a complete token
  (`sk-ant-oat01-` and at least 20 token characters), the CLI also gets `TOKEN_EXIT_GRACE` (5 s)
  to exit. After that the server stops it as it stops a timed-out sign-in and seals the token.
- **The dialog follows the terminal's ending.**
  - On `ok` it closes by itself, the subscriptions reload, and a toast says "<label> is
    connected", adding "as <email>" when the account view reports an email (UI-022).
  - On `error` it shows a banner with the account's recorded reason, Try again and Close.
  - On `cancelled`, or when the terminal is gone (`not_found`), the banner says so.
  - Try again starts a new sign-in (`relogin`) in the same dialog.
- **Closing a running sign-in cancels it** through `POST /api/admin/accounts/login/{lid}/cancel`.
  The account keeps its state, as for a replaced sign-in. The endpoint answers 204 also once the
  sign-in has ended, and then changes nothing.
- A failure's recorded reason names the exit as "exit code N" or "stopped by signal N", not
  Rust's `Some(N)`, because the banner now shows it.

## Alternatives

- **Detect success from a credentials file or `claude auth status`.** `setup-token` produces
  neither.
- **Detect success in the browser from the rendered terminal text.** Viewers get masked output by
  design, and screen text is fragile.
- **A success screen that stays until the person closes it.** The feedback asked for the dialog
  to close. The toast and the subscription's card show the lasting state.
- **Keep an abandoned sign-in running so a reload can reattach.** The UI never reattaches; it
  offers "Sign in again", which replaces the sign-in anyway.

## Consequences

- A successful sign-in no longer leaves the terminal in front of the person. A failure says why
  and can be retried without leaving the dialog.
- A reload or a closed tab still leaves the sign-in running until its timeout. Only the dialog's
  own close cancels it.
- Cancelling within the 5-second grace after the token appeared discards that token. That is the
  person's explicit choice, and the dialog has normally closed by then.
- The email appears in the success toast only once the account view reports it (task #230). A
  `setup-token` credential cannot read Claude's profile itself.
