---
yulius-task: 898bca0f-9f9d-4d71-b338-858a223d1297
status: in-progress
---

# SPEC-302: Key header lines are no longer redaction needles (#599)

Status: in-progress
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

- R1. A line of a key block's file is a redaction needle only when, its surrounding spaces
  trimmed, it is not empty and holds only base64 or base64url symbols (`A-Z a-z 0-9 + / = - _`)
  and is not armour (`-----…`). The needle is the trimmed line.
- R2. RFC 1421 header lines (`<token>: <value>`), blank lines and any other text with a space,
  `:` or another symbol outside those alphabets are not needles.
- R3. PEM and OpenSSH body lines (base64) stay needles, including the PGP checksum line (`=…`); the
  whole stored value stays a needle as before.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Spec for base64-only key needles
- [ ] Regression test with a synthetic encrypted PEM fails
- [ ] key_lines keeps only base64 body lines
- [ ] ADR, SPEC-186 and changes record the rule
- [ ] Independent review answered and gates green
- [ ] Merged into dev on green CI

## Acceptance criteria

- `key_lines` of a synthetic encrypted PEM (`Proc-Type`, `DEK-Info`, a blank line, base64 body)
  returns only its body lines.
- `key_lines` of a bundle with text between its blocks and of a synthetic PGP block with
  `Version:`/`Hash:` headers returns only base64 lines.
- The existing bundle and OpenSSH tests still give every body line.

## Test plan

- Unit test in `dotenv.rs` (`header_lines_and_blank_lines_are_not_key_lines`): synthetic
  encrypted PEM, PGP block with headers and checksum, bundle with an openssl `subject=` line in
  between, a body line with trailing spaces. Fails before the fix (headers returned).
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
- D3. Out of scope: public certificate body lines in a bundle (base64 too, so still needles and
  over-redacted). Said on #599.
