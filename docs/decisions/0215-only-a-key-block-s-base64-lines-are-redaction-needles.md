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

Blank lines and armour lines are never needles. Another line, trimmed, is a needle whole when:

- it holds only base64 or base64url symbols (`A-Z a-z 0-9 + / = - _`) and whitespace; or
- it holds other text and a run of more than 32 consecutive base64 symbols (`PUBLIC_RUN_LEN`). A
  key body line is 64 (PEM) or 70 (OpenSSH) symbols; an IV in `DEK-Info` is at most 32 hex
  digits, and header values hold a comma, colon, dot or space, which end a run.

A line is kept whole, never cut: its needle then matches only that whole line, which is what a
`.env` copy shows, so a header line alone (`Proc-Type: 4,ENCRYPTED` in a transcript) is never
redacted, and no tokenisation can miss a body glued to its header or split by a separator that
is no whitespace (a zero-width space).

Whitespace and mixed lines matter because the API (or an agent writing the value) can store a key
whose body breaks are spaces or dropped, with `\n` only around the armour, and `key_block`
accepts it: the body then sits on one line, alone or after its headers (`Proc-Type: 4,ENCRYPTED
DEK-Info: … <body>`), and a `.env` copy shows that line. The Secrets page never writes that shape (it
escapes the clipboard's own breaks), but a needle dropped there would leak the body. The whole
stored value stays a needle as before; it holds literal `\n`, so it never matches a `.env` copy,
where the breaks are real.

## Alternatives

- **Skip `<token>: <value>` headers and blank lines only** (the issue's wording): leaves a
  bundle's in-between text and a PGP signed message's text as needles. Rejected.
- **No whitespace inside a line**: drops a body joined by spaces, which `key_block` accepts, and
  so leaks it in a `.env` copy (review round 1). Rejected.
- **Drop every line with a non-base64 symbol**: drops a body joined onto its header line, and
  leaks it the same way (review round 2). Rejected.
- **The runs of whitespace-separated base64 tokens of a mixed line** (review round 2's fix):
  misses a body glued to its header or split by a zero-width space, and leaks it (review round
  3). Rejected.
- **Every base64 run of a mixed line, with no length floor**: header words (`GnuPG v2`,
  `Synthetic`) pass `MIN_NEEDLE_LEN` and would be over-redacted again. Rejected.
- **Standard base64 alphabet only**: a body line in the URL-safe alphabet (and the synthetic
  OpenSSH fixtures, which hold `-`) would lose its needle, the one outcome that leaks. Rejected:
  `-` and `_` cost at most an over-redaction.
- **Parse each format (PEM headers, OpenSSH, PGP armour)**: more code for the same result, since
  every key body a key block can hold (PEM, PKCS#8, OpenSSH, PGP) is base64. Rejected.

## Consequences

- Public text inside a key block no longer turns into `[secret:NAME]`.
- A public certificate's body in a bundle is base64 too, so it stays a needle and is still
  over-redacted (out of scope, said on #599).
- Text with only words, `=` and spaces (`issuer=CN = Synthetic CA`), a folded header's
  continuation of base64 symbols, and a comment holding an unspaced 40-hex fingerprint stay
  needles whole: that exact line is over-redacted, rare, never a leak.
- A line with other text whose base64 runs are all 32 symbols or fewer is not a needle. No
  private key's body is that short.
