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
  connection, answering `TARGETS`, `UTF8_STRING` and `text/plain;charset=utf-8`), then the paste
  key is pressed on the display in order with the other keys, so Chrome fires a genuine `paste`
  event (and pastes into an editable field) as for a local paste. The paste key is Shift+Insert:
  xterm.js makes Ctrl+V the `^V` character and cancels it (measured), while it leaves
  Shift+Insert to the browser.
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
  other keys, and a paste keeps its order with keys sent before and after it. A newer paste does
  not replace the CLIPBOARD while Chrome still reads the previous one.
- **PST-007** The pasted text is held only for a bounded time (30 s, or until another owner takes
  the CLIPBOARD or a newer paste replaces it), then the selection is released and the text erased.

## Plan

- [x] Write the spec and the failing paste tests
- [x] Own the display's CLIPBOARD and press the paste key on the X thread
- [x] Paste on the instance: a real paste on a display, real keys without one, paste_failed when unread
- [x] Send a viewer paste as one message and tell the person when it failed
- [x] Type an agent's browser_type text as real key presses
- [x] Prove 100 and 5 000 character pastes into real Chrome pages through the viewer path
- [x] Record the decision and keep the docs true
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- **AC-1 (PST-001, PST-006)** On Xvfb with real Chrome, through the viewer WebSocket: a page that
  only listens to `paste` and `keydown` receives a 100-character and a 5 000-character paste as
  `paste` events with the exact text; an xterm.js terminal receives both pastes as its data; keys
  typed before and after a paste arrive in order around it.
