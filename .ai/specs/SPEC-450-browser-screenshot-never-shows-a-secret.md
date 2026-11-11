---
yulius-task: 82e50053-d61d-4639-b3a6-6950286a81d3
status: in-progress
---

# SPEC-450: browser_screenshot never shows a secret typed with browser_fill_secret

Status: done
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
- **Review round 3.** A value in a password field could still be edited in place (`browser_click`
  into it, `browser_type`), saved and shown edited on the next page, with no screenshot in between
  to expose the site. The memory of a stopped browser (stopped by the agent's own idleness) went
  with it, while edited copies stayed on the site. And an agent's own page could frame the bound
  site: in Chromium an out-of-process frame is out of the probe's reach, listed by
  `Page.getFrameTree` without its address or not at all, and only its own `iframe` target
  (`Target.getTargets`, `parentFrameId`) says where it is (seen on Google Chrome).
- **Review round 4.** No leak path was left, but the rule that a filled document needs the value
  whole in a password field refused a single-page app's dashboard after a sign-in: the field goes
  away while the document stays (`pushState`), and the site was then exposed until Yulius
  restarted. An edit and its Enter in one `browser_type` were checked only after the Enter had
  submitted the form, and a frame inside another out-of-process frame names a parent no target
  lists.
- **Review round 5.** Any password field holding the value vouched for a fill: a second fill into
  a cross-origin frame's text box, or into a field that cuts it, went unexposed while the first
  still held it. A password revealed before an edit, or a page unreadable before it, let the edit
  go unwatched.
- **What a snapshot already returns (scope).** The result redactor matches a secret's exact value
  and its encodings. A value edited in place (`browser_click` into the field, `browser_type " "`)
  is no longer that value, so `browser_snapshot` returns the edited copy as text: on `dev` before
  this change, with no screenshot involved (reported on #1113). A screenshot guard cannot close
  that path. This change makes a screenshot show nothing a snapshot of the same page would redact,
  and adds the edit checks of SSC-002 as defense in depth, not as a guarantee against every
  edit.
- **The e2e job on this branch (met on the way).** Yulius CI's `e2e` job failed at 52fb2584 on
  tests this change does not touch: the host Yulius moved to on 2026-11-11 is faster and showed
  races that fail every pull request.
  - An app defect. Opening a task's page, `handleRoute` (`app.js`) tells the tool panel that the
    sandbox is arriving (`panels.arrive`), reads the tasks, and only then makes the sandbox the
    panel's context (`setContext`). A tool opened in between opened for no sandbox: the Shell said
    "Open a sandbox first." and never took the keyboard (SPEC-170 TKS-002), and the Superagent was
    the Global one (SPEC-428 SAT-005). A person on a slow connection meets the same. Fixed here
    (SSC-007); `dev` meanwhile got waits in the tests that met it (#1150).
  - Races in tests only: production protection read the stored switch while it was being sent;
    the places editor test typed before README.md's text was in Monaco's model (#1087; `dev`
    has since fixed that in the Editor itself, #1159, and dropped the wait); the
    left-composer onboarding test (#581) set its hold on recovery reads while an earlier read was
    still on its way, so two or three waited there instead of one; the 320 px navigation drawer
    test (SPEC-122) measured its rows while the drawer still slid in from the left, when a part
    of a row that is not drawn (it measures as zeros) lies right of every row.

## Requirements

- **SSC-001 The sandbox's browser remembers what was filled.** `browser_fill_secret` of field
  `value`/`password` types through `tool_type_secret`. A user name is not a secret (it is in the
  description) and a TOTP code expires, so those are typed as `browser_type` types, unguarded, as
  the redactor treats them. The browser remembers each value (at most 32) and each line of at
  least 8 characters of a multi-line one, PEM armour lines aside (at most 1024). It also remembers
  each document a value went into (the main frame's loader id from `Page.getFrameTree`, read
  before and after typing) and each host (at most 256 each). The `BrowserManager` keeps this per
  sandbox, so a restarted browser (idle, crash) starts with it; only documents, which belong to
  the old process, are forgotten (a value lost after typing, below, keeps refusing). A fill whose
  document cannot be told, or beyond 32 values, is refused before anything is typed.
- **SSC-002 Only a password field keeps a value out of the pixels.** Right after typing, the page
  is read, with where the typing went: the focused element, through open shadow roots and
  same-origin frames. Unless that element is a password field holding the value whole and the
  value is nowhere in `shown`, its host (before and after a navigation meanwhile) is exposed:
  every page of it is refused from then on. A value typed into a cross-origin frame or a closed
  shadow root, or cut or reformatted by the field, is therefore refused, whatever another
  password field holds; typed into a frame the probe cannot read, the hosts of the page's frames
  are exposed too, so another site framing them is refused. Any other typing into a document a value went into (`browser_type`, a
  user name) may edit it (a character into its middle):
  - before the typing, a page that shows such a value (a password revealed) or cannot be read
    exposes its host, as an edit would go unseen; a read that failed because a navigation
    committed meanwhile exposes nothing, unless the page now shown is itself one a value went
    into (restored from the back/forward cache with its loader id);
  - a value whole in a password field before the typing and not after a line of it, checked
    before that line's Enter may submit the form, exposes the host.
  A value gone from the page between typings is no edit: a single-page app's sign-in removes the
  field and keeps the document, and keeps screenshots. When the page after a fill cannot be read,
  or its document is lost, or the fill names no host to expose (an opaque page), or it went into
  a frame whose targets cannot be listed, or a bound is passed, every screenshot is refused until
  Yulius restarts.
- **SSC-003 Any page that may show a secret is refused.** Before capturing any tab, once a value
  was typed or the sandbox has agent secrets, a probe reads the page. `shown` is the `textContent`
  of its documents (all but `head`), open shadow roots, same-origin frames, objects and embeds,
  plus the values and placeholders of fields drawn as text; `masked` is the password field values.
  The screenshot is refused when:
  - the page's host is exposed;
  - it frames a host a value was typed into, or an exposed one, in a frame the probe cannot read:
    - a frame of another origin in the frame tree, or one the tree calls opaque (`null`), also
      when the page's own origin is opaque;
    - an `iframe` target (a frame in a process of its own, as Chrome runs a sandboxed frame)
      whose parent frame is the tab's, is not said, or is no target listed, also on the page's
      own host;
    - a frame at an address of the page's own origin that the probe did not read. The frame tree
      says the origin of a frame's address whether or not it is sandboxed, so the probe says how
      many such frame documents it read (`frames`; none when it does not say), and fewer than
      the tree lists counts as a frame of the page's own host out of reach: a sandboxed frame
      kept in the page's process, or any frame of a page sandboxed by its
      `Content-Security-Policy`;
  - its `shown` holds a remembered value or line, which also exposes its host;
  - its `shown` holds any agent secret the redactor hides for the sandbox (its matcher: values,
    their encodings and lines). That also covers a stored key nobody filled.
- **SSC-004 Fail closed.** No screenshot when the document or the frame targets cannot be read,
  when the probe fails, throws, answers malformed or reads over 4 000 000 characters, or when the
  agent secrets cannot be loaded. The refusal is a tool error (`SecretOnPage`) pointing to
  `browser_snapshot` and names no value.
- **SSC-005 Nothing slips between look and capture.** A screenshot holds the instance's input lock
  from its look at the page to its last capture, and the agent's typing and clicks and the
  viewer's CDP input take the same lock. After the capture the page is read again: unless the same
  document passes again, the image is dropped. The probe runs in an isolated world
  (`Page.createIsolatedWorld`), where page scripts cannot change what it reads.
- **SSC-006 The value never leaves for the page; every agent.** The probe takes no argument:
  Yulius compares. The check is in the browser instance, so a task's agent and both Superagents
  get it.
- **SSC-007 A tool chosen while its sandbox's page loads opens in that sandbox** (the web app's
  tool panel; found by the e2e job). While a sandbox's page loads and no sandbox is the panel's
  context yet, a tool the person opens (rail, link) is remembered and shown pressed, and opens in
  that sandbox, the one whose page started loading last, once it is the context, taking the
  keyboard as a tool opened then does; the tool chosen last wins. Choosing it again or hiding the
  tools cancels it; another sandbox's page starting to load ends the wait at once, and a page
  that ends outside a sandbox drops it (what it was opened with, a browser tab or a file, belongs
  to the sandbox it was chosen for); another page of the same sandbox keeps it. The rail's tools
  are enabled while a sandbox arrives. Unchanged: a tool opened while another sandbox is still
  the context (A shown, B loading) opens in A.

## Plan

- [x] Regression test: a screenshot after a plain-text fill shows the secret
- [x] The browser remembers filled values per tab and refuses a screenshot of a page that shows one
- [x] An unreadable page or a fill seen nowhere refuses screenshots until the document changes
- [x] ADR-0291, architecture, API and changes describe the guard
- [x] Review round 1: filled values guarded per browser and per document, probe and capture under one lock
- [x] Review round 2: exposed origins, the redactor's check and a second look after the capture
- [x] Independent review rounds 1 and 2 answered
- [x] Review round 3: edits in filled documents, memory across browser restarts, framed sites
- [x] Review round 4: a single-page sign-in keeps screenshots, edits checked as typed
- [x] Review round 5: a fill judged by its field, unseen edits expose, the snapshot bar
- [x] Independent review round 6 approves the head
- [x] Review round 6 minors: fills into unreadable frames, frames by origin, a navigation before typing
- [x] Independent review of the round 6 fixes approves the head
- [x] Gates green with evidence
- [x] Final review: untested fail-closed branches, sandboxed frames, restored pages
- [x] A tool chosen while its sandbox's page loads opens in that sandbox
- [x] The e2e job's other failures: production protection and the #1087 editor test
- [ ] Independent review of the final head

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
  `superagent_browser.rs` covers the Global Superagent's refusal, and
  `a_screenshot_is_refused_when_the_secrets_cannot_be_checked` the agent secrets failing to load.
- `crates/yulius-browser/tests/real_chromium.rs` (opt-in, `YULIUS_TEST_CHROMIUM`): on a real page
  a password fill keeps screenshots, its reveal and a text-field fill on another origin refuse
  them, and a third site framing the filled one is refused, so the probe expression, the isolated
  world, the frame tree and the `iframe` targets work in Chromium. A page of a site signed in to
  that frames its own origin keeps its screenshots; with that frame sandboxed, or the page
  itself (`Content-Security-Policy: sandbox`), it is refused. The page's named elements shadow
  `document.documentElement` and `document.querySelectorAll`, which the probe reads through the
  prototypes.
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
  - `typing_into_a_filled_password_field_can_expose_its_site`: an edit, and an edit with its
    Enter in one call.
  - `a_page_framing_a_filled_site_is_not_captured`: frame tree and `iframe` targets, a parent
    nothing lists, another tab's.
  - `a_long_key_fits_and_its_armour_is_not_a_secret`.
  - `a_single_page_sign_in_keeps_screenshots`.
  - `a_fill_is_judged_by_the_field_it_went_into`: a second fill into a cross-origin frame or a
    cutting field.
  - `typing_where_an_edit_cannot_be_seen_exposes_the_site`: a revealed password, an unreadable
    page.
  - `a_fill_into_an_unreadable_frame_exposes_the_framed_site`: its frames' hosts, frames by origin.
  - `a_navigation_during_the_look_before_typing_exposes_nothing`.
  - `a_restored_filled_page_during_the_look_exposes_its_site`.
  - `fills_that_cannot_be_placed_refuse_every_screenshot`: no host, frame targets unknown.
  - `a_frame_of_the_pages_own_site_is_readable_only_when_the_probe_read_it`: read (captured),
    one of two read, a probe that does not say, sandboxed in the page's process, a sandboxed
    page, a frame target on the page's own host; a site nothing was typed into keeps its
    screenshots. The fake answers as Chrome 157 does (the origin of the frame's address).
  - `a_frame_the_tree_calls_opaque_is_not_readable`: a frame tree that says `null`, also for the
    page itself, whatever the probe says it read.
  - `a_page_showing_any_hidden_secret_is_not_captured`.
- `crates/yulius-browser/tests/manager.rs`: `typed_secrets_outlive_the_browser`,
  `a_lost_secret_refuses_screenshots_after_a_restart`.
- SSC-007, in the mock with the task page's read of the tasks held
  (`hold=/api/sandboxes/sb_studio/tasks`):
  - `e2e/tests/terminals.e2e.ts`: "the Shell chosen while its sandbox’s page loads opens there
    with the keyboard", "a tool chosen twice while its sandbox’s page loads does not open", "a
    tool chosen while its sandbox’s page loads does not open once the tools are hidden", "the tool
    chosen last on the way into a sandbox opens there" (from Settings, outside a sandbox), "a tool
    chosen for a sandbox left while its page loads does not open in another", "a tool chosen
    while its sandbox’s page loads opens beside the page shown in the end" (another task of the
    same sandbox), "a tool waits for the sandbox whose page started loading last" (studio, then
    product, then studio again, each held).
  - `e2e/tests/agent-panel.e2e.ts`: "the Superagent chosen while its sandbox’s page loads is that
    sandbox’s own", "the Superagent chosen for a sandbox is dropped when its page is left for
    Settings".
- `e2e/tests/onboarding.e2e.ts`, "a composer left before its reads answer adds no second toast to
  the start": holds the recovery reads once those asked earlier have answered (the mock counts
  the answers, `recoveryAnswered`).
- `e2e/tests/navigation.e2e.ts`, "the navigation drawer rows fit a 320 px phone with task and
  pull request numbers": measures the parts of a row that are drawn.

## Decisions

- **Refuse, not mask** ([ADR-0291](../../docs/decisions/0291-a-screenshot-of-a-page-showing-a-filled-secret-is-refused.md)).
  Masking (CSS `-webkit-text-security` on fields before the capture) changes the page the person
  watches, misses values the page echoes outside fields, cannot reach cross-origin frames, and
  fails open when the style does not apply. A refusal keeps every case closed, and
  `browser_snapshot` reads the page with the value redacted.
- **Hide in pixels what a snapshot hides in text.** The redactor matches exact values; an edit in
  place already defeats it in `browser_snapshot` (pre-existing, reported on #1113). The screenshot
  guard is held to the same bar, and its edit checks are defense in depth.
- **Judge a fill and each edit by where the value is, not by where it is not.** An exact
  substring search is defeated by editing the value; requiring it whole in a password field right
  after the fill, and after each line typed into its document, is not. A value that went anywhere
  else exposes its host, since the site may show an edited copy on any of its pages. That costs
  screenshots of that host until Yulius restarts, not the sign-in with a password. At screenshot
  time a filled document is not required to still hold the value: a single-page app's sign-in
  removes the field, which is no edit, and edits are only typed.
- **Per sandbox, not per tab or per browser, plus the redactor's matcher.** Tabs are the agent's
  to open and close, and a browser stops when the agent leaves it idle; values and exposed hosts
  stay with the sandbox until Yulius restarts (bounded). The matcher the MCP results are redacted
  with refuses a page showing any agent secret of the sandbox, so a screenshot hides what a
  snapshot hides, also after a Yulius restart.
- **A framed site is judged by its host.** The frame cannot be read, so a page framing a host a
  secret was typed into is refused whatever the frame shows; a frame of another site (an ad, a
  captcha) is not.
- **A frame of the page's own site is readable only when the probe read it.** Chrome's frame
  tree says the origin of a frame's address whether or not the frame is sandboxed, also for a
  page sandboxed by its `Content-Security-Policy`, and Chrome runs a sandboxed frame in a
  process of its own: a target on the page's own host (seen on Chrome 157). The tree's origins
  only prove that a frame is of another site. So the probe says how many frames of the page's
  origin it read, and a frame target counts whatever its host. Rejected: trusting the tree's
  origin (a sandboxed frame of the site signed in to was captured on Chrome); counting every
  frame element the probe could not read (any ad or captcha frame would cost such a site its
  screenshots).
- **Compare in Yulius, read in an isolated world, look again after the capture.** Sending the
  value into the page would hand it to whatever origin the tab shows by then; the isolated world
  keeps page scripts from spoofing the probe; the second look catches a page that changed by
  itself while the image was taken.
- **SSC-007: remember the choice.** Rejected: keeping the rail disabled until the sandbox is the
  context (the person's click is lost, and on a slow connection the tools look dead); making the
  sandbox the context before its tasks are read (the panel's context carries the page's task, not
  known yet, and the tools would open for a task the page does not show); waits in the tests only
  (as `dev` has since #1150: the tests pass, the person still gets a Shell without a sandbox).
  Remembering costs one variable in `panels.js` and changes nothing once a sandbox is the context.

## Out of scope

- Pixels a person sees (the viewer's live view), and input a person gives on the virtual display
  (XTest mouse and keys, which do not pass through the instance's lock): the person may see what
  they allowed.
- A value drawn in a canvas or an image, or shown through CSS (`content: attr(…)`), or in a
  closed shadow root.
- Every edit of a filled value that defeats exact matching: `browser_snapshot` returns such a copy
  as text already (pre-existing). Edits typed into a password field holding it, and typing where
  an edit cannot be seen, are caught; others are not (a value already moved by the page into a
  field the probe cannot read before it is edited, for example).
- An edited copy:
  - on another host than the one it was typed into (exposure is by host, not by registrable
    domain);
  - kept by the site past a Yulius restart (the memory is in the server process);
  - seen from another sandbox's browser signed in to the same account;
  - made by the page or the site itself after a fill that stayed whole in a password field (a
    script reformatting it, a server truncating it).
  Exact copies are still refused by the redactor's matcher, outside frames the probe cannot
  read.
- A never-filled agent secret inside a frame of another site.
- A page changing and changing back between the two looks of one screenshot.
- SSC-007: a tool opened while another sandbox is still the context (A shown, B's page loading)
  opens in A and stays there when B arrives, as before.

## Evidence

- **Regression first.** `ea114884` added `browser::a_screenshot_never_shows_a_secret_typed_into_a_page`
  and the fake's field model; on `dev` (911e3127) it failed: `browser_screenshot` after a fill into
  a text field returned the PNG (`"isError":false`, `"mimeType":"image/png"`). Each review round's
  tests were committed before its fix and failed on the head before it (`f94c4421` on 17b646fc,
  `75b584dd` and `e8fa42d2` on f7b9fb93 and 64832478).
- **Mutation checks** (each rule removed by hand, the named tests then fail, restored): the
  shown-value rule, origin exposure at fill time, the frame rule, `iframe` targets, a parent no
  target lists, the edit check while typing, the second look after the capture, the input lock,
  the redactor's check (instance and app), lines of multi-line values, PEM armour, the 32-value
  cap, the memory kept across browser restarts and its "every screenshot" flag, and an
  unloadable matcher becoming an empty one.
- **Real Chromium** (`YULIUS_TEST_CHROMIUM` set to Google Chrome on this host, run as root through
  a local `--no-sandbox` wrapper that is not committed):
  `cargo nextest run -p yulius-browser --features fake -E 'test(drives_a_real_chromium)'` passed
  on 3d3a06cf. A password fill keeps screenshots, its reveal and a text-field fill on another origin
  refuse them, and a third site framing the filled one is refused. Seen on that Chrome:
  `Page.getFrameTree` lists an out-of-process frame without its address
  (`"securityOrigin":"://","url":""`) or not at all, while `Target.getTargets` lists it as an
  `iframe` target with its URL and `parentFrameId`. The page's elements named `documentElement` and
  `querySelectorAll` did not affect the probe in its isolated world even without the prototype
  getters, which stay as defense. Asked again for the final review (Chrome 157.0.8103.50, raw
  CDP): `Page.getFrameTree` gives `securityOrigin` as the origin of the frame's address for a
  sandboxed frame and for a page sandboxed by `Content-Security-Policy: sandbox` (whose
  `self.origin` is `null`), never `null`; a sandboxed frame is an `iframe` target at its address
  with the page as parent, and with `--disable-features=IsolateSandboxedIframes` a child in the
  tree; `contentDocument` is `null` for each. The test, with a page framing its own origin
  (captured), a sandboxed frame of it and a sandboxed page (refused), failed at the sandboxed
  frame on `1c26954f` and passes with `ed174310`, with both ways of running sandboxed frames.
- **Gate.** `scripts/dev ci` at 3d3a06cf: 3302 tests passed (16 skipped), 152 script tests OK; it
  failed only `cargo deny --locked check` on RUSTSEC-2026-0486 (rustix 1.1.5, new, failing every
  pull request), fixed by `ada375dc` (`cargo update -p rustix@1.1.5`, lockfile only; `cargo deny
  --locked check`: advisories ok, bans ok, licenses ok, sources ok). `scripts/dev ci` at ada375dc:
  exit 0: fmt, clippy, the macOS build, 3302 tests passed (16 skipped), 152 script tests OK,
  `cargo deny` ok, docs built.
- **The e2e job** (`cd e2e && npx e2e run`, 11 workers here). Four SSC-007 tests were committed
  first (`03b5bf75`) and failed on that head: the Shell showed "Open a sandbox first." without the
  keyboard, the Superagent was the Global one; they pass with `6ef301ee`, which also brought the
  "tools hidden" test. Removing the "chosen again" and the "tools hidden" guards by hand fails
  their tests. The onboarding race was
  reproduced by letting every recovery read arrive at the same moment in the mock (failed 3 of 3,
  "expected 3 to be 1") and passes under the same probe with `fa895b4e`. The whole suite at
  `fa895b4e`, with `dev` (e2dd441d) merged: exit 0, 907 passed, 2 skipped (skipped on `dev` too).
  Before the onboarding fix it ran three times: 907 passed twice, and once two failed, the
  onboarding test and "the navigation drawer rows fit a 320 px phone with task and pull request
  numbers". That one failed once in seven whole runs and never in 17 runs alone (every row
  measured as spilling right after the drawer opened). Its cause (see Context) showed when the
  rows were measured from the click on, frame by frame: 12 of 12 rows "spill" until the drawer
  is half in, none when only the drawn parts are measured; `0772b389` measures those, and the
  test then passed 10 times alone and with its file.
- **The final head.** `scripts/dev ci` at 65ca08d7: exit 0: fmt, clippy, the macOS build, 3305
  tests passed (16 skipped), 152 script tests OK, `cargo deny` ok, docs built. The review of
  65ca08d7 requested changes: a waiting tool opened in whichever sandbox became the context (a
  Shell or a Superagent session in the sandbox the person went to instead); the un-press of a
  tool dropped outside a sandbox had no test; a sandboxed frame inside a page whose own origin is
  opaque counted as readable; two wordings here. The tests came first (`0e9eb172`: the Shell
  opened in product, the frame's page was captured; the Settings test fails with the un-press
  removed), the fixes in `928fdce4`. After them: `cargo fmt --check`, `cargo clippy -p
  yulius-browser --all-features --all-targets -- -D warnings`, `cargo nextest run -p
  yulius-browser --all-features` (105 passed) and the whole e2e suite (exit 0, 909 passed, 2
  skipped). With `dev` merged, `scripts/dev ci` at 08a7d002: exit 0, 3308 tests passed (16
  skipped), 152 script tests OK.
- **The second review of the final head** (d8d0a527) found its four findings answered and asked
  for two more things. The sandboxed-frame rule was proven on the fake only: Chrome was asked
  (above), the rule did not hold there, and the tests came first (`1c26954f`), the fix in
  `ed174310`. "The sandbox whose page started loading last" had no test: `09694541` and
  `13392caa` add two, `b27261b8` ends the wait at once when another sandbox's page starts
  loading, and `75fd0239` keeps that sandbox as one id. Removed by hand, each rule fails a test
  (restored after): the count of frames read (also on Chrome), the frame target on the page's
  own host (also on Chrome), a probe that does not say, the probe's own counting (on Chrome),
  an opaque origin taken for the page's own; the end of the wait for another sandbox, the id of
  the sandbox arriving last, the end of the wait for the same sandbox only, the un-press outside
  a sandbox. After them, `scripts/dev ci` at 53c0dd15: exit 0, 3311 tests passed (16 skipped),
  152 script tests OK, `cargo deny` ok, docs built; the whole e2e suite there: exit 0, 911
  passed, 2 skipped. With `dev` (dbec951d) merged again: `cargo clippy --workspace --all-features
  --all-targets -- -D warnings` ok, the browser and app crates' tests (580 passed) and the whole
  e2e suite (exit 0, 913 passed, 1 skipped); the whole gate at the final head is Yulius CI's.
- **Security scan.** `scripts/dev security`: "Security scan disabled: Vidoc is turned off in
  Yulius, for this project or the whole instance; nothing was scanned" (exit 0): disabled, not
  passed.
- **Review.** Independent review rounds in fresh sessions (read-only, no builds while the gate
  ran); rounds 1 to 5 requested changes, each answered test-first in this branch; round 6 approved
  64832478 with minors, fixed in `e8fa42d2`/`3d3a06cf`; the review of 0e4ffd03 found two of those
  fail-closed branches untested (now pinned by tests that fail without them, checked by removing
  each), a sandboxed frame counted as readable and a restored page left unwatched, fixed
  test-first; the final head is reviewed again before the merge. Pre-existing findings, reported
  on #1113 (this task creates no issues): the redactor's exact matching lets an edit in place
  through `browser_snapshot`; `browser_fill_secret` checks only the main frame's URL, so a focused
  cross-origin frame receives the value.
- **Not tested here.** macOS at run time (the gate only compiles for it), a person's display input,
  and real sites (single-page sign-ins, framing with cookies) beyond the fake server and the local
  Chromium pages. CI does not run the real Chromium test, so the probe's own script and what
  Chrome says about frames are checked only by that opt-in test (here on Chrome 157).
