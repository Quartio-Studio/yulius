# 0313. Redaction fingerprints are the same on every request and say a few bits of a value

## Status

Accepted. Specified in [SPEC-483](../../.ai/specs/SPEC-483-deep-security-and-quality-audit-fix-ever.md)
(slice 1); task #465. Amends SPEC-002 ADR-20 (VLT-007) and
[ADR-0014](0014-secrets-vault.md) ("redaction is an oracle for weak values"), extends
[ADR-0239](0239-the-superagent-gets-a-fingerprint-file-as-a-task-run-does.md) and SPEC-410
GST-004.

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
- A run's set also held the two-factor seed of a secret, which never enters a sandbox: Yulius
  types the code, not the seed.

## Decision

- **A sandbox's set is fixed.** The salt is derived from the server's workspace secret and the
  sandbox id, and nothing else in a fingerprint is random, so every request and every launch
  gets the same set until a secret changes. Asking again tells nothing more.
- **A fingerprint says a tag instead of a rolling hash.** The tag is the top `tag_bits(len)`
  bits of a 16-bit hash of the needle's first 8 bytes (`redact::needle_tag`): 3 bits for a
  needle of 8 to 15 bytes, one more each time the length doubles, at most 16. The hash ignores
  the case of letters and reads `-` and `_` as `+` and `/`, so the two hexadecimal forms of a
  value share a tag, and so do its base64 and base64url forms. The redactor hashes each 8-byte
  window of the output once, looks the tags up and confirms a candidate with the salted SHA-256
  as before, once per needle length. The set has no `base` any more; its `version` is 2.
- **A run's set covers what can be in its sandbox.** No two-factor seed. In a playground a
  guest joined, no agent secret and no instance Vidoc key, only the values its MCP servers
  hold themselves, which still reach its runs; the sessions running at the join get their
  files rewritten then. A store that cannot say whether a guest joined gives no set (the
  wrapper then runs nothing), never an empty one.
- **Sessions from before version 2 keep redacting from their file.** A contained session
  binds the `yulius` binary it started with, an update freezes and thaws sessions instead of
  restarting them, and that wrapper reads only the old format. It cannot read the endpoint's
  answer and falls back on its `scrub.json`. A file in the old format therefore stays in the
  old format, written by this server with `base` 0: the old rolling hash at base 0 is a
  needle's last byte plus one. A starting server rewrites every live run's and Superagent's
  file in place (`App::refresh_fingerprint_files`), and a request from a wrapper that does not
  name version 2 brings the asking session's file up to date before it is answered. The
  endpoint itself answers version 2 only. When such a file's fingerprints cannot be made, one
  this server wrote stays as it is, and one an older server wrote becomes a bare
  `{"version":1}`: its wrapper runs nothing with it, and the next rewrite still knows the
  format.

## What a holder of a set can compute

The digests were always a test for a guess, and for a value inside base64 text the needle is a
digest of a part of the value (whole 3-byte groups at each alignment), so a guess of that part
can be tested alone. The tags add, per encoding, their bits about the first 8 bytes. The search
that is left for a random value, in SHA-256 computations:

| Value | Bits in it | Tag bits | Digests alone | With the tags |
| --- | --- | --- | --- | --- |
| 12 letters and digits | 71 | 17 | 2^54 | 2^43 |
| 16 letters and digits | 95 | 21 | 2^71 | 2^62 |
| 20 letters and digits | 119 | 21 | 2^107 | 2^86 |
| 32 letters and digits | 191 | 26 | 2^179 | 2^153 |
| 16 hexadecimal digits | 64 | 21 | 2^48 | 2^35 |
| 32 hexadecimal digits | 128 | 26 | 2^120 | 2^94 |

One graphics card computes about 2^34 SHA-256 a second: 2^43 is minutes, 2^62 is years of one
card. A value of 20 random letters and digits, or of 32 hexadecimal digits, is out of reach (20
hexadecimal digits are not: 2^51); one of 12 letters and digits is not, with or without the
tags. Each line of a key block and each MCP server's value is a needle set of its own, with its
own tags about its own first 8 bytes; a line of a key has far more bits than those.

A file in the old format says more, to the session that holds it: the last byte of every form
of a value, which is its last character and, through the base64 forms, most of the two before
it (up to about 18 bits). It holds what that session's set holds, also a value added or changed
while the session lives and a new owner's personal values after a hand-over. With it the search
for 16 letters and digits falls from 2^62 to about 2^56; 20 stay near 2^80.

## Alternatives

- **Limit the requests.** The launch file alone holds one equation per encoding, and a run gets
  a new file at every launch.
- **Keep the rolling hash with a fixed base.** One 61-bit linear equation per encoding stays.
- **Publish only some bits of the rolling hash of the whole needle.** The equation is over
  every byte, so the bits of all encodings add up over the whole value (a meet in the middle
  uses them), and the redactor still rolls one hash per needle length for every byte of output.
- **More tag bits.** Fewer digests to compute, more said about a value: with 4 bits for the
  shortest needles and a tag per hexadecimal case, 16 letters and digits were left with 2^51.
- **Fewer tag bits.** With 2 bits the same value keeps 2^67, and redaction costs about twice
  the digests. Three bits stay faster than the old matcher at every size measured.
- **Redact on the server.** No fingerprints at all, but every command's output would go through
  Yulius, and commands could not run while it restarts.
- **Leave production secrets out of a run's set.** A value that was copied into `.env` before
  it became a production secret is still in the sandbox and must be redacted there.
- **Keep the agent secrets in a guest playground's set**, for values copied into `.env` before
  the guest joined. The guest reads those files anyway; for every other secret the set would
  give the guest its name, its length and a test for guesses.
- **Restart the sessions that ran across the update.** A restart ends what a session left
  running (a build, a gate), in every live task at once.
- **Answer the old format from the endpoint.** Any run could then ask for it; a file is
  written in it only for a session whose file already was.

## Consequences

- The value of an agent secret cannot be computed from its fingerprints. Weak values stay
  guessable against the digests, as accepted before, and the tags shorten that search by the
  bits in the table. Values that untrusted code could reach before this change may be known to
  it: rotate them once the sessions from before the update have ended, and make new ones 20
  random letters and digits or 32 hexadecimal digits, or longer.
- Values copied into `.env` in a playground before a guest joined are no longer redacted in
  that sandbox's command output (the guest can read the file); Yulius' own redaction of the
  terminal, transcripts and tool results it serves still covers them.
- Redaction is faster: on 1 MB of output, 126 ms with 10 secrets (460 ms before), 205 ms with
  40 (781 ms) and 314 ms with 200, a project's limit (819 ms), in a release build on the
  development host, the secrets' lengths spread over 20 values.
- `POST /internal/scrub/fingerprints` takes `{version}` and answers `{version: 2, salt, names,
  needles: [{secret, len, tag, digest}]}`.
- A session from before version 2 redacts from its file alone, a little slower than before:
  its wrapper now confirms every window that ends in a needle's last byte (1 MB: 614 ms with 10
  secrets, 896 ms with 40, 923 ms with 200). Uncontained, it runs the new wrapper, which reads
  version 2 only: while the server cannot be reached its commands do not run, as without a
  file.
- The old format's writer (`LegacyFingerprintSet`, `legacy_fingerprints_for`) is of use only
  while a session launched before this change is alive; it goes with a later release (#1239).
- When the workspace secret cannot be saved, the salt changes with each server start; the tags
  do not, so a new start still tells nothing more about a value.
