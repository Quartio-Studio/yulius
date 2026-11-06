---
yulius-task: a9aefba3-c8cb-46dc-bce9-96cb7fee5cba
status: done
---

# SPEC-431: A revealed agent-secret key is copied with its line breaks

Status: done
Date: 2026-11-06
Owner: Yulius task #413
Scope authorization: task #413, asked by the project's owner ("secrets with \n are copied with \n
while should be copied as linebreaks"). Fixes #1096.

## Goal

A person who reveals an agent secret that is a key (PEM, OpenSSH) and copies it gets the key file:
real line breaks and one final break, as `secret_copy_to_env` already writes it into a `.env`.

## Context

- Symptom: on the Secrets page, **Show** then **Copy value** on an agent secret holding a pasted key
  copies `-----BEGIN …-----\nMII…\n-----END …-----` with literal `\n`; pasted into a file it is not
  a usable key.
- Root cause: an agent secret never spans lines, so a pasted key is stored with each break written
  as literal `\n` (ADR-0156, SPEC-193). Every reader of that form turns it back into the key file
  (the `.env` copy through `dotenv::key_block`, `secret_verify` by reading each `\n` as a break),
  except the reveal:
  `reveal_agent_secret` (`crates/yulius-app/src/vault/agents.rs`) returns the stored form, and the
  page shows and copies it as returned.
- Why tests missed it: the Rust reveal tests use one-line values (`STRIPE`, passwords) and a
  flattened key (kept as stored), and the e2e test that reveals pasted keys
  (`secrets.e2e.ts`, SPEC-193) asserted the escaped form, with the mock returning it as stored.
- What else the cause touches: the **Show** box displays the same one-line form. Team secrets are
  unaffected: their values hold real line breaks (ADR-0214).

## Requirements

- RVK-001: `POST /api/agent-secrets/{id}/reveal` (purpose `show` or `copy`) returns a value that is
  a key block stored with literal `\n` as the key file it stands for (`dotenv::key_block`: real
  line breaks, one final break), so both the Show box and its copy carry the line breaks.
- RVK-002: any other value is returned as stored, a `\n` in it included (a one-line JSON's escapes
  mean something to its readers; ADR-0156), and so is an older flattened key.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- A key block stored with literal `\n` is revealed with real line breaks and exactly one final break,
  for `show` and for `copy`.
- A value with a literal `\n` that is not a key block is revealed unchanged.
- The existing reveal tests (flattened key kept as stored, passwords) still pass.

## Test plan

- `secrets::a_revealed_key_block_comes_back_as_its_key_file` (HTTP, `crates/yulius/tests/secrets.rs`):
  stores an escaped synthetic key and a one-line text holding `\n`, reveals both for show and copy;
  fails before the fix (literal `\n` returned).
- `secrets.e2e.ts` "a pasted multi-line key keeps its line breaks as \n…": Show and Copy value of a
  pasted PEM and OpenSSH key give the key file (the mock reveals as the server does).
- Gate: `scripts/dev ci`.

## Decisions

- [ADR-0275](../../docs/decisions/0275-a-revealed-agent-secret-key-is-its-key-file.md): the server
  reveals a key block as its file, with the same rule as the `.env` copy, rather than the page
  unescaping it or every `\n` being turned into a break.

## Evidence

- Red: `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^secrets::a_revealed_key_block/)'`
  at e59d5f66 failed: `left: "-----BEGIN SYNTHETIC KEY-----\\nc3lu…"` (literal `\n`), right: the key file.
- Green: with the fix, the `secrets*` integration tests 68/68 passed; after review round 1, the
  `dotenv` unit tests and `secrets*` integration tests 81/81 passed.
- e2e: `npx e2e run tests/secrets.e2e.ts` failed on the new expectation before the mock change
  (`expected "-----BEGIN MOCK PRIVATE KEY-----\\nbW9j…"` to be the key file) and passed 28/28 after it.
- Gate: `scripts/dev ci` exit 0 at 22999554 and again at 8dee57d2 (3168 tests passed, 16 skipped;
  lint, macOS, deps, shear, docs).
- Security: `yulius security-scan` printed "Security scan disabled" (Vidoc is off for this project):
  not scanned, not passed.
- Review (fresh session, `om-code-review`): round 1 requested changes (own: mock/e2e still pinned the
  escaped form; `key_block`'s unescaped copy unwiped; two doc nits), all fixed in 9b10b518, 957c7831,
  8dee57d2; round 2 approved 8dee57d2.
- Not tested: a real browser against a running `yulius serve` with a real key (covered by the HTTP
  test and the mock-mode e2e); the wipe of `key_block`'s copy is not observable by a test.
