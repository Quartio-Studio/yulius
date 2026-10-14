# 0093. A shown secret value stays masked until its eye is pressed

## Status

Accepted. Specified in [SPEC-114](../../.ai/specs/SPEC-114-value-in-passwords-should-be-hidden-by-d.md).

## Context

After the two-factor code, a secret's Show dialog printed its value in plain text. Anyone looking
at the screen saw it, including during a screen share. The person asked: "Value in passwords
should be hidden by default even after show, and shown after explicitly clicking eye icon. On
hover there should be option to copy + the current copy button".

## Decision

The Show dialog still fetches the value (the audited reveal, VLT-002), but draws it masked: a
fixed run of 12 dots, with the value kept out of the DOM. An eye toggle button in the value box
("Show value", `aria-pressed`, its icon eye or eye-off) draws the value and removes it again.
Every dialog opens masked. A "Copy" icon button in the box, visible on `:hover` or `:focus-within`
like a task row's quick Archive, copies the value whether masked or not, and the "Copy value"
button beside the box stays. The dialog's notes and the two-factor setup key stay shown.

## Alternatives

- Fetch the value only when the eye is pressed: a second request and a second audit entry for one
  look, while Show already names the reveal.
- Mask with CSS (`-webkit-text-security`, blur) and keep the value in the DOM: selection,
  find-in-page and extensions still read it, and the dots would tell its length.
- A disabled password `<input>` with a show toggle: a form control for read-only text, harder to
  copy from and announced as an editable field.

## Consequences

- Seeing a value takes one more click; copying it does not.
- The audit trail still records "revealed" when Show is pressed, even if the eye is never pressed.
- The eye toggle and quick copy live in `maskedValue()` on the Secrets page; another page that
  shows a secret would reuse it rather than draw the value directly.
