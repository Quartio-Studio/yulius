# 0275. A revealed agent-secret key is its key file

## Status

Accepted. Specified in [SPEC-431](../../.ai/specs/SPEC-431-secrets-with-n-are-copied-with-n-while-s.md);
task #413, fixes #1096. Builds on [ADR-0156](0156-a-pasted-multi-line-secret-is-stored-with-escaped-line-breaks.md)
(a pasted key is stored with literal `\n`) and [ADR-0150](0150-the-superagent-uses-agent-secrets-as-a-task-does.md)
(a key block is written to a `.env` as its file).

## Context

An agent secret never spans lines, so a key is stored with each line break written as literal `\n`.
The `.env` copy and `secret_verify` read that form back as the key file (`dotenv::key_block`), but
the reveal returned it as stored: a person who pressed **Show** and **Copy value** got
`-----BEGIN …-----\nMII…` on one line, which is not a usable key.

## Decision

`POST /api/agent-secrets/{id}/reveal` returns a value that `dotenv::key_block` reads as a key block
as that key file (real line breaks, one final break), for `show` and `copy`. Any other value, one
holding a literal `\n` included, and an older flattened key are returned as stored. The page shows
the value with its breaks (`white-space: pre-wrap`) and copies it as returned.

## Alternatives

- Unescaping in the page before copying: a second copy of the key-block rule in JavaScript that
  could drift from the server's, and the Show box would still display the stored form.
- Turning every literal `\n` into a line break: a one-line JSON (a service account holding
  `"-----BEGIN …\n…"` inside a string) would become invalid JSON; ADR-0156 rejected the same for
  pasting.
- A separate `file` field next to `value`: two forms of one secret for the page to choose from, and
  the stored one-line form is of no use to a person.

## Consequences

- Show, Copy value and the `.env` copy give the same bytes for a key.
- The reveal no longer shows how a key is stored; editing replaces the value anyway, and the page
  escapes a pasted key again on save.
- API clients revealing a key get real line breaks where they got `\n`.
