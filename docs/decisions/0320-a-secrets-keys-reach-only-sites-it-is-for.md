# 0320. A secret's keys reach only sites it is for: a focused frame and a page on its way are asked too

## Status

Accepted. Specified in [SPEC-483](../../.ai/specs/SPEC-483-deep-security-and-quality-audit-fix-ever.md)
(slice 8, findings E8 and E9); task #465. Closes the two gaps
[0319](0319-a-secret-is-typed-into-the-page-the-browser-shows-when-the-typing-begins.md) left
open in its consequences.

## Context

ADR-0319 asks the tab's document, with the input lock held, whether the secret may be typed
into it. The keys do not go to "the tab's document": they go to the element with the focus,
and to whatever document is there when they arrive.

- **A frame of another site (E8).** A page of the secret's host can hold a frame of another
  site: an advertisement, a widget, a comment box, a page an agent's own content embeds. With
  the focus in that frame (a click into it before, or no selector and the frame focused), the
  secret was typed into the other site and answered `filled` on the secret's host. Proven on
  Google Chrome 157: a page at `127.0.0.2` framing `localhost`, the focus in the frame's field.
- **A navigation under way (E9).** An agent clicks a link and asks for the fill before the new
  page arrives. Yulius's own navigations wait for the lock, but this one was asked for before
  it. ADR-0319 reported such a move afterwards (`SecretAstray`), with the secret already typed
  into the new page.

What a real Chrome does in the second case was measured: after a click on a link whose server
answers after 1.5 or 4 seconds, Chrome 157 announces the navigation at once
(`Page.frameRequestedNavigation`, `Page.frameStartedNavigating` with the address) and answers
`Page.getFrameTree` only when the new document is there, with the new address, for a link to
another site and to the same one. So on that browser the look at the page already waits and
ADR-0319's rule refuses the new page. That holding is not something the protocol promises.

## Decision

- **The frames are asked when the focus is in one.** After the field is focused, with the lock
  held, the page is read once more. When its focused element is a frame, every frame of
  another origin the page holds (in its tree and in processes of their own, nested ones too)
  must be at an address the secret may be typed into; one that is not, or whose address is not
  said, gets nothing typed and the tool names its site (`SecretFrameRefused`). A page that
  cannot be read then, or whose frames cannot be listed, gets nothing typed. The focus in the
  page itself asks nothing of its frames.
- **Where the tab is going is asked.** The browser remembers, per tab and frame, the address
  Chrome announced a navigation to another document for, until Chrome says the frame arrived
  or stopped loading, the tab is gone, or 30 seconds passed. A fill on a tab whose page is on
  its way to an address the secret may not be typed into types nothing and names that site
  (`SecretPageLeaving`). A navigation inside the document (a fragment, `pushState`) is none.
- **One answer for the caller.** Both are, for the application and the agent, a page the
  secret is not for (the kind `SecretPageRefused`, 403): only the sentence differs.

## Alternatives

- **Asking only the focused frame.** The page says that its focused element is a frame, not
  which document inside it has the focus when frames nest, and a frame in another process
  answers on another session that may not be attached. Asking all frames of other sites is
  simple and fails closed; the price is in the consequences.
- **Refusing every fill while the focus is in a frame.** Sign-in forms in a frame of an
  identity provider are common, and a secret bound to both hosts is the honest way to say so.
- **Relying on Chrome holding its answers during a navigation.** It is what Chrome 157 does,
  not a contract, and Chromium builds differ by version. The announcement costs a map entry.
- **Waiting for the navigation to end, then typing.** Then the secret is typed into a page the
  agent did not look at. The agent is told and decides.
- **Cancelling the navigation.** Yulius would change what a page does to type into it.

## Consequences

- A fill with the focus in a frame fails when the page also holds a frame of a site the
  secret is not bound to, even when the focus is in another frame. The agent names a field of
  the page with `selector`, or asks for the frame's host (`secret_request_hosts`).
- A fill right after a click that leaves for another site fails at once on a browser that
  answers during the navigation, and after the new page arrived on one that holds its answer
  (Chrome 157: the page rule then names the new page). A navigation that takes longer than
  the 10 seconds of a DevTools call fails the fill as "the tab's page cannot be identified".
  Nothing is typed in any of these.
- A navigation Chrome announced and never finished nor gave up (a link that became a
  download) keeps fills to that tab refused for up to 30 seconds when its address is not one
  the secret is for.
- Still reported afterwards, not prevented: a navigation the page starts by itself after the
  look (a script's redirect on the last key). The page is one the secret is for.
