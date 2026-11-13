# 0320. A secret's keys reach only sites it is for: the focus, the page's frames and a page on its way are asked too

## Status

Accepted. Specified in [SPEC-483](../../.ai/specs/SPEC-483-deep-security-and-quality-audit-fix-ever.md)
(slice 8, findings E8, E9 and E10); task #465. Closes the two gaps
[0319](0319-a-secret-is-typed-into-the-page-the-browser-shows-when-the-typing-begins.md) left
open in its consequences.

## Context

ADR-0319 asks the tab's document, with the input lock held, whether the secret may be typed
into it. The keys do not go to "the tab's document": they go to the element with the focus,
and to whatever document is there when they arrive.

- **A document of another site inside the page (E8).** A page of the secret's host can hold
  one: an advertisement, a widget, a comment box, a page an agent's own content embeds, in an
  `iframe`, an `object`, an `embed` or a frame inside a shadow root. With the focus in it (a
  click into it before, a selector naming it, a widget that takes the focus by itself), the
  secret was typed into the other site and answered `filled` on the secret's host. Proven on
  Google Chrome 157: a page at `127.0.0.2` holding `localhost` in a frame, in an `object` and
  in a frame inside a closed shadow root.
- **A navigation under way (E9).** An agent clicks a link and asks for the fill before the new
  page arrives. Yulius's own navigations wait for the lock, but this one was asked for before
  it. ADR-0319 reported such a move afterwards (`SecretAstray`), with the secret already typed
  into the new page.
- **What is typed beside the secret (E10).** `browser_fill_secret` also types a secret's user
  name and its one-time code. Neither is hidden from screenshots, so they went through
  `browser_type`'s path, with none of the above and none of ADR-0319: on the address Yulius
  had last heard of the tab. A one-time code signs in while it lasts.

What a real Chrome does in the second case was measured: after a click on a link whose server
answers after 1.5 or 4 seconds, Chrome 157 announces the navigation at once
(`Page.frameRequestedNavigation`, `Page.frameStartedNavigating` with the address) and answers
`Page.getFrameTree` only when the new document is there, with the new address, for a link to
another site and to the same one. So on that browser the look at the page already waits and
ADR-0319's rule refuses the new page. That holding is not something the protocol promises.

What the page can say about its focus is limited: `document.activeElement` names a frame
element, not the document in it; for a closed shadow root it names the root's host, whatever
has the focus inside; a fenced frame is in no list of frames.

## Decision

- **Only a text field of the page is the page's own.** After the field is focused, with the
  lock held, the page is read once more. The focus counts as in the page when it is in an
  `input`, a `textarea` or editable content of a document the page's probe read (the page's,
  or a frame's of its origin, through open shadow roots). Anything else (a frame, an object,
  an embed, an element that takes no text, the host of a closed shadow root, nothing, an
  answer this code does not know) can be a document of another site: then every frame of
  another origin the page holds (in its tree, and as targets of their own) must be at an
  address the secret may be typed into. One that is not, or whose address is not said, gets
  nothing typed and the tool names its site (`SecretFrameRefused`). The focus on a fenced
  frame is refused whatever the frames are. A page that cannot be read then, or whose frames
  cannot be listed, gets nothing typed.
- **The page is asked again after the focus.** The document is read a second time once the
  field has the focus. When it is no longer the one the rule was asked of (a navigation
  arrived meanwhile), nothing is typed: the new page is named when the secret is not for it,
  and otherwise the fill is to be asked again, as the focused field went with the old page.
- **Where the page's frames are going is asked last.** The browser remembers, per tab and
  frame, the address Chrome announced a navigation to another document for
  (`Page.frameRequestedNavigation` in this tab, `Page.frameStartedNavigating`), until Chrome
  says the frame arrived, stopped loading or is gone, the tab's session ended, or 30 seconds
  passed. Right before the keys, a page with a frame (itself or one in its tree) on its way to
  an address the secret may not be typed into gets nothing typed and that site is named
  (`SecretPageLeaving`). A navigation inside the document (a fragment, `pushState`) is none.
  The record holds 256 frames per tab. When more of a tab's are on their way, nothing is
  known of that tab for 30 seconds, and when Chrome's events were skipped, of none: fills
  then answer "try again" instead of typing. An address longer than 2048 bytes is kept as no
  address, which no secret is for (a cut one could read as another site's: what comes before
  the host can be that long).
