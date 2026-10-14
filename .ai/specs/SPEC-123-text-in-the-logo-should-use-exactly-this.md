---
yulius-task: 47754f02-0d19-40e6-a6fd-ea0b2efd46d1
status: in-progress
---

# SPEC-123: The wordmark is set like the composer's quote

Status: in progress
Date: 2026-10-14
Owner: Yulius task #110
Scope authorization: task #110 ("Text in the logo should use exactly this font - first letter big,
rest small, spacing the same, font the same etc", with a screenshot of the quote above the new-task
composer: Rob Pike's "Data dominates…"). Refines SPEC-001 UI-041 (Tenor Sans for titles and the
wordmark).

## Goal

The text beside the logo ("Yulius") reads in the same type as the quote above the composer. It
uses the same face, weight, size and letter spacing. It is written as the name is spelled, with a
capital first letter and the rest lowercase, not in spaced capitals.

## Context

**What the screenshot shows.** The quote (`.composer-quote blockquote`, `styles.css` "Brand
details") is set in `--font-display` (Tenor Sans, self-hosted, one weight: 400). It uses 400 at
`--fs-xl` / `--lh-xl` (22 / 28 px), `letter-spacing: 0` and no text transform, in `--text`.

**What the wordmark does today.** It appears in two places:
- the sidebar's `.brand-name` (`layout.js`, home link, beside the 34 px mark);
- the sign-in and setup screens' `.auth-logo` (`pages/auth.js`, `app.js`).

Both use `--font` (Instrument Sans) at weight 600, `text-transform: var(--wordmark-case)` and
`letter-spacing: var(--wordmark-tracking)`. Those tokens are `uppercase` and `0.18em`. So the name
reads "YULIUS" in widely spaced bold capitals of another face. The sidebar uses 17 / 24 px
(`--fs-lg`); the auth screens use 22 / 28 px. `tokens.css` explains the earlier choice: "the thin
display face looked spindly beside the mark". The founder now asks for the quote's face instead.

**Related work.** SPEC-001 UI-041 already names Tenor Sans for "titles, wordmark". `tokens.css`
lists `--fs-xl` for "page titles, wordmark". The brand lab (`lab.js`, `?lab`) labels
`--font-display` "Titles and wordmark", and drives `--wordmark-case` and `--wordmark-tracking` from
its "Wordmark in capitals" and "Wordmark tracking" controls. No ADR covers the wordmark. No open
task touches the sidebar brand.

## Requirements

- WM-1: The sidebar wordmark and the sign-in/setup wordmark use the quote's computed font family,
  weight, size, line height and letter spacing, in the text colour.
- WM-2: The wordmark shows the name as written in `BRAND.name` ("Yulius"): first letter capital,
  the rest lowercase, with no text transform.
- WM-3: The face is the self-hosted Tenor Sans at its real weight (400), so the browser never
  synthesises a bold.
- WM-4: The brand lab can still restyle the wordmark through `--font-display`, `--wordmark-case`
  and `--wordmark-tracking`.
- WM-5: The sidebar brand still fits: the name and tagline stay inside the sidebar at its narrowest
  width (200 px below 1024 px) and at 390 px in the drawer, and no WCAG A/AA violations.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] Regression test: the sidebar and sign-in wordmarks are set like the quote
- [ ] Wordmark set in the quote's face: Tenor Sans 400, mixed case, no tracking, 22/28 px
- [ ] Brand docs, ADR-0100 and SPEC-001 say how the wordmark is set
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- In mock mode on a sandbox's composer, `.brand-name` matches `.composer-quote blockquote` in
  computed `font-family`, `font-weight`, `font-size`, `line-height` and `letter-spacing`. Its
  `text-transform` is `none` and its rendered text is "Yulius".
- On the signed-out sign-in screen, `.auth-logo` has the same values and the same rendered text.
- A Tenor Sans face at weight 400 (`tokens.css`'s `@font-face`) is loaded once the page has loaded.
- With the old CSS the new test fails (rendered text "YULIUS", weight 600).
- The brand still fits at a 1000 px window (200 px sidebar) and in the 390 px drawer. The existing
  suites `home`, `sign-in`, `quality`, `phone`, `responsive` and `layout` still pass.

## Test plan

New `e2e/tests/wordmark.e2e.ts` (WM-1 to WM-3, WM-5):
- "the sidebar wordmark is set like the composer's quote": `/s/studio`, compare computed styles of
  `.brand-name` with `.composer-quote blockquote`; read `innerText`; check the font is loaded.
- "the sign-in wordmark is set like the quote": the quote's computed type is read on the
  composer first (the sign-in screen has none), then compared with `.auth-logo` under
  `&signedout=1`.
- "the sidebar brand fits a narrow sidebar and the phone drawer": the brand text's right edge stays
  inside the sidebar at 1000 px and in the drawer at 390 px, with no WCAG violations.
- "the brand lab keeps the wordmark set like the quote in every Roma proposal" (WM-4, review
  round 1): under `?lab=roma` … `roma-3` the wordmark's letter spacing and case are the quote's.

## Decisions

- **Set the wordmark in `--font-display` at 400 and keep the token hooks.** Changing the tokens
  (`--wordmark-case: none`, `--wordmark-tracking: 0`) and the two rules' face and weight keeps the
  brand lab working. A literal copy of the quote's rule would not. Recorded as ADR-0100. It reverses
  the earlier "solid wordmark" choice because the founder now asks for this face explicitly.
- **22 / 28 px in the sidebar too.** "Exactly this font" includes its size. `--fs-xl` is already
  the scale's size for the wordmark. Mixed-case Tenor Sans at 22 px is narrower than the old
  tracked capitals at 17 px, so the sidebar keeps its width.
- **Leave the tagline ("by Quartio team") as it is.** The request names the text in the logo. The
  tagline stays the quiet caption that the existing comment asks for.

## Evidence

To be filled in when the work is done.
