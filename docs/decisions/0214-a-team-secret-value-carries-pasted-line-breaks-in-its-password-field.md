# 0214. A team secret's value carries pasted line breaks in its password field

## Status

Accepted. Specified in [SPEC-299](../../.ai/specs/SPEC-299-team-secret-value-field-keeps-pasted-lin.md);
task #285, fixes #592. The team secret counterpart of
[ADR-0156](0156-a-pasted-multi-line-secret-is-stored-with-escaped-line-breaks.md), which stays the
rule for agent secrets.

## Context

A team secret's Value on the Secrets page is an `<input type="password">`, a one-line control. The
HTML value sanitisation of such a field strips U+000A and U+000D (Chrome first turns a pasted line
break into a space), so a multi-line value pasted there (a PEM key, recovery codes) was stored on
one line and Copy gave the flattened text back. The server keeps line breaks in a team secret
(`vault::field` refuses only NUL and oversize values). Unlike an agent secret, which never spans
lines and so stores a key with literal `\n` (ADR-0156), a team secret is read and copied back by
people: it must come back exactly as pasted. And it must stay masked while typed or pasted.

## Decision

- **The field stays a password input.** A paste whose text holds a line break is taken by the
  page: each LF is inserted as U+E00A and each CR as U+E00D (private-use characters the input
  keeps), as typed so it can be undone, over the selection. The form turns them back into LF and
  CR when it sends the value. A CRLF paste is therefore stored as CRLF: Copy returns the original
  text. A paste without a line break is the browser's.
- **A shown value keeps its lines.** `.secret-value` has `white-space: pre-wrap`.
- **The field says so.** Its hint, its description for assistive technology, says a pasted value
  keeps its line breaks.

## Alternatives

- **A `<textarea>` masked with `-webkit-text-security`.** Non-standard, its text is selectable,
  read aloud and in reach of extensions, and password managers do not treat it as a password.
- **A "multi-line" toggle that switches to a plain textarea.** Unmasks the value.
- **Literal `\n`, as for agent secrets (#591).** Changes the text people copy back.
- **Normalising CRLF to LF.** Simpler, but Copy would not return the original bytes, which a
  checksum or a strict tool may need.
- **Keeping the value in a JavaScript variable beside the field.** Every edit (typing, deleting,
  undo) would have to be mirrored; the marks keep one source of truth, the field.

## Consequences

- Pasted multi-line team secrets round-trip; agent secrets are unchanged (their field writes key
  breaks as `\n`).
- U+E00A or U+E00D typed or pasted into a team secret's Value become line breaks. They are
  private-use characters that no secret is expected to hold.
- Each pasted line break counts as one masked dot.
- A line break typed with the keyboard is still impossible: Enter submits the form. Multi-line
  values come from a paste or `yulius secrets import`.
