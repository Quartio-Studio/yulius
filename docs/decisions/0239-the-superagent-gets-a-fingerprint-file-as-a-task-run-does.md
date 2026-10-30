# 0239. The Superagent gets a fingerprint file as a task run does

## Status

Accepted. Specified in [SPEC-352](../../.ai/specs/SPEC-352-something-got-broken-with-yulius-fix.md);
task #334. Supersedes, in [ADR-0121](0121-the-superagent-gets-the-sandbox-mcp-servers-and-the-redaction.md)
and SPEC-147, "The Superagent has no fingerprint file". The rest of both stands.

## Context

The Superagent runs its Bash commands, hooks and stdio MCP servers through `yulius --scrub-exec`,
which loads the redaction fingerprints from Yulius with the person's workspace token. A task run
falls back to its `<run>/scrub.json` when Yulius cannot be reached. The Superagent had no such file,
so ADR-0121 accepted that its commands do not run "for at most a restart".

On 2026-10-30 a restart was enough to make the Superagent useless: every command answered
"Yulius is restarting … try again". The person was using it to find out why tasks had stopped,
and it could only report that Yulius was down. Task sessions kept working through the same
restart.

## Decision

- A Superagent's shell gets `scrub.json` in its run directory (`<data>/shells/<id>/`), with the
  fingerprints of the agent secrets that apply to its person in its sandbox. The file is written
  at the shell's start, with mode 0600, and is bound read-only in a contained session.
  `YULIUS_SCRUB_FILE` names it. This is `App::vault_superagent_setup`, next to the proxy
  variables.
- The wrapper's order is unchanged: Yulius first, then the file, otherwise fail closed.

## Alternatives

- **Keep failing closed.** This is the defect: a restart (an update builds and installs first)
  blocks the person exactly when they need the Superagent.
- **Run commands unredacted while Yulius is unreachable.** This would leak agent secret values to
  the model and the screen.
- **A longer fingerprint timeout.** This does not cover a restart, and it would slow every
  command while Yulius is slow.

## Consequences

- The file holds salted hashes, never values: the same set the person's own task runs get in
  theirs. It is read only while Yulius cannot be reached. Whenever Yulius runs, SAM-003's access
  check still decides.
- Like a task run's file, it is written once. An agent secret added later is redacted from a
  Superagent's output during a restart only after the Superagent starts again.
