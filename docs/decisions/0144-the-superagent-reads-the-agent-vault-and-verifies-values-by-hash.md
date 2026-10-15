# 0144. The Superagent reads the agent vault and verifies values by their hash

## Status

Accepted. Specified in [SPEC-178](../../.ai/specs/SPEC-178-superagent-should-have-access-to-agent-s.md);
task #165. Amends SPEC-037 MGT-006 ([ADR-0029](0029-cockpit-manages-through-mcp.md)): the
Superagent's tools are no longer the management tools alone. Builds on
[ADR-0121](0121-the-superagent-gets-the-sandbox-mcp-servers-and-the-redaction.md).

## Context

A person asked the Superagent to check that the project's agent secrets were there ("names only,
no values") and that a three-line private key in a file was the one in the vault. It could do
neither: it is served only the task and automation tools, and the vault tools belong to tasks.
Since ADR-0121 it redacts agent secrets as a task run does, but redaction matches per line and per
encoding, so it cannot tell whether a whole key matches.

Task agents use agent secrets without seeing them: the egress proxy adds them to requests for
bound hosts, `browser_fill_secret` types them, `secret_copy_to_env` writes them to an ignored
`.env` once a person allows it, and `secret_request_hosts` asks a person to bind hosts. The
Superagent is not behind the proxy, has no worktree or browser, and runs as the person, whose
plain terminal reads their home unredacted; agent secret values are shown to people only after a
two-factor unlock (ADR-0042). An agent secret never spans lines (`check_agent_value`), so a
multi-line key is stored with literal `\n`.

## Decision

- **The Superagent lists the vault.** `secrets_list` joins its tools: the records a task of
  its sandbox gets (names, descriptions, hosts, requested hosts, fields), never a value, without
  the proxy's placeholder and injection, which it cannot use, and with its
  own guidance: no placeholders in its session, values checked with `secret_verify`, secrets
  managed on the Secrets page (its link), and work that uses a secret belongs in a task.
- **Values are verified by their SHA-256.** A new tool, `secret_verify {secret, sha256}`, served
  to the Superagent only, compares the candidate's hash with the stored value and with
  its variants: the value with a final line break (`trailing_newline`) and the value with its
  literal `\n` as LF or CRLF line breaks, with or without a final one (`escaped_newlines`). It
  answers the kind of match (or `different`) and, on a match, the lines the value stands for, never
  the value or a hash Yulius computed from it. The variants are hashed in place (no copy of the value) and all compared
  in constant time, without an early exit. The caller computes the hash in a shell (`sha256sum key.pem`), so the value
  reaches neither the model nor the screen. Production secrets (SPEC-134) are refused. Each
  distinct check is audited once per vault generation and sandbox (`vault.agent.verify`, actor
  `cockpit:<login>`), so a check repeated after a rotation is audited again.
- **Not for task agents.** On a match the hash the caller sends is the value's unsalted SHA-256,
  and it stays in the caller's transcript (Bash output, tool input), which redaction does not
  catch. A task's transcript is read by every member who can view the project; the Superagent's
  conversation is its person's own, and they hold the candidate already.
- **Both run without asking.** They join `COCKPIT_READ_ONLY_TOOLS`: they change nothing but the
  audit.
- **The rest stays a task's.** `secret_request_hosts`, `secret_copy_to_env` and
  `browser_fill_secret` are still refused to the Superagent.

## Alternatives

- **Put the Superagent behind the egress proxy** so it uses secrets as a task does. All of the
  person's traffic would go through Yulius, a decision ADR-0121 left on its own; it is not needed
  to answer what the person asked. Not now.
- **`secret_copy_to_env` for the Superagent.** It has no worktree; a copy into the person's
  checkout puts the value where their plain terminal reads it without a two-factor unlock.
  Rejected.
- **`secret_request_hosts` for the Superagent.** The person in front of it can bind hosts on the
  Secrets page, and the request flow flags a task as needing a person, which the Superagent has
  not. Not now.
- **Send the candidate value to Yulius** and compare there, with any normalisation. The value
  would pass through the model's tool call, transcript and screen. Rejected.
- **Rely on redaction** (print the file, look for `[secret:NAME]`). Per line and per encoding: a
  key with a line missing or reordered still shows only labels, and a multi-line file never
  matches a value stored with `\n`. Not enough.
- **Return a hash of the stored value** for the caller to compare. That hands out an unsalted
  hash of every secret, which can be attacked offline. Rejected.
- **`secret_verify` for task agents too.** A matching candidate's hash would sit in a transcript
  every viewer of the project reads, open to an offline attack on a short value (a sign-in
  password). Rejected.
- **A salted check** (Yulius hands out a salt, the caller hashes salt and candidate). The salt sits
  next to the hash in the same transcript, so a targeted offline attack still works. Not enough to
  serve task agents.

## Consequences

- The Superagent answers "which secrets does this project have" and "is this the key in the
  vault" without anyone seeing a value. Task agents keep `secrets_list` and do not verify.
- `secret_verify` is an equality oracle, which redaction and the salted fingerprints a person's
  processes already read give too; it adds exactness and the line count, not a way to learn an
  unknown value. A low-entropy secret could be guessed by trying candidates, as with redaction;
  every distinct candidate leaves an audit entry.
- On a match the person's own Superagent conversation (kept in their home) holds the value's
  unsalted SHA-256; whoever reads that home could attack a short value offline. The person reads
  it unredacted anyway (ADR-0121), and agent secrets meant to be verified are keys, not short
  passwords.
- The audit holds one `vault.agent.verify` entry per vault generation, sandbox, caller, secret and
  candidate while Yulius runs (the once-per-key cache, holding a digest of the candidate, is
  bounded and cleared when full).
- Using a secret from the Superagent still needs a task (proxy, browser, `.env`).
