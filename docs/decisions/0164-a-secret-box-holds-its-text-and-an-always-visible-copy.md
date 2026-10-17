# 0164. A secret box holds its text and an always-visible copy

## Status

Accepted. Specified in [SPEC-207](../../.ai/specs/SPEC-207-this-ui-is-bad-fix-for-example-copy-icon.md).
Supersedes the hover copy and the kept Copy button of
[0093](0093-a-shown-secret-value-stays-masked.md); its masking stays.

## Context

ADR-0093 put a "Copy" icon button in a shown secret's value box, visible only on `:hover` or
`:focus-within`, and kept a "Copy value" text button beside the box. In use, the icon left an
empty gap after the eye until the box was hovered, and the text button, shorter than the box and
outside it, looked detached; the notes were bare text under a label. The person sent a screenshot
of that dialog: "this UI is bad, fix. For example copy icon is missing and overall".

## Decision

Each piece of a revealed secret sits in one framed box with its own copy icon button, always
visible:

- the value: the masked text (ADR-0093), the eye, then "Copy value";
- the notes: wrapping text that scrolls past 240 px, then "Copy notes";
- the two-factor setup key: the key, then "Copy key".

There is no separate Copy text button. Every copy goes through the Secrets page's `copyValue()`
(toast, clipboard cleared after 30 s, VLT-002). After a copy the button shows a check and the
tooltip "Copied" for 2 s; its accessible name stays constant, and the toast announces the result.
The box is a labelled group (`role="group"`, `aria-labelledby`), not inside a `<label>`, so a
click on its text does not press its first button.

## Alternatives

- Keep the hover copy and drop only the text button: the copy stays invisible to anyone who does
  not hover, and on touch screens.
- Keep the text button and drop the icon: a second-height control outside the box, and it squeezes
  the value at 390 px.
- A read-only `<input>` with the buttons as adornments: announced as an editable field, and it
  would hold the masked value in the DOM (ADR-0093).

## Consequences

- One copy control per secret field, in the same place in every vault dialog.
- Notes can be copied whole; a person copying one line still selects it.
- `copyButton()`, `readField()` and `maskedValue()` in `pages/secrets.js` are the pattern for any
  other page that shows a secret.
