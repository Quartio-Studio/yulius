---
yulius-task: 82e50053-d61d-4639-b3a6-6950286a81d3
status: in-progress
---

# SPEC-450: browser_screenshot never shows a secret typed with browser_fill_secret

Status: in-progress
Date: 2026-11-11
Owner: Yulius task #432
Scope authorization: task #432 (`/goal`, from task #430 working through open issues), "Solve
GitHub issue #1113": a secret typed into a plain-text field shows unredacted in
`browser_screenshot`. Extends SPEC-002 ADR-20 / AC-VLT-6 (`browser_fill_secret`: the agent never
receives the value) and [SPEC-437](SPEC-437-add-browser-use-for-superagents.md) (the Superagents
use the same tools).

## Goal

A screenshot an agent (a task's or a Superagent) takes never shows an agent secret value it typed
with `browser_fill_secret`. A password field, drawn as dots, keeps screenshots working as today.

## Context

- **Symptom (#1113).** `browser_fill_secret` types a secret into a field of a page on a bound host
  and the agent never receives the value; `browser_snapshot` text is redacted (`[secret:NAME]`) by
  the result redactor. `browser_screenshot` returns pixels, which the redactor cannot read: after a
  fill into `<input type="text">` (an API-key field), a screenshot shows the value.
- **Root cause.** `tool_browser_fill_secret` (`crates/yulius-app/src/vault/runtime.rs`) types
  through the plain `tool_type` and remembers nothing about the fill, and
  `tool_screenshot_within` (`crates/yulius-browser/src/instance.rs`) captures
  `Page.captureScreenshot` without looking at the page. Nothing connects the two, so no state
  could refuse or mask a later screenshot.
- **Why tests missed it.** The fill test
  (`crates/yulius/tests/browser.rs` `agents_type_secrets_only_into_pages_of_their_hosts`) checks
  the typed text, the result and the redacted snapshot, never a screenshot after the fill; the
  fake DevTools server has no model of a field's value, so it could not have shown one.
- **Where the value can be after a fill.** In the field typed into (a text field, a textarea, a
  password field), copied elsewhere on the page by its script (a "your key: …" echo), revealed
  later by the page ("show password" turns `type=password` into `type=text`), restored by history
  navigation, or out of the main document's reach (a cross-origin iframe, a closed shadow root).
  A fill-time record of "it was a password field" misses the reveal and the restore; only a look
  at the page at screenshot time sees them.

## Requirements

- **SSC-001 A shown filled value refuses the screenshot.** The browser remembers, per tab, each
  secret value `browser_fill_secret` typed into it (field `value`/`password`; a user name is not
  a secret and a TOTP code is short-lived, as the redactor treats them), until the tab closes.
  Before capturing a tab that has one, it reads the page's readable text (visible text, input and
  textarea values other than password and hidden inputs, open shadow roots and same-origin frames,
  recursively) and refuses the screenshot when any of them contains a remembered value. The
  refusal is a tool error pointing to `browser_snapshot`, and names no value.
- **SSC-002 A password field keeps screenshots.** A value found only in password fields
  (`input[type=password]`, drawn as dots) does not refuse; once the page shows it (the field
  became `type=text`), SSC-001 refuses.
- **SSC-003 Fail closed.** When the page cannot be read (the probe fails, throws, or its result is
  malformed or over its bound), the screenshot is refused. A fill whose value the page shows
  nowhere readable right after typing (typed into a cross-origin frame or a closed shadow root,
  reformatted or cut by the field) refuses screenshots of that tab while its main frame keeps the
  same document (`Page.getFrameTree` loader id); when the document cannot be told, it refuses.
- **SSC-004 Navigation frees the tab.** Once the page no longer shows the value (navigated to
  another document, the field cleared), screenshots work again; a value that comes back (history
  navigation restores the field) refuses them again.
- **SSC-005 The value never leaves for the page.** Comparison happens in Yulius: the page returns
  its text, the value is never sent into the page (which may have navigated to another origin).
- **SSC-006 Every agent.** The check is in the browser instance's screenshot, so a task's agent,
  the sandbox Superagent and the Global Superagent get it alike.

## Plan

- [ ] Regression test: a screenshot after a plain-text fill shows the secret
- [ ] The browser remembers filled values per tab and refuses a screenshot of a page that shows one
- [ ] An unreadable page or a fill seen nowhere refuses screenshots until the document changes
- [ ] ADR-0291, architecture, API and changes describe the guard
- [ ] Independent review and its answers
- [ ] Gates green with evidence

## Acceptance criteria

- After `browser_fill_secret` into a plain-text field, `browser_screenshot` of that tab is an
  error pointing to `browser_snapshot`; the result holds no value and no image. (#1113 AC 1)
- After a fill into a password field, `browser_screenshot` returns the image as today; after the
  field turns into a text field it is refused. (#1113 AC 2)
- After navigating the tab to another document, screenshots work again.
- A fill the page shows nowhere refuses screenshots until the document changes; a page the probe
  cannot read refuses them.
- Integration tests over the fake DevTools server cover these cases (#1113 AC 3).

## Test plan

- The fake DevTools server models fields: a field per selector (text by default, password when a
  test says so), focused by `browser_type`'s focus function, filled by `Input.insertText`, cleared
  by `Page.navigate` (which also gives a new loader id), and answers the secret probe and
  `Page.getFrameTree`.
- `crates/yulius/tests/browser.rs`: plain-text fill → screenshot refused (written first, fails on
  dev); password fill → screenshot works; field revealed → refused; navigate → works; probe fails
  → refused.
- `crates/yulius-browser/tests/instance.rs`: unseen fill → refused until navigation; a probe answer
  over its bound is refused; a value typed with plain `tool_type` guards nothing.

## Decisions

- **Refuse, not mask** ([ADR-0291](../../docs/decisions/0291-a-screenshot-of-a-page-showing-a-filled-secret-is-refused.md)).
  Masking (CSS `-webkit-text-security` on fields before the capture) changes the page the person
  watches, misses values the page echoes outside fields, cannot reach cross-origin frames, and
  fails open when the style does not apply. A refusal keeps every case closed, and
  `browser_snapshot` reads the page with the value redacted.
- **Look at the page at screenshot time, not only at fill time.** A fill-time "password field"
  flag misses a revealed password and a value restored by history navigation; reading the page
  sees both. The fill-time look only decides whether the value can be found at all (SSC-003).
- **Compare in Yulius.** Sending the value into the page to search it would hand it to whatever
  origin the tab shows by then.
- **Remember values until the tab closes**, held in `Zeroizing` strings: a value can come back
  (SSC-004), and re-reading the vault would miss a rotated secret still on the page.

## Out of scope

- Pixels a person sees (the viewer's live view): the person may see what they allowed.
- A value drawn in a canvas, or typed by the agent itself with `browser_type` (the agent then knows
  it).
- A browser restart: the remembered values go with the crashed instance; a new browser starts
  without the old pages.
