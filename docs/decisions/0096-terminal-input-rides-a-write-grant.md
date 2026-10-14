# 0096. Terminal input rides a per-viewer write grant tied to the store's access epoch

## Status

Accepted. Specified in [SPEC-118](../../.ai/specs/SPEC-118-when-i-write-something-in-a-claude-termi.md).

## Context

Typing into a task terminal or a shell lagged (task #103). Each input frame re-checked write
access before it reached the PTY. That meant three reads on the store's single SQLite thread, which
every part of the server shares, and a GitHub API call whenever the 5-minute permission cache had
expired. `resize`, `active` and `take_control` did the same. The viewer's socket loop also ran its
60 s access re-check inline, so while that check waited on the database or GitHub, no output
reached the viewer and no input reached the PTY.

The per-input check mattered for facts that live in the database: a task handed to another owner
(SPEC-070), a removed sandbox, a disabled user. The next keystroke had to be refused, and it still
must be. A GitHub downgrade already waited for the 5-minute permission cache.

## Decision

- The store keeps an in-memory **access epoch** (`PermissionStore::access_epoch`). It grows on the
  database thread right after any write that can change who may type: a task's owner (hand-over,
  take-over, delete), a sandbox's removal, a user's disabling, instance-admin flag or GitHub link,
  a revoked session or token, a cached GitHub level that changed, a cleared permission cache.
- Each terminal viewer (task terminal or shell) holds a **write grant**
  (`yulius_app::write_grant`). Opening the terminal confirms it, at the epoch read before the
  open's own checks. An input, `resize`, `active` or `take_control` frame is written at once while
  the epoch is unchanged. When the epoch has moved, the full check runs before the frame. A grant
  older than 30 s is re-confirmed in a background task while input keeps flowing, at most one at a
  time. A refusal drops the grant, so the next frame is checked again.
- The periodic re-check (sign-in, view and write access; `viewer_recheck_ms`, 60 s, now also for
  shells) runs in a spawned task, one at a time per viewer. The socket loop selects on its
  verdict. Its close codes and `state` frames are unchanged.

## Alternatives

- **Drop the per-input check and rely on the 60 s re-check.** Simplest. But a previous owner could
  keep typing into the agent for up to a minute after a hand-over, which breaks SPEC-070's
  immediate refusal.
- **A second database connection for reads.** The SQLite file is in WAL mode, so reads would not
  queue behind writes. But keystrokes would still do three reads each, still call GitHub when the
  cache expires, and the in-loop re-check would still freeze output. It also changes the store's
  one-thread design.
- **Local echo in the browser (predictive echo, as in mosh).** The person would see characters
  before the agent does. But Claude Code draws its own full-screen input box, and a guessed echo
  would be wrong as often as it is right. It does nothing for the server-side wait.

## Consequences

- When access is unchanged, a keystroke costs only memory reads before it reaches the PTY.
- A GitHub downgrade still needs the permission cache to expire. Then the keystroke that starts the
  background re-confirmation goes through, and the ones after it are refused. Before, that
  keystroke was refused too, after waiting for GitHub.
- Every write listed above invalidates every viewer's grant, so each open terminal checks once on
  its next keystroke. These writes are rare. A new write that changes access must use
  `SqliteStore::call_access`, or access changes it makes reach terminals only within 30 s.
