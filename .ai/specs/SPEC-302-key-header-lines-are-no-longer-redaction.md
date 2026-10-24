---
yulius-task: 898bca0f-9f9d-4d71-b338-858a223d1297
status: done
---

# SPEC-302: Key header lines are no longer redaction needles (#599)

Status: done
Date: 2026-10-24
Owner: Yulius task #288
Scope authorization: task #288: "Fix it in one pull request into `dev` whose description says
`Fixes #599`. […] the fix must not drop a line that carries key material (base64 body lines and
OpenSSH body lines stay needles)." Public certificate body lines stay out of scope.

## Goal

Only the lines of a stored key block that carry key material become redaction needles. An
encrypted PEM's `Proc-Type: 4,ENCRYPTED` and `DEK-Info: …`, PGP armour's `Version: …` and
`Hash: SHA256`, blank lines and the text between a bundle's blocks no longer turn into
`[secret:NAME]` in a sandbox's transcripts and tool results.

## Context

- Symptom (#599): once an encrypted PEM is stored as an agent secret, any text in that sandbox
  holding `Proc-Type: 4,ENCRYPTED` shows `[secret:NAME]` in its place.
- Code path: `PlainMatcher` construction in `matcher_for` (`crates/yulius-app/src/vault/mod.rs`)
  adds `needles(line)` for each line of `dotenv::key_lines(value)` (SPEC-186 SAV-005: a key copied
  into a `.env` shows one line per line). `key_lines` (`crates/yulius-domain/src/vault/dotenv.rs`)
  keeps every line of the key file except its `-----` armour. `needles` drops forms under
  `MIN_NEEDLE_LEN` (8 bytes), but header lines are longer, so they survive.
- Why the tests missed it: every `key_lines` fixture had pure base64 bodies.
- Widened by SPEC-193 (core team comment on #599): `key_block` takes any END label, so a key and
  certificate bundle is a key block, and the text between its blocks (`subject=CN = …`) became
  needles too. Every case over-redacts; none leaks.

## Requirements

- R1. Blank lines and armour lines (`-----BEGIN|END <label>-----`) of a key block's file are not
  needles. Another line, trimmed, of only base64 or base64url symbols (`A-Z a-z 0-9 + / = - _`)
  and whitespace is a needle whole: a stored key whose body breaks became spaces still has its
  body on a line (review round 1).
- R2. A line with other text (RFC 1421 headers `<token>: <value>`, a bundle's `subject=…`) gives a
  needle for each run of whitespace-separated base64 tokens holding a token of at least 32
  symbols, cut from the line as it stands: a body joined onto its headers stays a needle (review
  round 2); headers alone give none.
- R3. PEM and OpenSSH body lines (base64) stay needles, including the PGP checksum line (`=…`); the
  whole stored value stays a needle as before.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Spec for base64-only key needles
- [x] Regression test with a synthetic encrypted PEM fails
- [x] key_lines keeps only base64 body lines
- [x] ADR, SPEC-186 and changes record the rule
- [x] Independent review answered and gates green
- [x] Merged into dev on green CI

## Acceptance criteria

- `key_lines` of a synthetic encrypted PEM (`Proc-Type`, `DEK-Info`, a blank line, base64 body)
  returns only its body lines.
- `key_lines` of a bundle with text between its blocks and of a synthetic PGP block with
  `Version:`/`Hash:` headers returns only base64 lines.
- The existing bundle and OpenSSH tests still give every body line.

## Test plan

- Unit test in `dotenv.rs` (`header_lines_and_blank_lines_are_not_key_lines`): synthetic
  encrypted PEM, PGP block with headers and checksum, bundle with an openssl `subject=` line in
  between, a body line with spaces around it and one joined by spaces. Fails before the fix
  (headers returned), and the joined line failed the first fix (review round 1).
- Integration test `secrets_env::an_encrypted_pem_redacts_its_body_not_its_headers`: an
  encrypted PEM stored through the API; `redact_for_sandbox` keeps its `Proc-Type`/`DEK-Info`
  lines and replaces its body. Fails before the fix.
- Existing tests `a_key_and_certificate_bundle_is_written_as_its_file` and
  `a_key_stored_with_escaped_line_breaks_is_written_as_its_file` unchanged and green.
- Full gate `scripts/dev ci`.

## Decisions

- D1. Base64-only rule (the core team's suggestion) over the issue's narrower "skip
  `<token>: <value>` and blank lines": it also drops a bundle's in-between text and PGP
  signed-message text, and every key body format a key block can hold (PEM, PKCS#8, OpenSSH, PGP)
  is base64. The base64url symbols `-` and `_` are kept too so a body in that alphabet never loses
  its needles (an over-redaction at worst). Recorded in ADR-0215.
- D2. Lines are classified and emitted trimmed: a body line with stray spaces keeps a needle (the
  trimmed text is part of the line a `.env` shows), never dropped.
- D2b. 32 symbols marks key material on a mixed line: body lines are 64 (PEM) or 70 (OpenSSH)
  symbols and no header word is that long; header values (`4,ENCRYPTED`, `AES-128-CBC,<iv>`)
  hold a comma. Whitespace is Unicode whitespace, so a non-breaking space also separates.
- D3. Text made only of words, `=` and spaces (`issuer=CN = Synthetic CA`) or a folded RFC 1421
  header's continuation of base64 symbols stays a needle: over-redaction, never a leak.
- D4. Out of scope: public certificate body lines in a bundle (base64 too, so still needles and
  over-redacted). Said on #599.

## Evidence

- Root cause: `key_lines` kept every line not starting with `-----`; `matcher_for` makes each a
  needle, and the headers pass `MIN_NEEDLE_LEN`. Fixtures had only base64 bodies.
- Regression test first (d02fe7d1): `cargo test -p yulius-domain --lib vault::dotenv` failed,
  `key_lines` returned `Proc-Type: 4,ENCRYPTED`, `DEK-Info: …` and `""`.
- Integration test with the dotenv fix reverted: `cargo nextest run -p yulius -E
  'binary_id(yulius::main) & test(/^secrets_env::/)'` failed at
  `an_encrypted_pem_redacts_its_body_not_its_headers` (headers redacted); with the fix, the
  `secrets` and `secrets_env` files: 41 passed.
- Review round 1 (fresh session): request changes, one major in own code: a body line joined by
  spaces (accepted by `key_block`) lost its needle and would leak in a `.env` copy. Fixed
  test-first: the extended test failed (the joined line missing), then passed with whitespace
  allowed inside a line. Nits answered: architecture paragraph rewrapped; folded header
  continuations noted as over-redaction (D3, ADR-0215).
- `scripts/dev ci`: exit 0, 2602 tests passed, 16 skipped (after the round-1 fix).
- `scripts/dev security`: unrun, Vidoc out of quota (HTTP 429, exit 2) for both changed code
  files (ADR-0104).
- Review round 2 (fresh session): request changes, one major in own code: a body joined onto its
  header line (`Proc-Type: … DEK-Info: … <body>`) was dropped whole and would leak. Fixed
  test-first: `a_body_joined_onto_a_header_line_keeps_its_base64_runs` failed (`key_lines`
  returned `[]`), then passed with base64 runs kept from mixed lines. One minor answered: ADR-0215
  now says the joined shape comes through the API or an agent (the Secrets page escapes the
  clipboard's breaks) and that the whole-value needle never matches a `.env` copy.
- Not tested: a real encrypted key from `openssl` (fixtures are synthetic by rule).
