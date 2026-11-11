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
  password field). Copied elsewhere on the page by its script (a "your key: …" echo). Revealed
  later by the page ("show password" turns `type=password` into `type=text`). Restored by history
  navigation, or re-rendered by the server into the next page (a form shown again after a
  validation error). Stored and shown back on another page (a settings page), also after the
  browser restarted. Or out of the main document's reach (a cross-origin iframe, a closed shadow
  root). A fill-time record of "it was a password field" misses the reveal and the restore; only a
  look at the page at screenshot time sees them.
- **Review round 1 (what the first fix left open).** The first version remembered values per
  tab and refused only an exact match of the value. An agent could insert a character into the
  middle of the filled value (`browser_click` into the field, `browser_type " "`) and get the
  rest in the pixels. It could also read the key echoed by a settings page in another tab, or a
  tab reopened after the first one closed. A second fill of the same value into a password field
  marked an unseen first fill as seen. And a click between the screenshot's look at the page and
  its capture could reveal a password.
- **Review round 2.** The per-document rule held within the filled document only: an edited copy
  in the next document of the same site (a form re-rendered after a validation error) was matched
  by exact text only. A page changing by itself between the look and the capture (a navigation
  committing, a script revealing a key) was not seen. And the remembered values went with a
  browser that stopped (idle for 30 minutes, a Yulius restart), while the signed-in profile and
  the pages that show a stored key stayed.

## Requirements

- **SSC-001 The browser remembers what was filled.** `browser_fill_secret` of field
  `value`/`password` types through `tool_type_secret`. The browser instance remembers each value,
  and each line of at least 8 characters of a multi-line one (at most 32 texts), and each document
  it went into (the main frame's loader id from `Page.getFrameTree`, read before and after typing;
  at most 256), until the browser stops, whatever its tabs do. A user name is not a secret (it is
  in the description) and a TOTP code expires, so those are typed as `browser_type` types,
  unguarded, as the redactor treats them. A fill whose document cannot be told, or beyond the
  bounds, is refused before anything is typed.
- **SSC-002 Only a password field keeps a value out of the pixels.** Right after typing, the page
  is read. Unless the value is whole in a password field and nowhere in `shown`, its origin
  (`location.origin`) is exposed: every page of it is refused until the browser stops. In a
  document a value was typed into, a screenshot also needs that value whole in a password field
  and nowhere in `shown`. A value edited in place, cut or reformatted, typed into a cross-origin
  frame or revealed is therefore refused, and a page failing this exposes its origin. When the
  page after typing cannot be read, or its document is lost, every screenshot is refused.
- **SSC-003 Any page showing a secret is refused.** Before capturing any tab, once a value was
  typed or the sandbox has agent secrets, a probe reads the page: `shown` is the `textContent` of
  its body (or root element), open shadow roots and same-origin frames, plus the values and
  placeholders of fields drawn as text; `masked` is the password field values. A page whose
  `shown` holds a remembered value is refused, and its origin exposed. A page whose `shown` holds
  any agent secret the redactor hides for the sandbox (its matcher, which covers values, their
  encodings and lines) is refused too: that covers a stored key after a browser restart.
- **SSC-004 Fail closed.** No screenshot when the document cannot be told, when the probe fails,
  throws, answers malformed or reads over 4 000 000 characters, or when the agent secrets cannot be
  loaded. The refusal is a tool error (`SecretOnPage`) pointing to `browser_snapshot` and names no
  value.
- **SSC-005 Nothing slips between look and capture.** A screenshot holds the instance's input lock
  from its look at the page to its last capture, and the agent's typing and clicks and the
  viewer's CDP input take the same lock. After the capture the page is read again: unless the same
  document passes again, the image is dropped. The probe runs in an isolated world
  (`Page.createIsolatedWorld`), where page scripts cannot change what it reads.
- **SSC-006 The value never leaves for the page; every agent.** The probe takes no argument:
  Yulius compares. The check is in the browser instance, so a task's agent and both Superagents
  get it.

## Plan

- [x] Regression test: a screenshot after a plain-text fill shows the secret
- [x] The browser remembers filled values per tab and refuses a screenshot of a page that shows one
- [x] An unreadable page or a fill seen nowhere refuses screenshots until the document changes
- [x] ADR-0291, architecture, API and changes describe the guard
- [x] Review round 1: filled values guarded per browser and per document, probe and capture under one lock
- [x] Review round 2: exposed origins, the redactor's check and a second look after the capture
- [x] Independent review rounds 1 and 2 answered
- [ ] Independent review round 3 approves the head
- [ ] Gates green with evidence

