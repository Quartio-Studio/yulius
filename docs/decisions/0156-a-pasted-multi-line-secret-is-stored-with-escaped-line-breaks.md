# 0156. A pasted multi-line secret is stored with escaped line breaks

## Status

Accepted. Specified in [SPEC-193](../../.ai/specs/SPEC-193-creating-multiline-keys-fails-make-sure.md);
task #179, fixes #591. Builds on [ADR-0150](0150-the-superagent-uses-agent-secrets-as-a-task-does.md)
(a key block stored with literal `\n` is written to a `.env` as its file) and
[ADR-0144](0144-the-superagent-reads-the-agent-vault-and-verifies-values-by-hash.md)
(`secret_verify` answers `escaped_newlines`). Widens ADR-0150's key block to any END label (its
SPEC-186 review had asked for matching labels).

## Context

An agent secret never spans lines (`check_agent_value`): it must fit an HTTP header. A multi-line
key (PEM, OpenSSH) is therefore stored with literal `\n`, and the vault reads that form back as
the key file. But the Secrets page's value field is `<input type="password">`, a one-line control:
the browser turns a pasted key's line breaks into spaces (Chrome) or drops them before the page
reads its value. Four keys pasted that way were stored flattened without a word; `secret_verify`
answered `different` for each original file and a `.env` copy would have been an unusable key.

## Decision

- **The field escapes a pasted key.** A paste that is a key block (`-----BEGIN <label>-----` on
  its first line, `-----END <label>-----` on its last, LF, CRLF or CR breaks) is inserted at the
  cursor, or over the selection, undoably, with each break written as literal `\n`; spaces around
  each line and blank lines around the key are dropped (a key file's final break comes back in a
  `.env` copy, and `secret_verify` accepts the file with it). Any other text pastes as the browser
  pastes it: only a key block's `\n` is read back as line breaks (`dotenv::key_block`), and in other
  text, such as pretty-printed JSON, a `\n` outside a string would break it where a space does not.
- **The server refuses a flattened key.** `check_agent_value` refuses a value that opens with
  `-----BEGIN <label>-----`, closes with `-----END <label>-----` (any label), has something between
  and no literal `\n`: a one-line key block is never a usable key file, whatever path it came by (a
  drag, an autofill, the API). The message says how to keep the breaks.
- **Any END label closes a key block.** A key and its certificate in one file (`cat key.pem
  cert.pem`) starts `-----BEGIN PRIVATE KEY-----` and ends `-----END CERTIFICATE-----`: the page
  escapes it, the server refuses it flattened, and `dotenv::key_block` (ADR-0150) writes it to a
  `.env` as its file, so every line between the armours, the certificate's included, is a
  redaction needle (review rounds 2 and 3).
- **Older flattened keys are marked, not repaired.** The agent secrets list says `flattened: true`
  for one, worked out from the decrypted value, to the people who manage it only; the page shows
  "Paste again" and says to edit it and paste the key again.

## Alternatives

- A `<textarea>` for the value: keeps real line breaks, but shows the value on screen (the field is
  masked today) and the server still refuses real breaks; converting them on submit would hide the
  same problem in a different place.
- Accepting real line breaks on the server and escaping them there: the server never receives them,
  since the browser rewrites the paste first. It would help API clients only, which already send
  `\n`.
- Repairing older flattened values (spaces back to line breaks): a space cannot always be told from
  the key's own (an encrypted PEM's `Proc-Type: 4,ENCRYPTED` headers, a comment), so a repair
  could silently produce a different key. The person who holds the key pastes it again instead.
- Escaping every multi-line paste: the first version did, and the independent review found it
  turns a pretty-printed service-account JSON (valid with spaces) into invalid JSON.
- Doing nothing on the server: a drag, an autofill or another client would still store a broken
  key silently, which is the defect.

## Consequences

- A pasted key works with `secret_verify` (`escaped_newlines`) and `secret_copy_to_env` (its file
  byte for byte), proven with real `ssh-keygen` and `openssl` keys in SPEC-193's Evidence.
- Listing agent secrets decrypts each value its caller manages, in memory, to tell whether it is
  flattened; only the boolean leaves.
- A value shaped like a one-line key block can no longer be stored, even deliberately; a person
  who wants that writes its breaks as `\n`.
- A multi-line value that is not a key still loses its line breaks to the browser, as before; an
  agent secret cannot hold them.
- Team secrets are untouched: their values may hold real line breaks, which a person copies back,
  so escaping is wrong for them; their one-line value field has the same paste problem (#592).
