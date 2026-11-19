# 0380. An end-of-day digest built from task state, off by default

## Status

Accepted (2026-11-19). Specified in
[SPEC-512](../../.ai/specs/SPEC-512-marcin-s-five-issues-1412-1418-1413-1107.md) part 3; task #494,
issue #1413. Builds on [ADR-0367](0367-an-idle-session-is-parked-and-resumed-by-a-message.md)
(parked sessions) and [ADR-0351](0351-a-personal-telegram-channel-behind-a-notification-channel-port.md) (the
personal Telegram channel); changes neither.

## Context

- A person with many tasks has to open each project to learn which wait for a decision, which
  left an action in their last message and which are finished. On a small instance the idle
  sessions held 9 of 11 GB until someone read every last message and stopped them by hand.
- Parking (ADR-0367) frees the memory after 30 minutes, but nothing gathers what is left for the
  person, and a parked task's last message is no longer in front of anyone.
- The browser's notification center is built from the event stream while a page is open; the
  Telegram channel sends single events. Neither keeps a summary for later.

## Decision

- **An instance setting, off by default.** `enabled`, `at` (`HH:MM`, UTC) and
  `park_idle_hours` (1 to 720, or none), set by instance administrators in Settings and stored in
  `instance.digest`. An instance that changes nothing sends nothing and parks nothing more.
- **Once a day from the background tick.** Due when today's time passed and it has not run
  since; the run is taken with one conditional write of `digest_ran_at`, so restarts and ticks
  never send it twice. Turning it on never sends at once.
- **Built from task state and the transcript, no model.** Per person, in order: tasks waiting for
  them (an open `needs_you`, a question in the terminal, or another person's request that names
  them), tasks parked since the last digest with how they ended and the end of the agent's last
  message (300 characters), and how many still work (ten named). Nothing to report: no digest.
  Every text is redacted as the sandbox's transcripts are.
- **Its parks are narrower than ADR-0367's.** With a threshold, it first parks running tasks
  quiet that long whose agent waits for its turn (not Done) and asks nobody, under every other
  guard of ADR-0367. Done tasks and tasks waiting for a person are never parked by the digest.
- **Stored for the browser, sent to Telegram.** The last seven digests per person are kept; the
  person's open pages get `digest.ready`, the bell reads the latest when a page opens and opens
  `/digest`; a linked chat gets one message unless the person turned `digest` off among its
  choices (on by default).

## Alternatives

- **A model that picks the actions out.** Better wording, but costs a subscription's usage every
  night and can drop an action; the last message's end is what the agent itself left. Kept as a
  later option.
- **A per-person browser switch.** The issue's switch is the instance's; a person who does not
  want it ignores one bell entry a day, and the Telegram copy can be turned off.
- **Sending only to open pages.** A digest made at night would be lost to everyone who closed the
  tab.
- **Letting the digest park Done and waiting tasks too.** The issue rules it out; ADR-0367 already
  parks them after its grace period.

## Consequences

- One message a day says what is left for the person, with links, including what parked tasks
  asked for in their last words.
- An instance that turned ADR-0367's parking off or set it long can still free quiet sessions
  once a day.
- A schema change (migration 0070): two `instance` columns, a `digests` table and the Telegram
  choice.
