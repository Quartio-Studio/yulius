---
yulius-task: 7c918f4c-5e63-4317-b00e-274ed0147e65
status: done
---

# SPEC-444: A personal Telegram channel for notifications (part 1 of #1107)

Status: done
Date: 2026-11-07
Owner: Yulius task #2 (fork studenciak00owl/yulius, proposed upstream to Quartio-Studio/yulius `dev`)
Scope authorization: upstream issue Quartio-Studio/yulius#1107 ("Personal Telegram channel"),
direction agreed by an upstream maintainer on 2026-11-07; this task implements its part 1 only
(instance setup, personal linking, outbound notifications, long polling, the channel interface).
Intake (replies, new messages, `/status`, `/task`) is part 2 and recorded below as later.

## Goal

A person who closed every Yulius tab still learns, on their phone, that a task needs them: each
person links their own Telegram chat to a bot an instance administrator configured once, and
Yulius sends that chat the notifications their notification center would show them, without
any inbound port.

## Context

- The notification center is built in the browser from the event stream
  (`crates/yulius-server/web/assets/notifications.js`, SPEC-001 UI-020): task tones (Needs you,
  Your turn, Done, failed, stopped abnormally), browser action requests, automation runs and
  account problems. Desktop notifications reach the system only while a tab is open. The server
  pushes nothing outward (no Web Push, e-mail, webhook or chat).
- Which events a person's stream receives is decided in `routes/events.rs` from the event's
  envelope (`yulius_app::events::Envelope`: sandbox, `admin_only`, `user`, `writers_only`) and the
  person's current access (`App::can_view`, `can_work_in`, guests). Account events are
  `admin_only`.
- A task's "needs you" group is computed on the server by `AttentionFacts::group`
  (`yulius-domain/src/task.rs`) through `App::task_group`, the same rule as `taskInfo` in
  `status.js`.
- Instance service keys (Jev, Vidoc; SPEC-001 ADM-001/002, SPEC-195) are sealed with the instance
  master key in the `instance` row, never returned by any API, audited when set or removed, and
  take effect without a restart (`jev_key.rs`, `vidoc_key.rs`). "In the vault" in #1107 means
  this sealing; an agent secret would put the bot token in reach of agents.
- Text served from a sandbox's sessions to people passes `App::redact_for_sandbox` (the vault's
  matcher of that sandbox's agent secrets, SES-004).
