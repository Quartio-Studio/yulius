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

Public lines of a stored key block stop being redaction needles: an encrypted PEM's
`Proc-Type: 4,ENCRYPTED` and `DEK-Info: …`, PGP armour's `Version: …` and `Hash: SHA256`, blank
lines and OpenSSL's `subject=`/`issuer=` between a bundle's blocks no longer turn into
`[secret:NAME]` in a sandbox's transcripts and tool results. Every line that may hold key
material stays a needle.

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
  needles.
- R2. A trimmed line of public text of a known shape is not a needle: an RFC 1421 header
  (`<token>: <value>`, the token letters, digits and `-`, the value not empty) or a line starting
  `subject=` or `issuer=`, unless it is only base64 symbols and whitespace or holds a run of more
  than 32 consecutive base64/base64url symbols (a body joined onto it, by spaces or glued).
- R3. Every other line stays a needle, trimmed, as before this change: base64 body lines (PEM,
  OpenSSH, PGP), a body joined by spaces or split by a zero-width space, a key's OpenSSL text dump
  (colon-separated hex), text between a bundle's blocks of any other shape. The PGP checksum line
  (`=…`) is returned but, at 5 symbols, falls under `MIN_NEEDLE_LEN`. The whole stored value
  stays a needle as before.

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
- `key_lines` of a bundle with `subject=`/`issuer=` between its blocks and of a synthetic PGP
  block with `Version:`/`Hash:` headers returns only their body lines.
- A header line with a body joined to it (by spaces or glued) stays a needle;
  `DEK-Info: AES-256-CBC,<32 hex>` and a header with a 32-symbol word do not, one with a
  33-symbol run does.
- A key's colon-hex dump between a bundle's blocks stays a needle, line by line.
- The existing bundle and OpenSSH tests still give every body line.

## Test plan

- Unit test in `dotenv.rs` (`header_lines_and_blank_lines_are_not_key_lines`): synthetic
  encrypted PEM, PGP block with headers and checksum, bundle with OpenSSL `subject=`/`issuer=`
  lines in between, a body line with spaces around it and one joined by spaces. Fails before the
  fix (headers returned), and the joined line failed the first fix (review round 1).
- Unit test `a_line_with_a_key_length_base64_run_stays_a_needle`: a body joined to its headers by
  spaces and glued, a PGP comment running into a body, zero-width-space, BOM and NBSP
  separators, the 32/33-symbol threshold, and header-only lines (review rounds 2 and 3).
- Unit test `a_key_dump_between_blocks_stays_a_needle`: a bundle holding a key's
  `privateExponent:` colon-hex dump, `Private-Key:`/`localKeyID:` headers and `subject=`; the
  hex lines stay needles (review round 4).
- Integration test `secrets_env::an_encrypted_pem_redacts_its_body_not_its_headers`: an
  encrypted PEM stored through the API; `redact_for_sandbox` keeps its `Proc-Type`/`DEK-Info`
  lines and replaces its body. Fails before the fix.
- Existing tests `a_key_and_certificate_bundle_is_written_as_its_file` and
  `a_key_stored_with_escaped_line_breaks_is_written_as_its_file` unchanged and green.
- Full gate `scripts/dev ci`.

## Decisions

- D1. Name the public shapes and keep every other line, not guess which lines hold key material.
  The first three rules (base64 lines only; then with spaces; then with base64 runs) each missed
  a shape of key material found by review: a body joined by spaces, glued to its header or split
  by a zero-width space, a key's colon-hex dump. Dropping only named public shapes cannot drop
  material outside them. The core team's base64-only suggestion is the rejected first rule;
  the issue's header rule is kept, with the run check. Recorded in ADR-0215.
- D2. Lines are emitted trimmed: the trimmed text is part of the line a `.env` shows, so it still
  matches; needles match only whole lines, so a header kept inside one is redacted only there.
- D3. A run of more than 32 base64 symbols keeps a header line a needle: an IV is at most 32 hex
  digits, a body line is 64 (PEM) or 70 (OpenSSH) symbols.
- D4. Over-redaction left, never a leak: a public certificate's body (out of scope, said on
  #599), a PGP signed message's text, a dump's labels (`privateExponent:`, `Bag Attributes`) and
  public values, a folded header's continuation.

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
- `scripts/dev ci`: green after rounds 1 and 2. After round 3 its tests passed (2603) but `cargo doc`
  failed: `key_lines`' doc linked the private `PUBLIC_RUN_LEN`; the doc now names the length. With
  `origin/dev` merged (#886): exit 0, 2606 tests passed, 16 skipped.
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
- Review round 4 (fresh session): request changes, one major in own code: a key's OpenSSL text
  dump (`privateExponent:` and colon-separated hex) between a bundle's blocks was dropped and
  would leak. Fixed test-first: `a_key_dump_between_blocks_stays_a_needle` failed (hex lines
  missing), then passed once the rule was inverted to drop only named public shapes (D1); every
  earlier test passes unchanged. Nits: SPEC-186 and architecture say the rule in short; ADR-0215
  lists `Bag Attributes` with the over-redacted shapes (renamed for the new rule).
- Not tested: a real encrypted key from `openssl` (fixtures are synthetic by rule).
