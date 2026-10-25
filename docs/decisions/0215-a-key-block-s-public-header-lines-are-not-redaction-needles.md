# 0215. A key block's public header lines are not redaction needles

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
`Hash: SHA256`), blank lines and, since ADR-0156, OpenSSL's `subject=`/`issuer=` lines between a
bundle's blocks are public, common text longer than the 8-byte `MIN_NEEDLE_LEN`: once such a key
was stored, any transcript or tool result in the sandbox holding them showed `[secret:NAME]`
instead. Nothing leaked; text was over-redacted.

A key block can hold key material in more shapes than base64 body lines. The API (or an agent
writing the value) can store a key whose body breaks are spaces or dropped, with `\n` only
around the armour, so the body shares a line with its headers, glued or not; and a bundle can
hold a key's OpenSSL text dump (`openssl genpkey -text`), whose private exponent and primes are
colon-separated hex. A `.env` copy shows each such line, and the whole stored value's needle
never matches it (it holds literal `\n` where the copy has real breaks), so a line dropped from
`key_lines` that holds key material leaks.

## Decision

Every trimmed line of a key block stays a needle except:

- blank lines and armour lines (`-----BEGIN|END <label>-----`);
- public text of a known shape: a named public header (`<name>: <value>`, the name, in any case,
  one of RFC 1421's `Proc-Type` and `DEK-Info`, OpenPGP armour's `Version`, `Comment`, `Hash`,
  `Charset` and `MessageID`, and OpenSSL's PKCS#12 `localKeyID` and `friendlyName`) or a line
  starting `subject=` or `issuer=`, as long as its value is not empty and holds no `:` and no
  `(0x`, and the line holds no run of more than 32 consecutive base64 or base64url symbols
  (`A-Z a-z 0-9 + / = - _`). Any other `<token>: <value>` line is not public: a passphrase pasted
  beside the key (`Passphrase: …`) or an `xxd` line (`00000000: 3082 …`) stays a needle.
  A `:` in the value is another header or a key's colon-hex dump joined onto it (`Private-Key:
  (…) privateExponent: 2a:9f:…`); `(0x` is OpenSSL's inline integer, which a key's dump joined
  onto a named header would bring (`Version: 1844… (0x…)`, a toy key's prime); an IV in
  `DEK-Info` is at most 32 hex digits, while a key body line is 64 (PEM) or 70 (OpenSSH)
  symbols, so a body joined onto a header keeps the line a needle.

Needles are trimmed lines, matched as substrings: a header kept as part of a needle is redacted
only where the whole trimmed line's text appears (a `.env` copy).

## Alternatives

- **Keep only base64 lines, then base64 lines plus lines with base64 runs** (this change's first
  three review rounds): each missed a shape that holds key material (a body joined by spaces,
  glued to its header or split by a zero-width space, then a key's colon-hex dump) and leaked
  it. Rejected: guessing which lines hold key material can always miss one; naming the public
  shapes cannot drop material outside them.
- **Skip `<token>: <value>` headers and blank lines only** (the issue's wording): a body glued
  onto a header line, a passphrase pasted as `Passphrase: …` and an `xxd` dump line would leak
  (review rounds 3, 5 and 6). Rejected for named headers with the run and `:` checks.
- **Parse each format (PEM, OpenSSH, PGP armour, OpenSSL dumps)**: much more code, and a format
  it does not know would still need a default. Rejected.

## Consequences

- An encrypted PEM's headers, PGP armour headers, blank lines and OpenSSL's `subject=`/`issuer=`
  lines no longer turn into `[secret:NAME]`.
- Other public text inside a key block stays a needle and is over-redacted where that whole
  line appears, never leaked: a public certificate's body (out of scope, said on #599), a PGP
  signed message's text, a dump's labels (`Private-Key: (2048 bit)`, `privateExponent:`,
  `Bag Attributes`) and public values (modulus), a folded header's continuation, a header not
  named above, and a `subject=` with a run over 32 base64 symbols (OpenSSL 1.0's
  `subject=/C=…/O=…/CN=…` with no space or dot).
- A public header whose value holds `:` or `(0x` stays a needle: a PGP `Comment: https://…`,
  `openssl x509 -text`'s `Version: 3 (0x2)`; over-redacted, never leaked.
- A secret written as the value of a named public header, or after `subject=`/`issuer=`, with no
  `:`, no `(0x` and no base64 run over 32 symbols, would not be a needle (a key could only land
  there by a person typing it so). None of the formats checked in review (PEM, PKCS#8, encrypted
  PEM, OpenSSH, PGP armour, OpenSSL `-text` dumps, `asn1parse`, PKCS#12 `-nodes`, PuTTY, xxd)
  writes one; a `hexdump -C` or `od` dump joined by spaces onto a named header would be such a
  line, and no tool writes one inside armour.
