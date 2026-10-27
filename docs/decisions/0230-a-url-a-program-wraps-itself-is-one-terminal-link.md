# 0230. A URL a program wraps itself is one terminal link

## Status

Accepted. Specified in [SPEC-339](../../.ai/specs/SPEC-339-links-breaks-when-squeezed-fix.md);
task #323, fixes #940. Extends SPEC-001 UI-018 (web links in the terminal).

## Context

Claude Code lays out its transcript itself. It breaks a URL longer than the row at the right edge,
continues it on the next row at the text's indent, and positions the rows with cursor moves. It
sends no OSC 8 hyperlink. xterm marks only its own soft wraps as continuations, so the web-links
addon sees two unrelated lines: in a narrow terminal, a click on a long URL opened its first
fragment, and the rest was plain text.

## Decision

- A link provider in `termlinks.js` (`createContinuedUrlProvider`) joins rows that a program broke
  itself and links the URLs that cross such a break. It is registered before the web-links addon:
  where both match the first row, xterm keeps the provider registered first, so the link opens the
  whole URL. Every other URL is still the addon's.
- A row continues into the next only if all of these hold:
  - it ends in the last column, or in one of the last 2 in the prompt, whose rows all pad one (a
    row is the prompt's when walking up through blank and indented unmarked rows reaches `❯`, not a
    menu's `❯ 1.`);
  - the next row is not soft-wrapped;
  - the next row's text starts where the row's text starts: after one marker such as `❯`, `●`,
    `⏺`, `-`, `⎿` or `▎` and a list number (`1.`), under blanks or under a repeated quote bar;
  - the word that spans the break is longer than the row's text;
  - the next row does not start a new `scheme://`, unless the cut fell inside the scheme.

  Each rule matches Claude Code 2.1.292's measured layout. A run longer than 32 rows is no link.
- The link is one range from the URL's first cell to its last, across rows, and it opens through
  the terminal's usual `onLink`.

## Alternatives

- **Replace the web-links addon with our own provider for every URL.** That changes behaviour that
  works today and gains nothing for one-row URLs.
- **Join on the server, or have Claude Code send OSC 8 links.** The server would have to interpret
  a TUI's cursor moves, and Claude Code's output is not ours to change.
- **Join any URL that touches the right edge onto the next row.** Tool output often has a line
  ending near the edge above an unrelated line at the same indent, and a wrong join opens a wrong
  address, which is worse than a fragment.

## Consequences

- Long URLs in Claude Code's prompt, answers, lists and code open whole in a narrow terminal.
- A program that wraps differently (another indent, a wider right margin) keeps today's
  behaviour: its URL is linked only up to the break.
- The underline of a joined link also covers the indent cells of its continuation rows.
- A URL that genuinely ends at the edge, followed by a word long enough that the two exceed the
  row, is joined: that screen looks the same as a longer URL cut there.
- A URL longer than 32 rows is not linked at all, rather than linked to a cut address.
- A shell whose prompt also draws `❯` (starship, pure) has the indented rows under it treated as
  prompt rows: a URL there that ends one column short of the edge, followed by a long enough
  word at the same indent, is joined. Shell output rarely starts indented.
- If Claude Code changes its layout, the rules in `findContinuedUrls` and its e2e test, which
  replays recorded rows, change together.
