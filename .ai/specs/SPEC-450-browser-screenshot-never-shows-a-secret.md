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
- **Review round 1 (what the first fix left open).** The first version remembered values per
  tab and refused only an exact match of the value. An agent could insert a character into the
  middle of the filled value (`browser_click` into the field, `browser_type " "`) and get the
  rest in the pixels. It could also read the key echoed by a settings page in another tab, or a
  tab reopened after the first one closed. A second fill of the same value into a password field
  marked an unseen first fill as seen. And a click between the screenshot's look at the page and
  its capture could reveal a password. The requirements below close each of these.

## Requirements

- **SSC-001 The browser remembers what was filled.** `browser_fill_secret` of field
  `value`/`password` types through `tool_type_secret`. The browser instance remembers each value
  (distinct, at most 32) and each document it went into (the main frame's loader id from
  `Page.getFrameTree`, read before and after typing; at most 256), until the browser stops,
  whatever its tabs do. A user name is not a secret (it is in the description) and a TOTP code
  expires, so those are typed as `browser_type` types, unguarded, as the redactor treats them. A
  fill whose document cannot be told is refused before anything is typed.
- **SSC-002 A filled document needs the value whole and only in password fields.** Before
  capturing any tab, once a value was typed, a probe reads the page's text and fields
  (`shown`: body, open shadow roots and same-origin frames as `textContent`, the values of fields
  drawn as text; `masked`: password field values). In a document a value was typed into, the
  screenshot is refused unless that value is found whole in a password field and nowhere in
  `shown`. A value typed into a cross-origin frame, edited in place, cut or reformatted by the
  field, or revealed is therefore refused.
- **SSC-003 Any other page is refused while it shows a value.** In every tab, a page whose `shown`
  holds a remembered value is refused (a settings page echoing the stored key, a field history
  navigation restored).
- **SSC-004 Fail closed.** No screenshot when the document cannot be told, the probe fails or
  throws, or its answer is malformed or over 4 000 000 characters. The refusal is a tool error
  (`SecretOnPage`) pointing to `browser_snapshot` and names no value.
- **SSC-005 Nothing slips between look and capture.** A screenshot holds the instance's input lock
  from its look at the page to its last capture, and typing and clicks take the same lock, so no
  agent or viewer input lands in between. The probe runs in an isolated world
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
- [ ] Independent review and its answers
- [ ] Gates green with evidence

## Acceptance criteria

- After `browser_fill_secret` into a plain-text field, `browser_screenshot` of that tab is an
  error pointing to `browser_snapshot`; the result holds no value and no image (#1113 AC 1). The
  same holds after a character is typed into the middle of the value.
- After a fill into a password field, `browser_screenshot` returns the image as today; after the
  field turns into a text field it is refused (#1113 AC 2).
- After navigating the tab to another document, screenshots work again. A page in any tab that
  shows the value, also after the filled tab closed, is refused.
- A user-name fill does not refuse screenshots.
- A fill the page shows nowhere, a page the probe cannot read and a document that cannot be told
  refuse screenshots; input waits for a screenshot's capture.
- Integration tests over the fake DevTools server cover these cases (#1113 AC 3).

## Test plan

- The fake DevTools server models fields: one per selector (text by default, password when a
  test says so), focused by `browser_type`'s focus function, filled by `Input.insertText`, and
  cleared by `Page.navigate`, which also gives a new loader id that is unique in the browser. It
  answers `Page.getFrameTree`, `Page.createIsolatedWorld` and the probe, which a test can
  override or delay (the delayed reply runs while other requests go on).
- `crates/yulius/tests/browser.rs` `a_screenshot_never_shows_a_secret_typed_into_a_page`, written
  first and failing on dev, covers the plain-text fill, the edited value, `browser_type`
  guarding nothing, navigation, the unguarded user name, another tab echoing the key, the password
  field and its reveal, and an unreadable page. `superagent_browser.rs` covers the Global
  Superagent's refusal.
- `crates/yulius-browser/tests/instance.rs`:
  - `typed_secrets_refuse_screenshots_while_a_page_may_show_them`: an unseen fill until
    navigation, a restored or echoed value, probe answers over the bound or malformed, an edited
    password, another tab after the filled one closed, the 32-value cap, a failing probe.
  - `a_secret_needs_a_known_document`.
  - `input_waits_for_a_screenshot_to_capture`.

## Decisions

- **Refuse, not mask** ([ADR-0291](../../docs/decisions/0291-a-screenshot-of-a-page-showing-a-filled-secret-is-refused.md)).
  Masking (CSS `-webkit-text-security` on fields before the capture) changes the page the person
  watches, misses values the page echoes outside fields, cannot reach cross-origin frames, and
  fails open when the style does not apply. A refusal keeps every case closed, and
  `browser_snapshot` reads the page with the value redacted.
- **The filled document is judged by where the value is, not by where it is not.** An exact
  substring search is defeated by editing the value; requiring it whole in a password field is
  not, and costs only screenshots of a document that should not be photographed anyway.
- **Per browser, not per tab.** Tabs are the agent's to open and close; values and documents stay
  until the browser stops (bounded, a fill beyond is refused).
- **Compare in Yulius, read in an isolated world.** Sending the value into the page would hand it
  to whatever origin the tab shows by then; the isolated world keeps page scripts from spoofing
  the probe.

## Out of scope

- Pixels a person sees (the viewer's live view): the person may see what they allowed.
- A value drawn in a canvas or an image, or echoed edited (spaced out) on a page other than the
  one it was typed into, for example after a history navigation restored an edited field into a
  new document. Hidden text styled by CSS (`text-transform`) is still read as `textContent`, so it
  is found.
- A browser restart: the remembered values go with the stopped instance; a new browser starts
  without the old pages.
