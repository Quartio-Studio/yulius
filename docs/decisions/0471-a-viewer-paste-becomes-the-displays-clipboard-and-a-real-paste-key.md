# 0471. A viewer paste becomes the display's CLIPBOARD and a real paste key

## Status

Accepted for [SPEC-639](../../.ai/specs/SPEC-639-browser-paste-into-terminals-and-other-p.md).

## Context

The Browser panel sent a person's paste as `char` text. Text over 64 characters went to CDP
`Input.insertText`, which fires no key and no `paste` event. Terminals (xterm.js) and other pages
that read key or `paste` events ignored it, so a Claude OAuth code pasted into a sign-in terminal
was lost without a sign. Agents' `browser_type` used `Input.insertText` too. The sandbox browser
runs on a private Xvfb whose X thread already applies all viewer input in order
([ADR-0423](0423-a-webpage-copy-returns-through-the-clicking-viewers-connection.md) reads its
CLIPBOARD after a page's Copy button).

What Chrome 158 does, traced on Xvfb with `xtrace`:

- xterm.js turns Ctrl+V into the `^V` character and cancels the event, so Chrome never pastes
  into a terminal on Ctrl+V. It leaves Shift+Insert, Chrome's other paste key, to the browser.
- Chrome reads one paste several times: `TARGETS`, `UTF8_STRING`, then `TARGETS` again. If the
  CLIPBOARD changes between those reads, or one is left unanswered, the page's `paste` event
  gets empty text after Chrome's one-second timeout.

## Decision

- A viewer's paste is its own message, `paste`, with the whole text (at most 32 KiB; the viewer
  refuses more with a message instead of cutting it). The viewer socket's message bound is
  256 KiB, so 32 KiB of text survives JSON escaping.
- On a display, one X-thread command makes the text the CLIPBOARD (owned by an unmapped window of
  Yulius's X connection, answering `TARGETS`, `UTF8_STRING` and `text/plain;charset=utf-8`) and
  presses Shift+Insert. Held Ctrl, Alt and Meta are lifted around it, since Ctrl+Insert is Copy.
  Chrome reads the selection and fires a genuine `paste` event, in order with the keys queued
  before and after.
- Delivery is judged by a client fetching the text within 3 s. A paste whose text nobody fetched
  is answered `paste_failed`, and the panel says so. Nothing is typed instead, since typed keys
  outside a field would trigger the page's shortcuts.
- A newer paste replaces the CLIPBOARD only once the page asked nothing for 100 ms, and while
  a paste nobody asked for yet may still be read (at most 1 s in all). Chrome asks when the
  page's main thread handles the key: 27 ms on an idle page, up to 360 ms on a busy one
  (measured in review round 3). Once a paste was replaced unread, the next unread one holds a
  newer paste only 200 ms, so repeated pastes into a page that takes none do not hold the
  input a second each. This also covers a paste from another viewer. The X thread
  keeps answering events while it waits, and the viewer's connection does not wait for a
  paste's outcome: the paste is queued in order with that viewer's keys, and `paste_failed`
  arrives when it is known.
- The viewer leaves a person's own Ctrl+V, ⌘V and Shift+Insert to their browser, whose `paste`
  event carries their clipboard; forwarded as keys they would paste the sandbox's CLIPBOARD.
- The text is held 30 s at most, or until another client takes the CLIPBOARD or a newer paste
  replaces it. Then the selection is let go (only when it is still Yulius's) and the text erased.
- Where the CLIPBOARD cannot be taken, the X thread types the text as key presses, lifting the
  viewer's held Ctrl/Alt/Meta. Without a display, a paste is typed as CDP key events, one per
  character. `Input.insertText` remains only for composed IME text (`char`) and secret typing.
- `browser_type` types its text as CDP key events: a key down with the character's text and a
  key up; `\n` presses Enter and `\t` presses Tab (moving the focus, as a person's Tab does);
  other control characters are left out. They go to the agent's tab through its own DevTools
  session and need no display. `browser_fill_secret` keeps `Input.insertText`: its page rules
  and edit checks are per line and pinned by SPEC-450 and SPEC-483.

## Alternatives

- Ctrl+V: what a person presses locally, but xterm.js swallows it as `^V` (measured), so a
  terminal, the case that failed, would still get nothing.
- Typing a paste as XTest key presses: no `paste` event, so pages that read `clipboardData` miss
  it. 32 000 characters become 64 000 or more X events, and each non-US character needs a
  remapped keycode.
- Dispatching a synthetic `paste` event from page JavaScript: untrusted (`isTrusted` false), and
  editors and terminals do not paste it into fields.
- CDP `Input.insertText` with a longer key-press threshold: still no `paste` event, and long codes
  would still be lost on pages that ignore insertion.
- Counting a `TARGETS` request as delivery, or a fixed delay between pastes: both emptied the
  first of two quick pastes in Chrome (the second paste replaced the CLIPBOARD during the first
  one's reads, and a sleeping thread left a read unanswered).

## Consequences

Pastes reach terminals and other key- or paste-driven pages exactly as a local paste would,
including through cross-origin frames. A page that binds Shift+Insert to something else gets that
instead. The viewer hears of a paste nobody read after the 3 s window. The pasted text lives in
Yulius's memory and in the sandbox's CLIPBOARD for at most 30 s; a page granted clipboard-read
could read it meanwhile, as on a person's own computer. `browser_type` takes two CDP calls per
character, slower than one insertion per line: 5 000 characters took 1.5 s in Chrome 158, so the
largest text (64 KiB) holds the input lock, and with it screenshots, for about 20 s. Text typed
with no field focused can trigger the page's own shortcuts, as a person's typing would. A secret
typed by `browser_fill_secret` into a terminal is still not taken.
