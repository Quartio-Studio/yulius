# 0351. A personal Telegram channel behind a notification channel port

## Status

Accepted (2026-11-16). Specified in
[SPEC-498](../../.ai/specs/SPEC-498-accept-outside-pr-1130-personal-telegram.md) (part 1 of
#1107); task #480, from outside pull request #1130 by studenciak00owl, which numbered it 0287. Builds on the instance service keys (SPEC-001 ADM-002,
SPEC-195) and the event stream's visibility rule (SPEC-002 §7.8).

## Context

Yulius tells a person that a task needs them only in an open browser tab: the notification center
and desktop notifications are built in the browser from the event stream, and the server pushes
nothing outward. A task can wait in Needs you for hours while its person is away from a computer.
People already run a sidecar that polls the REST API with one token and forwards to Telegram;
it serves one person, polls every few seconds and keeps the bot token in a file.

## Decision

- **One bot per instance, set by an instance administrator.** The token is checked with
  Telegram's `getMe`, then sealed with the instance master key in the `instance` row like the Jev
  and Vidoc keys: never returned, never logged, audited without its value, in effect without a
  restart. It is not an agent secret, so no agent run can reach it. Removing it, or setting
  another bot's token, ends polling and unlinks every chat.
- **Personal linking by a single-use code issued to a browser session.** Settings → Notifications
  gives a 10-minute code (stored as its hash) and `t.me/<bot>?start=<code>`; the private chat that
  sends `/start <code>` is bound to that person. One chat per person, one person per chat. An API
  token cannot ask for a code, so a leaked CLI or agent token cannot attach a chat. `/stop` or
  Unlink ends it; both are audited (`telegram.linked`, `telegram.unlinked`). Unlinked chats get one
  fixed reply, at most every 10 minutes, and change nothing.
- **Long polling, no inbound port.** The server calls `getUpdates` in a loop with exponential
  backoff (1 s to 60 s). The Bot API base URL is configurable, HTTPS except on loopback, so tests
  run against a fake server.
- **Notifications come from the UI event bus under the event stream's own rule.** A notifier
  subscribes to the same bus as `WS /api/events` and decides who may see each envelope with the
  same function the stream uses (moved into the application layer), re-read per event. Task
  events are classified with the server's "needs you" rule (`AttentionFacts`), so the chat says
  what the notification center says, including what it learned after #1130 was written: nothing
  while a session switches, one message per sandbox-wide Docker episode, and a request for control
  to the task's owner. Text passes the transcripts' redaction; at most 5 messages a
  minute per person, the rest folded into one summary.
- **A `NotificationChannel` port** (`check`, `send`, `receive`) is the only thing Telegram
  implements. Classification, visibility, choices, redaction and the rate bound never name
  Telegram; part 2 (replies and new tasks) adds handling of received messages without reshaping it.

## Alternatives

- **A generic outbound webhook per person**: simplest server side, but no intake, the URL is
  itself a secret, and most people still need a relay to reach their phone.
- **Web Push through a service worker**: no third party, but per browser rather than per person,
  iOS needs the site installed, and there is no intake.
- **The token as an agent secret in the vault**: reuses the vault UI, but makes it reachable by
  agent runs through the egress proxy, which a bot token that can message every person must not be.
- **A webhook instead of long polling**: lower latency, but needs a public HTTPS endpoint, which
  instances on a private network do not have.
- **Per-person bots**: no shared token, but every person must talk to @BotFather; one bot is what
  the sidecar proved in daily use.

## Consequences

- Notification text passes Telegram's servers (bot chats are not end-to-end encrypted). Redaction
  keeps agent secrets out, and the event choices let a person keep it to Needs you.
- Migration 0063 adds the bot's columns to `instance` and two tables, `telegram_links` and
  `telegram_link_codes`, both removed with their person.
- The "already told" memory lives in the process: when a bot becomes active (a start, a bot set in
  Settings) the notifier first reads what is the case into it (every task, subscription and
  request for control), so nothing is repeated, a change while the server was down is not sent,
  and every change after it is, whatever moved.
- Part 2 (replies to a notification become a `task_message`, new messages become tasks, `/status`,
  `/task`) adds to the receive side only.
