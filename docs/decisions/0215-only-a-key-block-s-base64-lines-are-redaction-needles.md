# 0215. Only a key block's base64 lines are redaction needles

## Status

Accepted. Specified in [SPEC-302](../../.ai/specs/SPEC-302-key-header-lines-are-no-longer-redaction.md);
task #288, fixes #599. Refines [ADR-0150](0150-the-superagent-uses-agent-secrets-as-a-task-does.md)
(a key block's lines are needles, SPEC-186 SAV-005) and
[ADR-0156](0156-a-pasted-multi-line-secret-is-stored-with-escaped-line-breaks.md) (a key block
takes any END label, so a key and certificate bundle is one).

## Context

A key copied into a `.env` shows one line per line, so redaction adds a needle for each line of
a key block (`dotenv::key_lines`). It kept every line but the `-----` armour. An encrypted PEM's
RFC 1421 headers (`Proc-Type: 4,ENCRYPTED`, `DEK-Info: …`), PGP armour headers (`Version: …`,
`Hash: SHA256`) and, since ADR-0156, the text between a bundle's blocks (`subject=CN = …`) are
public, common text longer than the 8-byte `MIN_NEEDLE_LEN`: once such a key was stored, any
transcript or tool result in the sandbox holding them showed `[secret:NAME]` instead. Nothing
leaked; text was over-redacted.

## Decision

A line of a key block's file is a needle only when, trimmed, it is not empty, is not armour, and
holds only base64 or base64url symbols (`A-Z a-z 0-9 + / = - _`). The needle is the trimmed line.
The whole stored value stays a needle as before.

## Alternatives

- **Skip `<token>: <value>` headers and blank lines only** (the issue's wording): leaves a
  bundle's in-between text and a PGP signed message's text as needles. Rejected.
- **Standard base64 alphabet only**: a body line in the URL-safe alphabet (and the synthetic
  OpenSSH fixtures, which hold `-`) would lose its needle, the one outcome that leaks. Rejected:
  `-` and `_` cost at most an over-redaction.
- **Parse each format (PEM headers, OpenSSH, PGP armour)**: more code for the same result, since
  every key body a key block can hold (PEM, PKCS#8, OpenSSH, PGP) is base64. Rejected.

## Consequences

- Public text inside a key block no longer turns into `[secret:NAME]`.
- A public certificate's body in a bundle is base64 too, so it stays a needle and is still
  over-redacted (out of scope, said on #599).
- A non-base64 line stored inside a key block (not key material by any format) is no longer
  looked for on its own; it stays redacted as part of the whole value.
