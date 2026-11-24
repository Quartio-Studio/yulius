# 0471. A viewer paste becomes the display's CLIPBOARD and a real Ctrl+V

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

## Decision

- A viewer's paste is its own message, `paste`, with the whole text (at most 32 KiB; the viewer
  refuses more with a message instead of cutting it).
- On a display, one X-thread command makes the text the CLIPBOARD (owned by an unmapped window of
  Yulius's X connection, answering `TARGETS`, `UTF8_STRING` and `text/plain;charset=utf-8`) and
  presses Ctrl+V (only V when the viewer already holds Control). Chrome reads the selection and
  fires a genuine `paste` event, in order with the keys queued before and after.
- Delivery is judged by a client asking for the selection within 3 s. A paste nobody asked for is
  answered `paste_failed`, and the panel says so; nothing is typed instead, since typed keys
  outside a field would trigger the page's shortcuts.
- The text is held 30 s at most, or until another client takes the CLIPBOARD or a newer paste
  replaces it; then the selection is let go (only when still Yulius's) and the text erased.
- Where the CLIPBOARD cannot be taken the X thread types the text as key presses, lifting the
  viewer's held Ctrl/Alt/Meta. Without a display a paste is typed as CDP key events per
  character. `Input.insertText` remains only for composed IME text (`char`) and secret typing.
- `browser_type` types its text as CDP key events (a key down with the character's text and a
  key up; `\n` Enter, `\t` Tab), which reach the agent's tab whether or not it is shown and need
  no display. `browser_fill_secret` keeps `Input.insertText`: its page rules and edit checks are
  per line and pinned by SPEC-450 and SPEC-483.

## Alternatives

- Typing a paste as XTest key presses: no `paste` event (pages that read `clipboardData` miss it),
  32 000 characters become 64 000 or more X events, and a remapped keycode per non-US character.
- Dispatching a synthetic `paste` event from page JavaScript: untrusted (`isTrusted` false), and
  editors and terminals do not paste it into fields.
- CDP `Input.insertText` with a longer key-press threshold: still no `paste` event, and long codes
  would still be lost on pages that ignore insertion.
- Answering the viewer before Chrome asks for the text: no way to tell a lost paste.

## Consequences

Pastes reach terminals and other key- or paste-driven pages exactly as a local paste would,
including through cross-origin frames. A viewer's socket waits up to 3 s on a paste nobody reads.
The pasted text lives in Yulius's memory and the sandbox's CLIPBOARD for at most 30 s; a page
granted clipboard-read could read it meanwhile, as on a person's own computer. `browser_type`
takes two CDP calls per character, slower than one insertion per line. A secret typed by
`browser_fill_secret` into a terminal is still not taken.
