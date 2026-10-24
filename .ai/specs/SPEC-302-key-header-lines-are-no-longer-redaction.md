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
- R2. A line with other text (RFC 1421 headers `<token>: <value>`, a bundle's `subject=…`) is a
  needle whole when it holds a run of more than 32 consecutive base64 symbols: a body joined onto
  its headers, by spaces or glued, or split by a separator that is no whitespace (a zero-width
  space), stays a needle (review rounds 2 and 3). Headers alone have no such run (an IV is at
  most 32 hex digits) and are not needles.
- R3. PEM and OpenSSH body lines (base64) stay needles; the PGP checksum line (`=…`) is returned
  by `key_lines` but, at 5 symbols, falls under `MIN_NEEDLE_LEN` (it is no key material). The
  whole stored value stays a needle as before.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Spec for base64-only key needles
- [x] Regression test with a synthetic encrypted PEM fails
- [x] key_lines keeps only base64 body lines
- [x] ADR, SPEC-186 and changes record the rule
- [x] Independent review answered and gates green
- [x] Pull request ready to merge on green CI

## Acceptance criteria

- `key_lines` of a synthetic encrypted PEM (`Proc-Type`, `DEK-Info`, a blank line, base64 body)
  returns only its body lines.
- `key_lines` of a bundle with text between its blocks and of a synthetic PGP block with
  `Version:`/`Hash:` headers returns only base64 lines.
- A line with headers and a body joined to them (by spaces, glued, or across a zero-width space)
  stays a needle; `DEK-Info: AES-256-CBC,<32 hex>` and a 32-symbol header word do not, a
  33-symbol run does.
- The existing bundle and OpenSSH tests still give every body line.

## Test plan

- Unit test in `dotenv.rs` (`header_lines_and_blank_lines_are_not_key_lines`): synthetic
  encrypted PEM, PGP block with headers and checksum, bundle with an openssl `subject=` line in
  between, a body line with spaces around it and one joined by spaces. Fails before the fix
  (headers returned), and the joined line failed the first fix (review round 1).
- Unit test `a_line_with_a_key_length_base64_run_stays_a_needle`: a body joined to its headers by
  spaces and glued, a PGP comment running into a body, zero-width-space, BOM and NBSP
  separators, the 32/33-symbol threshold, and header-only lines. Fails before the fix (review
  rounds 2 and 3).
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
- D2b. Over 32 consecutive base64 symbols marks key material on a line with other text: body
  lines are 64 (PEM) or 70 (OpenSSH) symbols, while an IV is at most 32 hex digits and header
  values hold a comma, colon, dot or space. Such a line is kept whole, not cut: the needle then
  matches only that whole line (what a `.env` copy shows), never a header alone elsewhere, and no
  tokenisation can miss a body (review round 3 found glued and zero-width-space shapes the
  round-2 token runs missed).
- D3. Text made only of words, `=` and spaces (`issuer=CN = Synthetic CA`), a folded RFC 1421
  header's continuation of base64 symbols, or a comment holding an unspaced 40-hex fingerprint
  stays a needle whole: over-redaction of that exact line, never a leak.
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
- `scripts/dev ci`: exit 0, 2603 tests passed, 16 skipped after the round-3 fix (also green after rounds 1 and 2).
- `scripts/dev security`: unrun, Vidoc out of quota (HTTP 429, exit 2) for both changed code
  files (ADR-0104).
- Review round 2 (fresh session): request changes, one major in own code: a body joined onto its
  header line (`Proc-Type: … DEK-Info: … <body>`) was dropped whole and would leak. Fixed
  test-first: `a_body_joined_onto_a_header_line_keeps_its_base64_runs` failed (`key_lines`
  returned `[]`), then passed with base64 runs kept from mixed lines. One minor answered: ADR-0215
  now says the joined shape comes through the API or an agent (the Secrets page escapes the
  clipboard's breaks) and that the whole-value needle never matches a `.env` copy.
- Review round 3 (fresh session): request changes, one major in own code: a body glued to its
  header (no whitespace) or split by a zero-width space lost its needle. Fixed test-first:
  `a_line_with_a_key_length_base64_run_stays_a_needle` failed against the round-2 token runs,
  then passed once a line with a run over 32 base64 symbols is kept whole (simpler than the
  runs, which were removed). Minors answered: threshold pinned (32 not kept, 33 kept), R3 says the
  checksum falls under `MIN_NEEDLE_LEN`, acceptance criteria and test plan name the joined shape.
  Nits: the "no header word" claim softened (D3), the last Plan item renamed to what is true
  before the merge, wording fixed.
- Not tested: a real encrypted key from `openssl` (fixtures are synthetic by rule).
