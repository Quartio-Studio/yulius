---
yulius-task: 6a31666a-6dc5-4f49-980c-7f43df00e5c1
status: in-progress
---

# SPEC-381: the proxy's Vidoc-off refusal stays within the scanner's message cut

Status: in-progress
Date: 2026-11-02
Owner: Yulius task #363
Scope authorization: task #363, Fixes #981 (a nit left at the review round limit of task #338,
SPEC-356, ADR-0242).

## Goal

A test fails when the egress proxy's refusal toward Vidoc (`VIDOC_OFF_REFUSAL`) outgrows the
part of an error body the scanner keeps, so the scan never prints a refusal cut short without
anyone noticing. ADR-0242's one unwrapped Consequences line is wrapped at 100 columns.

## Context

- `VIDOC_OFF_REFUSAL` (`crates/yulius-app/src/vidoc_key.rs`) is 194 characters. The proxy answers
  it as `format!("{message}\n")` (`crates/yulius-egress/src/proxy.rs`), a 195-character body.
- `_message` in `scripts/vidoc_scan.py` keeps the first 200 characters of an error body, JSON
  message or plain text, as a bare `[:200]` twice. The scanner prints the kept text as the reason
  the scan did not run (`Usage`), after removing the leading "Yulius: ".
- `vidoc_switch::the_scanner_recognizes_the_proxys_refusal` (`crates/yulius/tests/vidoc_switch.rs`)
  checks the marker and that the scanner's test uses the proxy's text, not its length. Matching
  survives any cut because the marker comes first; only the end of the reason would be lost.
- ADR-0242's Consequences bullet ending "...nothing is rescanned by itself." has a 159-column line.
  The Status line (104 columns, with the spec link) is like most ADRs' and stays as it is.

## Requirements

- **VRC-001** The scanner's cut of an error body's message is one named constant,
  `MESSAGE_LIMIT = 200` in `scripts/vidoc_scan.py`, used by both branches of `_message`; its
  behavior is unchanged.
- **VRC-002** A Rust test reads `MESSAGE_LIMIT` from the scanner's source (`SCANNER`) and fails
  when the proxy's body, `VIDOC_OFF_REFUSAL` with its line end, is longer than it, or when the
  constant is missing.
- **VRC-003** The scanner's own test of the refusal asserts that its whole reason, to the last
  word, reaches the output.
- **VRC-004** ADR-0242's 159-column line is wrapped at 100 columns with its words unchanged.

## Plan

- [ ] Read the product context and related work, then write and validate the spec
- [ ] Rust test fails when the refusal outgrows the scanner's named cut
- [ ] Scanner test sees the whole refusal and the ADR-0242 line is wrapped
- [ ] Gates, review and delivery

## Acceptance criteria

- `vidoc_switch::the_scanner_recognizes_the_proxys_refusal` asserts the refusal's body fits
  `MESSAGE_LIMIT`, and fails with the refusal lengthened past it (VRC-002).
- `test_a_refusal_by_yulius_because_vidoc_is_off_says_so` asserts the reason's end is printed
  (VRC-003).
- No line of ADR-0242 but its Status line is over 100 columns; the text is the same (VRC-004).

## Test plan

- Before `MESSAGE_LIMIT` exists, the extended Rust test fails (no constant in `SCANNER`); after,
  it passes. A temporary longer refusal (6 more characters) makes it fail, then is reverted.
- `python3 -m unittest scripts.test_vidoc_scan` (the refusal test and the rest).
- Run `vidoc_switch::` with nextest, then `scripts/dev ci`.

## Decisions

- **A test on the length, not a scanner that keeps a Yulius refusal whole.** Keeping a body whole
  because it starts with the marker would let any answer that carries the marker print an
  unbounded reason; the cut bounds the output of every error. The refusal is Yulius's own text,
  so a test that names the limit is enough and keeps the scanner simpler. Reversible; no ADR.
- **The limit is read from the scanner's source.** The Rust test already reads `SCANNER` for the
  marker; a named constant makes the limit one place both sides agree on, instead of a 200
  repeated in Rust.

## Evidence

(filled in at the end)