- **AC-2 (PST-001, PST-007)** On a private Xvfb, after a paste a test X client finds the display's
  CLIPBOARD owned, gets the exact text for `UTF8_STRING` and the targets for `TARGETS`, sees a
  Shift+Insert press and release (the viewer's held Control lifted around it), and after the
  hold the selection is released.
- **AC-3 (PST-003)** Without a display, a paste and `browser_type` send CDP `Input.dispatchKeyEvent`
  per character and no `Input.insertText`.
- **AC-4 (PST-004)** A paste no client reads ends in `paste_failed` for the viewer; the panel
  shows a message for it, for a too-long paste and for a closed socket.
- **AC-5 (PST-002)** A paste over 32 KiB is refused by the server (invalid) and by the viewer (a
  message), never cut.
- **AC-6 (PST-006)** Short `char` text still types as keys in order with `down`/`up` keys.

## Test plan

- Unit (browser crate): selection request answers (targets, text, refused targets), the paste
  window, the quiet before a newer paste, and hold expiry; Shift+Insert with and without a held
  Control.
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
- The X thread both owns the selection and presses the paste key in one command, so the paste is
  in order with keys queued before and after it and no other client can take the selection in
  between.
- The paste key is Shift+Insert, not Ctrl+V: the real xterm.js page received `\x16` (^V) for
  Ctrl+V and no paste, and pasted on Shift+Insert, which Chrome treats as Paste like Ctrl+V.
- Delivery is judged by Chrome fetching the text within the paste window (3 s): that is the only
  sign X gives that the page read it (a `TARGETS` request alone gives the page nothing). Not
  fetched means the page took no paste (for example the focus is not in the page); the viewer
  is told, nothing is typed instead (typed keys outside a field would trigger the page's
  shortcuts).
- The viewer's connection does not wait for a paste's outcome (review round 1): the paste is
  queued in order inside the connection's loop, and the outcome arrives as a future the loop
  serves, so a paste nobody reads no longer freezes that viewer's video for 3 s.
- A newer paste waits, answering X events, until the page fetched the previous text (also when
  another viewer queued it) and then asked nothing for 100 ms: Chrome reads
  one paste several times, and a CLIPBOARD replaced between those reads (or a read left
  unanswered by a sleeping thread) gave the first of two quick pastes empty text after Chrome's
  one-second timeout (traced with `xtrace`).
- `browser_type` uses CDP key events, not the display: they reach the agent's tab even when it is
  not the shown one and work without a display.
- `browser_type` keeps two CDP calls per character under the input lock: measured, 5 000
  characters take 1.5 s in Chrome 158, so the largest text (64 KiB) holds screenshots for about
  20 s, which an agent's typing of that size may take; chunking the lock would let a screenshot
  show half-typed text. A tab presses Tab (as a person's would); other control characters are
  left out and not counted.
- `browser_fill_secret` keeps `Input.insertText`: its page rules and per-line edit checks are
  pinned by SPEC-450 and SPEC-483 tests built on insertion. A secret typed into a terminal is not
  taken; that is a separate change.
- [ADR-0471](../../docs/decisions/0471-a-viewer-paste-becomes-the-displays-clipboard-and-a-real-paste-key.md)
  records the paste path.

## Evidence

- **Real Chrome through the viewer path (AC-1, PST-005)**: `YULIUS_LIVE_DISPLAY=1
  YULIUS_LIVE_CHROME=<wrapper: google-chrome --no-sandbox> cargo test -p yulius --test main
  live_display::a_viewer_paste -- --ignored` (Google Chrome 158.0.8127.36 on Xvfb, the host
  policy of `yulius host setup` installed): passed 3 runs in a row ("two pastes and three keys
  arrived in 377 ms"). The page that only listens to `paste` and `keydown` received
  `key a, paste 100, key b, paste 5000, key c`, both texts exact (ASCII; Polish letters and an
  emoji in the 5 000), all trusted; the xterm.js terminal received `short + "q" + long` as its
  data, and an agent's `browser_type "echo ok ż\n"` after it as `echo ok ż\r`. With the first
  paste sent the old way (`key`/`char`) the same test failed: the page got keys `a` and `b` and
  no `paste` event.
- **What real Chrome showed on the way** (`xtrace` of Chrome's X connection, timestamps of the
  page's events): Ctrl+V into xterm.js gave the terminal `\x16` and no paste; the first of two
  quick pastes came empty after a one-second stall, because a newer paste replaced the
  CLIPBOARD during Chrome's reads (`TARGETS`, `UTF8_STRING`, `TARGETS`) or a fixed sleep left
  one read unanswered. Hence Shift+Insert, "fetched text" as the delivery signal and the quiet
  wait (Decisions). xclip as owner with the same keys pasted in 3 ms, which ruled out the key
  path.
- **Xvfb in CI (AC-2)**: `display::copy_input_tests::a_paste_owns_the_clipboard_and_presses_shift_insert_on_a_private_display`
  (a stand-in client gets Shift+Insert, reads TARGETS then the exact 100 and 5 000 character
  texts, sees a held Control lifted and pressed again, and an unread paste ends Unread) and
  `xclipboard::tests::native::the_paste_owner_answers_only_its_text_and_lets_it_go` pass;
  the display test failed with `Unread` against the Ctrl+V version before Shift+Insert.
- **Server path (AC-3..AC-6)**: `browser::a_viewer_paste_is_a_real_paste_on_the_display_in_order_with_keys`,
  `browser::a_headless_paste_is_typed_as_key_presses`, `browser::agent_tools_drive_the_tabs_the_viewer_sees`
  and `browser::viewer_input_is_real_display_input_and_copy_reaches_the_clipboard` pass; the
  sixfold-escaped 32 KiB paste failed (socket closed) before the 256 KiB bound.
  `cargo nextest run -p yulius-browser --all-features`: 154 passed.
- **Viewer (AC-4, AC-5)**: `npx e2e run tests/browser.e2e.ts`: 8 passed before the merge of dev
  at 320803966, including "a paste in the browser panel is sent whole and a lost one is said".
  After it 7 pass: "expired clipboard actions preserve focus and explain how to copy again" fails
  (`toBeFocused`), the integration failure Yulius reported on dev at 320803966 (task #619; task
  #616 owns its repair); this change does not touch copy offers.
- `scripts/dev quick` passed. The pre-existing opt-in `live_display::the_real_chrome_streams_takes_real_input_and_stays_out_of_containment`
  fails on this host at its first click ("timed out waiting for a trusted click"), a path this
  change does not touch: the click lands while Chrome is not yet the active window; the new test
  waits for the page to take a key first. Not run on dev here.
- **Review round 1** (Fable 5.2, request changes at `b349a02ed`), answered:
  - Blocker, clippy in two test lines: fixed in `dce9ec090` (CI run `90921d7a` passed).
  - Major, no test of the input lock for a paste: `viewer_clicks_and_keys_wait_for_the_hosts_browser_policy`
    and `an_enter_typed_before_the_policy_keeps_input_off_until_a_restart` now send a `paste`;
    with the `viewer_input_allowed()` check removed from the paste both failed.
  - Major, the quiet wait untested in CI: `a_newer_paste_waits_until_the_page_stopped_reading_the_one_before`
    (a Chrome-like client re-reading the targets 30 ms after the text; two pastes queued at
    once; a client that never stops asking). Without the wait it failed (`(Unread, Read)`), with
    the bound raised tenfold it failed ("held the paste 3.07 s"). It also showed the minor about
    a second viewer's paste: the wait now covers a paste whose text was not fetched yet.
  - Minor, the viewer's connection blocked on a paste: fixed as decided above;
    `a_paste_on_its_way_does_not_hold_up_the_viewers_connection` failed (timed out) with the
    outcome awaited in the loop.
  - Minor, typed fallback untested: `a_paste_without_the_clipboard_is_typed_as_key_presses`
    (an X thread without a paste owner types `aB\r\n` as keys, a held Control lifted and pressed
    again).
  - Minor, weak refusal assertions and a stale SelectionClear: the owner test asserts no text for
    a refused or let-go request, and a clear of an earlier ownership leaves the text (it failed
    before the owner checked the server's owner).
  - Minor, Tab and control characters: tested, counted, and named in the tool description.
  - Minor, the 64 KiB `browser_type` holding the lock: measured and kept (Decisions).
  - Minor, e2e for a closed socket and no tab: both toasts are asserted; the limit message names
    characters or KiB as the case is (nit).
  - Nits: `chars` counts only typed characters; stale "Ctrl+V" and "paste" comments fixed.
  - The ADR's "whether or not it is shown" for background tabs was not run against Chrome and is
    no longer claimed.
- After the session moved to another sandbox, three `yulius-browser::manager` tests
  (`one_browser_per_sandbox_with_isolated_profiles_and_full_shutdown`,
  `crash_is_detected_and_the_next_use_restarts`,
  `shutdown_escalates_to_sigkill_for_a_browser_ignoring_sigterm`; "the process group's
  descendants are killed") fail here on the unchanged merge base 320803966 too (16 of 19 pass
  there); they passed in this task earlier and in CI. The other 153 browser crate tests pass.
- Security scan: disabled for this project in Yulius (not run, not passed).

