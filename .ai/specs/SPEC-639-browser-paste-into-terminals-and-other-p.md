---
yulius-task: 124bb01c-015c-4ba2-9229-4e806da3baa2
status: in-progress
---

# SPEC-639. A paste in the Browser panel reaches the page as a real paste

## Goal

A person who pastes into the Browser panel (Ctrl+V, ⌘V, or their browser's Paste menu) gets the
text into the page as a real paste, also when the page is a terminal (xterm.js) or another page
that reads key or `paste` events. An agent's `browser_type` reaches such pages too. A paste that
cannot be delivered is never lost silently: the panel says so.

## Context

A person spent an hour signing in a Claude subscription: in the sandbox Chrome, the quartio-ops
Settings page's "Sign in a Claude account" xterm terminal asks for "Paste code here if prompted",
and Ctrl+V / ⌘V on the Browser panel did nothing. Agents' browser tools fail the same way.

- `crates/yulius-server/web/assets/browserstream.js`: the viewer's `paste` handler sent the
  clipboard text as `key` `char` messages (8 000 characters each).
- `crates/yulius-browser/src/instance.rs`, `display_key`: a `char` text of up to
  `TYPED_TEXT_MAX_CHARS` (64) characters is typed as real key presses on the X display; longer
  text goes to CDP `Input.insertText`. Without a display, any text longer than one character goes
  to `Input.insertText`. `browser_type` (`type_lines`) inserts every line with `Input.insertText`.
- xterm.js and other pages that read `keydown`/`keypress`/`paste` ignore `Input.insertText` (it
  fires no key and no `paste` event; xterm.js reads its hidden textarea only from those events). A
  Claude OAuth code is longer than 64 characters, so its paste was lost without any sign.
- The sandbox display is a private Xvfb per sandbox (SPEC-002 ADR-15); the X thread in
  `xserver.rs` owns the connection and applies XTest input in order. `xclipboard.rs` reads the
  CLIPBOARD after a page's Copy button (SPEC-584, ADR-0423).
- Parallel task #622 ("paste things copied outside into the sandbox") is about the same symptom
  family; this task owns the Browser panel's paste and the agent typing path.

## Requirements

- **PST-001** A viewer's paste is one `paste` message holding the whole text. On a display, the
  text becomes the CLIPBOARD selection of the sandbox's private X display (owned by Yulius's X
  connection, answering `TARGETS`, `UTF8_STRING`, `text/plain;charset=utf-8`, `STRING` and
  `TEXT`), then Ctrl+V is pressed on the display in order with the other keys, so Chrome fires a
  genuine `paste` event (and pastes into an editable field) as for a local paste.
- **PST-002** The limits stay: at most 32 000 characters and `KEY_TEXT_MAX_BYTES` (32 KiB) of text
  per paste. The viewer refuses a longer paste with a message instead of cutting it.
- **PST-003** Without a display, or when the display's CLIPBOARD cannot be taken, the text is
  typed as real key presses (CDP key events per character without a display; XTest key presses on
  the display), never through `Input.insertText`. `Input.insertText` stays only for text no key
  types: IME composition (`char` messages).
- **PST-004** A paste is never dropped silently: when the page does not read the pasted text
  within the paste window, or the paste cannot be sent or applied, the viewer says so
  (`paste_failed`, or a local message when the text is too long or the socket is closed).
- **PST-005** `browser_type` types its text as real key presses through CDP (each character a key
  down with its text and a key up; `\n` presses Enter), so an agent can fill an xterm terminal.
  The secret paths (`browser_fill_secret`) keep their page rules and edit checks per line.
- **PST-006** Short typed text (`char` up to 64 characters, `down`/`up` keys) keeps its order with
  other keys, and a paste keeps its order with keys sent before and after it.
- **PST-007** The pasted text is held only for a bounded time (30 s, or until another owner takes
  the CLIPBOARD or a newer paste replaces it), then the selection is released and the text erased.

## Plan

- [ ] Write the spec and the failing paste tests
- [ ] Own the display's CLIPBOARD and press Ctrl+V on the X thread
- [ ] Paste on the instance: a real paste on a display, real keys without one, paste_failed when unread
- [ ] Send a viewer paste as one message and tell the person when it failed
- [ ] Type an agent's browser_type text as real key presses
- [ ] Prove 100 and 5 000 character pastes into real Chrome pages through the viewer path
- [ ] Record the decision and keep the docs true
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- **AC-1 (PST-001, PST-006)** On Xvfb with real Chrome, through the viewer WebSocket: a page that
  only listens to `paste` and `keydown` receives a 100-character and a 5 000-character paste as
  `paste` events with the exact text; an xterm.js terminal receives both pastes as its data; keys
  typed before and after a paste arrive in order around it.
- **AC-2 (PST-001, PST-007)** On a private Xvfb, after a paste a test X client finds the display's
  CLIPBOARD owned, gets the exact text for `UTF8_STRING` and the targets for `TARGETS`, sees a
  Ctrl+V press and release, and after the hold the selection is released.
- **AC-3 (PST-003)** Without a display, a paste and `browser_type` send CDP `Input.dispatchKeyEvent`
  per character and no `Input.insertText`.
- **AC-4 (PST-004)** A paste no client reads ends in `paste_failed` for the viewer; the panel
  shows a message for it, for a too-long paste and for a closed socket.
- **AC-5 (PST-002)** A paste over 32 KiB is refused by the server (invalid) and by the viewer (a
  message), never cut.
- **AC-6 (PST-006)** Short `char` text still types as keys in order with `down`/`up` keys.

## Test plan

- Unit (browser crate): selection request answers (targets, text, refused targets), the paste
  window and hold expiry, Ctrl+V with and without a held Control.
- Native Xvfb test (runs in CI where Xvfb exists): AC-2 with a requestor client on the display.
- Fake CDP instance tests: AC-3, AC-5, the order of paste and keys, and the outcome on the fake
  display.
- Server route test: a `paste` message goes through the input budget and a failure reaches the
  viewer as `paste_failed`.
- Browser e2e (`e2e/tests/browser.e2e.ts`): the panel's paste sends one `paste` message; too long
  and `paste_failed` show messages.
- Opt-in live test (`crates/yulius/tests/live_display.rs`, Xvfb + ffmpeg + Chrome): AC-1, run here
  with Google Chrome.

## Decisions

- The paste is its own viewer message, not a `char`: the server must know a paste from IME text to
  deliver it as a paste, and one message keeps the text whole (one CLIPBOARD value).
- The X thread both owns the selection and presses Ctrl+V in one command, so the paste is in order
  with keys queued before and after it and no other client can take the selection in between.
- Delivery is judged by Chrome asking the selection for text within the paste window (3 s): that
  is the only sign X gives that the page read it. Not asked means the page took no paste (for
  example the focus is not in the page); the viewer is told, nothing is typed instead (typed keys
  outside a field would trigger the page's shortcuts).
- `browser_type` uses CDP key events, not the display: they reach the agent's tab even when it is
  not the shown one and work without a display.
- [ADR-0471](../../docs/decisions/0471-a-viewer-paste-becomes-the-displays-clipboard-and-a-real-ctrl-v.md)
  records the paste path.