- **The frames are asked again after the keys.** A frame's script can take the focus back
  between the look and the keys. When, after the typing, the focus is not in a text field of
  the page and the page holds a frame of a site the secret is not for (or the focus is on a
  fenced frame), the tool fails with that site (`SecretFrameAstray`), and the secret's
  activity records it, as for ADR-0319's page that moved on.
- **A user name and a one-time code go where the secret goes.** They are typed under all of
  the above and ADR-0319 (`tool_type_beside_secret`), and stay what they were for
  screenshots: not remembered as typed secrets.
- **One answer for the caller.** The refusals are, for the application and the agent, a page
  the secret is not for (the kind `SecretPageRefused`, 403), the reports a secret gone astray
  (`SecretAstray`): only the sentences differ.

## Alternatives

- **Asking the frames only when a frame element has the focus.** The first version of this
  change. It failed open for an `object`, an `embed` and a frame inside a closed shadow root
  (review of slice 8; both typed into on Chrome 157). Naming what is safe (a text field of a
  document that was read) instead of what is dangerous fails closed for the next container.
- **Asking only the focused frame.** The page does not say which document has the focus when
  frames nest or sit in a closed shadow root, and a frame in another process answers on
  another session that may not be attached. Asking all frames of other sites is simple and
  fails closed; the price is in the consequences.
- **Refusing every fill while the focus is in a frame.** Sign-in forms in a frame of an
  identity provider are common, and a secret bound to both hosts is the honest way to say so.
- **Relying on Chrome holding its answers during a navigation.** It is what Chrome 157 does,
  not a contract, and Chromium builds differ by version. The announcement costs a map entry.
- **Waiting for the navigation to end, then typing.** Then the secret is typed into a page the
  agent did not look at. The agent is told and decides.
- **Cancelling the navigation, or freezing the page's scripts while typing.** Yulius would
  change what a page does to type into it; ADR-0319 rejected the second already.
- **Only the tab's own navigation, not its frames'.** The focus can be in a frame of the
  page's origin that is itself leaving for another site.
- **A record without a bound, or one that drops its oldest entry.** A page makes frames
  faster than Chrome settles them; dropping one silently would type blind for that frame.

## Consequences

- A fill whose focus is not in a text field of the page fails when the page holds a frame of
  a site the secret is not bound to, wherever the focus really is. That includes a field
  inside a closed shadow root (a packaged sign-in component) on a page that also holds, say,
  a captcha's or an analytics frame. The agent names a field of the page with `selector`, or
  asks for the frame's host (`secret_request_hosts`).
- A page the probe cannot read (more than 4 000 000 characters of text) gets no secret typed;
  before, it was typed and every screenshot refused afterwards.
- A fill right after a click that leaves for another site fails at once on a browser that
  answers during the navigation, and after the new page arrived on one that holds its answer
  (Chrome 157: the page rule then names the new page). A navigation that takes longer than
  the 10 seconds of a DevTools call fails the fill as "the tab's page cannot be identified".
  Nothing is typed in any of these.
- A navigation Chrome announced and never finished nor gave up (a link that became a
  download, an address of another program) keeps fills to that tab refused for up to 30
  seconds when its address is not one the secret is for. So does a frame of the page that is
  on its way to such an address, also when the focus is in a field of the page itself.
- The address asked is the one announced: a redirect on the way to another site is seen when
  it arrives (the second look, or ADR-0319's report).
- A user name or a one-time code is no longer typed into a tab that moved on, a frame of
  another site or a page on its way to one; the answers are the secret's.
- Still reported afterwards, not prevented: a navigation the page starts by itself after the
  look (a script's redirect on the last key), a frame's script that takes the focus between
  the look and the keys, and a navigation Chrome announced in the last moment before the keys
  (its events are read apart from its answers, so one sent just before the last answer may
  not have been read yet). The page is one the secret is for.
- The report after the keys can be a false alarm: a page whose own script moves the focus
  off the field while it is typed (to its button, say) and that holds a frame of another site
  answers as if that frame could have the secret, though it is in the page's field. It says
  "may".
- Not asked: frames inside a frame that runs in a process of its own, when they run in that
  frame's process (they are of its site, or documents it wrote itself). The outer frame is at
  an address the secret is bound to; what that site embeds from itself is its page.
