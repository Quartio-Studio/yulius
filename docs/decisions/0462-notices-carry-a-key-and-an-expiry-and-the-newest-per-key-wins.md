# 0462. Notices carry a key and an expiry, and the newest per key wins

## Status

Accepted (2026-11-22). Specified in
[SPEC-624](../../.ai/specs/SPEC-624-notifications-that-replace-and-expire-a.md); task #608. The
owner approved it on 22 Nov 2026 after reviewing the inbox of iTermX, a separate tool. Builds on
[ADR-0351](0351-a-personal-telegram-channel-behind-a-notification-channel-port.md) (the Telegram
channel) and [ADR-0380](0380-an-end-of-day-digest-built-from-task-state.md) (the digest). Changes
neither.

## Context

- The bell, the Telegram chat and the digest collect stale and repeated items. Each new red CI
  run of the same pull request adds a bell entry, because entries are keyed by episode. A task
  that goes from Needs you to Done leaves both messages in the chat. Yesterday's digest stays
  beside today's.
- Nothing expires. The bell keeps its last 100 entries until someone clears them.
- Every person-facing notice except the digest is derived in each client from events, so
  Telegram, the digest, MCP reads and the browser cannot agree on what replaced what.
- Script watches (#614) need a place to post their output, so that each run replaces the last.

## Decision

- **A stored notice.** Producers call `App::post_notice` with a project (or none), an optional
  recipient, a key and an optional expiry. The notice is stored in `notices` (migration 0091),
  published as `notice.posted` to the people its envelope reaches (the event stream's rule), and
  sent to their Telegram chats under their channel choices.
- **Newest wins per (project, recipient, key).** A unique index holds one notice per scope. A
  replacement deletes the older notice and its dismissals, and inserts the newer one with a new
  id, in one transaction.
- **An expiry, never for a decision.** An expiry is `never`, `30m`, `2h`, `3d` (up to 90 days),
  or a time. A notice without one gets its kind's default: one day for reports, three for good
  news, seven for failures. Reads leave out expired notices, and the background tick deletes them.
  A notice that waits for a person's decision is stored without an expiry. Posting one with an
  expiry is refused rather than shortened.
- **Task state stays the truth for task notices.** Their key is `task:<task id>`, so a newer
  state of the same task replaces the earlier entry or message. They are not copied into the
  table.
- **Telegram replaces by deleting.** After a keyed message is sent, the bot deletes its earlier
  message with the same scope in that chat (`telegram_keyed`). Telegram refuses to delete
  messages older than 48 hours, and those stay.
- **The digest lists live notices.** The digest lists up to 20 live notices with their keys, and
  is itself keyed `digest`.

## Alternatives

- **Keys only in each client.** The bell already had local keys. Telegram, the digest and MCP
  would each need their own copy of the rules, and the copies would drift.
- **Editing the Telegram message instead of deleting it.** An edit does not notify, so a new
  failure would arrive silently.
- **Letting a decision expire with a last "expired" message.** The request still vanishes while
  it waits for a person.
- **A global key per person, as in iTermX.** Two projects can both have `gate:quick::a`, and a
  project-wide notice would need one row per person, with dismissals and replacement tracked for
  each row.

## Consequences

- One API serves Yulius's own repeated notices and script watches (`watch:<name>`).
- The table is bounded: 200 notices per scope, and expired ones are pruned every tick.
- People see one entry per subject, which goes away by itself unless it waits for them.
