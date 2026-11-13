# 0319. A secret is typed into the page the browser shows when the typing begins

## Status

Accepted. Specified in [SPEC-483](../../.ai/specs/SPEC-483-deep-security-and-quality-audit-fix-ever.md)
(slice 7, finding E3); task #465. Completes the rule of SPEC-002 ADR-20 (`browser_fill_secret`
types an agent secret only into a page of one of its hosts) and builds on the input lock of
[SPEC-450](../../.ai/specs/SPEC-450-browser-screenshot-never-shows-a-secret.md).

## Context

`browser_fill_secret` checked the tab's address as Yulius had last heard it (`tabs()`, fed by
Chrome's target events), then asked the browser to type. The browser focused the field,
waited for the input lock (a screenshot holds it from its look at the page to the capture),
and typed into whatever document was there by then. `browser_navigate` took no lock. An agent
could start a slow screenshot, ask for the fill on a tab showing the secret's host, and
navigate that tab to a site of its choice: once the screenshot ended, the secret was typed
there, and the tool answered `filled` on the secret's host, as the audit entry did.

## Decision

- **The browser asks the page, with the lock held.** `BrowserInstance::tool_type_secret` takes
  the rule ("may a page at this address get the secret") from its caller and applies it to the
  tab's document as read after the input lock is taken. A page the secret may not go to gets
  nothing typed and the tool says where the tab is (`SecretPageRefused`).
- **Yulius's own navigations wait for that lock.** `browser_navigate`, `browser_open`'s
  navigation, a viewer's address bar, back, forward and reload take the input lock: none of
  them is asked of the browser between that look and the last key. The lock covers the
  request, not the moment the new page takes over: a navigation already under way when the
  typing begins (a link an agent clicked just before, an Enter it typed, a `Page.navigate`
  that timed out) can still land in between. That is the next rule's case.
- **A page that moves on meanwhile is reported.** A page can still move on while it is typed
  into, by itself or by a navigation that was under way. When the document changed and the
  one it changed to may not get the secret, the tool fails with where the tab went
  (`SecretAstray`), whatever else the typing ran into, and the secret's activity records it,
  instead of `filled`. What SPEC-450 does for such a document (no screenshot of it or of its site) stays.

## Alternatives

- **Checking the address again in the application before typing.** It would read the same
  cache of target events and leave the wait for the lock between check and keys. Rejected.
- **Passing the bound hosts to the browser adapter.** The adapter would have to know host
  patterns and ports, which are the vault's. A rule as a function keeps one implementation
  (`page_host` and `bound_to`). Chosen.
- **Freezing the page while typing** (pausing its scripts or its navigations over DevTools).
  A paused page does not take input the way a person's typing arrives, and sign-in pages
  navigate on purpose after a field is filled. Reporting where it went is honest and enough.
- **No lock on navigations, only the report afterwards.** Then an agent chooses where the
  secret goes and Yulius only says so afterwards. Rejected.

## Consequences

- A navigation asked for while a secret is typed or a screenshot is taken starts when that
  ends (both are short). The other way round too: a screenshot, a fill, an agent's click or a
  headless viewer's key waits behind a navigation's request, which Chrome answers within the
  DevTools call's 10 seconds.
- Two gaps stay, found by this slice's review and listed as findings E8 and E9 of SPEC-483: the
  keys go to the focused element, which can be a frame of another site inside a page of the
  secret's host; and a navigation under way when the typing begins is reported afterwards, not
  prevented.
- A fill on a tab that moved on since the agent looked fails with the site it is on; the agent
  navigates back or asks for that host.
- A sign-in page that navigates by itself while the secret is typed to a host the secret is
  not bound to (a redirect to another domain on the last key) answers with an error naming that
  host although the sign-in may have worked; the agent reads the page to see.
