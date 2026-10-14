# 0100. The wordmark is set like the composer's quote

## Status

Accepted. Specified in [SPEC-123](../../.ai/specs/SPEC-123-text-in-the-logo-should-use-exactly-this.md).

## Context

The name beside the logo was set in Instrument Sans at 600, in capitals tracked 0.18em ("YULIUS").
That was chosen because the thin display face had looked spindly beside the mark. The quote above
the new-task composer is set in Tenor Sans, the brand's display face. The person pointed at that
quote and asked: "Text in the logo should use exactly this font - first letter big, rest small,
spacing the same, font the same etc".

## Decision

The wordmark is set the way `.composer-quote blockquote` is set:
- `--font-display` (Tenor Sans) at 400, its only self-hosted weight;
- `--fs-xl` / `--lh-xl` (22 / 28 px);
- no letter spacing;
- the name as `BRAND.name` spells it ("Yulius"), in `--text`.

This applies to the sidebar's `.brand-name` and to the sign-in and setup screens' `.auth-logo`.
The tokens `--wordmark-case` and `--wordmark-tracking` stay and now default to `none` and `0`, so
the brand lab still restyles the wordmark. The tagline under it is unchanged.

## Alternatives

- Keep the capitals and tracking and only change the face: the person asked for the first letter
  big and the rest small, and for the same spacing.
- Keep the sidebar's 17 px: "exactly this font" includes its size. The type scale already names
  `--fs-xl` for the wordmark, and mixed case at 22 px is no wider than the old tracked capitals at
  17 px.
- Copy the quote's rule into the wordmark without the tokens: the brand lab's "Wordmark in capitals"
  and "Wordmark tracking" controls would stop working.

## Consequences

- The wordmark and the quote share a typographic voice. A change to `--font-display` changes both,
  together with the page titles.
- The sidebar's brand is 4 px taller (the name's line is 28 px instead of 24 px).
- `e2e/tests/wordmark.e2e.ts` compares the wordmark's computed type with the quote's, so a later
  restyle of one without the other fails the test.
