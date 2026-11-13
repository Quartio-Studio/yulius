# 0313. Redaction fingerprints are the same on every request and say a few bits of a value

## Status

Accepted. Specified in [SPEC-483](../../.ai/specs/SPEC-483-deep-security-and-quality-audit-fix-ever.md)
(slice 1); task #465. Amends SPEC-002 ADR-20 (VLT-007) and
[ADR-0014](0014-secrets-vault-and-agent-secret-proxy.md) ("redaction is an oracle for weak
values"), extends [ADR-0239](0239-the-superagent-gets-a-fingerprint-file-as-a-task-run-does.md)
and SPEC-410 GST-004.

## Context

The Bash wrapper and the `PostToolUse` hook run inside the sandbox and redact agent secrets from
what a command or a tool returns. They must not hold a value, so the server gives them
fingerprints of every value and its encodings: `POST /internal/scrub/fingerprints` (the run's
token) and the run's `scrub.json`. A fingerprint was `{len, rolling, digest}`: a polynomial
rolling hash of the needle modulo 2^61 − 1 at a public `base`, and `SHA-256(salt || needle)`.

The accepted risk was a guessing oracle: a run can test a guess against the digest, as it can by
printing it (ADR-0014, the 8-character minimum). The audit of task #465 found more than that:

- The rolling hash is the needle's polynomial evaluated at `base`, and the server drew a new
  random `base` for every request. A run that asked as many times as a value has bytes solved
  for the bytes. A test did it: 28 requests returned the 28 characters of its value. Every agent
  secret that reaches a run was readable this way, production ones and the instance Vidoc key
  included, by any process holding the run's token.
- At one base the hash is still a 61-bit linear equation over the value's bytes for each
  encoding: enough to find a short value without guessing it.
- A playground that a guest joined gives none of its runs an agent secret (GST-004), but its
  runs still got the fingerprints: names, lengths and something to test guesses against.

## Decision

- **A sandbox's set is fixed.** The salt is derived from the server's workspace secret and the
  sandbox id, and nothing else in a fingerprint is random, so every request and every launch
  gets the same set until a secret changes. Asking again tells nothing more.
- **A fingerprint says a tag instead of a rolling hash.** The tag is the top `tag_bits(len)`
  bits of a 16-bit hash of the needle's first 8 bytes (`redact::needle_tag`): 4 bits for a
  needle of 8 to 15 bytes, one more each time the length doubles, at most 16. The redactor
  hashes each 8-byte window of the output once, looks the tags up and confirms a candidate with
  the salted SHA-256 as before. The set has no `base` any more; its `version` is 2.
- **What a holder of the set learns about a value** is its length, the salted digest (the
  accepted guessing oracle) and, for each encoding, those few bits about its first 8 bytes. The
  bytes after the eighth are in no tag.
- **A playground a guest joined gets an empty set**, as its runs get no agent secret.
- **A starting server rewrites the fingerprint file of every live run and adopted Superagent**
  (`App::refresh_fingerprint_files`), in place. A session that ran across the update held a
  version 1 file, which told too much and which the wrapper no longer reads.

## Alternatives

- **Limit the requests.** The launch file alone holds one equation per encoding, and a run gets
  a new file at every launch.
- **Keep the rolling hash with a fixed base.** One 61-bit linear equation per encoding stays.
- **Publish only some bits of the rolling hash of the whole needle.** The equation is over
  every byte, so the bits of all encodings add up over the whole value (a meet in the middle
  uses them), and the redactor still rolls one hash per needle length for every byte of output.
- **More tag bits.** Fewer digests to compute, more said about a value. With 4 bits for the
  shortest needles a 16-character random value keeps about 63 bits unknown to a holder of the
  set; with 8 it would keep about 47.
- **Redact on the server.** No fingerprints at all, but every command's output would go through
  Yulius, and commands could not run while it restarts.
- **Leave production secrets out of a run's set.** A value that was copied into `.env` before
  it became a production secret is still in the sandbox and must be redacted there.

## Consequences

- The value of an agent secret cannot be computed from its fingerprints. Weak values stay
  guessable against the digest, as accepted before; the tags make that search shorter by at
  most their bits (about 30 for a 16-character value, never more than the first 8 bytes hold).
- Redaction costs less for the usual project and more for the largest: on 1 MB of output, 94 ms
  with 10 secrets (460 ms before), 267 ms with 40 (781 ms) and 1.2 s with 200, a project's
  limit (0.8 s), in a release build on the development host.
- `POST /internal/scrub/fingerprints` answers `{version: 2, salt, names, needles: [{secret,
  len, tag, digest}]}`. A version 1 file is not read; a starting server replaces it.
- When the workspace secret cannot be saved, the salt changes with each server start; the tags
  do not, so a new start still tells nothing more about a value.
