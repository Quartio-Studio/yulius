---
yulius-task: 7f4e80ab-d201-4a2b-94d9-ab61afbbaff1
status: in-progress
---

# SPEC-515: an Antigravity Google sign-in stays open until its setup is finished

## Goal

When an administrator adds an Antigravity subscription by Google sign-in, the sign-in window
stays open after the authorization code is pasted. The Antigravity CLI's setup goes on after the
sign-in (for example, the data-use and data-retention choice), and the administrator finishes it
in the same terminal. The window closes when they press Done (issue #1421).

## Context

- Symptom (the user): "for google gemini, setup has more steps than pasting the code (disabling
  data retention etc), so the window shouldnt disappear at the first time".
- Root cause: `App::run_login` (`crates/yulius-app/src/accounts.rs`) handles every provider's
  credential the same way. Once `holds_credential` finds one (for Google, the token file
  `.gemini/antigravity-cli/antigravity-oauth-token` in the sign-in's home), the CLI gets
  `TOKEN_EXIT_GRACE` (5 s). Then `stop_login` sends it Terminate. The sign-in ends `ok`, and
  `openLoginTerminal` (`web/assets/pages/admin.js`) closes the dialog on `ok`. agy writes its
  token file as soon as the code is accepted, before its own setup screens, so those screens are
  cut off after 5 s.
- Why the tests missed it: the behaviour was intended. `an_antigravity_sign_in_ends_once_its_token_file_is_written`
  pins it, and ADR-0361 accepted it ("A sign-in that asks more after writing the file … is cut
  then; the run's own TUI asks it again"). Nothing after the sign-in had been seen live then.
  A run's TUI never shows the setup again either: runs are headless (`-p`) or start in the
  task's terminal with the home Yulius prepares.
- Where the choice is kept: agy 1.5.10's strings show the data-use setting as
  `UserSettings.TelemetryEnabled` of Google's Cloud Code API (`SetTelemetryEnabled`, "Failed to
  save remote settings"). It belongs to the Google account, not the temporary home, so it
  outlives the sign-in. Only the token file needs sealing, as today.
- The cancel path already keeps a credential that exists when the sign-in is cancelled (SPEC-243
  LGN-002, ADR-0184): a cancel then ends `ok`. The window's Done reuses it.
- What else the cause could break: Claude's `setup-token` and Codex's device sign-in exit by
  themselves right after their credential, so they keep the grace. Only Google changes.

## Requirements

- R1: A Google sign-in is not stopped when its token file appears. It runs on until the
  administrator ends it (Done, closing the window, or a cancel), the CLI exits, or the sign-in
  times out (`LOGIN_TIMEOUT`, 15 min).
- R2: Once the token file is there, the login terminal's `state` frame says `status: "signed_in"`
  (still writable). Viewers that connect later get it too. The sign-in stays `running` for the
  registry (its slot, its cancel).
- R3: However a Google sign-in that holds its token file ends (cancel, exit, timeout), the token
  file as it is then is sealed and the subscription is `ok`.
- R4: Once the terminal says `signed_in`, the window says Antigravity is signed in and the rest of
  the setup can be finished below, and offers Done. Done and closing the window end the sign-in.
  The success toast follows, as for any other sign-in.
- R5: Claude and Codex sign-ins are unchanged (grace, then stop, and the window closes by itself).

## Plan

- [x] Reproduce the defect and find its root cause
- [x] A Google sign-in runs on after its token file and says signed_in, sealed when it ends
- [x] The sign-in window stays open with Done once Antigravity is signed in
- [x] ADR 0369, architecture and changes say a Google sign-in ends with Done
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- `accounts::an_antigravity_sign_in_runs_on_after_its_token_file_until_done` (replacing
  `an_antigravity_sign_in_ends_once_its_token_file_is_written`): with a 50 ms grace, the CLI
  writes its token file and keeps running. 300 ms later it has had no Terminate, the account is
  still `pending_login`, the terminal's state says `signed_in` and input still reaches the CLI.
  A cancel then answers `ok`, seals that token file and wipes the home.
- `accounts::an_antigravity_sign_in_that_exits_after_its_setup_is_sealed`: the CLI writes its
  token file, then exits by itself. The sign-in ends `ok` with the file sealed.
- `accounts::an_antigravity_sign_in_left_open_is_sealed_at_its_timeout`: with a short sign-in
  timeout, the token file is sealed when it expires (`ok`).
- The existing Claude grace test (`a_cli_that_keeps_running_after_the_token_is_stopped` or its
  equivalent) still passes unchanged.
- e2e `an Antigravity subscription signs in with Google in the login terminal`: after the code,
  the dialog stays open with "Antigravity is signed in", Done shows the success toast and closes
  it, and the mock's sign-in was ended through the cancel route.

## Test plan

- Regression first: the new `accounts` integration tests and the e2e change, committed before the
  fix and shown failing (Terminate is sent, the dialog closes).
- Then `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^accounts::/)'`,
  `cargo nextest run -p yulius-app -E 'test(/login/)'`, the Antigravity e2e file, and `scripts/dev
  quick`. The whole gate is Yulius CI's.

## Decisions

- **The CLI runs on, and the person ends it with Done** (chosen). Other options were: a longer
  grace (any fixed time either cuts a slow person off or holds a fast one); typing into the TUI
  to finish its setup (Yulius would make the person's choices, and the screens were never seen
  live); detecting the end of the setup from the screen (agy's screens are undocumented and
  change with versions). Only the person knows when they are done.
- **Signalled as a `signed_in` state, not an early seal**: sealing the token file at once and
  again at the end would give a sign-in two credential writes and two `account.updated` events.
  Ending it seals the file as it is then, including a token the CLI refreshed meanwhile. Until
  Done, tasks waiting for an Antigravity subscription keep waiting. The window says so, and the
  15-minute timeout seals it anyway.
- **Done is the cancel route**: its rule (a credential already there is kept, and the answer says
  `ok`) is exactly what Done needs, so there is no new endpoint. The audit log records
  `account.login_cancelled` and then `account.login_succeeded`, as for a Claude sign-in cancelled
  within its grace.
- ADR [0369](../../docs/decisions/0369-a-google-sign-in-runs-until-the-person-is-done.md)
  records this. It replaces ADR-0361's "stopped 5 s after its token file appears".
