# 0291. A screenshot of a page showing a filled secret is refused

## Status

Accepted. Specified in
[SPEC-450](../../.ai/specs/SPEC-450-browser-screenshot-never-shows-a-secret.md); task #432,
issue #1113. Completes SPEC-002 ADR-20 (`browser_fill_secret`: the agent never receives the
value) for pixels; applies to every agent of [0281](0281-the-superagents-use-the-project-browser.md).

## Context

`browser_fill_secret` types an agent secret into a page of a bound host, and the agent never
receives the value: every MCP result is redacted (`[secret:NAME]`), so `browser_snapshot` shows
the name. `browser_screenshot` returns pixels, which no redactor reads. After a fill into a field
drawn as text (an API-key input), a screenshot showed the value to the agent. Nothing connected
the fill to later captures.

The value can also be on a page somewhere other than the field it was typed into:
- the page's script may echo it;
- "show password" may turn a password field into a text field;
- history navigation may restore it, or the server may render an edited copy into the next page;
- a settings page may show the stored key in any tab, also after the browser restarted;
- a cross-origin frame or a closed shadow root keeps it out of the main document's reach.

The agent drives tabs, clicks and typing, and can run tools concurrently.

## Decision

- **Refuse, do not mask.** `browser_fill_secret` of a value types through `tool_type_secret`. A
  user name or TOTP code is typed unguarded, as the redactor does not hide them either. The
  `BrowserManager` keeps per sandbox, across browser restarts and whatever the tabs do, all in
  `Zeroizing` strings:
  - each value (32) and each line of a multi-line one, PEM armour aside (1024);
  - each host a value went into, and each exposed host (256 each);
  - each document it went into, by the main frame's loader id from `Page.getFrameTree`, before and
    after typing (256), forgotten with the browser process.
  A fill into a document that cannot be told, or beyond 32 values, is refused before anything is
  typed; past another bound every screenshot of that browser is refused.
- **Only a password field keeps a value out of the pixels.** Right after typing, the page is read.
  Unless the value is whole in a password field and shown nowhere, its host is exposed, and every
  page of it is refused from then on. Any other typing into a document a value went into may edit
  it in place: a value whole in a password field before the typing and not after a line of it
  (checked before that line's Enter) exposes the host. A value gone from the page is no edit (a
  single-page sign-in). A page that cannot be read after a fill refuses every screenshot until
  Yulius restarts.
- **Look at every page before the capture.** Once a value was typed or the sandbox has agent
  secrets, a probe reads the page about to be captured, in any tab. It runs in an isolated world
  (`Page.createIsolatedWorld`), and returns:
  - `shown`: the `textContent` of its documents (all but `head`), open shadow roots,
    same-origin frames, objects and embeds, and the values and placeholders of fields drawn as
    text;
  - `masked`: the values of password fields.
- **When it refuses.** The screenshot is a tool error (`BrowserError::SecretOnPage`) pointing to
  `browser_snapshot`, in any of these cases:
  - the page's host is exposed;
  - it frames a host a value went into, or an exposed one, where the probe cannot read: another
    host in the frame tree, or an `iframe` target (`Target.getTargets`) whose parent frame is the
    tab's, not said, or no target listed, as Chromium lists an out-of-process frame without its
    address or not at all in the tree;
  - `shown` holds a remembered value, which also exposes the host;
  - `shown` holds any agent secret the redactor's matcher finds for the sandbox (values, their
    encodings, lines);
  - the document, the frame targets, the page or the agent secrets cannot be read.
- **Read the page as it is.** The probe reads the document's own properties through the
  prototypes, so a page's named elements cannot hide its text.
- **Leave no gap around the capture.** A screenshot holds the instance's input lock from the probe
  to its last capture, and the agent's typing and clicks take it too. After the capture, the page
  is read again, and the image is dropped unless the same document passes again.
- **Compare in Yulius.** The probe takes no argument: the page returns its text and Yulius
  searches it. The value never enters a page, which may show another origin by then.

## Alternatives

- **Mask fields before the capture** (`-webkit-text-security: disc` on the filled field or all
  text fields, removed afterwards): it changes the page the person watches, misses values the page
  echoes outside fields, cannot style cross-origin frames, and fails open when the style does not
  apply (a re-rendered element, a canvas).
- **Refuse only an exact match of the value** (the first version): typing one character into the
  middle of the filled value defeats it.
- **The strict rule for the filled document only** (the second version): an edited copy in the
  next page of the site (a form shown again) is matched by exact text only.
- **Remember fills per tab, or only in the browser instance**: the agent opens and closes tabs,
  and a browser stops after 30 minutes without use, while the site keeps what was typed.
- **Check exposure only at screenshots**: a value edited in a password field and saved, with no
  screenshot in between, reaches the next page unnoticed.
- **Require the value in a password field of its document at every screenshot** (the third
  version): a single-page app's sign-in removes the field and keeps the document, which then
  refused its dashboard and exposed the site.
- **Read frames from `Page.getFrameTree` only**: an out-of-process frame is listed there without
  its address, or not at all.
- **Record at fill time whether the field was a password field**: it misses a revealed password
  and a value restored by history navigation.
- **Send the value (or its hash) into the page to search it**: hands the value to whatever origin
  the tab shows by then; a hash cannot find a value inside longer text.
- **Refuse every screenshot once a secret was filled**: safe, but breaks screenshots after every
  sign-in with a password, which the issue keeps working.

## Consequences

- An agent that typed a key into a text field reads that site with `browser_snapshot` until
  Yulius restarts; so does an agent whose page framed a site it signed in to. A page of another
  site is refused only while it shows an agent secret, even scrolled out of view. A password
  sign-in keeps screenshots.
- A screenshot in a sandbox with agent secrets costs a `Page.getFrameTree`, a `Target.getTargets`,
  an isolated world and a probe, twice; one without any captures as before. Agent input waits for a
  screenshot's capture.
- Not detected: a value drawn in a canvas, an image, through CSS (`content: attr(…)`) or in a
  closed shadow root; an edited copy on another host, kept by the site past a Yulius restart, seen
  from another sandbox's browser, or made by the page or the site after a fill that stayed in a
  password field; a never-filled agent secret in a frame of another site; a page that changes and
  changes back between the two looks. Exact copies are still refused by the redactor's matcher.
  Input a person gives on the virtual display does not take the lock; the person's own live view
  is not affected.
