# 0392. A linked Telegram chat answers and starts tasks when its person turns intake on

## Status

Accepted (2026-11-20). Specified in
[SPEC-512](../../.ai/specs/SPEC-512-marcin-s-five-issues-1412-1418-1413-1107.md) part 4; task #494,
issue #1107 (part 2, intake). Builds on
[ADR-0351](0351-a-personal-telegram-channel-behind-a-notification-channel-port.md) (the personal
channel and its port), [ADR-0331](0331-anyone-tidies-a-task-and-only-its-owners-consent-passes-control.md)
and [ADR-0379](0379-a-needs-you-names-who-may-answer-it.md) (who steers or answers a task).

## Context

- Part 1 sends notifications to a person's linked chat; the chat's own messages get one help
  reply. #1107's recommended option also takes work back: a reply reaches its task, a message
  becomes a task, `/status` and `/task`, off by default and on per person.
- A stolen Telegram account must not start tasks on its own, and a reply must not let a second
  person steer someone else's task.
- Telegram's bot API downloads files of at most 20 MB.

## Decision

- **Off by default, on per person, from a browser.** Each person's linked chat has `intake` and a
  default project (one they may work in), set in Settings → Notifications only from a browser
  session, as linking is; audited `telegram.intake`. Off, nothing is created or sent to a task;
  a chat that changes hands starts with it off.
- **A reply is the person's `task_message`.** The channel's `send` returns the message's id; the
  server remembers which of the bot's messages were about which task (the last 500 per chat,
  forgotten when the chat changes hands). A reply to one is a `task_message` from the person,
  with every guard their own agent's has: the owner rule, a named person's answer (to the
  request the notice told of, never a later one), the owner's personal reach, a parked task
  resumed; it is labelled as from Telegram. The bot's answers about a task count too; a reply
  to a message about no task starts nothing. Intake runs off the poll, so the next poll
  confirms the update and a restart does not hand a message over twice. Each chat has one
  lane, a queue and one worker, so its messages are handled in the order sent; it holds 20
  waiting (a whole poll answer, as after a restart), and a message past that is not lost
  silently: it is logged and the chat is told to send it again (once per quiet window). Each
  message is handled only if, when its turn comes, the link it came through still holds with
  intake on: turning intake off or `/stop` ends what waits too.
- **A message is a task as the person.** Any other message (text, caption, photo, document,
  voice, audio, video; an album is one) starts a task in the default project through
  `create_task_as` (access, onboarding gate) under the tools' hourly bound, origin `telegram`
  ("From Telegram"). Like any task the person starts, it holds their personal secrets and MCP
  servers: that is why intake is the person's own choice and off by default. Its
  files, each at most 20 MB and at most 10, are fetched by the channel (`fetch_file`) into the
  person's drafts folder and named in the prompt.
- **Commands.** `/status` (what waits for them, as the digest's first section), `/task <project>
  <text>` and `/help`. Answers that carry a task's text are redacted like transcripts.

## Alternatives

- **Intake on for everyone once a bot is set.** Widens what a stolen chat can do without its
  person choosing it; the issue rules it out.
- **A reply always starts a new task.** Loses the conversation the notification was about.
- **Taking control on a reply.** Moves the task's GitHub identity; ADR-0365 keeps that for an
  explicit take.
- **Uploading files into the worktree.** A draft attachment is outside every worktree until the
  task starts, as the web's new-task attachments are.

## Consequences

- A person answers and starts work from their phone, as themselves, and the task says it came
  from Telegram.
- A schema change (migration 0073, schema version 75): two `telegram_links` columns and the
  `telegram_sent` table. The port gains message ids, replies, files and `fetch_file`.
