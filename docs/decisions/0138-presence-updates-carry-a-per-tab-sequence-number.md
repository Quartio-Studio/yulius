# 0138. Presence updates carry a per-tab sequence number

## Status

Accepted. Specified in [SPEC-170](../../.ai/specs/SPEC-170-fix-489-439-463-tools-and-the-keyboard-s.md)
(TKS-003); task #135, issue #463. Amends [ADR-0091](0091-notepad-presence-is-relayed-not-stored.md):
the relay stays stateless, and receivers now order one tab's updates themselves.

## Context

Presence in a notepad (SPEC-112) and in a file (SPEC-127) is relayed by the server, stored
nowhere, and sent by one browser loop (`presence.js`) that keeps one update on its way at a time so
the others get them in order. A page that hides or goes away cannot wait for that update's answer:
it sends its leave (`selection: null`) at once, with `keepalive`. The leave can then reach the
server before the earlier update, and the others draw the earlier selection last; the stale caret
stays until the tab says hello again or 45 s pass.

## Decision

- Every presence update carries `seq`, a number the tab increments for each update it sends (from
  1 at page load; one counter for all the tab's notepads and files). The presence requests accept
  it (an unsigned integer, default 0) and the server relays it unchanged in `note.presence` and
  `file.presence`.
- A receiver remembers, per tab (`user_id` + `client`), the newest `seq` it heard, also after that
  tab left, and drops an update whose `seq` is not above it. `seq` 0 (a page from before this
  change) is taken as it comes. A tab not heard of for 45 s (the presence expiry) is forgotten, so
  what a receiver keeps stays bounded.

## Alternatives

- **Make the leave wait for the update on its way.** A page going away may be gone before the
  answer comes; the leave would then never be sent, which is worse than a late one.
- **Order on the server** (keep the newest `seq` per tab and refuse older ones). It would make the
  relay stateful (ADR-0091) for no gain: the receivers must keep the order anyway, as events of one
  tab may still interleave with a reconnect's.
- **A timestamp instead of a counter.** `Date.now()` can step back (a clock change); a counter
  cannot, and a tab only compares its own numbers.

## Consequences

- A leave sent as a page goes can no longer leave a stale caret behind, in Notes and the Editor.
- The wire grows by one integer per update. Older pages send no `seq` and keep the old behaviour.
- A tab's counter restarts on reload, with a new `client` id, so the numbers of two loads never
  meet.
