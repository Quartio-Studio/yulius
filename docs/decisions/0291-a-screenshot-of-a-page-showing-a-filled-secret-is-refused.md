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

The value can also be on the page somewhere other than the field it was typed into: the page's
script may echo it, "show password" may turn a password field into a text field, history
navigation may restore it, and a cross-origin frame or a closed shadow root keeps it out of the
main document's reach.

## Decision

- **Refuse, do not mask.** The browser instance remembers per tab each value
  `browser_fill_secret` typed (`tool_type_secret`; field `value`/`password`, in `Zeroizing`
  strings, at most 32 per tab, until the tab closes). Before `Page.captureScreenshot` of such a
  tab, a probe function reads the page's visible text and the values of fields drawn as text,
  through open shadow roots and same-origin frames, and the values of password fields apart from
  them. When the readable text holds a remembered value, the screenshot is a tool error
  (`BrowserError::SecretOnPage`) pointing to `browser_snapshot`.
- **Compare in Yulius.** The probe takes no argument: the page returns its text and Yulius
  searches it. The value is never sent into a page, which may show another origin by then.
- **Fail closed.** A probe that fails, throws, answers malformed or reads over 4 000 000
  characters refuses the screenshot. A value the probe finds nowhere right after typing it
  (a cross-origin frame, a closed shadow root, a field that cut or reformatted it) refuses
  screenshots while the tab's main frame keeps that document (`Page.getFrameTree` loader id), and
  always when the loader id cannot be read.
- **Password fields keep screenshots.** A value only in `input[type=password]` is drawn as dots,
  so it does not refuse, until the page turns the field into a text field.

## Alternatives

- **Mask fields before the capture** (`-webkit-text-security: disc` on the filled field or all
  text fields, removed afterwards): it changes the page the person watches, misses values the page
  echoes outside fields, cannot style cross-origin frames, and fails open when the style does not
  apply (a re-rendered element, a canvas).
- **Record at fill time whether the field was a password field**, refusing screenshots of a
  non-password fill until navigation: it misses a revealed password and a value restored by
  history navigation, and a navigation would free a page that still shows it.
- **Send the value (or its hash) into the page to search it**: hands the value to whatever origin
  the tab shows by then; a hash cannot find a value inside longer text.
- **Refuse every screenshot of a tab once a secret was filled**: safe, but breaks screenshots
  after every sign-in with a password, which the issue keeps working.

## Consequences

- An agent that typed a key into a text field reads the page with `browser_snapshot` until the
  page no longer shows it. A screenshot of a page that shows the value somewhere the probe reads
  is refused even if the value is scrolled out of view.
- Each screenshot of a tab that had a fill costs one probe (and a `Page.getFrameTree` when a fill
  went unseen); tabs without fills are captured as before.
- Values drawn in a canvas, an image or a closed shadow root after a fill that the probe saw
  elsewhere are not detected. A browser restart forgets the values along with the old pages. The
  person's own live view is not affected.