- Prior art (#1107): a ~600-line sidecar polling the REST API, tied to one token and one
  Telegram user. The server knows the exact events and the people, so it can do this per person.

## Requirements

### Instance setup

- TGC-001: an instance administrator (superadmin or instance admin) sets the Telegram bot token
  in Settings → Administration. The server checks it with Telegram's `getMe` before saving: a
  token Telegram refuses is not saved and the answer says so; an unreachable Telegram saves
  nothing and says to try again. The saved token is sealed with the master key, never logged,
  audited (`telegram.bot_set`, `telegram.bot_removed`) without its value, and never returned:
  `GET /api/admin/telegram` answers its state (bot username, who set it and when, polling state).
- TGC-002: removing the token stops the channel at once (polling and sending end, without a
  restart) and unlinks every chat. Setting a token of another bot (another `getMe` id) also
  unlinks every chat; setting the same bot's token again keeps them.
- TGC-003: the server long-polls `getUpdates` (no webhook, no inbound port). After an error from
  Telegram or the network it retries with exponential backoff (1 s doubling to at most 60 s, reset
  after a successful poll); the admin state shows `retrying` with the last error's kind.
- TGC-004: the Telegram Bot API base URL is configurable (`YULIUS_TELEGRAM_API_URL`, default
  `https://api.telegram.org`); a non-HTTPS URL is accepted only for a loopback host (tests run a
  fake server). The token never appears in a log line, an error or an API answer.

### Personal linking

- TGC-010: Settings → Notifications shows a "Telegram" row to every signed-in person: Link
  Telegram once a bot is configured (before that, who can add one), or the linked state with
  Unlink and the event choices.
- TGC-011: Link Telegram (`POST /api/me/telegram/link`) answers a single-use code valid for
  10 minutes and the link `https://t.me/<bot>?start=<code>`. Only a browser session may ask for
  it: an API token (CLI, remote MCP, agent) is refused. A new code replaces the person's previous
  one. The code is stored only as its hash.
- TGC-012: the bot binds the private chat that sends `/start <code>` with a valid, unexpired,
  unused code to that code's person and consumes the code. A person has at most one chat and a
  chat at most one person: the person's previous chat, or the chat's previous person, is unlinked.
  Audited `telegram.linked`. Group chats never link.
- TGC-013: unlink from Settings (`DELETE /api/me/telegram`) or by sending `/stop` in the linked
  chat. Audited `telegram.unlinked` with where it came from (`settings`, `telegram`).
- TGC-014: a chat that is not linked gets one fixed reply (how to link) and nothing else: nothing
  is created or changed. It gets that reply at most once per 10 minutes (bounded memory). A
  linked chat's help reply to anything but `/stop` is bounded the same way.

### Notifications

- TGC-020: a linked person chooses what reaches the chat: Needs you (on by default), Done,
  Failed or stopped abnormally, and (instance administrators only) Subscription problems
  (sign in again, usage limit, error). The choices are kept on the server per person.
- TGC-021: a notification reaches a person only for an event their own event stream would carry
  (the same envelope and access rule as `WS /api/events`, re-read at each event: a person who
  lost access to a sandbox, was disabled or removed gets nothing more from it).
- TGC-022: a task notification is sent when a task enters the Needs-you group (`AttentionFacts`),
  a new `needs_you` request arrives while it is there, it becomes Done, or it ends failed or
  stopped abnormally (`failed_*`, `stopped_*` except a person's stop). It carries the sandbox,
  task number and title, the `needs_you` action/problem (or "Decide"), and a link
  `{PUBLIC_URL}/s/{slug}/t/{n}`. A task first seen after a restart is notified only for a state
  entered after the server started.
- TGC-023: every notification's text passes the same redaction as transcripts: a task's text
  `App::redact_for_sandbox` of its sandbox; an account's text the instance-wide redaction of
  credential shapes. Plain text only (no Telegram markup), at most 4000 characters.
- TGC-024: at most 5 messages per person per minute. Notifications beyond that in the same
  minute are folded into one summary sent when the minute ends ("3 more notifications: …", at
  most 20 lines kept, then a count), only to the chat still linked to that person while they are
  enabled. Sending is a bounded queue; a full queue drops with a log.

### Channel interface

- TGC-030: a `NotificationChannel` port (identity check, `send`, `receive`) is the only thing the
  Telegram code implements; the event side (classification, visibility, rate bound, redaction)
  never names Telegram, so another channel (Slack, ntfy, e-mail) can be added beside it.

### Later (part 2, not in this change)

- TGC-101 (later): with intake switched on per person, a reply to a notification goes to that
  task as a `task_message` under the owner rule.
- TGC-102 (later): a new message (text, photo, document, voice note up to 20 MB) becomes a task
  in the person's default sandbox, labelled "from Telegram", under `task_create`'s bounds, gates
  and access checks.
- TGC-103 (later): `/status` lists the person's tasks that need them; `/task <sandbox> <text>`
  targets another sandbox.
- TGC-104 (later): CI failing on the person's pull request as an event choice.

### Known limits (accepted for part 1)

- The notifier follows the bus like an event stream but cannot resync: if it falls more than 1024
  events behind, the missed notices are logged and lost. It skips all work while nobody is linked.
- The poll's update offset lives in memory: after a restart or a new bot, the last batch Telegram
  had not yet confirmed is handled again (a `/stop` answered twice, a used code called invalid).
- Telegram's 429 answer is not retried with its `retry_after`; the message is dropped with a log.
- `WS /api/events` now decides "playground guest" again at each re-check from the refreshed person
  instead of once at connect (the shared `App::envelope_reaches`); the result is the same for
  every existing account.

## Plan

- [x] Spec, ADR and the channel port with Telegram's wire types
- [x] Bot token stored sealed, checked with getMe, set and removed by instance admins
- [x] Long polling with backoff against a fake Telegram, stopped when the token goes
- [x] Link codes, /start and /stop binding, unlink from Settings, audited
- [x] Notifications classified, filtered by the event stream's rule, redacted and rate bound
- [x] Settings rows for the bot and for linking, with mock routes and an e2e test
- [x] Docs, changes, gates green and an independent review in a fresh session

## Acceptance criteria

- An instance administrator sets a token: the fake Telegram's `getMe` is asked, the stored
  value is sealed (not the token's bytes), the view never contains it, `telegram.bot_set` is
  audited; a refused token is not saved; a member is refused (TGC-001).
- Removing the token stops polling (the fake sees no more `getUpdates`) and unlinks (TGC-002).
- After the fake answers errors, polling resumes when it recovers and the state says `retrying`
  meanwhile (TGC-003).
- A browser session gets a code and a `t.me` link; a bearer token is refused; `/start <code>`
  from a private chat links it (audited), a second use or an expired code does not, a group chat
  does not; `/stop` and Settings unlink (audited) (TGC-011..013).
- An unlinked chat's messages get the fixed reply once and create nothing (TGC-014).
- A task entering Needs you sends the linked person a message with the task's link; a person
  without access to that sandbox gets nothing; a person who chose not to get Done gets no Done
  (TGC-020..022).
- A `needs_you` whose text holds a known agent secret reaches Telegram redacted (TGC-023).
- Seven notifications in one minute: five messages, then one summary of two (TGC-024).

## Test plan

- Unit (`yulius-domain::notify`): classification of task transitions, message text and links,
  the per-person rate bound and summary, link-code shape, update parsing.
- Unit (`yulius::wiring::telegram`): the HTTP client against a local fake: `getMe`, `sendMessage`,
  `getUpdates` offsets, refused token, malformed answers, the token absent from errors.
- Integration (`crates/yulius/tests/telegram.rs`): the whole server with a fake Telegram Bot API
  (axum on 127.0.0.1) through `YULIUS_TELEGRAM_API_URL`-equivalent wiring: every acceptance
  criterion above, waiting on conditions, never on sleeps.
- Store: migration 0050 on an older database; links, codes and the token round-trip.
- e2e (`e2e/tests/telegram.e2e.ts`, mock mode): the administrator sets and removes the bot; a
  person links (code and link shown), changes event choices and unlinks; no WCAG violations.

## Decisions

- [ADR-0287](../../docs/decisions/0287-a-personal-telegram-channel-behind-a-notification-channel-port.md):
  one bot per instance with its token sealed like the instance's service keys, personal linking by
  a single-use browser-issued code, long polling, and a `NotificationChannel` port fed from the
  UI event bus under the event stream's own visibility rule.
- Reversible defaults: 5 messages a minute and 20 summary lines; the unlinked reply at most every
  10 minutes; removing or changing the bot unlinks every chat (re-linking takes a minute, a stale
  chat of another bot would never receive anything).
- The change is larger than the ~400-line guideline: the issue's part 1 acceptance criteria are
  one vertical slice (a bot nobody can link, or links that send nothing, are not shippable).

## Evidence

All commands were run in this worktree on 2026-11-07.

- `scripts/dev ci` at `019944a3`: exit 0. Repository check, script unit tests, `cargo check`,
  `fmt --check`, `clippy -D warnings` (Linux and macOS targets), `nextest`: 3228 tests passed,
  16 skipped, then deps and the rest of the gate. An earlier run at `1fcd1fc3` failed one test,
  `telegram::a_summary_never_reaches_a_chat_unlinked_meanwhile`. Under load, the test moved the
  clock before the notifier had read all its events. The test now waits on a notice to a third
  person before it moves the clock (`019944a3`). All 9 Telegram tests then passed 8 stress
  iterations (`--stress-count 8`) with every CPU busy.
- Unit tests: `yulius-domain` `notify::tests` (11: classification, signatures, message text and
  links, truncation, the rate bound and its summary, chat commands, code shape). `yulius-store`
  `telegram::tests` (3: the bot round trip, single-use and expiring codes, one chat per person and
  one person per chat). `yulius` `wiring::telegram::tests` (5: HTTPS except loopback, token shape,
  getMe/sendMessage/getUpdates against a local fake, a refused token and an unreachable API typed
  without the token, malformed updates). `yulius-app` `telegram::tests` (code alphabet).
- Integration, `crates/yulius/tests/telegram.rs` (9, against a fake Bot API on 127.0.0.1; the
  real Telegram is never called):
  - TGC-001/002: an administrator sets and removes the bot, and another bot unlinks every chat.
  - TGC-011..013: a single-use code links a chat, and both ways of unlinking are audited.
  - TGC-014: an unlinked chat gets one fixed reply, and a linked chat's help is bounded the same
    way.
  - TGC-020..023: Needs you reaches only the people who can view the task, redacted (a known
    agent secret value never arrives), and the Done and failure choices work.
  - TGC-024: a burst is folded into one summary, and no summary reaches a chat unlinked meanwhile.
  - TGC-003: polling retries after errors and recovers.
  - A state entered before the start is not told again.
  - Subscription notices reach administrators only, redacted. A disabled person gets nothing.
    A relinked chat is audited.
- e2e `tests/telegram.e2e.ts` (4: the bot card, a member without it, link, choices and unlink,
  the administrator's subscription choice; no WCAG violations): passed. In the full suite
  (`npx e2e run`, 871 tests, 106 min), 868 passed, 1 was skipped and 2 failed in files this change
  does not touch: `navigation.e2e.ts` ("sidebar rows show the task number…", a 118 s action
  timeout) and `secrets.e2e.ts` ("a failed copy shows no check…", a title not yet updated). Both
  files passed when run again (39/39). They are timing races in tests that already existed. Filing
  them as an issue failed: GitHub answered HTTP 410 (issues are off on this fork). They are listed
  for the upstream pull request instead.
- `scripts/dev security`: **not run** (exit 2: this instance has no Vidoc key). It neither passed
  nor failed. Upstream CI's `security` job scans the pull request with its own key.
- Independent review in a fresh session (skill `om-code-review`), two rounds:
  - Round 1: one major finding, a held summary sent to an unlinked or relinked chat. It was fixed
    test-first. The minor findings were fixed or recorded under Known limits.
  - Round 2: APPROVE at `7fe3c75e`, with one minor finding: every bus event read the links. That
    was fixed in `1fcd1fc3`.