## Acceptance criteria

- After `browser_fill_secret` into a plain-text field, `browser_screenshot` of that tab is an
  error pointing to `browser_snapshot`; the result holds no value and no image (#1113 AC 1). The
  same holds after a character is typed into the middle of the value, and for every page of that
  origin afterwards.
- After a fill into a password field, `browser_screenshot` returns the image as today; after the
  field turns into a text field it is refused (#1113 AC 2).
- Another origin is captured, unless a page shows a filled value or any agent secret, in any tab,
  also after the filled tab closed.
- A user-name fill does not refuse screenshots.
- A fill the page shows nowhere, a page the probe cannot read, a document that cannot be told and
  a page that changed during the capture refuse screenshots; input waits for a screenshot's
  capture.
- Integration tests over the fake DevTools server cover these cases (#1113 AC 3).

## Test plan

- The fake DevTools server models fields: one per selector (text by default, password when a
  test says so), focused by `browser_type`'s focus function, filled by `Input.insertText`, and
  cleared by `Page.navigate`, which also gives a new loader id that is unique in the browser. It
  answers `Page.getFrameTree`, `Page.createIsolatedWorld` and the probe with the page's origin. A
  test can set, queue or delay the probe's answers (a delayed reply runs while other requests go
  on) and fail one numbered call.
- `crates/yulius/tests/browser.rs` `a_screenshot_never_shows_a_secret_typed_into_a_page`, written
  first and failing on dev, covers:
  - the plain-text fill, the edited value, `browser_type` guarding nothing and the exposed origin;
  - another origin and the unguarded user name;
  - another tab showing the key or a never-filled agent secret;
  - the password field and its reveal;
  - an unreadable page.
  `superagent_browser.rs` covers the Global Superagent's refusal.
- `crates/yulius-browser/tests/real_chromium.rs` (opt-in, `YULIUS_TEST_CHROMIUM`): on a real page
  a password fill keeps screenshots, its reveal and a text-field fill on another origin refuse
  them, so the probe expression, the isolated world and the frame tree work in Chromium.
- `crates/yulius-browser/tests/instance.rs`:
  - `typed_secrets_refuse_screenshots_while_a_page_may_show_them`: an unseen fill and its origin,
    the isolated world's frame and context, another origin echoing the value, probe answers over
    the bound or malformed, an edited password, another tab after the filled one closed, the
    32-value cap, a failing probe.
  - `a_fill_outside_a_password_field_exposes_its_site`: the next page of the site, with no
    screenshot in between.
  - `a_multi_line_secret_is_matched_line_by_line`.
  - `a_secret_needs_a_known_document`: the 256-document cap, a document lost after typing, an
    unknown document.
  - `nothing_slips_between_the_look_and_the_capture`: input waits, a page changed during the
    capture drops the image.
  - `a_page_showing_any_hidden_secret_is_not_captured`.

## Decisions

- **Refuse, not mask** ([ADR-0291](../../docs/decisions/0291-a-screenshot-of-a-page-showing-a-filled-secret-is-refused.md)).
  Masking (CSS `-webkit-text-security` on fields before the capture) changes the page the person
  watches, misses values the page echoes outside fields, cannot reach cross-origin frames, and
  fails open when the style does not apply. A refusal keeps every case closed, and
  `browser_snapshot` reads the page with the value redacted.
- **Judge a filled place by where the value is, not by where it is not.** An exact substring
  search is defeated by editing the value; requiring it whole in a password field is not. A value
  that went anywhere else exposes its whole origin, since the site may show an edited copy on any
  of its pages. That costs screenshots of that site until the browser stops (30 minutes idle), not
  the sign-in with a password.
- **Per browser, not per tab, plus the redactor's matcher.** Tabs are the agent's to open and
  close; values and documents stay until the browser stops (bounded, a fill beyond is refused).
  After that, the matcher the MCP results are redacted with refuses a page showing any agent
  secret of the sandbox, so a screenshot hides what a snapshot hides.
- **Compare in Yulius, read in an isolated world, look again after the capture.** Sending the
  value into the page would hand it to whatever origin the tab shows by then; the isolated world
  keeps page scripts from spoofing the probe; the second look catches a page that changed by
  itself while the image was taken.

## Out of scope

- Pixels a person sees (the viewer's live view), and input a person gives on the virtual display
  (XTest mouse and keys, which do not pass through the instance's lock): the person may see what
  they allowed.
- A value drawn in a canvas or an image, or shown through CSS (`content: attr(…)`); an edited copy
  on a page of another origin than the one it was typed into.
- A page changing and changing back between the two looks of one screenshot.
