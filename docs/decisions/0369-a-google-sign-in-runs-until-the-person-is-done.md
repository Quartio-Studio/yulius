# 0369. A Google sign-in runs on until the person is done

## Status

Accepted on 2026-11-19 (task #497, issue #1421: "for google gemini, setup has more steps than
pasting the code (disabling data retention etc), so the window shouldnt disappear at the first
time"). Specified in [SPEC-515](../../.ai/specs/SPEC-515-for-google-gemini-setup-has-more-steps-t.md).
Replaces the Consequences line of
[0361](0361-antigravity-is-a-third-harness-picked-at-random-by-default.md) "stopped 5 s after its
token file appears", and narrows [0184](0184-a-captured-token-ends-the-sign-in-and-a-closed-dialog-cancels-it.md)'s
grace to sign-ins that end by themselves (Claude, Codex).

## Context

- The Antigravity CLI (`agy`) has no sign-in-only command: its Google sign-in is its whole TUI.
  It writes its token file (`.gemini/antigravity-cli/antigravity-oauth-token`) as soon as the
  authorization code is accepted, and then goes on with its own setup, such as whether Google may
  use the person's data.
- Since ADR-0361, the server stopped it 5 s after the file appeared (ADR-0184's grace), and the
  dialog closed on `ok`. The setup was cut off. ADR-0361 expected that a run's own TUI would ask
  it again, but runs never show it: they are headless or start with the home Yulius prepares.
- agy 1.5.10 keeps the data-use choice as a setting of the Google account (`UserSettings`'
  `TelemetryEnabled` in Google's Cloud Code API), not in the temporary home. Finishing the setup
  in the sign-in's terminal is enough, and only the token file needs sealing.

## Decision

- A Google sign-in is not stopped when its token file appears. The login terminal's `state`
  frame says `status: "signed_in"` and keeps taking input. The sign-in stays `running` for its
  slot, its bounds and its cancel.
- It ends when the person presses Done or closes the dialog (both are the cancel route, which
  keeps a credential already there and answers `ok`), when the CLI exits, or at the 15-minute
  timeout. Each of these seals the token file as it is then.
- The dialog then says Antigravity is signed in and the rest of its setup can be finished below,
  and offers Done instead of Cancel sign-in. Claude and Codex sign-ins keep the 5 s grace and
  close by themselves.

## Alternatives

- **A longer grace**: any fixed time either cuts a slow person off or keeps a fast one waiting.
- **Typing the setup's answers**: Yulius would make the person's choices, on screens that are
  undocumented and were never seen live.
- **Detecting the end of the setup from the screen**: agy's screens change between versions, and
  a wrong guess cuts the setup again.
- **Sealing at once and again at the end**: two credential writes and two `account.updated`
  events per sign-in, for tasks that start a few minutes earlier. The single seal at the end
  also takes a token the CLI refreshed meanwhile.
- **A separate finish route**: the cancel route already has exactly the needed rule, so a second
  route would only duplicate it.

## Consequences

- Antigravity tasks waiting for capacity start once the person presses Done, not as soon as the
  code is accepted. The dialog says so.
- A dialog left open holds its sign-in, and one of the 8 sign-in slots, until Done or the
  15-minute timeout. The timeout still seals the token file.
- The audit log records `account.login_cancelled` and then `account.login_succeeded` for Done, as
  for a Claude sign-in cancelled within its grace.
- The signed-in TUI stays reachable to instance administrators for longer. It could run a shell
  command typed into it, which they can already do (ADR-0361), and Yulius types nothing into it.
- A setup choice that agy stores only in its home would be lost with the temporary home. None
  is known; if one appears, the run's write-back would have to carry it.
