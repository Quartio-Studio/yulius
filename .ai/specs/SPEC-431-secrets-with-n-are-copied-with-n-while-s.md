---
yulius-task: a9aefba3-c8cb-46dc-bce9-96cb7fee5cba
status: in-progress
---

# SPEC-431: A revealed agent-secret key is copied with its line breaks

Status: in-progress
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
  (`dotenv::key_block`: the `.env` copy, `secret_verify`), except the reveal:
  `reveal_agent_secret` (`crates/yulius-app/src/vault/agents.rs`) returns the stored form, and the
  page shows and copies it as returned.
- Why tests missed it: the reveal tests use one-line values (`STRIPE`, passwords) and a flattened
  key (which is kept as stored); none reveals an escaped key block.
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
- [ ] Write the fix spec and the regression test that fails
- [ ] Fix the cause until the regression test and every gate pass
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- A key block stored with literal `\n` is revealed with real line breaks and exactly one final break,
  for `show` and for `copy`.
- A value with a literal `\n` that is not a key block is revealed unchanged.
- The existing reveal tests (flattened key kept as stored, passwords) still pass.

## Test plan

- `secrets::a_revealed_key_block_comes_back_as_its_key_file` (HTTP, `crates/yulius/tests/secrets.rs`):
  stores an escaped synthetic key and a one-line text holding `\n`, reveals both for show and copy;
  fails before the fix (literal `\n` returned).
- Gate: `scripts/dev ci`.

## Decisions

- [ADR-0275](../../docs/decisions/0275-a-revealed-agent-secret-key-is-its-key-file.md): the server
  reveals a key block as its file, with the same rule as the `.env` copy, rather than the page
  unescaping it or every `\n` being turned into a break.
