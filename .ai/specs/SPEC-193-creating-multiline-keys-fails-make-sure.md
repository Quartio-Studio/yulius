---
yulius-task: d14805d5-b16c-4a05-84d8-e78ab1b3e874
status: in-progress
---

# SPEC-193: A multi-line agent secret pasted into the Secrets page keeps its line breaks

Status: in-progress
Date: 2026-10-16
Owner: Yulius task #179 (fixes #591)
Scope authorization: task #179, requested by MrFishPL: "Creating multiline keys fails. Make sure
value can be multiline", with another agent's report and its five "done when" points.

## Goal

A person pastes a multi-line key (PEM, OpenSSH) into the Value field of "Add agent secret" or
"Edit", and the vault stores it in the form the rest of Yulius reads as a key: one line with
literal `\n` (an agent secret never spans lines). `secret_verify` then answers `escaped_newlines`
for the original key file and `secret_copy_to_env` writes the key file back. A key stored
flattened before the fix is not rewritten; its managers see that it must be pasted again.

## Context

- Symptom (#591): a pasted key is stored with every line break turned into a space, silently.
  Reproduced by another agent with four keys (two `BEGIN PRIVATE KEY` PEMs, two `BEGIN OPENSSH
  PRIVATE KEY`): each stored value matched only its key with line breaks replaced by spaces.
- Root cause: `agentForm` in `web/assets/pages/secrets.js` renders the value as
  `<input type="password">`, a single-line control. The browser rewrites pasted line breaks
  before the page reads `value` (Chrome replaces them with spaces; the HTML value sanitisation of
  a single-line input strips them). The server never sees a line break.
- The rest already handles the escaped form: `check_agent_value` refuses real line breaks;
  `dotenv::key_block` writes a key stored with literal `\n` as its file, final line break
  included (SPEC-186 SAV-005, ADR-0150); `secret_verify` answers `escaped_newlines` for the file
  with LF or CRLF breaks and with or without a final one (SPEC-178, ADR-0144). Tests prove both for
  values sent with literal `\n` through the API (`cockpit.rs`, `secrets_env.rs`).
- Why tests missed it: every test sends the escaped value straight to the API; no test pastes a
  multi-line value into the page, and a flattened key passes `check_agent_value` (spaces only).

## Requirements

- MLK-001 (paste): Pasting a key block (once blank lines around it are dropped, its first line
  is `-----BEGIN <label>-----` and its last `-----END <label>-----`, at least three lines) with
  LF, CRLF or CR line breaks into the agent secret Value field, in Add and in Edit, inserts it with
  each break written as literal `\n`, at the cursor or over the selection, as one undo step. Spaces
  around each line and blank lines around the key are dropped (a key file's final line break: the
  `.env` copy adds it back, `secret_verify` accepts the file with it). Any other text (one line, an
  already escaped value, pretty-printed JSON whose `\n` would break it) pastes as the browser
  pastes it (review round 1).
- MLK-002 (refuse): Creating or updating an agent secret whose value is a key flattened onto one
  line (it opens with `-----BEGIN <label>-----` and closes with `-----END <label>-----`, the same
  label, with something between and no literal `\n`) is refused with a message saying its line
  breaks were lost and how to keep them. A one-line value is never a usable key file.
- MLK-003 (mark): Secrets stored flattened before the fix are not rewritten. The agent secrets list
  says `flattened: true` for one, to the people who manage it only (it is worked out from the
  decrypted value; nobody else learns anything about the value), and the Secrets page marks its
  card: a "Paste again" badge and a line saying its line breaks were lost and to Edit it and paste
  the key again.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Pasted line breaks are stored as \n in Add and Edit
- [x] Flattened keys are refused on save and marked on the Secrets page
- [x] Real PEM and OpenSSH keys round-trip through secret_verify and the .env copy
- [ ] Fix the cause until the regression test and every gate pass
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- Pasting a multi-line PEM and an OpenSSH key (LF and CRLF, with a final line break) in Add and in
  Edit stores them as `\n`-escaped single lines (e2e, read back through Show).
- `secret_verify` with the original file's SHA-256 answers `escaped_newlines` and
  `secret_copy_to_env` writes a key `ssh-keygen -y -f` and `openssl pkey` accept (existing
  integration tests for the escaped form; real generated keys in the Evidence).
- An already escaped value pastes unchanged; CRLF, CR and a final line break are covered.
- Creating or updating with a flattened key is refused (domain and API tests).
- A secret stored flattened before the fix keeps its value, shows `flattened: true` to its
  managers only, and its card says to paste it again (API and e2e tests).

## Test plan

- Domain unit tests: `flattened_key` (spaces, stripped breaks, mismatched labels, escaped keys,
  ordinary values) and `check_agent_value` refusing a flattened key.
- Integration (`crates/yulius/tests/secrets.rs`): the API refuses a flattened key on create and on
  update; a record stored flattened directly in the store lists `flattened: true` to a manager,
  `false` to a reader, and keeps its value.
- e2e (`e2e/tests/secrets.e2e.ts`, mock): paste events carrying a CRLF PEM in Add and an LF
  OpenSSH key with a final break in Edit; the revealed values are the escaped forms; the mock's
  flattened secret shows "Paste again" until it is edited; an already escaped value, JSON and text
  after the END line paste as the browser pastes them; an indented key with CR breaks and blank
  lines around it is trimmed; Ctrl+Z undoes the paste in one step.
- Manual evidence: real keys from `ssh-keygen` and `openssl genpkey`, escaped by the page's
  function, stored through the API, checked with `secret_verify` and copied with
  `secret_copy_to_env`, then accepted by `ssh-keygen -y -f` and `openssl pkey`.

## Decisions

- [ADR-0156](../../docs/decisions/0156-a-pasted-multi-line-secret-is-stored-with-escaped-line-breaks.md):
  the page escapes pasted line breaks (rather than a textarea, which would show the value, or a
  server that accepts real breaks, which never receives them); the server refuses a flattened key;
  older flattened secrets are marked, not repaired (the lost breaks cannot be told apart from
  spaces in every key, e.g. an encrypted PEM's headers).

## Review

Independent review in a fresh session (round 1, head `6d766e16`): request changes, all findings
own code, all fixed in this change.

| # | Severity | Finding | Answer |
| --- | --- | --- | --- |
| 1 | medium | Every multi-line paste was escaped: pretty-printed JSON became invalid | Only a key block is escaped; other text pastes as before (e2e: JSON, text after END, escaped value) |
| 2 | low | Spaces around a pasted key's lines left it unusable, unwarned | Each line is trimmed (e2e: indented key, CR breaks, `\n \n` after it) |
| 3 | low | The Evidence the ADR cites did not exist | Evidence section below |
| 4 | nit | The paste could not be undone | `execCommand('insertText')`, `setRangeText` as fallback (e2e: Ctrl+Z) |
| 5 | nit | `escapeLineBreaks` exported, unused | Module-private `escapedKey` |
| 6 | nit | Mock checked a flattened key before the name and length | Same order as the server |
| 7 | nit | PATCH row did not say a flattened key is refused | Added |

## Evidence

- Regression tests failed before the fix (commit `07b461aa`): the domain tests
  `a_key_flattened_onto_one_line_is_recognised` and `a_flattened_key_is_refused`, the integration
  test `secrets::a_flattened_key_is_refused_and_an_older_one_is_marked_for_its_managers` (201 for a
  flattened key) and the e2e test (`expected "" to be "-----BEGIN MOCK PRIVATE KEY-----\\n…"`).
  The review-round cases failed before their fix too (commit `e6133bcb`: JSON was escaped).
- Real keys (generated in the session scratchpad, never in the repository): `ssh-keygen -t
  ed25519`, `ssh-keygen -t rsa -b 2048`, `openssl genpkey` ed25519 and RSA 2048, plus CRLF copies
  of the OpenSSH ed25519 and PEM RSA keys. Each was escaped by the page's own function (extracted
  from `secrets.js` and run in Node), stored through the API, checked with `secret_verify` (the
  original file's SHA-256) and copied with `secret_copy_to_env` in a throwaway integration test
  (not committed). All six answered `escaped_newlines` with 3, 28, 28, 7, 7 and 27 lines. The
  `.env` value was byte-identical to the LF file. `ssh-keygen -y -f` and `openssl pkey` accepted
  every written key, and their public keys matched the originals'. The revised function (review
  round 1) produces the same six stored values.
- `cargo nextest run -p yulius-domain`: 344 passed. The new integration test passed.
- `scripts/dev ci` (round 0 head): exit 0, 1966 tests passed, 18 skipped.
- `npx e2e run` (full suite, round 0 head): 575 passed, 1 skipped. `npx e2e run
  tests/secrets.e2e.ts` after the review fixes: 12 passed.
- `scripts/dev security`: exit 2, unrun. Vidoc rejects the key (HTTP 401) since 2026-10-14
  (#470, ADR-0104). This is not a pass. Yulius CI runs it as the `security` job.
