# 0475. browser_fill_secret types real key presses

## Status

Accepted for [SPEC-648](../../.ai/specs/SPEC-648-browser-fill-secret-types-real-key-press.md).
Supersedes the `browser_fill_secret` part of
[ADR-0471](0471-a-viewer-paste-becomes-the-displays-clipboard-and-a-real-paste-key.md).

## Context

ADR-0471 made `browser_type` type real key presses through CDP, so agents can fill xterm.js
terminals, and left `browser_fill_secret` on `Input.insertText` because its page rules and edit
checks were pinned by SPEC-450 and SPEC-483 tests built on insertion. It named no safety reason.
Inserted text fires only an `input` event, which xterm.js (5.x) drops while it has seen a key go
down and not up: the text stays in its hidden helper textarea.
Measured in Chrome 158: with a key held in the viewer (as Ctrl is for Ctrl+V), an inserted user
name never reached the terminal. Pages that read only keys drop insertion always. A secret, user
name or one-time code typed into a terminal (a CLI sign-in prompt in the sandbox browser) could
so be lost while the tool answered `filled` (#1740).

## Decision

- `browser_fill_secret` types the value, the user name and the code with the same key presses
  as `browser_type`: per character a key down carrying its text and a key up, through the tab's
  own DevTools session; each `\n` presses Enter. Everything around the keys is unchanged and
  stays under the input lock: the page rule asked of the page as read with the lock held, the
  focus and frame checks, the leaving check, the record of the secret before the first key, the
  edit checks after each line before its Enter, and the read after typing with its astray
  reports and exposure.
- A text holding a tab or another control character (other than `\n`, and a `\r` that ends a
  line) is refused before anything is typed or recorded. A pressed Tab moves the focus, so the
  rest of the secret would go to the next field, which may show it; other control characters
  have no key and would be left out, typing a different secret.
- A terminal gets no rule of its own: its focus is a text field, not a password field, so a
  secret typed into it exposes the tab's host and screenshots of it are refused, as for any field
  that does not mask it. A terminal draws its echo on a canvas the page-text probe cannot read,
  which this rule already assumes.

## Alternatives

- Keep insertion and add key typing only where the focus is a terminal: the probe cannot tell a
  terminal's helper textarea from any other textarea, and two ways of typing would make a
  secret's arrival depend on the page.
- Insert a tab, or a control character, as one inserted character amid the keys: a terminal, the
  case being fixed, would still drop it, and the secret would arrive cut without a sign.
- Refuse typing a secret while no text field has the focus, since keys then reach the page's
  shortcuts: it would also refuse fields inside closed shadow roots and frames of the secret's
  own sites, which the probe cannot read and SPEC-483 E8 allows.

## Consequences

Secrets reach terminals and other pages that read keys. Key events expose nothing new: the page
allowed to get the secret reads its field either way, and the text crosses the same DevTools
session. A navigation of the page's own can now land between two keys rather than two lines;
the read after typing reports it as before (SPEC-483 E3). Typing takes two DevTools calls per
character, so the input lock, and with it screenshots, is held a little longer (a 64-character
secret about 20 ms). Keys typed while nothing takes text can trigger the allowed page's own
shortcuts, as a person's typing would; the host is then exposed unless a password field holds
the secret. A secret with a tab can no longer be typed by an agent; a person types it.
