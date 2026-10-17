# 0163. The CLI imports secrets after a typed yes, and shows no value

## Status

Accepted. Specified in [SPEC-205](../../.ai/specs/SPEC-205-to-cli-add-and-option-to-import-secrets.md);
task #192.

## Context

A person asked for a CLI option to import secrets "for agents and team", which "should be safe,
leave no logs and require confirmation". Until now a secret entered the vault only through the
Secrets page, one at a time. People hold keys on their computers, often in a project's `.env`.

The vault already has the create calls the page uses (`POST /api/team-secrets`,
`POST /api/sandboxes/{sid}/agent-secrets`), and every `/api` route accepts the bearer token that
`yulius login` stores. Creating a secret reveals nothing, so it needs no two-factor code, and each
create writes a vault audit entry with the name, never the value. The server logs no request
bodies.

A secret can leak from a CLI in several places: the shell history and the process list (a value
passed as an argument), the screen and anything that records it (a value or its length printed),
error messages that quote input, temporary files, and memory that outlives the run.

## Decision

- **`yulius secrets import --to team|agent [FILE|-]`, client side.** The CLI reads the secrets and
  creates them one by one through the existing vault API with the stored token; the server is not
  changed. `--sandbox <owner/name|slug|id>` (default: the checkout's GitHub repository) and
  `--instance` choose where agent secrets go; agent secrets start with no hosts (SPEC-121).
- **Values never come from arguments.** They come from a dotenv file, from stdin (`-`), or typed at
  the terminal with echo off, one name and value at a time. No option takes a value.
- **Values never go out.** What the command prints holds names, classes, counts and server
  messages; never a value or its length. Dotenv errors name line numbers. Nothing is written to
  disk; values live in a wrapper without `Display` whose `Debug` is redacted and which is wiped
  from memory when dropped. Creates are sent once, never retried (a retried create could make a
  duplicate). The vault audit keeps the name, as it does for the page.
- **A typed `yes` at the terminal, always.** Before anything is sent the command shows the server,
  the target and every name as `new`, `exists` or `skipped: <reason>`, then reads the answer from
  `/dev/tty` (so stdin can carry the file). Anything else, end of input or no terminal cancels;
  no flag skips it.
- **Nothing is overwritten.** A team secret with the same title, or an agent secret with the same
  name in the same scope, is skipped. Invalid entries (agent name rules, a value too short or
  spanning lines) are skipped with the reason before asking; a PEM or OpenSSH key is sent with
  literal `\n`, as the Secrets page stores it.

## Alternatives

- **A server endpoint that takes a whole file.** One request instead of many, but a new API that
  parses secrets server side and duplicates the validation of the create calls. The client-side
  import reuses calls already audited and tested. Rejected.
- **A `--yes` flag for scripts.** The request asks for confirmation; a flag that skips it would be
  the path everyone copies into scripts. Scripts can still create secrets through the API.
  Rejected.
- **`--value` or `NAME=value` arguments.** Simple, but values land in the shell history and the
  process list. Rejected.
- **Replacing existing secrets.** Useful for rotation, but an import that silently replaces a
  working key can break every agent using it; the Secrets page rotates one secret with its
  context. A later `--replace`, behind the same confirmation, stays possible.
- **Deleting the source file after the import.** It is the person's file; the command reminds them
  that it still holds the values instead.

## Consequences

- A person moves a project's `.env` into the agent vault, or the team's shared keys into the team
  vault, in one confirmed command; a comment above a key becomes its description or notes.
- An import cannot run unattended: it needs a terminal for the `yes`.
- A run cut short by an expired sign-in or an unreachable server says which secrets were imported
  and which were not sent; running it again skips the imported ones as existing.
- `${…}` in a value is kept literally; nothing reads the person's environment.
- Memory wiping is best effort: the HTTP client's request buffers are not under the command's
  control.
