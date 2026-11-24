---
yulius-task: ebfb64ea-0281-455e-856b-d4e0115e4672
status: in-progress
---

# SPEC-648: browser_fill_secret types real key presses so a secret reaches a terminal

## Goal

A secret, its user name or its one-time code typed with `browser_fill_secret` reaches pages that
read key events, such as an xterm.js terminal (a CLI sign-in prompt in the sandbox browser),
as `browser_type` does since SPEC-639. Every SPEC-450 and SPEC-483 guarantee holds as before.
Issue #1740.

## Context

- `browser_type` types real key presses since SPEC-639 (PR #1738, ADR-0471): `type_keys` sends a
  key down carrying the character's text and a key up per character through CDP
  `Input.dispatchKeyEvent`.
- `browser_fill_secret` (`tool_type_secret`, `tool_type_beside_secret` in
  `crates/yulius-browser/src/instance.rs`) still calls `type_lines(..., Typing::Inserted)`: one
  `Input.insertText` per line. xterm.js drops inserted text in some states (a key held, see Root
  cause) and key-only pages always do, so a secret typed into a terminal can be lost while the
  tool answers `filled`.
- ADR-0471 kept insertion for secrets only because the SPEC-450/SPEC-483 tests were built on it
  (its decision and consequences name the gap). It gives no safety reason against keys.
- The secret path (`type_text` with a rule), all under the input lock: the page is read as it is
  now and asked (SPEC-483 E3), the field is focused, where the focus is is read (E8), the frames
  and where they are going are asked (E9), the secret is recorded for screenshots (SPEC-450),
  the text is typed, then the document, the focus and the frames are read again (astray reports)
  and the hosts are exposed unless the focused password field holds the value. For a user name or
  code, other typed secrets are watched and checked for edits after each line, before its Enter
  (`check_edits`).

## Root cause

`type_text` types a secret with `Typing::Inserted`, which sends `Input.insertText`. That fires no
key event, only an `input` event of type `insertText`. xterm.js (the vendored 5.x) takes such an
event only while it has not seen a key go down without its key up (`_keyDownSeen`, and
`_keyPressHandled`, both cleared by the key up); otherwise the text stays in its hidden helper
textarea and never reaches the terminal. A key a person holds in the viewer (Ctrl of Ctrl+V, the
case SPEC-639 met) is such a state. Measured in Chrome 158 for this spec: with a viewer key held,
an inserted user name sat in `.xterm-helper-textarea` and the terminal's data stayed empty; with
nothing held, insertion reached the terminal. Terminals and pages that read only key events drop
inserted text in every state. Key presses arrive in all of these. The tests missed it because the
fake applies `Input.insertText` to any focused field, and no test types a secret into a page that
ignores insertion.

## Requirements

- FSK-001: `browser_fill_secret` types the value, the user name and the code as real key presses
  through `type_keys` (a key down with the character's text and a key up), line by line, Enter
  for each `\n`. No `Input.insertText` is sent for it.
- FSK-002: Everything before and after the keys stays as it is, under the input lock: the page
  rule asked of the page as read with the lock held, the field focused, the focus and frame
  checks, the leaving check, the record of the secret before the first key, the edit checks after
  each line and before its Enter for a user name or code, and the read after typing with its
  astray reports and exposure.
- FSK-003: A text holding a tab or another control character (other than `\n`, and a `\r` that
  ends a line) is refused before anything is typed or recorded, with `InvalidInput` and a message
  that holds no part of the text. A pressed Tab would move the focus in the middle of a secret,
  and the rest of it would go to the next field, which may show it; other control characters
  have no key and `type_keys` would leave them out, typing a different secret.
- FSK-004: A secret typed into a terminal (its focus is a text field, not a password field, and
  its echo is drawn on a canvas the page-text probe cannot read) counts the tab's host as exposed:
  screenshots of it are refused, as for any field that is not a password field. A user name or
  code typed there is not remembered (SPEC-483 E10), unchanged.
- FSK-005: A navigation of the page's own can land between two keys instead of between two
  lines. As before (SPEC-483 E3), the page it moved to is asked after the typing, the secret is
  recorded for it, and one it may not go to is reported as `SecretAstray`.

## Plan

- [x] Write the spec and record the key-typing decision
- [x] Show a secret never reaches a page that ignores inserted text
- [x] Type browser_fill_secret as real key presses, refusing tabs and control characters
- [x] Port the SPEC-450/SPEC-483 secret tests to key events
- [x] Prove a secret fills a real xterm.js terminal in Chrome
- [x] Keep the docs true and add the changes entry
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- AC-1 (FSK-001): on the fake, a page that ignores `Input.insertText` (a terminal) receives a
  secret, a user name and a code typed by `tool_type_secret` / `tool_type_beside_secret`; no
  `Input.insertText` is sent. Fails before the change.
- AC-2 (FSK-002, FSK-005): the SPEC-450 and SPEC-483 secret tests in
  `crates/yulius-browser/tests/instance.rs` and `crates/yulius/tests/browser.rs` (and
  `superagent_browser.rs`) pass with key typing: their "nothing typed" checks count key events,
  their mid-typing navigations land during a key event.
- AC-3 (FSK-003): a secret, user name or code with a tab or another control character is refused
  with nothing typed and nothing recorded; `\r\n` line ends are typed as before.
- AC-4 (FSK-004): after a secret is typed into a terminal-like text field, a screenshot of the tab
  is refused (fake), and in real Chrome (opt-in live test) the xterm.js terminal receives the
  user name and the secret while a viewer key is held (where insertion is dropped), and a
  following `browser_screenshot` is refused.

## Test plan

- `crates/yulius-browser/tests/instance.rs`: a new test with a fake page that drops
  `Input.insertText` (`FakeCdpServer::set_inserted_text_ignored`) and reads the field's value
  (`FakeCdpServer::field_value`); a refusal test for tabs and control characters; the existing
  secret tests switched to `Input.dispatchKeyEvent` counts.
- `crates/yulius/tests/browser.rs`, `superagent_browser.rs`: the MCP secret tests switched to key
  events (typed text read back from the key downs).
- `crates/yulius/tests/live_display.rs`: a new opt-in test that fills the xterm.js fixture with
  an agent secret bound to the fixture site and checks the terminal's data and the screenshot
  refusal (`YULIUS_LIVE_DISPLAY=1`, real Chrome, Xvfb).
- Each touched test binary in full; by-hand mutation: undo the switch to keys and the control
  character refusal, watch the new tests fail.

## Decisions

- Real key presses for every `browser_fill_secret` field, recorded in
  [ADR-0475](../../docs/decisions/0475-browser-fill-secret-types-real-key-presses.md), which
  supersedes the secret part of ADR-0471. Key events add no exposure: the page allowed to get
  the secret already reads the field, and the text crosses the same DevTools session either way.
- Tabs and control characters are refused, not inserted as one character: an inserted tab would
  still be lost in a terminal (the case this fixes), and mixing two ways of typing in one secret
  makes its arrival depend on the page. Secrets, user names and codes with a tab are not known in
  practice; the refusal says so instead of typing a different text.
- A secret typed into a terminal counts as exposed by the existing rule (only a focused password
  field holding it keeps it out of the pixels); no terminal-specific rule.
- Keys typed while nothing takes text (no field focused) can trigger the allowed page's own
  shortcuts. This is accepted: the page is one the secret may go to (or every frame it holds is),
  and the read after typing exposes its host unless a password field holds the secret. Refusing
  such typing would also refuse fields inside closed shadow roots, which the probe reports as
  `other`.

## Evidence

- Red first (commit `437746354`): `scripts/cargo nextest run -p yulius-browser --features fake
  --test instance -E 'test(a_secret_reaches_a_page_that_takes_only_keys) |
  test(a_secret_with_a_tab_or_control)'` failed both: the field stayed empty
  (`left: None, right: Some("pw-secret-4471")`), and `"zq\t7731xy"` was typed (`Ok(TypeResult {
  chars: 9 })`).
- After the fix: `yulius-browser --features fake` (all binaries) 160 passed;
  `yulius::main` files `browser::`, `superagent_browser::`, `secrets_scopes::`, `cockpit::`,
  `vidoc_key::` 148 passed; `yulius-app` mcp/browser/vault tests 118 passed; `cargo clippy -p
  yulius-browser --features fake --all-targets -D warnings` clean; `scripts/dev quick` passed.
- The ported "nothing typed" checks used to count `Input.insertText`, which the fix no longer
  sends, so they would have passed vacuously; they now count `Input.dispatchKeyEvent`, and the
  mid-typing navigations land during a key event.
- Live (opt-in, real Chrome 158 on Xvfb, `--no-sandbox` wrapper, the host's managed browser
  policy installed as `yulius host setup` does, `YULIUS_LIVE_DISPLAY_BASE=147`):
  `live_display::an_agent_secret_fills_an_xterm_terminal` passed (1.4 s), and
  `live_display::a_viewer_paste_reaches_a_paste_listener_and_an_xterm_terminal` passed beside it.
- Mutation by hand (the line `self.type_keys(session, line)` in `type_lines` put back to
  `Input.insertText`, then restored): 9 of 42 `instance` tests failed (both new ones, the six
  SPEC-483 secret tests and `click_and_type_tools_pass_selectors_as_data`). The live test first
  passed with insertion: xterm.js takes an inserted `input` event while no key is held. With a
  viewer key held, it failed with the mutation ("timed out waiting for the user name in the
  terminal", the text left in xterm's helper textarea) and passed with keys. The test now holds
  that key.

