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

- **Refuse, do not mask.** `browser_fill_secret` of a value (not a user name or TOTP code, which
  the redactor does not hide either) types through `tool_type_secret`. The browser instance
  remembers each value (at most 32) and each document it went into (the main frame's loader id
  from `Page.getFrameTree`, before and after typing; at most 256) in `Zeroizing` strings, until the
  browser stops, whatever its tabs do. A fill into a document that cannot be told, or beyond the
  bounds, is refused before anything is typed.
- **Look at the page before every capture.** Once a value was typed, a probe reads the page about
  to be captured, in any tab: `shown` is the `textContent` of its body, open shadow roots and
  same-origin frames plus the values of fields drawn as text, and `masked` is password field
  values. The screenshot is a tool error (`BrowserError::SecretOnPage`) pointing to
  `browser_snapshot` when `shown` holds a remembered value, or when the page is a document a
  value was typed into and that value is not found whole in a password field. That covers a value
  edited in place, cut or reformatted, typed into a cross-origin frame, or revealed.
- **Fail closed and leave no gap.** An unknown document, a probe that fails, throws, answers
  malformed or reads over 4 000 000 characters is refused. The probe runs in an isolated world
  (`Page.createIsolatedWorld`), out of reach of page scripts. A screenshot holds the instance's
  input lock from the probe to its last capture, and typing and clicks take it too.
- **Compare in Yulius.** The probe takes no argument: the page returns its text and Yulius
  searches it. The value never enters a page, which may show another origin by then.

## Alternatives

- **Mask fields before the capture** (`-webkit-text-security: disc` on the filled field or all
  text fields, removed afterwards): it changes the page the person watches, misses values the page
  echoes outside fields, cannot style cross-origin frames, and fails open when the style does not
  apply (a re-rendered element, a canvas).
- **Refuse only an exact match of the value** (the first version): typing one character into the
  middle of the filled value defeats it.
- **Remember fills per tab**: the agent opens and closes tabs, and another tab may show the
  stored value.
- **Record at fill time whether the field was a password field**: it misses a revealed password
  and a value restored by history navigation.
- **Send the value (or its hash) into the page to search it**: hands the value to whatever origin
  the tab shows by then; a hash cannot find a value inside longer text.
- **Refuse every screenshot once a secret was filled**: safe, but breaks screenshots after every
  sign-in with a password, which the issue keeps working.

## Consequences

- An agent that typed a key into a text field reads that page with `browser_snapshot`. Other
  pages are refused only while they show the value, even scrolled out of view. A password
  sign-in keeps screenshots.
- Once a value was typed, each screenshot in that browser costs a `Page.getFrameTree`, an
  isolated world and a probe; a browser where nothing was filled captures as before. Input waits
  for a screenshot's capture.
- Not detected: a value drawn in a canvas or image, and an edited copy on a page other than the
  one it was typed into (for example an edited field that history navigation restored into a new
  document). A browser restart forgets the values along with the old pages. The person's own
  live view is not affected.
